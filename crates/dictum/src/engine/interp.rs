//! The spec interpreter: a direct tree walk over the typed IR, written for
//! obviousness, not speed.
//!
//! It has two jobs. It implements `explain` (with a `Trace`), so an explanation
//! describes the per-rule IR with exact spans and the fast path carries no recording
//! code. And it is the oracle of the differential test, reached through the public
//! API: `rule.eval(ctx)` must equal `rule.explain(ctx).into_result()`.
//!
//! Because `explain` runs it in production, it meets the same bar as the closure
//! tree: no panics (IR invariant failures become `Internal`) and the same budget
//! charges at the same points.
//!
//! Budget charges (the closure tree must make the same ones):
//! - one step per element a predicate visits, before its body runs;
//! - `len` steps for `x in array`, before scanning (so a sorted constant set costs
//!   what a scan does), and one per entry for `keys`, `values`, `sum`, `join`,
//!   `max`/`min` of an array, structural `==`, and the deep check of `any`;
//! - one alloc per element of a built array or map (literals, `filter`, `map`,
//!   `split`, `keys`, `values`, `+` of arrays) and one per byte of a built string,
//!   before building it. Constants are part of the rule and cost nothing.
//!
//! TODO differential test (proptest, dev-dependency):
//! ```text
//! strategy: random well-typed source over a fixed test root struct
//!           (generated from a typed grammar, then compiled, so the parser and
//!            checker are exercised too)
//! property: for random facts and `now`,
//!           rule.eval(ctx) == rule.explain(ctx).into_result()          // compiler, folder
//!           RuleSet of N variants: Ok(values) == every standalone Rule value,
//!           Err(e) == the first rule (insertion order) whose standalone eval fails
//!                                                                    // sharing
//! equality: values equal, or both errors with the same ErrorCode and span
//!           (cases that hit a budget limit are skipped: the RuleSet charges shared
//!            work to the first rule that computes it)
//! ```

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::ops::ControlFlow;

use dictum_check::ir::{
    self, BinaryOp, CompareKind, Conversion, EqKind, ExprKind, FloatOp, InOp, IndexOp, IntOp, Intrinsic, NodeId,
    PredicateOp, SliceOp, UnaryOp,
};
use dictum_check::resolve::{self, Promote};
use dictum_check::{Tag, Type};
use dictum_syntax::Builtin;
use dictum_syntax::Span;
use dictum_syntax::ast::{self, CompareOp, LogicOp, Pointer};

use crate::EvalErrorKind;
use crate::engine::SchemaCore;
use crate::engine::builtins;
use crate::engine::frame::{Fault, Frame, Scope};
use crate::engine::ops;
use crate::engine::trace::{Recorded, Trace};
use crate::engine::value::{Bytes, MapRef, Repr, Seq, Str, Value};
use crate::func::CallError;

/// Evaluate `program` in `frame`. When `trace` is given, record every node's outcome
/// into it by `NodeId`.
///
/// Uses `ops` for every primitive, so the closure tree and this walk differ only in
/// control flow, specialization and caching.
pub(crate) fn eval<'a>(
    program: &ir::Program,
    schema: &SchemaCore,
    patterns: &BTreeMap<NodeId, Arc<regex::Regex>>,
    frame: &mut Frame<'a>,
    trace: Option<&mut Trace<'a>>,
) -> Result<Value<'a>, Fault> {
    let mut walk = Walk { schema, patterns, trace };
    frame.slots.clear();
    frame.scopes.reserve(usize::from(program.max_predicate_depth()));
    for e in program.lets() {
        let v = walk.expr(e, frame)?;
        frame.slots.push(v);
    }
    walk.expr(program.body(), frame)
}

struct Walk<'p, 't, 'a> {
    schema: &'p SchemaCore,
    patterns: &'p BTreeMap<NodeId, Arc<regex::Regex>>,
    trace: Option<&'t mut Trace<'a>>,
}

type Res<'a> = Result<Value<'a>, Fault>;

fn internal(what: &'static str, span: Span) -> Fault {
    Frame::fault(EvalErrorKind::Internal(what), span)
}

fn mismatch(expected: &'static str, found: &Value<'_>, span: Span) -> Fault {
    Frame::fault(EvalErrorKind::TypeMismatch { expected, found: found.tag().name() }, span)
}

/// Attach `span` to an `ops` error.
fn at(span: Span) -> impl Fn(EvalErrorKind) -> Fault {
    move |kind| Frame::fault(kind, span)
}

// Typed reads. The checker proved the types, so a mismatch is a dictum bug.
fn int(v: &Value<'_>, span: Span) -> Result<i64, Fault> {
    v.as_int().ok_or_else(|| internal("expected an int", span))
}
fn float(v: &Value<'_>, span: Span) -> Result<f64, Fault> {
    v.as_float().ok_or_else(|| internal("expected a float", span))
}
fn boolean(v: &Value<'_>, span: Span) -> Result<bool, Fault> {
    v.as_bool().ok_or_else(|| internal("expected a bool", span))
}
fn string<'v>(v: &'v Value<'_>, span: Span) -> Result<&'v str, Fault> {
    v.as_str().ok_or_else(|| internal("expected a string", span))
}
fn time(v: &Value<'_>, span: Span) -> Result<jiff::Timestamp, Fault> {
    v.as_time().ok_or_else(|| internal("expected a time", span))
}
fn duration(v: &Value<'_>, span: Span) -> Result<jiff::SignedDuration, Fault> {
    v.as_duration().ok_or_else(|| internal("expected a duration", span))
}
fn seq<'v, 'a>(v: &'v Value<'a>, span: Span) -> Result<&'v Seq<'a>, Fault> {
    v.as_seq().ok_or_else(|| internal("expected an array", span))
}
fn map<'v, 'a>(v: &'v Value<'a>, span: Span) -> Result<&'v MapRef<'a>, Fault> {
    v.as_map().ok_or_else(|| internal("expected a map", span))
}
#[cfg(feature = "decimal")]
fn decimal(v: &Value<'_>, span: Span) -> Result<rust_decimal::Decimal, Fault> {
    match v.0 {
        Repr::Decimal(d) => Ok(d),
        _ => Err(internal("expected a decimal", span)),
    }
}

fn steps(f: &mut Frame<'_>, n: usize, span: Span) -> Result<(), Fault> {
    f.budget.step(u64::try_from(n).unwrap_or(u64::MAX)).map_err(at(span))
}

