//! Typed IR to a tree of type-specialized closures.
//!
//! A node's Rust return type follows its dictum type: an `int` node returns `i64`,
//! a `bool` node returns `bool`. `Int + Int` is then a closure over two
//! `Node<IntK>`s with no runtime type test and no `Value` in between. Nodes of other
//! types return `Value`.
//!
//! The compiler trusts the IR invariants. If one does not hold it returns `Ice`
//! (internal compiler error) instead of panicking; `Rule::compile` reports it as
//! E0900. There is no other failure mode.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use dictum_check::ir::{
    self, BinaryOp, CompareKind, Const, Conversion, EqKind, ExprKind, FloatOp, InOp, IntOp, NodeId,
};
use dictum_syntax::Span;
use dictum_syntax::ast::{self, LogicOp, Pointer};

use crate::EvalErrorKind;
use crate::engine::SchemaCore;
use crate::engine::analysis::CacheSlot;
use crate::engine::frame::{self, Fault, Frame, Res, at, internal};
use crate::engine::interp;
use crate::engine::ops;
use crate::engine::set::ConstSet;
use crate::engine::value::{Repr, Seq, Value};

/// The Rust type a node of some dictum type evaluates to.
pub(crate) trait Kind: 'static {
    type Out<'a>;
}

pub(crate) enum BoolK {}
pub(crate) enum IntK {}
pub(crate) enum FloatK {}
pub(crate) enum ValueK {}

impl Kind for BoolK {
    type Out<'a> = bool;
}
impl Kind for IntK {
    type Out<'a> = i64;
}
impl Kind for FloatK {
    type Out<'a> = f64;
}
impl Kind for ValueK {
    type Out<'a> = Value<'a>;
}

/// A compiled node producing `K::Out`.
pub(crate) type Node<K> = Box<dyn for<'a> Fn(&mut Frame<'a>) -> Result<<K as Kind>::Out<'a>, Fault> + Send + Sync>;

// Boxing through these functions gives each closure its higher-ranked signature.
fn bool_node(f: impl for<'a> Fn(&mut Frame<'a>) -> Result<bool, Fault> + Send + Sync + 'static) -> Node<BoolK> {
    Box::new(f)
}
fn int_node(f: impl for<'a> Fn(&mut Frame<'a>) -> Result<i64, Fault> + Send + Sync + 'static) -> Node<IntK> {
    Box::new(f)
}
fn float_node(f: impl for<'a> Fn(&mut Frame<'a>) -> Result<f64, Fault> + Send + Sync + 'static) -> Node<FloatK> {
    Box::new(f)
}
fn value_node(f: impl for<'a> Fn(&mut Frame<'a>) -> Res<'a> + Send + Sync + 'static) -> Node<ValueK> {
    Box::new(f)
}

/// A compiled node, tagged by what it returns. Only `bool`, `int` and `float` are
/// specialized; every other type is a `Value`.
pub(crate) enum Compiled {
    Bool(Node<BoolK>),
    Int(Node<IntK>),
    Float(Node<FloatK>),
    Value(Node<ValueK>),
}

impl Compiled {
    fn into_value(self) -> Node<ValueK> {
        match self {
            Compiled::Bool(n) => value_node(move |f| Ok(Value::bool(n(f)?))),
            Compiled::Int(n) => value_node(move |f| Ok(Value::int(n(f)?))),
            Compiled::Float(n) => value_node(move |f| Ok(Value::float(n(f)?))),
            Compiled::Value(n) => n,
        }
    }
}

/// A compiled program.
pub(crate) struct Tree {
    lets: Vec<Node<ValueK>>,
    body: Node<ValueK>,
    max_predicate_depth: u16,
}

