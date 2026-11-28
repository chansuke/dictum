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
//! Budget charges (the closure tree makes the same ones by calling the same functions,
//! `frame::steps` and `frame::alloc`;
//! only a constant it folded at compile time costs it nothing):
//! - one step per element a predicate visits, before its body runs (the first
//!   element of a `reduce` without an initial value included);
//! - `len` steps for `x in array`, before scanning (so a sorted constant set costs
//!   what a scan does), and one per entry for `keys`, `values`, `sum`, `join`,
//!   `max`/`min`/`mean`/`median` of an array, `concat`, `flatten`, `take`,
//!   `reverse`, `toPairs`, `fromPairs`, structural `==`, and the deep check of
//!   `any`; one per value `toJSON` writes or `fromJSON` reads; for `uniq`, one per
//!   element and one per element kept before it; for `sort` and the sorting of
//!   `sortBy`, `n * (log2(n) + 1)`;
//! - one alloc per element of a built array or map (literals, `filter`, `map`,
//!   `split`, `keys`, `values`, `+` of arrays, `concat`, `flatten`, `uniq`, `take`,
//!   `reverse`, `sort`, `sortBy`, `fromPairs`, `fromJSON`, the numbers `median`
//!   sorts, for `groupBy` one per element and one per group, and for `toPairs` three
//!   per entry) and one per byte of a built string, before building it. Constants
//!   are part of the rule and cost nothing.

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
use crate::engine::frame::{Fault, Frame, Res, Scope, Text, alloc, at, deeper, internal, steps};
use crate::engine::json;
use crate::engine::methods;
use crate::engine::ops;
use crate::engine::sprint::go_string;
use crate::engine::trace::{Recorded, Trace};
use crate::engine::value::{Bytes, Depth, MAX_VALUE_DEPTH, MapRef, Repr, Seq, Str, Value};
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

pub(super) fn mismatch(expected: &'static str, found: &Value<'_>, span: Span) -> Fault {
    Frame::fault(EvalErrorKind::TypeMismatch { expected, found: found.tag().name() }, span)
}