fn alloc(f: &mut Frame<'_>, n: usize, span: Span) -> Result<(), Fault> {
    f.budget.alloc(u64::try_from(n).unwrap_or(u64::MAX)).map_err(at(span))
}

/// A computed string, charged one alloc per byte before it is kept.
fn built(f: &mut Frame<'_>, s: String, span: Span) -> Res<'static> {
    alloc(f, s.len(), span)?;
    Ok(Value::string(s))
}

fn promote<'a>(v: Value<'a>, p: Promote, span: Span) -> Res<'a> {
    match p {
        Promote::None => Ok(v),
        #[allow(clippy::cast_precision_loss)]
        Promote::IntToFloat => Ok(Value::float(int(&v, span)? as f64)),
        #[cfg(feature = "decimal")]
        Promote::IntToDecimal => Ok(Value(Repr::Decimal(rust_decimal::Decimal::from(int(&v, span)?)))),
    }
}

impl<'a> Walk<'_, '_, 'a> {
    /// Evaluate one node and record its outcome.
    fn expr(&mut self, e: &ir::Expr, f: &mut Frame<'a>) -> Res<'a> {
        let r = self.compute(e, f);
        if let Some(trace) = self.trace.as_deref_mut() {
            let element = f.scopes.last().and_then(|s| u64::try_from(s.index).ok());
            let outcome = match &r {
                Ok(v) => Recorded::Value(v.clone()),
                Err(fault) => Recorded::Failed(fault.kind.code()),
            };
            trace.record(e.id(), outcome, element);
        }
        r
    }

    fn compute(&mut self, e: &ir::Expr, f: &mut Frame<'a>) -> Res<'a> {
        let span = e.span();
        match e.kind() {
            ExprKind::Const(c) => Ok(Value::from_const(c)),
            ExprKind::Root => Ok(Value::object(f.root)),
            ExprKind::Slot(s) => f.slots.get(s.index()).cloned().ok_or_else(|| internal("unset slot", span)),
            ExprKind::Pointer(p) => {
                let scope = f.scopes.last().ok_or_else(|| internal("pointer outside a predicate", span))?;
                match p {
                    Pointer::Current => Ok(scope.current.clone()),
                    Pointer::Index => Ok(Value::int(scope.index)),
                    Pointer::Acc => scope.acc.clone().ok_or_else(|| internal("#acc outside reduce", span)),
                }
            }
            ExprKind::Field { base, field, optional } => {
                let b = self.expr(base, f)?;
                match b.0 {
                    Repr::Nil if *optional => Ok(Value::NIL),
                    Repr::Object(o) => Ok(o.field(*field)),
                    _ => Err(internal("field of a non-struct", span)),
                }
            }
            ExprKind::Index { base, index, op, optional } => {
                let b = self.expr(base, f)?;
                if *optional && b.is_nil() {
                    return Ok(Value::NIL);
                }
                let i = self.expr(index, f)?;
                index_value(*op, &b, &i, span)
            }
            ExprKind::Slice { base, from, to, op, optional } => {
                let b = self.expr(base, f)?;
                if *optional && b.is_nil() {
                    return Ok(Value::NIL);
                }
                // A bound inside a character is reported at that bound.
                let spans = (from.as_deref().map_or(span, ir::Expr::span), to.as_deref().map_or(span, ir::Expr::span));
                let from = from.as_deref().map(|x| self.expr(x, f).and_then(|v| int(&v, x.span()))).transpose()?;
                let to = to.as_deref().map(|x| self.expr(x, f).and_then(|v| int(&v, x.span()))).transpose()?;
                slice_value(*op, &b, (from, to), spans, f, span)
            }
            ExprKind::Unary { op, arg } => {
                let v = self.expr(arg, f)?;
                unary(*op, v, span)
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let l = self.expr(lhs, f)?;
                let r = self.expr(rhs, f)?;
                self.binary(*op, l, r, e.id(), f, span)
            }
            ExprKind::Logic { op, operands } => {
                // `&&` stops at the first `false`, `||` at the first `true`.
                let stop = *op == LogicOp::Or;
                for x in operands {
                    let v = self.expr(x, f)?;
                    if boolean(&v, x.span())? == stop {
                        return Ok(Value::bool(stop));
                    }
                }
                Ok(Value::bool(!stop))
            }
            ExprKind::Compare { kind, first, rest } => {
                let mut prev = self.expr(first, f)?;
                for (op, x) in rest {
                    let cur = self.expr(x, f)?;
                    let ord = order(*kind, &prev, &cur, span)?;
                    let holds = match (op, ord) {
                        (_, None) => false,
                        (CompareOp::Lt, Some(o)) => o == Ordering::Less,
                        (CompareOp::Le, Some(o)) => o != Ordering::Greater,
                        (CompareOp::Gt, Some(o)) => o == Ordering::Greater,
                        (CompareOp::Ge, Some(o)) => o != Ordering::Less,
                    };
                    if !holds {
                        return Ok(Value::bool(false));
                    }
                    prev = cur;
                }
                Ok(Value::bool(true))
            }
            ExprKind::In { op, needle, haystack, negated } => {
                let n = self.expr(needle, f)?;
                let h = self.expr(haystack, f)?;
                Ok(Value::bool(membership(*op, &n, &h, f, span)? != *negated))
            }
            ExprKind::Conditional { cond, then, otherwise } => {
                let c = self.expr(cond, f)?;
                if boolean(&c, cond.span())? { self.expr(then, f) } else { self.expr(otherwise, f) }
            }
            ExprKind::Coalesce { lhs, rhs } => {
                let l = self.expr(lhs, f)?;
                if l.is_nil() { self.expr(rhs, f) } else { Ok(l) }
            }
            ExprKind::Array(xs) => {
                alloc(f, xs.len(), span)?;
                let mut out = Vec::with_capacity(xs.len());
                for x in xs {
                    out.push(self.expr(x, f)?);
                }
                Ok(Value::array(out))
            }
            ExprKind::Map(pairs) => {
                alloc(f, pairs.len(), span)?;
                let mut out: Vec<(Arc<str>, Value<'a>)> = Vec::with_capacity(pairs.len());
                for (k, v) in pairs {
                    let key = self.expr(k, f)?;
                    let key: Arc<str> = string(&key, k.span())?.into();
                    let value = self.expr(v, f)?;
                    if out.iter().any(|(seen, _)| *seen == key) {
                        return Err(Frame::fault(EvalErrorKind::DuplicateKey((*key).into()), k.span()));
                    }
                    out.push((key, value));
                }
                Ok(Value(Repr::Map(MapRef::Owned(out.into()))))
            }
            ExprKind::Range { from, to } => {
                let a = self.expr(from, f)?;
                let b = self.expr(to, f)?;
                Ok(Value(Repr::Array(Seq::Range { from: int(&a, from.span())?, to: int(&b, to.span())? })))
            }
            ExprKind::Predicate { op, seq, body } => {
                let s = self.expr(seq, f)?;
                let s = s.as_seq().cloned().ok_or_else(|| mismatch("an array", &s, seq.span()))?;
                self.predicate(*op, &s, body.as_deref(), f, span)
            }
            ExprKind::Intrinsic { op, args } => {
                let mut vals = Vec::with_capacity(args.len());
                for x in args {
                    vals.push(self.expr(x, f)?);
                }
                intrinsic(*op, &vals, f, span)
            }
            ExprKind::Call { func, args } => {
                let mut vals = Vec::with_capacity(args.len());
                for x in args {
                    vals.push(self.expr(x, f)?);
                }
                let imp = self.schema.payload(*func).ok_or_else(|| internal("unknown function", span))?;
                (imp.call)(&vals).map_err(|err| match err {
                    CallError::Function(error) => {
                        let name = self.schema.func_name(*func).unwrap_or("?").into();
                        Frame::fault(EvalErrorKind::Function { name, error }, span)
                    }
                    CallError::BadArgument => internal("function argument did not convert", span),
                })
            }
            ExprKind::Convert { op, arg } => {
                let v = self.expr(arg, f)?;
                match op {
                    Conversion::IntToFloat => promote(v, Promote::IntToFloat, span),
                    #[cfg(feature = "decimal")]
                    Conversion::IntToDecimal => promote(v, Promote::IntToDecimal, span),
                    Conversion::Some | Conversion::Erase => Ok(v),
                    Conversion::Check => check(v, e.ty(), f, span),
                }
            }
        }
    }

    fn binary(
        &mut self,
        op: BinaryOp,
        l: Value<'a>,
        r: Value<'a>,
        id: NodeId,
        f: &mut Frame<'a>,
        span: Span,
    ) -> Res<'a> {
        match op {
            BinaryOp::Dyn(op) => {
                let Some(b) = resolve::binary(op, l.tag(), r.tag()) else {
                    return Err(Frame::fault(
                        EvalErrorKind::TypeMismatch { expected: l.tag().name(), found: r.tag().name() },
                        span,
                    ));
                };
                let (l, r) = (promote(l, b.lhs, span)?, promote(r, b.rhs, span)?);
                self.binary(b.op, l, r, id, f, span)
            }
            BinaryOp::Int(op) => {
                let (a, b) = (int(&l, span)?, int(&r, span)?);
                let n = match op {
                    IntOp::Add => ops::add_int(a, b),
                    IntOp::Sub => a.checked_sub(b).ok_or(EvalErrorKind::Overflow),
                    IntOp::Mul => a.checked_mul(b).ok_or(EvalErrorKind::Overflow),
                    IntOp::Mod => ops::mod_int(a, b),
                };
                Ok(Value::int(n.map_err(at(span))?))
            }
            BinaryOp::Float(op) => {
                let (a, b) = (float(&l, span)?, float(&r, span)?);
                Ok(Value::float(match op {
                    FloatOp::Add => a + b,
                    FloatOp::Sub => a - b,
                    FloatOp::Mul => a * b,
                    FloatOp::Div => a / b,
                    FloatOp::Pow => ops::pow_float(a, b),
                }))
            }
            #[cfg(feature = "decimal")]
            BinaryOp::Decimal(op) => {
                use dictum_check::ir::DecimalOp;
                let (a, b) = (decimal(&l, span)?, decimal(&r, span)?);
                let zero = matches!(op, DecimalOp::Div | DecimalOp::Mod) && b.is_zero();
                if zero {
                    return Err(Frame::fault(EvalErrorKind::DivisionByZero, span));
                }
                let d = match op {
                    DecimalOp::Add => a.checked_add(b),
                    DecimalOp::Sub => a.checked_sub(b),
                    DecimalOp::Mul => a.checked_mul(b),
                    DecimalOp::Div => a.checked_div(b),
                    DecimalOp::Mod => a.checked_rem(b),
                };
                Ok(Value(Repr::Decimal(d.ok_or_else(|| Frame::fault(EvalErrorKind::Overflow, span))?)))
            }
            BinaryOp::Concat => {
                let (a, b) = (string(&l, span)?, string(&r, span)?);
                alloc(f, a.len().saturating_add(b.len()), span)?;
                let mut s = String::with_capacity(a.len().saturating_add(b.len()));
                s.push_str(a);
                s.push_str(b);
                Ok(Value::string(s))
            }
            BinaryOp::ConcatBytes => {
                let a = l.as_bytes().ok_or_else(|| internal("expected bytes", span))?;
                let b = r.as_bytes().ok_or_else(|| internal("expected bytes", span))?;
                alloc(f, a.len().saturating_add(b.len()), span)?;
                let joined: Arc<[u8]> = a.iter().chain(b).copied().collect();
                Ok(Value(Repr::Bytes(Bytes::Shared(joined))))
            }
            BinaryOp::ConcatArray => {
                let (a, b) = (seq(&l, span)?, seq(&r, span)?);
                alloc(f, a.len().saturating_add(b.len()), span)?;
                Ok(Value::array(a.iter().chain(b.iter()).collect()))
            }
            BinaryOp::TimeSubTime => Ok(Value(Repr::Duration(time(&l, span)?.duration_since(time(&r, span)?)))),
            BinaryOp::TimeAddDuration | BinaryOp::TimeSubDuration => {
                let (t, d) = (time(&l, span)?, duration(&r, span)?);
                let d = if op == BinaryOp::TimeSubDuration { d.checked_neg() } else { Some(d) };
                let t =
                    d.and_then(|d| t.checked_add(d).ok()).ok_or_else(|| Frame::fault(EvalErrorKind::Overflow, span))?;
                Ok(Value(Repr::Time(t)))
            }
            BinaryOp::DurationAdd | BinaryOp::DurationSub => {
                let (a, b) = (duration(&l, span)?, duration(&r, span)?);
                let d = if op == BinaryOp::DurationAdd { a.checked_add(b) } else { a.checked_sub(b) };
                Ok(Value(Repr::Duration(d.ok_or_else(|| Frame::fault(EvalErrorKind::Overflow, span))?)))
            }
            BinaryOp::Eq(kind) => Ok(Value::bool(equal(kind, &l, &r, f, span)?)),
            BinaryOp::Ne(kind) => Ok(Value::bool(!equal(kind, &l, &r, f, span)?)),
            BinaryOp::Matches => {
                let s = string(&l, span)?;
                if let Some(re) = self.patterns.get(&id) {
                    return Ok(Value::bool(re.is_match(s)));
                }
                let pattern = string(&r, span)?;
                let re = regex::Regex::new(pattern).map_err(|_| {
                    Frame::fault(EvalErrorKind::InvalidArgument { what: "regex", value: pattern.into() }, span)
                })?;
                Ok(Value::bool(re.is_match(s)))
            }
            BinaryOp::Contains => Ok(Value::bool(string(&l, span)?.contains(string(&r, span)?))),
            BinaryOp::StartsWith => Ok(Value::bool(string(&l, span)?.starts_with(string(&r, span)?))),
            BinaryOp::EndsWith => Ok(Value::bool(string(&l, span)?.ends_with(string(&r, span)?))),
        }
    }

    fn predicate(
        &mut self,
        op: PredicateOp,
        s: &Seq<'a>,
        body: Option<&ir::Expr>,
        f: &mut Frame<'a>,
        span: Span,
    ) -> Res<'a> {
        let mut kept: Vec<Value<'a>> = Vec::new();
        let mut hits: i64 = 0;
        for i in 0..s.len() {
            steps(f, 1, span)?;
            let elem = s.get(i).ok_or_else(|| internal("sequence shorter than its length", span))?;
            let index = i64::try_from(i).map_err(|_| internal("index past i64", span))?;
            let out = match body {
                Some(b) => {
                    f.scopes.push(Scope { current: elem.clone(), index, acc: None });
                    let r = self.expr(b, f);
                    f.scopes.pop();
                    r?
                }
                None => elem.clone(),
            };
            if op == PredicateOp::Map {
                alloc(f, 1, span)?;
                kept.push(out);
                continue;
            }
            let holds = match out.as_bool() {
                Some(b) => b,
                None if body.is_none() => return Err(mismatch("bool", &out, span)),
                None => return Err(internal("predicate body is not a bool", span)),
            };
            if !holds {
                if op == PredicateOp::All {
                    return Ok(Value::bool(false));
                }
                continue;
            }
            match op {
                PredicateOp::Any => return Ok(Value::bool(true)),
                PredicateOp::None => return Ok(Value::bool(false)),
                PredicateOp::Find => return Ok(elem),
                PredicateOp::FindIndex => return Ok(Value::int(index)),
                PredicateOp::Filter => {
                    alloc(f, 1, span)?;
                    kept.push(elem);
                }
                PredicateOp::One if hits == 1 => return Ok(Value::bool(false)),
                PredicateOp::All | PredicateOp::One | PredicateOp::Count | PredicateOp::Map => {}
            }
            hits = hits.saturating_add(1);
        }
        Ok(match op {
            PredicateOp::All | PredicateOp::None => Value::bool(true),
            PredicateOp::Any => Value::bool(false),
            PredicateOp::One => Value::bool(hits == 1),
            PredicateOp::Filter | PredicateOp::Map => Value::array(kept),
            PredicateOp::Count => Value::int(hits),
            PredicateOp::Find => Value::NIL,
            PredicateOp::FindIndex => Value::int(-1),
        })
    }
}

fn unary<'a>(op: UnaryOp, v: Value<'a>, span: Span) -> Res<'a> {
    let overflow = || Frame::fault(EvalErrorKind::Overflow, span);
    match op {
        UnaryOp::Not => Ok(Value::bool(!boolean(&v, span)?)),
        UnaryOp::NegInt => Ok(Value::int(int(&v, span)?.checked_neg().ok_or_else(overflow)?)),
        UnaryOp::NegFloat => Ok(Value::float(-float(&v, span)?)),
        #[cfg(feature = "decimal")]
        UnaryOp::NegDecimal => Ok(Value(Repr::Decimal(
            decimal(&v, span)?.checked_mul(rust_decimal::Decimal::NEGATIVE_ONE).ok_or_else(overflow)?,
        ))),
        UnaryOp::NegDuration => Ok(Value(Repr::Duration(duration(&v, span)?.checked_neg().ok_or_else(overflow)?))),
        UnaryOp::Dyn(op) => {
            // `+x` is `x` for every type `-x` accepts.
            let lookup = if op == ast::UnaryOp::Plus { ast::UnaryOp::Neg } else { op };
            let expected = if op == ast::UnaryOp::Not { "bool" } else { "a number" };
            match resolve::unary(lookup, v.tag()) {
                Some(_) if op == ast::UnaryOp::Plus => Ok(v),
                Some((op, _)) => unary(op, v, span),
                None => Err(mismatch(expected, &v, span)),
            }
        }
    }
}