impl Tree {
    /// Evaluate: the `let`s in order, then the body.
    pub(crate) fn run<'a>(&self, f: &mut Frame<'a>) -> Res<'a> {
        f.slots.clear();
        f.scopes.reserve(usize::from(self.max_predicate_depth));
        for l in &self.lets {
            let v = l(f)?;
            f.slots.push(v);
        }
        (self.body)(f)
    }
}

/// Internal compiler error: an IR invariant did not hold.
pub(crate) struct Ice(pub(crate) &'static str);

/// Original child spans from the unoptimized IR for nodes that report a runtime
/// error at a child's span (`Slice` bounds, `Map` keys, `Predicate` sequence).
///
/// `optimize` rewrites like `if false { a } else { b }` → `b` (`replace_with_child`)
/// replace a child with its sub-expression (and that sub-expression's narrower span),
/// which is right for errors raised *inside* `b`, but would shrink the span when the
/// *parent* reports a `CharBoundary`, `DuplicateKey`, or `TypeMismatch` against the
/// operand as written. Keeping the original child spans here keeps `eval` and
/// `explain` reporting identical spans.
pub(crate) enum ChildSpans {
    Slice(Span, Span),
    Map(Vec<Span>),
    Predicate(Span),
}

/// Collect the pre-optimization child spans of every `Slice`, `Map`, and `Predicate`
/// node in `program`, keyed by the parent's `NodeId`.
pub(crate) fn child_spans(program: &ir::Program) -> BTreeMap<NodeId, ChildSpans> {
    fn visit(e: &ir::Expr, out: &mut BTreeMap<NodeId, ChildSpans>) {
        match e.kind() {
            ExprKind::Slice { from, to, .. } => {
                let s = e.span();
                let spans = (from.as_deref().map_or(s, ir::Expr::span), to.as_deref().map_or(s, ir::Expr::span));
                out.insert(e.id(), ChildSpans::Slice(spans.0, spans.1));
            }
            ExprKind::Map(pairs) => {
                out.insert(e.id(), ChildSpans::Map(pairs.iter().map(|(k, _)| k.span()).collect()));
            }
            ExprKind::Predicate { seq, .. } => {
                out.insert(e.id(), ChildSpans::Predicate(seq.span()));
            }
            _ => {}
        }
        for c in e.children() {
            visit(c, out);
        }
    }
    let mut out = BTreeMap::new();
    for root in program.lets().iter().chain(core::iter::once(program.body())) {
        visit(root, &mut out);
    }
    out
}

pub(crate) struct Compiler<'c> {
    schema: &'c Arc<SchemaCore>,
    patterns: &'c BTreeMap<NodeId, Arc<regex::Regex>>,
    /// Shared-cache slot per node id (RuleSet only).
    slots: Option<&'c [Option<CacheSlot>]>,
    /// Integer operations proved not to overflow (`optimize::optimize`).
    unchecked: &'c BTreeSet<NodeId>,
    /// Pre-optimization child spans from `Checked::ir`, for `Slice`/`Map`/`Predicate`.
    child_spans: Option<&'c BTreeMap<NodeId, ChildSpans>>,
}

impl<'c> Compiler<'c> {
    pub(crate) fn new(
        schema: &'c Arc<SchemaCore>,
        patterns: &'c BTreeMap<NodeId, Arc<regex::Regex>>,
        slots: Option<&'c [Option<CacheSlot>]>,
        unchecked: &'c BTreeSet<NodeId>,
        child_spans: Option<&'c BTreeMap<NodeId, ChildSpans>>,
    ) -> Self {
        Compiler { schema, patterns, slots, unchecked, child_spans }
    }

    pub(crate) fn tree(&self, program: &ir::Program) -> Result<Tree, Ice> {
        let lets = program.lets().iter().map(|e| self.value(e)).collect::<Result<_, _>>()?;
        Ok(Tree { lets, body: self.value(program.body())?, max_predicate_depth: program.max_predicate_depth() })
    }

    /// `e` as a `Value` node.
    pub(crate) fn value(&self, e: &ir::Expr) -> Result<Node<ValueK>, Ice> {
        Ok(self.expr(e)?.into_value())
    }