// Typed reads. The checker proved the types, so a mismatch is a dictum bug.
pub(super) fn int(v: &Value<'_>, span: Span) -> Result<i64, Fault> {
    v.as_int().ok_or_else(|| internal("expected an int", span))
}
pub(super) fn float(v: &Value<'_>, span: Span) -> Result<f64, Fault> {
    v.as_float().ok_or_else(|| internal("expected a float", span))
}
pub(super) fn boolean(v: &Value<'_>, span: Span) -> Result<bool, Fault> {
    v.as_bool().ok_or_else(|| internal("expected a bool", span))
}
pub(super) fn string<'v>(v: &'v Value<'_>, span: Span) -> Result<&'v str, Fault> {
    v.as_str().ok_or_else(|| internal("expected a string", span))
}
fn time(v: &Value<'_>, span: Span) -> Result<jiff::Timestamp, Fault> {
    v.as_time().ok_or_else(|| internal("expected a time", span))
}
fn zoned<'v>(v: &'v Value<'_>, span: Span) -> Result<(jiff::Timestamp, &'v jiff::tz::TimeZone), Fault> {
    v.as_zoned().ok_or_else(|| internal("expected a time", span))
}
fn location<'v>(v: &'v Value<'_>, span: Span) -> Result<&'v jiff::tz::TimeZone, Fault> {
    v.as_location().ok_or_else(|| internal("expected a location", span))
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

pub(super) fn promote<'a>(v: Value<'a>, p: Promote, span: Span) -> Res<'a> {
    match p {
        Promote::None => Ok(v),
        #[expect(clippy::cast_precision_loss, reason = "int to float rounds to nearest, as Go's float64(n)")]
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
                index_value(*op, &b, &i, f, span)
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
                binary(*op, l, r, self.patterns.get(&e.id()).map(|re| &**re), f, span)
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
                    if !holds(*op, order(*kind, &prev, &cur, span)?) {
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
                Ok(Value::map(out))
            }
            ExprKind::Range { from, to } => {
                let a = self.expr(from, f)?;
                let b = self.expr(to, f)?;
                Ok(Value(Repr::Array(Seq::Range { from: int(&a, from.span())?, to: int(&b, to.span())? })))
            }
            ExprKind::Predicate { op, seq, arg, body } => {
                let s = self.expr(seq, f)?;
                let s = s.as_seq().cloned().ok_or_else(|| mismatch("an array", &s, seq.span()))?;
                let arg = arg.as_deref().map(|a| self.expr(a, f)).transpose()?;
                match body.as_deref() {
                    Some(b) => predicate(*op, &s, arg, Some(&mut |f: &mut Frame<'a>| self.expr(b, f)), f, span),
                    None => predicate(*op, &s, arg, None, f, span),
                }
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
                (imp.call)(&vals).map_err(|err| call_fault(err, self.schema.func_name(*func).unwrap_or("?"), span))
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
}

/// Whether `op` holds for operands ordered `ord`; unordered (a NaN) never holds.
pub(super) fn holds(op: CompareOp, ord: Option<Ordering>) -> bool {
    match (op, ord) {
        (_, None) => false,
        (CompareOp::Lt, Some(o)) => o == Ordering::Less,
        (CompareOp::Le, Some(o)) => o != Ordering::Greater,
        (CompareOp::Gt, Some(o)) => o == Ordering::Greater,
        (CompareOp::Ge, Some(o)) => o != Ordering::Less,
    }
}

/// The fault for a failed call of the schema function `name`.
pub(super) fn call_fault(err: CallError, name: &str, span: Span) -> Fault {
    match err {
        CallError::Function(error) => Frame::fault(EvalErrorKind::Function { name: name.into(), error }, span),
        CallError::BadArgument => internal("function argument did not convert", span),
    }
}

/// A strict binary operation on evaluated operands. `pattern` is the compiled
/// constant pattern of a `matches`, if any.
pub(super) fn binary<'a>(
    op: BinaryOp,
    l: Value<'a>,
    r: Value<'a>,
    pattern: Option<&regex::Regex>,
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
            binary(b.op, l, r, pattern, f, span)
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
            let mut s = Text::new();
            s.reserve(f, a.len().saturating_add(b.len()), span)?;
            s.push_str(f, a, span)?;
            s.push_str(f, b, span)?;
            Ok(s.into_value())
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
            let ((t, z), d) = (zoned(&l, span)?, duration(&r, span)?);
            let d = if op == BinaryOp::TimeSubDuration { d.checked_neg() } else { Some(d) };
            let t = d.and_then(|d| t.checked_add(d).ok()).ok_or_else(|| Frame::fault(EvalErrorKind::Overflow, span))?;
            Ok(Value(Repr::Time(t, z.clone())))
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
            if let Some(re) = pattern {
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

/// The element at `i` of a sequence being iterated.
fn nth<'a>(s: &Seq<'a>, i: usize, span: Span) -> Res<'a> {
    s.get(i).ok_or_else(|| internal("sequence shorter than its length", span))
}

/// `i` as an `#index`.
fn position(i: usize, span: Span) -> Result<i64, Fault> {
    i64::try_from(i).map_err(|_| internal("index past i64", span))
}

/// The body of a predicate, run with one element's scope pushed.
pub(super) type Body<'b, 'a> = &'b mut dyn FnMut(&mut Frame<'a>) -> Res<'a>;

/// Run `body` for the element at `index`, with `#acc` set to `acc`.
fn visit<'a>(
    body: &mut Body<'_, 'a>,
    s: &Seq<'a>,
    i: usize,
    acc: Option<Value<'a>>,
    f: &mut Frame<'a>,
    span: Span,
) -> Res<'a> {
    steps(f, 1, span)?;
    let current = nth(s, i, span)?;
    f.scopes.push(Scope { current, index: position(i, span)?, acc });
    let r = body(f);
    f.scopes.pop();
    r
}