/// `l == r` by `kind`.
fn equal(kind: EqKind, l: &Value<'_>, r: &Value<'_>, f: &mut Frame<'_>, span: Span) -> Result<bool, Fault> {
    Ok(match kind {
        EqKind::Bool => boolean(l, span)? == boolean(r, span)?,
        EqKind::Int => int(l, span)? == int(r, span)?,
        #[allow(clippy::float_cmp)]
        EqKind::Float => float(l, span)? == float(r, span)?,
        #[cfg(feature = "decimal")]
        EqKind::Decimal => decimal(l, span)? == decimal(r, span)?,
        EqKind::String => string(l, span)? == string(r, span)?,
        EqKind::Bytes => l.as_bytes() == r.as_bytes(),
        EqKind::Time => time(l, span)? == time(r, span)?,
        EqKind::Duration => duration(l, span)? == duration(r, span)?,
        EqKind::Structural => deep_equal(l, r, f, span)?,
        EqKind::Dyn => {
            let Some((kind, pl, pr)) = resolve::equality(l.tag(), r.tag()) else {
                return Err(Frame::fault(
                    EvalErrorKind::TypeMismatch { expected: l.tag().name(), found: r.tag().name() },
                    span,
                ));
            };
            let (l, r) = (promote(l.clone(), pl, span)?, promote(r.clone(), pr, span)?);
            equal(kind, &l, &r, f, span)?
        }
    })
}