    /// `e` as an `int` node. Another node (a field read, a slot) is wrapped with an
    /// extraction that reports `Internal` on a mismatch, as `interp` does.
    fn int(&self, e: &ir::Expr) -> Result<Node<IntK>, Ice> {
        let span = e.span();
        Ok(match self.expr(e)? {
            Compiled::Int(n) => n,
            other => {
                let v = other.into_value();
                int_node(move |f| interp::int(&v(f)?, span))
            }
        })
    }

    fn float(&self, e: &ir::Expr) -> Result<Node<FloatK>, Ice> {
        let span = e.span();
        Ok(match self.expr(e)? {
            Compiled::Float(n) => n,
            other => {
                let v = other.into_value();
                float_node(move |f| interp::float(&v(f)?, span))
            }
        })
    }

    fn bool(&self, e: &ir::Expr) -> Result<Node<BoolK>, Ice> {
        let span = e.span();
        Ok(match self.expr(e)? {
            Compiled::Bool(n) => n,
            other => {
                let v = other.into_value();
                bool_node(move |f| interp::boolean(&v(f)?, span))
            }
        })
    }

    fn values(&self, es: &[ir::Expr]) -> Result<Vec<Node<ValueK>>, Ice> {
        es.iter().map(|e| self.value(e)).collect()
    }

    /// Compile one node, then wrap it in its cache slot if it has one: a filled slot
    /// is returned as it is; an empty one is computed and filled on success only, so
    /// an error is never cached and each rule reports its own.
    fn expr(&self, e: &ir::Expr) -> Result<Compiled, Ice> {
        let compiled = self.uncached(e)?;
        let Some(CacheSlot(slot)) = self.slots.and_then(|s| s.get(e.id().index()).copied().flatten()) else {
            return Ok(compiled);
        };
        let (node, slot) = (compiled.into_value(), usize::try_from(slot).map_err(|_| Ice("cache slot"))?);
        Ok(Compiled::Value(value_node(move |f| {
            if let Some(v) = f.cache.get(slot).and_then(Option::as_ref) {
                return Ok(v.clone());
            }
            let v = node(f)?;
            if let Some(entry) = f.cache.get_mut(slot) {
                *entry = Some(v.clone());
            }
            Ok(v)
        })))
    }