/// A predicate over `s`. `body` runs once per visited element with that element's
/// scope pushed; without one (`all(bools)`) the element itself is the outcome. `arg`
/// is `sortBy`'s order or `reduce`'s initial `#acc`.
pub(super) fn predicate<'a>(
    op: PredicateOp,
    s: &Seq<'a>,
    arg: Option<Value<'a>>,
    body: Option<Body<'_, 'a>>,
    f: &mut Frame<'a>,
    span: Span,
) -> Res<'a> {
    let Some(mut body) = body else { return scan(op, s, None, f, span) };
    match op {
        PredicateOp::FindLast | PredicateOp::FindLastIndex => {
            for i in (0..s.len()).rev() {
                if boolean(&visit(&mut body, s, i, None, f, span)?, span)? {
                    return match op {
                        PredicateOp::FindLast => nth(s, i, span),
                        _ => Ok(Value::int(position(i, span)?)),
                    };
                }
            }
            Ok(if op == PredicateOp::FindLast { Value::NIL } else { Value::int(-1) })
        }
        PredicateOp::GroupBy => {
            let mut groups: Vec<(Arc<str>, Vec<Value<'a>>)> = Vec::new();
            let mut index: BTreeMap<Arc<str>, usize> = BTreeMap::new();
            for i in 0..s.len() {
                let key = visit(&mut body, s, i, None, f, span)?;
                let key = string(&key, span)?;
                let elem = nth(s, i, span)?;
                alloc(f, 1, span)?;
                match index.get(key).and_then(|&g| groups.get_mut(g)) {
                    Some((_, group)) => group.push(elem),
                    None => {
                        alloc(f, 1, span)?;
                        let key: Arc<str> = key.into();
                        index.insert(key.clone(), groups.len());
                        groups.push((key, alloc::vec![elem]));
                    }
                }
            }
            let groups: Vec<(Arc<str>, Value<'a>)> = groups.into_iter().map(|(k, xs)| (k, Value::array(xs))).collect();
            Ok(Value::map(groups))
        }
        PredicateOp::SortBy(kind) => {
            let desc = descending(arg.as_ref(), span)?;
            // Not sized from `s.len()`: a range is lazy, and its keys charge their steps
            // one by one before the sort charges the rest.
            let mut keyed: Vec<(Value<'a>, Value<'a>)> = Vec::new();
            for i in 0..s.len() {
                let key = visit(&mut body, s, i, None, f, span)?;
                keyed.push((key, nth(s, i, span)?));
            }
            charge_sort(keyed.len(), f, span)?;
            let keyed = sorted(keyed, |(key, _)| key, kind, desc, span)?;
            Ok(Value::array(keyed.into_iter().map(|(_, x)| x).collect()))
        }
        PredicateOp::Reduce => {
            let (mut acc, start) = match arg {
                Some(init) => (init, 0),
                None => {
                    steps(f, 1, span)?;
                    let first = s.get(0).ok_or_else(|| {
                        Frame::fault(
                            EvalErrorKind::InvalidArgument {
                                what: "array for reduce without an initial value",
                                value: "[]".into(),
                            },
                            span,
                        )
                    })?;
                    (first, 1)
                }
            };
            for i in start..s.len() {
                acc = visit(&mut body, s, i, Some(acc), f, span)?;
                // The one place a value can nest without bound: `[#acc]` each step.
                if acc.depth() > MAX_VALUE_DEPTH {
                    return Err(Frame::fault(EvalErrorKind::DepthLimit, span));
                }
            }
            Ok(acc)
        }
        op => scan(op, s, Some(body), f, span),
    }
}

/// The predicates that scan from the start and may stop early.
fn scan<'a>(op: PredicateOp, s: &Seq<'a>, mut body: Option<Body<'_, 'a>>, f: &mut Frame<'a>, span: Span) -> Res<'a> {
    let mut kept: Vec<Value<'a>> = Vec::new();
    let mut hits: i64 = 0;
    for i in 0..s.len() {
        let out = match body.as_mut() {
            Some(b) => visit(b, s, i, None, f, span)?,
            None => {
                steps(f, 1, span)?;
                nth(s, i, span)?
            }
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
            PredicateOp::Find => return nth(s, i, span),
            PredicateOp::FindIndex => {
                return Ok(Value::int(position(i, span)?));
            }
            PredicateOp::Filter => {
                alloc(f, 1, span)?;
                kept.push(nth(s, i, span)?);
            }
            PredicateOp::One if hits == 1 => return Ok(Value::bool(false)),
            PredicateOp::All | PredicateOp::One | PredicateOp::Count | PredicateOp::Map => {}
            PredicateOp::FindLast
            | PredicateOp::FindLastIndex
            | PredicateOp::GroupBy
            | PredicateOp::SortBy(_)
            | PredicateOp::Reduce => return Err(internal("predicate without a body", span)),
        }
        hits = hits.saturating_add(1);
    }
    Ok(match op {
        PredicateOp::All | PredicateOp::None => Value::bool(true),
        PredicateOp::Any => Value::bool(false),
        PredicateOp::One => Value::bool(hits == 1),
        PredicateOp::Filter | PredicateOp::Map => Value::array(kept),
        PredicateOp::Count => Value::int(hits),
        PredicateOp::Find | PredicateOp::FindLast => Value::NIL,
        PredicateOp::FindIndex | PredicateOp::FindLastIndex => Value::int(-1),
        PredicateOp::GroupBy | PredicateOp::SortBy(_) | PredicateOp::Reduce => {
            return Err(internal("predicate without a body", span));
        }
    })
}

pub(super) fn unary<'a>(op: UnaryOp, v: Value<'a>, span: Span) -> Res<'a> {
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
pub(super) fn equal(kind: EqKind, l: &Value<'_>, r: &Value<'_>, f: &mut Frame<'_>, span: Span) -> Result<bool, Fault> {
    Ok(match kind {
        EqKind::Bool => boolean(l, span)? == boolean(r, span)?,
        EqKind::Int => int(l, span)? == int(r, span)?,
        EqKind::Float => float(l, span)? == float(r, span)?,
        #[cfg(feature = "decimal")]
        EqKind::Decimal => decimal(l, span)? == decimal(r, span)?,
        EqKind::String => string(l, span)? == string(r, span)?,
        EqKind::Bytes => l.as_bytes() == r.as_bytes(),
        EqKind::Time => time(l, span)? == time(r, span)?,
        EqKind::Duration => duration(l, span)? == duration(r, span)?,
        EqKind::Structural => deep_equal(l, r, Depth::TOP, f, span)?,
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
pub(super) fn deep_equal(
    l: &Value<'_>,
    r: &Value<'_>,
    depth: Depth,
    f: &mut Frame<'_>,
    span: Span,
) -> Result<bool, Fault> {
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
                if !deep_equal(&x, &y, deeper(depth, span)?, f, span)? {
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
                    Some(y) if deep_equal(&x, &y, deeper(depth, span)?, f, span)? => {}
                    _ => return Ok(false),
                }
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// The ordering of `l` and `r` by `kind`; `None` when unordered (a NaN).
pub(super) fn order(kind: CompareKind, l: &Value<'_>, r: &Value<'_>, span: Span) -> Result<Option<Ordering>, Fault> {
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

pub(super) fn membership(op: InOp, n: &Value<'_>, h: &Value<'_>, f: &mut Frame<'_>, span: Span) -> Result<bool, Fault> {
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
                    if deep_equal(n, &x, Depth::TOP, f, span)? {
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

pub(super) fn index_value<'a>(op: IndexOp, b: &Value<'a>, i: &Value<'a>, f: &mut Frame<'a>, span: Span) -> Res<'a> {
    match op {
        IndexOp::Array => {
            let s = seq(b, span)?;
            let at = ops::index(int(i, span)?, s.len()).map_err(at(span))?;
            s.get(at).ok_or_else(|| internal("sequence shorter than its length", span))
        }
        IndexOp::Map => Ok(map(b, span)?.get(string(i, span)?).unwrap_or(Value::NIL)),
        IndexOp::String => {
            let c = ops::index_str(string(b, span)?, int(i, span)?).map_err(at(span))?;
            Text::copy(f, c, span)
        }
        IndexOp::Bytes => {
            let bytes = b.as_bytes().ok_or_else(|| internal("expected bytes", span))?;
            let at = ops::index(int(i, span)?, bytes.len()).map_err(at(span))?;
            Ok(Value::int(bytes.get(at).copied().map(i64::from).unwrap_or_default()))
        }
        IndexOp::Dyn => match (&b.0, &i.0) {
            (Repr::Array(_), Repr::Int(_)) => index_value(IndexOp::Array, b, i, f, span),
            (Repr::Str(_), Repr::Int(_)) => index_value(IndexOp::String, b, i, f, span),
            (Repr::Bytes(_), Repr::Int(_)) => index_value(IndexOp::Bytes, b, i, f, span),
            (Repr::Map(_), Repr::Str(_)) => index_value(IndexOp::Map, b, i, f, span),
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

pub(super) fn slice_value<'a>(
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
                _ => Text::copy(f, s.get(r).unwrap_or(""), span),
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
pub(super) fn check<'a>(v: Value<'a>, ty: &Type, f: &mut Frame<'a>, span: Span) -> Res<'a> {
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
                Ok(Value::map(out))
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

/// A part of the string `v`, borrowed from the facts when `v` is, else copied and charged.
fn narrowed<'a>(v: &Value<'a>, f: &mut Frame<'a>, span: Span, part: impl for<'x> Fn(&'x str) -> &'x str) -> Res<'a> {
    match &v.0 {
        Repr::Str(Str::Borrowed(b)) => Ok(Value(Repr::Str(Str::Borrowed(part(b))))),
        _ => Text::copy(f, part(string(v, span)?), span),
    }
}

/// A builtin applied to evaluated arguments.
pub(super) fn intrinsic<'a>(op: Intrinsic, args: &[Value<'a>], f: &mut Frame<'a>, span: Span) -> Res<'a> {
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
        Intrinsic::Trim => narrowed(arg(0)?, f, span, str::trim),
        Intrinsic::TrimCutset => {
            let cutset = s(1)?;
            narrowed(arg(0)?, f, span, |t| t.trim_matches(|c| cutset.contains(c)))
        }
        Intrinsic::TrimPrefix | Intrinsic::TrimSuffix => {
            let affix = if args.len() > 1 { s(1)? } else { " " };
            let prefix = op == Intrinsic::TrimPrefix;
            narrowed(arg(0)?, f, span, |t| {
                let stripped = if prefix { t.strip_prefix(affix) } else { t.strip_suffix(affix) };
                stripped.unwrap_or(t)
            })
        }
        Intrinsic::Upper => simple_case(s(0)?, true, f, span),
        Intrinsic::Lower => simple_case(s(0)?, false, f, span),
        Intrinsic::Split { after } => {
            let (text, sep) = (s(0)?, s(1)?);
            let n = if args.len() > 2 { int(arg(2)?, span)? } else { -1 };
            let parts = builtins::go_split(text, sep, after, n);
            alloc(f, parts.len(), span)?;
            let parts = parts.into_iter().map(|p| Text::copy(f, p, span)).collect::<Result<Vec<_>, _>>()?;
            Ok(Value::array(parts))
        }
        Intrinsic::Replace => {
            let (text, from, to) = (s(0)?, s(1)?, s(2)?);
            let hits =
                if from.is_empty() { text.chars().count().saturating_add(1) } else { text.matches(from).count() };
            let len = text.len().saturating_add(hits.saturating_mul(to.len()));
            let mut out = Text::new();
            out.reserve(f, len, span)?;
            out.write_reserved(span, |buf| {
                let mut last = 0;
                for (i, hit) in text.match_indices(from) {
                    buf.push_str(text.get(last..i).unwrap_or(""));
                    buf.push_str(to);
                    last = i.saturating_add(hit.len());
                }
                buf.push_str(text.get(last..).unwrap_or(""));
            })?;
            Ok(out.into_value())
        }
        Intrinsic::Repeat => {
            let (text, n) = (s(0)?, int(arg(1)?, span)?);
            if n < 0 {
                return Err(invalid("repeat count", &alloc::format!("{n}")));
            }
            let too_large = || Frame::fault(EvalErrorKind::AllocLimit, span);
            let n = usize::try_from(n).map_err(|_| too_large())?;
            let len = text.len().checked_mul(n).ok_or_else(too_large)?;
            let mut out = Text::new();
            out.reserve(f, len, span)?;
            if !text.is_empty() {
                out.write_reserved(span, |buf| (0..n).for_each(|_| buf.push_str(text)))?;
            }
            Ok(out.into_value())
        }
        Intrinsic::HasPrefix => Ok(Value::bool(s(0)?.starts_with(s(1)?))),
        Intrinsic::HasSuffix => Ok(Value::bool(s(0)?.ends_with(s(1)?))),
        Intrinsic::IndexOf => Ok(s(0)?.find(s(1)?).map_or(Value::int(-1), count)),
        Intrinsic::LastIndexOf => Ok(s(0)?.rfind(s(1)?).map_or(Value::int(-1), count)),
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
        Intrinsic::Center { median, of_array } => {
            let mut xs = if of_array {
                let xs = seq(arg(0)?, span)?;
                steps(f, xs.len(), span)?;
                xs.iter().map(|x| number(&x, span)).collect::<Result<Vec<f64>, Fault>>()?
            } else {
                args.iter().map(|x| number(x, span)).collect::<Result<Vec<f64>, Fault>>()?
            };
            if median {
                alloc(f, xs.len(), span)?;
                Ok(Value::float(builtins::median(&mut xs)))
            } else {
                Ok(Value::float(builtins::mean(&xs)))
            }
        }
        Intrinsic::Bit(op) => {
            let y = if args.len() > 1 { int(arg(1)?, span)? } else { 0 };
            Ok(Value::int(builtins::bit(op, int(arg(0)?, span)?, y).map_err(at(span))?))
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
            let mut out = Text::new();
            for (i, x) in xs.iter().enumerate() {
                let piece = x.as_str().ok_or_else(|| mismatch("string", &x, span))?;
                if i > 0 {
                    out.push_str(f, sep, span)?;
                }
                out.push_str(f, piece, span)?;
            }
            Ok(out.into_value())
        }
        Intrinsic::Concat => {
            let mut out = Vec::new();
            for xs in args {
                let xs = seq(xs, span)?;
                steps(f, xs.len(), span)?;
                alloc(f, xs.len(), span)?;
                out.extend(xs.iter());
            }
            Ok(Value::array(out))
        }
        Intrinsic::Flatten => {
            let mut out = Vec::new();
            flatten(seq(arg(0)?, span)?, &mut out, Depth::TOP, f, span)?;
            Ok(Value::array(out))
        }
        Intrinsic::Uniq(kind) => {
            let xs = seq(arg(0)?, span)?;
            steps(f, xs.len(), span)?;
            let mut out: Vec<Value<'a>> = Vec::new();
            for x in xs.iter() {
                steps(f, out.len(), span)?;
                let mut seen = false;
                for y in &out {
                    if equal(kind, &x, y, f, span)? {
                        seen = true;
                        break;
                    }
                }
                if !seen {
                    alloc(f, 1, span)?;
                    out.push(x);
                }
            }
            Ok(Value::array(out))
        }
        Intrinsic::Take => {
            let (xs, n) = (seq(arg(0)?, span)?, int(arg(1)?, span)?);
            if n < 0 {
                return Err(invalid("count", &alloc::format!("{n}")));
            }
            // Compared as an i64 first, so a count past `usize` (32-bit) takes everything.
            let n = usize::try_from(n).unwrap_or(usize::MAX).min(xs.len());
            steps(f, n, span)?;
            alloc(f, n, span)?;
            Ok(Value::array(xs.iter().take(n).collect()))
        }
        Intrinsic::Reverse => {
            let xs = seq(arg(0)?, span)?;
            steps(f, xs.len(), span)?;
            alloc(f, xs.len(), span)?;
            let mut out: Vec<Value<'a>> = xs.iter().collect();
            out.reverse();
            Ok(Value::array(out))
        }
        Intrinsic::Sort(kind) => {
            let xs = seq(arg(0)?, span)?;
            let desc = descending(args.get(1), span)?;
            charge_sort(xs.len(), f, span)?;
            Ok(Value::array(sorted(xs.iter().collect(), |x| x, kind, desc, span)?))
        }
        Intrinsic::Keys | Intrinsic::Values => {
            let m = map(arg(0)?, span)?;
            steps(f, m.len(), span)?;
            alloc(f, m.len(), span)?;
            let mut out = Vec::with_capacity(m.len());
            let mut failed = None;
            m.for_each(&mut |k, v| {
                let x = if op == Intrinsic::Keys { Text::copy(f, k, span) } else { Ok(v) };
                match x {
                    Ok(x) => {
                        out.push(x);
                        ControlFlow::Continue(())
                    }
                    Err(e) => {
                        failed = Some(e);
                        ControlFlow::Break(())
                    }
                }
            });
            match failed {
                Some(e) => Err(e),
                None => Ok(Value::array(out)),
            }
        }
        Intrinsic::ToPairs => {
            let m = map(arg(0)?, span)?;
            steps(f, m.len(), span)?;
            alloc(f, m.len().saturating_mul(3), span)?;
            let mut out = Vec::with_capacity(m.len());
            let mut failed = None;
            m.for_each(&mut |k, v| match Text::copy(f, k, span) {
                Ok(k) => {
                    out.push(Value::array(alloc::vec![k, v]));
                    ControlFlow::Continue(())
                }
                Err(e) => {
                    failed = Some(e);
                    ControlFlow::Break(())
                }
            });
            match failed {
                Some(e) => Err(e),
                None => Ok(Value::array(out)),
            }
        }
        Intrinsic::FromPairs => {
            let xs = seq(arg(0)?, span)?;
            steps(f, xs.len(), span)?;
            alloc(f, xs.len(), span)?;
            let mut out: BTreeMap<Arc<str>, Value<'a>> = BTreeMap::new();
            for x in xs.iter() {
                let text = |f: &mut Frame<'a>, v: &Value<'_>| -> Result<String, Fault> {
                    let mut out = Text::new();
                    go_string(v, &mut out, Depth::TOP, f, span)?;
                    Ok(out.as_str().into())
                };
                let Some((k, v)) = x.as_seq().filter(|p| p.len() == 2).and_then(|p| Some((p.get(0)?, p.get(1)?)))
                else {
                    return Err(invalid("pair", &text(f, &x)?));
                };
                let Some(k) = k.as_str() else {
                    return Err(invalid("pair key, not a string", &text(f, &k)?));
                };
                out.insert(k.into(), v);
            }
            let entries: Vec<(Arc<str>, Value<'a>)> = out.into_iter().collect();
            Ok(Value::map(entries))
        }
        Intrinsic::ToInt(_) => {
            let v = arg(0)?;
            match &v.0 {
                Repr::Int(n) => Ok(Value::int(*n)),
                Repr::Float(x) => {
                    // Truncation toward zero, as Go's int(x); out of range is an error.
                    let fits = x.is_finite() && *x > -9_223_372_036_854_777_856.0 && *x < 9_223_372_036_854_775_808.0;
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "used only when `fits` holds, so the cast truncates toward zero as Go's int(x)"
                    )]
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
                #[expect(clippy::cast_precision_loss, reason = "int to float rounds to nearest, as Go's float64(n)")]
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
            let mut out = Text::new();
            go_string(v, &mut out, Depth::TOP, f, span)?;
            Ok(out.into_value())
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
        Intrinsic::ToJson => Ok(json::encode(arg(0)?, f, span)?.into_value()),
        Intrinsic::FromJson => json::decode(s(0)?, f, span),
        Intrinsic::ToBase64 => {
            let text = s(0)?;
            let mut out = Text::new();
            out.reserve(f, text.len().div_ceil(3).saturating_mul(4), span)?;
            out.write_reserved(span, |buf| builtins::base64_encode(text.as_bytes(), buf))?;
            Ok(out.into_value())
        }
        Intrinsic::FromBase64 => {
            let text = s(0)?;
            let limit = usize::try_from(f.budget.alloc_left()).unwrap_or(usize::MAX);
            let bytes = builtins::base64_decode(text.as_bytes(), limit).map_err(|err| match err {
                builtins::Base64Error::Invalid(at) => invalid("base64", builtins::excerpt(text, at)),
                builtins::Base64Error::Limit => Frame::fault(EvalErrorKind::AllocLimit, span),
            })?;
            let decoded = String::from_utf8(bytes).map_err(|_| invalid("base64, not UTF-8 text", text))?;
            Text::copy(f, &decoded, span)
        }
        Intrinsic::TypeName => {
            // Go names a time and a duration by their package path, as its docs show
            // with `type(now()) == "time.Time"`.
            let name = match arg(0)?.tag() {
                Tag::Time => "time.Time",
                Tag::Duration => "time.Duration",
                t => t.name(),
            };
            Text::copy(f, name, span)
        }
        Intrinsic::Now => {
            let zone = if args.is_empty() { jiff::tz::TimeZone::UTC } else { location(arg(0)?, span)?.clone() };
            f.now.map(|t| Value(Repr::Time(t, zone))).ok_or_else(|| Frame::fault(EvalErrorKind::ClockUnset, span))
        }
        Intrinsic::LoadLocation => Ok(Value(Repr::Location(builtins::load_location(s(0)?).map_err(at(span))?))),
        Intrinsic::ParseDuration => Ok(Value(Repr::Duration(builtins::parse_go_duration(s(0)?).map_err(at(span))?))),
        Intrinsic::ParseDate => {
            // Go's argument forms: an optional location first, then the text, a layout
            // and a zone name that replaces the location.
            let (loc, rest) = match args.split_first() {
                Some((first, rest)) if first.as_location().is_some() => (location(first, span)?.clone(), rest),
                _ => (jiff::tz::TimeZone::UTC, args),
            };
            let text = |i: usize| {
                rest.get(i).ok_or_else(|| internal("missing date argument", span)).and_then(|v| string(v, span))
            };
            let loc = if rest.len() > 2 { builtins::load_location(text(2)?).map_err(at(span))? } else { loc };
            let layout = if rest.len() > 1 { Some(text(1)?) } else { None };
            let (t, z) = builtins::parse_date(text(0)?, layout, &loc).map_err(at(span))?;
            Ok(Value(Repr::Time(t, z)))
        }
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
        Intrinsic::Method { method, optional } => {
            let [recv, rest @ ..] = args else { return Err(internal("method call without a receiver", span)) };
            if optional && matches!(recv.0, Repr::Nil) {
                return Ok(Value::NIL);
            }
            methods::call(method, recv, rest, f, span)
        }
        Intrinsic::DynMethod { optional } => {
            let [name, recv, rest @ ..] = args else {
                return Err(internal("method call without a receiver", span));
            };
            if optional && matches!(recv.0, Repr::Nil) {
                return Ok(Value::NIL);
            }
            methods::call_dyn(string(name, span)?, recv, rest, f, span)
        }
    }
}

/// Whether the order argument of `sort` or `sortBy` asks for descending order.
fn descending(order: Option<&Value<'_>>, span: Span) -> Result<bool, Fault> {
    match order.map(|o| string(o, span)).transpose()? {
        None | Some("asc") => Ok(false),
        Some("desc") => Ok(true),
        Some(other) => {
            Err(Frame::fault(EvalErrorKind::InvalidArgument { what: "sort order", value: other.into() }, span))
        }
    }
}

/// The budget of sorting `n` elements, charged before the elements are collected.
fn charge_sort(n: usize, f: &mut Frame<'_>, span: Span) -> Result<(), Fault> {
    let log = usize::try_from(n.checked_ilog2().unwrap_or(0)).unwrap_or(usize::MAX);
    steps(f, n.saturating_mul(log.saturating_add(1)), span)?;
    alloc(f, n, span)
}

/// `xs` sorted stably by `key` under `kind`'s ordering, NaN first (last when
/// descending). The caller charges [`charge_sort`] first.
fn sorted<'a, T>(
    xs: Vec<T>,
    key: fn(&T) -> &Value<'a>,
    kind: CompareKind,
    desc: bool,
    span: Span,
) -> Result<Vec<T>, Fault> {
    let nan = |v: &Value<'_>| v.as_float().is_some_and(f64::is_nan);
    let mut cmp = |a: &T, b: &T| -> Result<Ordering, Fault> {
        let (a, b) = (key(a), key(b));
        let o = order(kind, a, b, span)?.unwrap_or_else(|| nan(b).cmp(&nan(a)));
        Ok(if desc { o.reverse() } else { o })
    };
    builtins::sort_by(xs, &mut cmp)
}

/// `flatten`: the elements of `xs`, with every nested array spliced in, to any depth.
fn flatten<'a>(
    xs: &Seq<'a>,
    out: &mut Vec<Value<'a>>,
    depth: Depth,
    f: &mut Frame<'a>,
    span: Span,
) -> Result<(), Fault> {
    steps(f, xs.len(), span)?;
    for x in xs.iter() {
        match x.as_seq() {
            Some(inner) => flatten(inner, out, deeper(depth, span)?, f, span)?,
            None => {
                alloc(f, 1, span)?;
                out.push(x);
            }
        }
    }
    Ok(())
}

/// An int or float as a float, for `mean` and `median`.
fn number(x: &Value<'_>, span: Span) -> Result<f64, Fault> {
    match x.0 {
        #[expect(clippy::cast_precision_loss, reason = "int to float rounds to nearest, as Go's float64(n)")]
        Repr::Int(n) => Ok(n as f64),
        Repr::Float(x) => Ok(x),
        _ => Err(mismatch("a number", x, span)),
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
            #[expect(clippy::cast_precision_loss, reason = "int to float rounds to nearest, as Go's float64(n)")]
            (Repr::Int(a), Repr::Float(b)) => Value::float(*a as f64 + b),
            #[expect(clippy::cast_precision_loss, reason = "int to float rounds to nearest, as Go's float64(n)")]
            (Repr::Float(a), Repr::Int(b)) => Value::float(a + *b as f64),
            (Repr::Float(a), Repr::Float(b)) => Value::float(a + b),
            _ => return Err(mismatch("a number", &x, span)),
        };
    }
    Ok(total)
}

/// Go's `unicode.ToUpper`/`ToLower` per character: a character whose case mapping
/// is more than one character (`ß`) stays as it is.
fn simple_case(s: &str, upper: bool, f: &mut Frame<'_>, span: Span) -> Res<'static> {
    let mut out = Text::new();
    for c in s.chars() {
        let mapped = if upper {
            let mut it = c.to_uppercase();
            match (it.next(), it.next()) {
                (Some(one), None) => one,
                _ => c,
            }
        } else {
            let mut it = c.to_lowercase();
            match (it.next(), it.next()) {
                (Some(one), None) => one,
                _ => c,
            }
        };
        out.push(f, mapped, span)?;
    }
    Ok(out.into_value())
}