/// Structural equality: `nil` equals only `nil`, arrays element-wise, maps by key
/// regardless of order. Values that cannot be compared are unequal (as Go's `==` on
/// interfaces of different types). One step per element compared.
fn deep_equal(l: &Value<'_>, r: &Value<'_>, f: &mut Frame<'_>, span: Span) -> Result<bool, Fault> {
    let Some((kind, pl, pr)) = resolve::equality(l.tag(), r.tag()) else { return Ok(false) };
    if kind != EqKind::Structural {
        let (l, r) = (promote(l.clone(), pl, span)?, promote(r.clone(), pr, span)?);
        return equal(kind, &l, &r, f, span);
    }
    match (&l.0, &r.0) {
        (Repr::Nil, Repr::Nil) => Ok(true),
        (Repr::Array(a), Repr::Array(b)) => {
            if a.len() != b.len() {
                return Ok(false);
            }
            steps(f, a.len(), span)?;
            for (x, y) in a.iter().zip(b.iter()) {
                if !deep_equal(&x, &y, f, span)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        (Repr::Map(a), Repr::Map(b)) => {
            if a.len() != b.len() {
                return Ok(false);
            }
            steps(f, a.len(), span)?;
            let mut entries = Vec::with_capacity(a.len());
            a.for_each(&mut |k, v| {
                entries.push((String::from(k), v));
                ControlFlow::Continue(())
            });
            for (k, x) in entries {
                match b.get(&k) {
                    Some(y) if deep_equal(&x, &y, f, span)? => {}
                    _ => return Ok(false),
                }
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// The ordering of `l` and `r` by `kind`; `None` when unordered (a NaN).
fn order(kind: CompareKind, l: &Value<'_>, r: &Value<'_>, span: Span) -> Result<Option<Ordering>, Fault> {
    Ok(match kind {
        CompareKind::Int => Some(int(l, span)?.cmp(&int(r, span)?)),
        CompareKind::Float => float(l, span)?.partial_cmp(&float(r, span)?),
        #[cfg(feature = "decimal")]
        CompareKind::Decimal => Some(decimal(l, span)?.cmp(&decimal(r, span)?)),
        CompareKind::String => Some(string(l, span)?.cmp(string(r, span)?)),
        CompareKind::Time => Some(time(l, span)?.cmp(&time(r, span)?)),
        CompareKind::Duration => Some(duration(l, span)?.cmp(&duration(r, span)?)),
        CompareKind::Dyn => {
            let Some((kind, pl, pr)) = resolve::ordering(l.tag(), r.tag()) else {
                return Err(Frame::fault(
                    EvalErrorKind::TypeMismatch { expected: l.tag().name(), found: r.tag().name() },
                    span,
                ));
            };
            let (l, r) = (promote(l.clone(), pl, span)?, promote(r.clone(), pr, span)?);
            order(kind, &l, &r, span)?
        }
    })
}

fn membership(op: InOp, n: &Value<'_>, h: &Value<'_>, f: &mut Frame<'_>, span: Span) -> Result<bool, Fault> {
    match op {
        InOp::Array(kind) => {
            let s = seq(h, span)?;
            steps(f, s.len(), span)?;
            for x in s.iter() {
                if equal(kind, n, &x, f, span)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        InOp::MapKey => Ok(map(h, span)?.get(string(n, span)?).is_some()),
        InOp::Range => match &h.0 {
            Repr::Array(Seq::Range { from, to }) => Ok((*from..=*to).contains(&int(n, span)?)),
            _ => Err(internal("expected a range", span)),
        },
        InOp::Dyn => match &h.0 {
            Repr::Array(s) => {
                steps(f, s.len(), span)?;
                for x in s.iter() {
                    if deep_equal(n, &x, f, span)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Repr::Map(m) => Ok(m.get(n.as_str().ok_or_else(|| mismatch("string", n, span))?).is_some()),
            _ => Err(mismatch("an array or map", h, span)),
        },
    }
}

fn index_value<'a>(op: IndexOp, b: &Value<'a>, i: &Value<'a>, span: Span) -> Res<'a> {
    match op {
        IndexOp::Array => {
            let s = seq(b, span)?;
            let at = ops::index(int(i, span)?, s.len()).map_err(at(span))?;
            s.get(at).ok_or_else(|| internal("sequence shorter than its length", span))
        }
        IndexOp::Map => Ok(map(b, span)?.get(string(i, span)?).unwrap_or(Value::NIL)),
        IndexOp::String => {
            let c = ops::index_str(string(b, span)?, int(i, span)?).map_err(at(span))?;
            Ok(Value(Repr::Str(Str::Shared(c.into()))))
        }
        IndexOp::Bytes => {
            let bytes = b.as_bytes().ok_or_else(|| internal("expected bytes", span))?;
            let at = ops::index(int(i, span)?, bytes.len()).map_err(at(span))?;
            Ok(Value::int(bytes.get(at).copied().map(i64::from).unwrap_or_default()))
        }
        IndexOp::Dyn => match (&b.0, &i.0) {
            (Repr::Array(_), Repr::Int(_)) => index_value(IndexOp::Array, b, i, span),
            (Repr::Str(_), Repr::Int(_)) => index_value(IndexOp::String, b, i, span),
            (Repr::Bytes(_), Repr::Int(_)) => index_value(IndexOp::Bytes, b, i, span),
            (Repr::Map(_), Repr::Str(_)) => index_value(IndexOp::Map, b, i, span),
            // An unknown name on a struct reads as nil, as a missing map key does.
            (Repr::Object(o), Repr::Str(name)) => {
                Ok(o.desc().field(name.as_str()).map_or(Value::NIL, |(id, _)| o.field(id)))
            }
            (Repr::Array(_) | Repr::Str(_) | Repr::Bytes(_), _) => Err(mismatch("int", i, span)),
            (Repr::Map(_) | Repr::Object(_), _) => Err(mismatch("string", i, span)),
            _ => Err(mismatch("an array, map, string, bytes or struct", b, span)),
        },
    }
}

fn slice_value<'a>(
    op: SliceOp,
    b: &Value<'a>,
    (from, to): (Option<i64>, Option<i64>),
    spans: (Span, Span),
    f: &mut Frame<'a>,
    span: Span,
) -> Res<'a> {
    match op {
        SliceOp::Array => {
            let s = seq(b, span)?;
            let r = ops::slice(from, to, s.len());
            alloc(f, r.len(), span)?;
            let mut out = Vec::with_capacity(r.len());
            for i in r {
                out.push(s.get(i).ok_or_else(|| internal("sequence shorter than its length", span))?);
            }
            Ok(Value::array(out))
        }
        SliceOp::String => {
            let s = string(b, span)?;
            let r = ops::slice_str(s, from, to).map_err(|kind| {
                let start = ops::slice(from, to, s.len()).start;
                let bound = match kind {
                    EvalErrorKind::CharBoundary { index } if usize::try_from(index) == Ok(start) => spans.0,
                    _ => spans.1,
                };
                Frame::fault(kind, bound)
            })?;
            match &b.0 {
                // A slice of a borrowed string borrows too.
                Repr::Str(Str::Borrowed(s)) => Ok(Value(Repr::Str(Str::Borrowed(s.get(r).unwrap_or(""))))),
                _ => built(f, s.get(r).unwrap_or("").into(), span),
            }
        }
        SliceOp::Bytes => {
            let bytes = b.as_bytes().ok_or_else(|| internal("expected bytes", span))?;
            let r = ops::slice(from, to, bytes.len());
            match &b.0 {
                Repr::Bytes(Bytes::Borrowed(bytes)) => {
                    Ok(Value(Repr::Bytes(Bytes::Borrowed(bytes.get(r).unwrap_or(&[])))))
                }
                _ => {
                    alloc(f, r.len(), span)?;
                    Ok(Value(Repr::Bytes(Bytes::Shared(bytes.get(r).unwrap_or(&[]).into()))))
                }
            }
        }
        SliceOp::Dyn => match &b.0 {
            Repr::Array(_) => slice_value(SliceOp::Array, b, (from, to), spans, f, span),
            Repr::Str(_) => slice_value(SliceOp::String, b, (from, to), spans, f, span),
            Repr::Bytes(_) => slice_value(SliceOp::Bytes, b, (from, to), spans, f, span),
            _ => Err(mismatch("an array, string or bytes", b, span)),
        },
    }
}

/// `Conversion::Check`: `v` (of type `any`) as a value of `ty`, all the way down,
/// one step per container element. An `int` where a `float` is expected is
/// promoted, as the checker does statically.
fn check<'a>(v: Value<'a>, ty: &Type, f: &mut Frame<'a>, span: Span) -> Res<'a> {
    let expected = || ty.non_optional().tag().map_or("a value", Tag::name);
    match ty {
        Type::Any => Ok(v),
        Type::Option(_) | Type::Nil if v.is_nil() => Ok(v),
        Type::Option(o) => check(v, o.inner(), f, span),
        Type::Float if v.as_int().is_some() => promote(v, Promote::IntToFloat, span),
        Type::Array(t) => {
            let s = v.as_seq().cloned().ok_or_else(|| mismatch(expected(), &v, span))?;
            if **t == Type::Any {
                return Ok(v);
            }
            steps(f, s.len(), span)?;
            let mut out = Vec::with_capacity(s.len());
            let mut changed = false;
            for x in s.iter() {
                let tag = x.tag();
                let y = check(x, t, f, span)?;
                changed |= y.tag() != tag;
                out.push(y);
            }
            if changed {
                alloc(f, out.len(), span)?;
                Ok(Value::array(out))
            } else {
                Ok(v)
            }
        }
        Type::Map(t) => {
            let m = v.as_map().cloned().ok_or_else(|| mismatch(expected(), &v, span))?;
            if **t == Type::Any {
                return Ok(v);
            }
            steps(f, m.len(), span)?;
            let mut entries: Vec<(Arc<str>, Value<'a>)> = Vec::with_capacity(m.len());
            m.for_each(&mut |k, x| {
                entries.push((k.into(), x));
                ControlFlow::Continue(())
            });
            let mut out = Vec::with_capacity(entries.len());
            let mut changed = false;
            for (k, x) in entries {
                let tag = x.tag();
                let y = check(x, t, f, span)?;
                changed |= y.tag() != tag;
                out.push((k, y));
            }
            if changed {
                alloc(f, out.len(), span)?;
                Ok(Value(Repr::Map(MapRef::Owned(out.into()))))
            } else {
                Ok(v)
            }
        }
        Type::Struct(d) => match &v.0 {
            Repr::Object(o) if core::ptr::eq(*d, o.desc()) => Ok(v),
            _ => Err(mismatch(d.name(), &v, span)),
        },
        _ if ty.tag() == Some(v.tag()) => Ok(v),
        _ => Err(mismatch(expected(), &v, span)),
    }
}

/// A builtin applied to evaluated arguments.
fn intrinsic<'a>(op: Intrinsic, args: &[Value<'a>], f: &mut Frame<'a>, span: Span) -> Res<'a> {
    let arg = |i: usize| args.get(i).ok_or_else(|| internal("missing builtin argument", span));
    let s = |i: usize| arg(i).and_then(|v| string(v, span));
    let overflow = || Frame::fault(EvalErrorKind::Overflow, span);
    let invalid = |what: &'static str, value: &str| {
        Frame::fault(EvalErrorKind::InvalidArgument { what, value: value.into() }, span)
    };
    let count = |n: usize| Value::int(i64::try_from(n).unwrap_or(i64::MAX));
    match op {
        Intrinsic::LenString => Ok(Value::int(ops::len_str(s(0)?))),
        Intrinsic::LenBytes => Ok(count(arg(0)?.as_bytes().ok_or_else(|| internal("expected bytes", span))?.len())),
        Intrinsic::LenArray => Ok(count(seq(arg(0)?, span)?.len())),
        Intrinsic::LenMap => Ok(count(map(arg(0)?, span)?.len())),
        Intrinsic::Trim => {
            let (v, t) = (arg(0)?, s(0)?.trim());
            match &v.0 {
                Repr::Str(Str::Borrowed(b)) => Ok(Value(Repr::Str(Str::Borrowed(b.trim())))),
                _ => built(f, t.into(), span),
            }
        }
        Intrinsic::Upper => built(f, simple_case(s(0)?, true), span),
        Intrinsic::Lower => built(f, simple_case(s(0)?, false), span),
        Intrinsic::Split => {
            let (text, sep) = (s(0)?, s(1)?);
            // As Go's strings.Split: an empty separator splits into characters.
            let parts: Vec<&str> = if sep.is_empty() {
                text.char_indices().map(|(i, c)| text.get(i..i.saturating_add(c.len_utf8())).unwrap_or("")).collect()
            } else {
                text.split(sep).collect()
            };
            alloc(f, parts.len().saturating_add(text.len()), span)?;
            Ok(Value::array(parts.into_iter().map(|p| Value(Repr::Str(Str::Shared(p.into())))).collect()))
        }
        Intrinsic::Replace => {
            let (text, from, to) = (s(0)?, s(1)?, s(2)?);
            let hits =
                if from.is_empty() { text.chars().count().saturating_add(1) } else { text.matches(from).count() };
            let len = text.len().saturating_add(hits.saturating_mul(to.len()));
            alloc(f, len, span)?;
            Ok(Value::string(text.replace(from, to)))
        }
        Intrinsic::HasPrefix => Ok(Value::bool(s(0)?.starts_with(s(1)?))),
        Intrinsic::HasSuffix => Ok(Value::bool(s(0)?.ends_with(s(1)?))),
        Intrinsic::IndexOf => Ok(s(0)?.find(s(1)?).map_or(Value::int(-1), count)),
        Intrinsic::AbsInt => Ok(Value::int(int(arg(0)?, span)?.checked_abs().ok_or_else(overflow)?)),
        Intrinsic::AbsFloat => Ok(Value::float(libm::fabs(float(arg(0)?, span)?))),
        Intrinsic::Ceil => Ok(Value::float(libm::ceil(float(arg(0)?, span)?))),
        Intrinsic::Floor => Ok(Value::float(libm::floor(float(arg(0)?, span)?))),
        Intrinsic::Round => Ok(Value::float(libm::round(float(arg(0)?, span)?))),
        Intrinsic::Extremum { kind, max, of_array } => {
            let pick = |best: &Value<'a>, x: &Value<'a>| -> Result<bool, Fault> {
                let o = order(kind, x, best, span)?;
                Ok(o == Some(if max { Ordering::Greater } else { Ordering::Less }))
            };
            if of_array {
                let xs = seq(arg(0)?, span)?;
                steps(f, xs.len(), span)?;
                let mut best: Option<Value<'a>> = None;
                for x in xs.iter() {
                    best = Some(match best {
                        Some(b) if !pick(&b, &x)? => b,
                        _ => x,
                    });
                }
                Ok(best.unwrap_or(Value::NIL))
            } else {
                let mut best = arg(0)?.clone();
                for x in args.get(1..).unwrap_or(&[]) {
                    if pick(&best, x)? {
                        best = x.clone();
                    }
                }
                Ok(best)
            }
        }
        Intrinsic::First => Ok(seq(arg(0)?, span)?.get(0).unwrap_or(Value::NIL)),
        Intrinsic::Last => {
            let xs = seq(arg(0)?, span)?;
            Ok(xs.len().checked_sub(1).and_then(|i| xs.get(i)).unwrap_or(Value::NIL))
        }
        Intrinsic::SumInt | Intrinsic::SumFloat => {
            let xs = seq(arg(0)?, span)?;
            steps(f, xs.len(), span)?;
            sum(xs, span)
        }
        Intrinsic::Join => {
            let xs = seq(arg(0)?, span)?;
            let sep = if args.len() > 1 { s(1)? } else { "" };
            steps(f, xs.len(), span)?;
            let mut out = String::new();
            for (i, x) in xs.iter().enumerate() {
                if i > 0 {
                    out.push_str(sep);
                }
                out.push_str(x.as_str().ok_or_else(|| mismatch("string", &x, span))?);
            }
            built(f, out, span)
        }
        Intrinsic::Keys | Intrinsic::Values => {
            let m = map(arg(0)?, span)?;
            steps(f, m.len(), span)?;
            alloc(f, m.len(), span)?;
            let mut out = Vec::with_capacity(m.len());
            m.for_each(&mut |k, v| {
                out.push(if op == Intrinsic::Keys { Value(Repr::Str(Str::Shared(k.into()))) } else { v });
                ControlFlow::Continue(())
            });
            Ok(Value::array(out))
        }
        Intrinsic::ToInt(_) => {
            let v = arg(0)?;
            match &v.0 {
                Repr::Int(n) => Ok(Value::int(*n)),
                Repr::Float(x) => {
                    // Truncation toward zero, as Go's int(x); out of range is an error.
                    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
                    let fits = x.is_finite() && *x > -9_223_372_036_854_777_856.0 && *x < 9_223_372_036_854_775_808.0;
                    #[allow(clippy::cast_possible_truncation)]
                    let n = *x as i64;
                    if fits { Ok(Value::int(n)) } else { Err(overflow()) }
                }
                Repr::Str(t) => t.as_str().parse::<i64>().map(Value::int).map_err(|_| invalid("int", t.as_str())),
                _ => Err(mismatch("int, float or string", v, span)),
            }
        }
        Intrinsic::ToFloat(_) => {
            let v = arg(0)?;
            match &v.0 {
                #[allow(clippy::cast_precision_loss)]
                Repr::Int(n) => Ok(Value::float(*n as f64)),
                Repr::Float(x) => Ok(Value::float(*x)),
                Repr::Str(t) => t.as_str().parse::<f64>().map(Value::float).map_err(|_| invalid("float", t.as_str())),
                _ => Err(mismatch("int, float or string", v, span)),
            }
        }
        Intrinsic::ToString(_) => {
            let v = arg(0)?;
            if let Repr::Str(_) = v.0 {
                return Ok(v.clone());
            }
            let mut out = String::new();
            go_string(v, &mut out, f, span)?;
            built(f, out, span)
        }
        #[cfg(feature = "decimal")]
        Intrinsic::ToDecimal(_) => {
            use rust_decimal::Decimal;
            let v = arg(0)?;
            let d = match &v.0 {
                Repr::Int(n) => Decimal::from(*n),
                Repr::Float(x) => Decimal::try_from(*x).map_err(|_| invalid("decimal", "float"))?,
                Repr::Str(t) => t.as_str().parse::<Decimal>().map_err(|_| invalid("decimal", t.as_str()))?,
                Repr::Decimal(d) => *d,
                _ => return Err(mismatch("int, float, string or decimal", v, span)),
            };
            Ok(Value(Repr::Decimal(d)))
        }
        Intrinsic::TypeName => Ok(Value(Repr::Str(Str::Shared(arg(0)?.tag().name().into())))),
        Intrinsic::Now => {
            f.now.map(|t| Value(Repr::Time(t))).ok_or_else(|| Frame::fault(EvalErrorKind::ClockUnset, span))
        }
        Intrinsic::ParseDuration => Ok(Value(Repr::Duration(builtins::parse_go_duration(s(0)?).map_err(at(span))?))),
        Intrinsic::ParseDate => Ok(Value(Repr::Time(builtins::parse_date(s(0)?).map_err(at(span))?))),
        Intrinsic::GetArray => {
            let xs = seq(arg(0)?, span)?;
            Ok(ops::index(int(arg(1)?, span)?, xs.len()).ok().and_then(|i| xs.get(i)).unwrap_or(Value::NIL))
        }
        Intrinsic::GetMap => Ok(map(arg(0)?, span)?.get(s(1)?).unwrap_or(Value::NIL)),
        Intrinsic::Dyn(b) => {
            // `ceil`, `floor` and `round` of an int: the int as a float, as the checker does.
            let rounding = matches!(b, Builtin::Ceil | Builtin::Floor | Builtin::Round);
            let mut args: Vec<Value<'a>> = args.to_vec();
            if rounding {
                for a in &mut args {
                    if a.as_int().is_some() {
                        *a = promote(a.clone(), Promote::IntToFloat, span)?;
                    }
                }
            }
            let tags: Vec<Tag> = args.iter().map(Value::tag).collect();
            let Some((op, _)) = resolve::builtin(b, &tags) else {
                let found =
                    args.iter().find(|a| a.tag() != Tag::String).or(args.first()).map_or("nothing", |a| a.tag().name());
                return Err(Frame::fault(EvalErrorKind::TypeMismatch { expected: b.name(), found }, span));
            };
            let op = match op {
                Intrinsic::Extremum { kind: CompareKind::Float, max, of_array: false } => {
                    for a in &mut args {
                        *a = promote(
                            a.clone(),
                            if a.as_int().is_some() { Promote::IntToFloat } else { Promote::None },
                            span,
                        )?;
                    }
                    Intrinsic::Extremum { kind: CompareKind::Float, max, of_array: false }
                }
                Intrinsic::Extremum { max, of_array: true, .. } => {
                    Intrinsic::Extremum { kind: CompareKind::Dyn, max, of_array: true }
                }
                other => other,
            };
            intrinsic(op, &args, f, span)
        }
    }
}

/// `sum`: an int while every element is an int, a float once one is a float.
fn sum<'a>(xs: &Seq<'a>, span: Span) -> Res<'a> {
    let mut total = Value::int(0);
    for x in xs.iter() {
        total = match (&total.0, &x.0) {
            (Repr::Int(a), Repr::Int(b)) => {
                Value::int(a.checked_add(*b).ok_or_else(|| Frame::fault(EvalErrorKind::Overflow, span))?)
            }
            #[allow(clippy::cast_precision_loss)]
            (Repr::Int(a), Repr::Float(b)) => Value::float(*a as f64 + b),
            #[allow(clippy::cast_precision_loss)]
            (Repr::Float(a), Repr::Int(b)) => Value::float(a + *b as f64),
            (Repr::Float(a), Repr::Float(b)) => Value::float(a + b),
            _ => return Err(mismatch("a number", &x, span)),
        };
    }
    Ok(total)
}

/// Go's `unicode.ToUpper`/`ToLower` per character: a character whose case mapping
/// is more than one character (`ß`) stays as it is.
fn simple_case(s: &str, upper: bool) -> String {
    s.chars()
        .map(|c| {
            let mut mapped = alloc::vec::Vec::new();
            if upper {
                mapped.extend(c.to_uppercase())
            } else {
                mapped.extend(c.to_lowercase())
            }
            match mapped.as_slice() {
                [one] => *one,
                _ => c,
            }
        })
        .collect()
}

/// `string(x)`: Go's `fmt.Sprint(x)`. One step per container element.
fn go_string(v: &Value<'_>, out: &mut String, f: &mut Frame<'_>, span: Span) -> Result<(), Fault> {
    use core::fmt::Write;
    match &v.0 {
        Repr::Nil => out.push_str("<nil>"),
        Repr::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Repr::Int(n) => {
            let _ = write!(out, "{n}");
        }
        Repr::Float(x) => builtins::go_float(*x, out),
        #[cfg(feature = "decimal")]
        Repr::Decimal(d) => {
            let _ = write!(out, "{d}");
        }
        Repr::Str(s) => out.push_str(s.as_str()),
        Repr::Bytes(b) => out.push_str(&String::from_utf8_lossy(b.as_slice())),
        Repr::Time(t) => builtins::go_time(*t, out),
        Repr::Duration(d) => builtins::go_duration(*d, out),
        Repr::Array(s) => {
            steps(f, s.len(), span)?;
            out.push('[');
            for (i, x) in s.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                go_string(&x, out, f, span)?;
            }
            out.push(']');
        }
        Repr::Map(m) => {
            steps(f, m.len(), span)?;
            // Go prints map keys sorted.
            let mut entries: Vec<(String, Value<'_>)> = Vec::with_capacity(m.len());
            m.for_each(&mut |k, x| {
                entries.push((k.into(), x));
                ControlFlow::Continue(())
            });
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            out.push_str("map[");
            for (i, (k, x)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                out.push_str(k);
                out.push(':');
                go_string(x, out, f, span)?;
            }
            out.push(']');
        }
        Repr::Object(o) => {
            let fields = o.desc().fields();
            steps(f, fields.len(), span)?;
            out.push('{');
            for i in 0..fields.len() {
                if i > 0 {
                    out.push(' ');
                }
                let id = u16::try_from(i).map_err(|_| internal("too many fields", span))?;
                go_string(&o.field(dictum_check::FieldId::new(id)), out, f, span)?;
            }
            out.push('}');
        }
    }
    Ok(())
}