    /// Compile one node. The evaluation order, the faults and their spans, and the
    /// budget charges are those of `interp::Walk::compute`; anything not specialized
    /// here calls the same `interp` function on evaluated operands.
    fn uncached(&self, e: &ir::Expr) -> Result<Compiled, Ice> {
        let span = e.span();
        Ok(match e.kind() {
            ExprKind::Const(c) => match c {
                Const::Bool(b) => {
                    let b = *b;
                    Compiled::Bool(bool_node(move |_| Ok(b)))
                }
                Const::Int(n) => {
                    let n = *n;
                    Compiled::Int(int_node(move |_| Ok(n)))
                }
                Const::Float(x) => {
                    let x = *x;
                    Compiled::Float(float_node(move |_| Ok(x)))
                }
                c => {
                    let c = c.clone();
                    Compiled::Value(value_node(move |_| Ok(Value::from_const(&c))))
                }
            },
            ExprKind::Root => Compiled::Value(value_node(|f| Ok(Value::object(f.root)))),
            ExprKind::Slot(s) => {
                let i = s.index();
                Compiled::Value(value_node(move |f| {
                    f.slots.get(i).cloned().ok_or_else(|| internal("unset slot", span))
                }))
            }
            ExprKind::Pointer(Pointer::Index) => Compiled::Int(int_node(move |f| {
                f.scopes.last().map(|s| s.index).ok_or_else(|| internal("pointer outside a predicate", span))
            })),
            ExprKind::Pointer(p) => {
                let acc = *p == Pointer::Acc;
                Compiled::Value(value_node(move |f| {
                    let scope = f.scopes.last().ok_or_else(|| internal("pointer outside a predicate", span))?;
                    if acc {
                        scope.acc.clone().ok_or_else(|| internal("#acc outside reduce", span))
                    } else {
                        Ok(scope.current.clone())
                    }
                }))
            }
            ExprKind::Field { base, field, optional } => {
                let (id, optional) = (*field, *optional);
                if matches!(base.kind(), ExprKind::Root) {
                    Compiled::Value(value_node(move |f| Ok(f.root.field(id))))
                } else {
                    let b = self.value(base)?;
                    Compiled::Value(value_node(move |f| match b(f)?.0 {
                        Repr::Nil if optional => Ok(Value::NIL),
                        Repr::Object(o) => Ok(o.field(id)),
                        _ => Err(internal("field of a non-struct", span)),
                    }))
                }
            }
            ExprKind::Index { base, index, op, optional } => {
                let (b, i, op, optional) = (self.value(base)?, self.value(index)?, *op, *optional);
                Compiled::Value(value_node(move |f| {
                    let b = b(f)?;
                    if optional && b.is_nil() {
                        return Ok(Value::NIL);
                    }
                    let i = i(f)?;
                    interp::index_value(op, &b, &i, span)
                }))
            }
            ExprKind::Slice { base, from, to, op, optional } => {
                let (b, op, optional) = (self.value(base)?, *op, *optional);
                let spans = match self.child_spans.and_then(|m| m.get(&e.id())) {
                    Some(ChildSpans::Slice(a, b)) => (*a, *b),
                    _ => (from.as_deref().map_or(span, ir::Expr::span), to.as_deref().map_or(span, ir::Expr::span)),
                };
                let from = from.as_deref().map(|x| self.int(x)).transpose()?;
                let to = to.as_deref().map(|x| self.int(x)).transpose()?;
                Compiled::Value(value_node(move |f| {
                    let b = b(f)?;
                    if optional && b.is_nil() {
                        return Ok(Value::NIL);
                    }
                    let from = from.as_ref().map(|x| x(f)).transpose()?;
                    let to = to.as_ref().map(|x| x(f)).transpose()?;
                    interp::slice_value(op, &b, (from, to), spans, f, span)
                }))
            }
            ExprKind::Unary { op, arg } => match op {
                ir::UnaryOp::Not => {
                    let a = self.bool(arg)?;
                    Compiled::Bool(bool_node(move |f| Ok(!a(f)?)))
                }
                ir::UnaryOp::NegInt => {
                    let a = self.int(arg)?;
                    Compiled::Int(int_node(move |f| {
                        a(f)?.checked_neg().ok_or_else(|| Frame::fault(EvalErrorKind::Overflow, span))
                    }))
                }
                ir::UnaryOp::NegFloat => {
                    let a = self.float(arg)?;
                    Compiled::Float(float_node(move |f| Ok(-a(f)?)))
                }
                op => {
                    let (a, op) = (self.value(arg)?, *op);
                    Compiled::Value(value_node(move |f| interp::unary(op, a(f)?, span)))
                }
            },
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, e.id(), span)?,
            ExprKind::Logic { op, operands } => {
                let operands = operands.iter().map(|x| self.bool(x)).collect::<Result<Vec<_>, _>>()?;
                // `&&` stops at the first `false`, `||` at the first `true`.
                let stop = *op == LogicOp::Or;
                Compiled::Bool(bool_node(move |f| {
                    for x in &operands {
                        if x(f)? == stop {
                            return Ok(stop);
                        }
                    }
                    Ok(!stop)
                }))
            }
            ExprKind::Compare { kind, first, rest } => self.compare(*kind, first, rest, span)?,
            ExprKind::In { op, needle, haystack, negated } => {
                let (n, negated) = (self.value(needle)?, *negated);
                let set = match (op, haystack.kind()) {
                    (InOp::Array(kind), ExprKind::Const(Const::Array(xs))) => {
                        ConstSet::new(xs, *kind).map(|set| (set, xs.len()))
                    }
                    _ => None,
                };
                match set {
                    // A search charges what the scan it replaces would.
                    Some((set, len)) => Compiled::Bool(bool_node(move |f| {
                        let n = n(f)?;
                        frame::steps(f, len, span)?;
                        Ok(set.contains(&n) != negated)
                    })),
                    None => {
                        let (h, op) = (self.value(haystack)?, *op);
                        Compiled::Bool(bool_node(move |f| {
                            let n = n(f)?;
                            let h = h(f)?;
                            Ok(interp::membership(op, &n, &h, f, span)? != negated)
                        }))
                    }
                }
            }
            ExprKind::Conditional { cond, then, otherwise } => {
                let c = self.bool(cond)?;
                match (self.expr(then)?, self.expr(otherwise)?) {
                    (Compiled::Bool(a), Compiled::Bool(b)) => {
                        Compiled::Bool(bool_node(move |f| if c(f)? { a(f) } else { b(f) }))
                    }
                    (Compiled::Int(a), Compiled::Int(b)) => {
                        Compiled::Int(int_node(move |f| if c(f)? { a(f) } else { b(f) }))
                    }
                    (Compiled::Float(a), Compiled::Float(b)) => {
                        Compiled::Float(float_node(move |f| if c(f)? { a(f) } else { b(f) }))
                    }
                    (a, b) => {
                        let (a, b) = (a.into_value(), b.into_value());
                        Compiled::Value(value_node(move |f| if c(f)? { a(f) } else { b(f) }))
                    }
                }
            }
            ExprKind::Coalesce { lhs, rhs } => {
                let (l, r) = (self.value(lhs)?, self.value(rhs)?);
                Compiled::Value(value_node(move |f| {
                    let l = l(f)?;
                    if l.is_nil() { r(f) } else { Ok(l) }
                }))
            }
            ExprKind::Array(xs) => {
                let xs = self.values(xs)?;
                Compiled::Value(value_node(move |f| {
                    frame::alloc(f, xs.len(), span)?;
                    let mut out = Vec::with_capacity(xs.len());
                    for x in &xs {
                        out.push(x(f)?);
                    }
                    Ok(Value::array(out))
                }))
            }
            ExprKind::Map(pairs) => {
                let key_spans = match self.child_spans.and_then(|m| m.get(&e.id())) {
                    Some(ChildSpans::Map(ks)) => ks.as_slice(),
                    _ => &[],
                };
                let pairs = pairs
                    .iter()
                    .enumerate()
                    .map(|(idx, (k, v))| {
                        let k_span = key_spans.get(idx).copied().unwrap_or_else(|| k.span());
                        Ok((self.value(k)?, k_span, self.value(v)?))
                    })
                    .collect::<Result<Vec<_>, Ice>>()?;
                Compiled::Value(value_node(move |f| {
                    frame::alloc(f, pairs.len(), span)?;
                    let mut out: Vec<(Arc<str>, Value<'_>)> = Vec::with_capacity(pairs.len());
                    for (k, k_span, v) in &pairs {
                        let key = k(f)?;
                        let key: Arc<str> = interp::string(&key, *k_span)?.into();
                        let value = v(f)?;
                        if out.iter().any(|(seen, _)| *seen == key) {
                            return Err(Frame::fault(EvalErrorKind::DuplicateKey((*key).into()), *k_span));
                        }
                        out.push((key, value));
                    }
                    Ok(Value::map(out))
                }))
            }
            ExprKind::Range { from, to } => {
                let (a, b) = (self.int(from)?, self.int(to)?);
                Compiled::Value(value_node(move |f| {
                    let from = a(f)?;
                    Ok(Value(Repr::Array(Seq::Range { from, to: b(f)? })))
                }))
            }
            ExprKind::Predicate { op, seq, arg, body } => {
                let seq_span = match self.child_spans.and_then(|m| m.get(&e.id())) {
                    Some(ChildSpans::Predicate(s)) => *s,
                    _ => seq.span(),
                };
                let (s, op) = (self.value(seq)?, *op);
                let arg = arg.as_deref().map(|a| self.value(a)).transpose()?;
                let body = body.as_deref().map(|b| self.value(b)).transpose()?;
                Compiled::Value(value_node(move |f| {
                    let v = s(f)?;
                    let s = v.as_seq().cloned().ok_or_else(|| interp::mismatch("an array", &v, seq_span))?;
                    let arg = arg.as_ref().map(|a| a(f)).transpose()?;
                    match &body {
                        Some(b) => interp::predicate(op, &s, arg, Some(&mut |f| b(f)), f, span),
                        None => interp::predicate(op, &s, arg, None, f, span),
                    }
                }))
            }
            ExprKind::Intrinsic { op, args } => {
                let op = *op;
                match self.values(args)?.as_mut_slice() {
                    [] => Compiled::Value(value_node(move |f| interp::intrinsic(op, &[], f, span))),
                    [a] => {
                        let a = core::mem::replace(a, value_node(|_| Ok(Value::NIL)));
                        Compiled::Value(value_node(move |f| {
                            let args = [a(f)?];
                            interp::intrinsic(op, &args, f, span)
                        }))
                    }
                    [a, b] => {
                        let a = core::mem::replace(a, value_node(|_| Ok(Value::NIL)));
                        let b = core::mem::replace(b, value_node(|_| Ok(Value::NIL)));
                        Compiled::Value(value_node(move |f| {
                            let args = [a(f)?, b(f)?];
                            interp::intrinsic(op, &args, f, span)
                        }))
                    }
                    args => {
                        let args: Vec<_> =
                            args.iter_mut().map(|a| core::mem::replace(a, value_node(|_| Ok(Value::NIL)))).collect();
                        Compiled::Value(value_node(move |f| {
                            let vals = args.iter().map(|a| a(f)).collect::<Result<Vec<_>, _>>()?;
                            interp::intrinsic(op, &vals, f, span)
                        }))
                    }
                }
            }
            ExprKind::Call { func, args } => {
                let call = self.schema.payload(*func).ok_or(Ice("unknown function"))?.call.clone();
                let name: String = self.schema.func_name(*func).unwrap_or("?").into();
                let mut take = |slot: &mut Node<ValueK>| core::mem::replace(slot, value_node(|_| Ok(Value::NIL)));
                match self.values(args)?.as_mut_slice() {
                    [] => Compiled::Value(value_node(move |_| {
                        call(&[]).map_err(|err| interp::call_fault(err, &name, span))
                    })),
                    [a] => {
                        let a = take(a);
                        Compiled::Value(value_node(move |f| {
                            let vals = [a(f)?];
                            call(&vals).map_err(|err| interp::call_fault(err, &name, span))
                        }))
                    }
                    [a, b] => {
                        let (a, b) = (take(a), take(b));
                        Compiled::Value(value_node(move |f| {
                            let vals = [a(f)?, b(f)?];
                            call(&vals).map_err(|err| interp::call_fault(err, &name, span))
                        }))
                    }
                    [a, b, c] => {
                        let (a, b, c) = (take(a), take(b), take(c));
                        Compiled::Value(value_node(move |f| {
                            let vals = [a(f)?, b(f)?, c(f)?];
                            call(&vals).map_err(|err| interp::call_fault(err, &name, span))
                        }))
                    }
                    [a, b, c, d] => {
                        let (a, b, c, d) = (take(a), take(b), take(c), take(d));
                        Compiled::Value(value_node(move |f| {
                            let vals = [a(f)?, b(f)?, c(f)?, d(f)?];
                            call(&vals).map_err(|err| interp::call_fault(err, &name, span))
                        }))
                    }
                    args => {
                        let args: Vec<_> = args.iter_mut().map(&mut take).collect();
                        Compiled::Value(value_node(move |f| {
                            let vals = args.iter().map(|a| a(f)).collect::<Result<Vec<_>, _>>()?;
                            call(&vals).map_err(|err| interp::call_fault(err, &name, span))
                        }))
                    }
                }
            }
            ExprKind::Convert { op, arg } => match op {
                Conversion::IntToFloat => {
                    let a = self.int(arg)?;
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "int to float rounds to nearest, as Go's float64(n)"
                    )]
                    Compiled::Float(float_node(move |f| Ok(a(f)? as f64)))
                }
                // The same value, now of an optional or `any` type.
                Conversion::Some | Conversion::Erase => self.expr(arg)?,
                op => {
                    let (a, op, ty) = (self.value(arg)?, *op, e.ty().clone());
                    Compiled::Value(value_node(move |f| {
                        let v = a(f)?;
                        match op {
                            #[cfg(feature = "decimal")]
                            Conversion::IntToDecimal => {
                                interp::promote(v, dictum_check::resolve::Promote::IntToDecimal, span)
                            }
                            Conversion::Check => interp::check(v, &ty, f, span),
                            _ => Err(internal("unexpected conversion", span)),
                        }
                    }))
                }
            },
        })
    }

    fn binary(&self, op: BinaryOp, lhs: &ir::Expr, rhs: &ir::Expr, id: NodeId, span: Span) -> Result<Compiled, Ice> {
        let overflow = move || Frame::fault(EvalErrorKind::Overflow, span);
        Ok(match op {
            BinaryOp::Int(op) if self.unchecked.contains(&id) => {
                // Lean `evalFast`: the ranges prove the result fits, so no check.
                let (a, b) = (self.int(lhs)?, self.int(rhs)?);
                Compiled::Int(match op {
                    IntOp::Add => int_node(move |f| Ok(a(f)?.wrapping_add(b(f)?))),
                    IntOp::Sub => int_node(move |f| Ok(a(f)?.wrapping_sub(b(f)?))),
                    IntOp::Mul => int_node(move |f| Ok(a(f)?.wrapping_mul(b(f)?))),
                    IntOp::Mod => return Err(Ice("an unchecked modulo")),
                })
            }
            BinaryOp::Int(op) => {
                let (a, b) = (self.int(lhs)?, self.int(rhs)?);
                Compiled::Int(match op {
                    IntOp::Add => int_node(move |f| {
                        let x = a(f)?;
                        ops::add_int(x, b(f)?).map_err(at(span))
                    }),
                    IntOp::Sub => int_node(move |f| {
                        let x = a(f)?;
                        x.checked_sub(b(f)?).ok_or_else(overflow)
                    }),
                    IntOp::Mul => int_node(move |f| {
                        let x = a(f)?;
                        x.checked_mul(b(f)?).ok_or_else(overflow)
                    }),
                    IntOp::Mod => int_node(move |f| {
                        let x = a(f)?;
                        ops::mod_int(x, b(f)?).map_err(at(span))
                    }),
                })
            }
            BinaryOp::Float(op) => {
                let (a, b) = (self.float(lhs)?, self.float(rhs)?);
                Compiled::Float(match op {
                    FloatOp::Add => float_node(move |f| Ok(a(f)? + b(f)?)),
                    FloatOp::Sub => float_node(move |f| Ok(a(f)? - b(f)?)),
                    FloatOp::Mul => float_node(move |f| Ok(a(f)? * b(f)?)),
                    FloatOp::Div => float_node(move |f| Ok(a(f)? / b(f)?)),
                    FloatOp::Pow => float_node(move |f| {
                        let x = a(f)?;
                        Ok(ops::pow_float(x, b(f)?))
                    }),
                })
            }
            BinaryOp::Eq(EqKind::Int) | BinaryOp::Ne(EqKind::Int) => {
                let (a, b, eq) = (self.int(lhs)?, self.int(rhs)?, matches!(op, BinaryOp::Eq(_)));
                Compiled::Bool(bool_node(move |f| {
                    let x = a(f)?;
                    Ok((x == b(f)?) == eq)
                }))
            }
            BinaryOp::Eq(EqKind::Float) | BinaryOp::Ne(EqKind::Float) => {
                let (a, b, eq) = (self.float(lhs)?, self.float(rhs)?, matches!(op, BinaryOp::Eq(_)));
                Compiled::Bool(bool_node(move |f| {
                    let x = a(f)?;
                    Ok((x == b(f)?) == eq)
                }))
            }
            BinaryOp::Eq(EqKind::Bool) | BinaryOp::Ne(EqKind::Bool) => {
                let (a, b, eq) = (self.bool(lhs)?, self.bool(rhs)?, matches!(op, BinaryOp::Eq(_)));
                Compiled::Bool(bool_node(move |f| {
                    let x = a(f)?;
                    Ok((x == b(f)?) == eq)
                }))
            }
            op => {
                // A constant pattern compiles once. One that the folder produced
                // (`"a" + "+"`) was not seen by `patterns`; an invalid one keeps failing
                // at run time, where `interp` reports it.
                let pattern = self.patterns.get(&id).cloned().or_else(|| match (op, rhs.kind()) {
                    (BinaryOp::Matches, ExprKind::Const(Const::Str(p))) => regex::Regex::new(p).ok().map(Arc::new),
                    _ => None,
                });
                let (a, b) = (self.value(lhs)?, self.value(rhs)?);
                Compiled::Value(value_node(move |f| {
                    let l = a(f)?;
                    let r = b(f)?;
                    interp::binary(op, l, r, pattern.as_deref(), f, span)
                }))
            }
        })
    }

    fn compare(
        &self,
        kind: CompareKind,
        first: &ir::Expr,
        rest: &[(ast::CompareOp, ir::Expr)],
        span: Span,
    ) -> Result<Compiled, Ice> {
        Ok(Compiled::Bool(match kind {
            CompareKind::Int => {
                let first = self.int(first)?;
                let rest = rest.iter().map(|(op, x)| Ok((*op, self.int(x)?))).collect::<Result<Vec<_>, Ice>>()?;
                bool_node(move |f| {
                    let mut prev = first(f)?;
                    for (op, x) in &rest {
                        let cur = x(f)?;
                        if !interp::holds(*op, Some(prev.cmp(&cur))) {
                            return Ok(false);
                        }
                        prev = cur;
                    }
                    Ok(true)
                })
            }
            CompareKind::Float => {
                let first = self.float(first)?;
                let rest = rest.iter().map(|(op, x)| Ok((*op, self.float(x)?))).collect::<Result<Vec<_>, Ice>>()?;
                bool_node(move |f| {
                    let mut prev = first(f)?;
                    for (op, x) in &rest {
                        let cur = x(f)?;
                        if !interp::holds(*op, prev.partial_cmp(&cur)) {
                            return Ok(false);
                        }
                        prev = cur;
                    }
                    Ok(true)
                })
            }
            kind => {
                let first = self.value(first)?;
                let rest = rest.iter().map(|(op, x)| Ok((*op, self.value(x)?))).collect::<Result<Vec<_>, Ice>>()?;
                bool_node(move |f| {
                    let mut prev = first(f)?;
                    for (op, x) in &rest {
                        let cur = x(f)?;
                        if !interp::holds(*op, interp::order(kind, &prev, &cur, span)?) {
                            return Ok(false);
                        }
                        prev = cur;
                    }
                    Ok(true)
                })
            }
        }))
    }
}
