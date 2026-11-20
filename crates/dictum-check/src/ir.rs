//! Typed IR: what the checker proves, in a form the evaluator can trust.
//!
//! Every overload is already resolved to a monomorphic operation (`Int(IntOp::Add)`,
//! `Eq(EqKind::String)`, ...), every implicit conversion is an explicit `Convert`
//! node, every name is an index. The evaluator never re-derives a typing decision;
//! for `any` operands it asks [`crate::resolve`] at run time, which is the same table
//! the checker used.
//!
//! `Expr` fields are private, so only this crate creates nodes. The evaluator reads
//! them and may only replace a node by a constant of the same type
//! ([`Expr::replace_with_const`]), so constant folding cannot make the IR ill-typed.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use dictum_syntax::Span;
use dictum_syntax::ast::{CompareOp, LogicOp, Pointer};

use crate::{FieldId, FuncId, Tag, Type};

/// Dense id of a node within one `Program`, assigned in post-order by the checker.
/// Side tables (explain traces, CSE slots) are `Vec`s indexed by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(u32);

impl NodeId {
    pub(crate) const fn new(id: u32) -> Self {
        NodeId(id)
    }

    /// Index into a side table of length `Program::node_count`.
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// A `let` slot: slot `i` holds `Program::lets()[i]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SlotId(u16);

impl SlotId {
    pub(crate) const fn new(id: u16) -> Self {
        SlotId(id)
    }

    /// Index into `Program::lets`.
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// A checked rule.
///
/// Invariants:
/// - every node's resolved operation agrees with its operands' types;
/// - `Slot(s)` inside `lets()[i]` has `s.index() < i`; in `body()` any slot is valid;
/// - `Pointer` occurs only inside a `Predicate` body and refers to the innermost one;
/// - `Field { field, .. }` indexes the base type's `StructDesc`;
/// - `Call { func, .. }` is valid in the `Env` the program was checked against;
/// - node ids are unique and `< node_count()`; depth `<= dictum_syntax::MAX_DEPTH`.
#[derive(Clone, Debug)]
pub struct Program {
    lets: Vec<Expr>,
    body: Expr,
    node_count: u32,
    max_predicate_depth: u16,
}

impl Program {
    /// `let` values in slot order. Evaluated eagerly, in order, before `body`.
    pub fn lets(&self) -> &[Expr] {
        &self.lets
    }

    /// The result expression; its type is assignable to the requested result type.
    pub fn body(&self) -> &Expr {
        &self.body
    }

    /// Upper bound (exclusive) of node ids.
    pub fn node_count(&self) -> u32 {
        self.node_count
    }

    /// Deepest predicate nesting, to size the evaluator's scope stack once.
    pub fn max_predicate_depth(&self) -> u16 {
        self.max_predicate_depth
    }

    /// Every root expression (lets, then body), for in-place folding.
    pub fn roots_mut(&mut self) -> impl Iterator<Item = &mut Expr> {
        self.lets.iter_mut().chain(core::iter::once(&mut self.body))
    }

    pub(crate) fn new(lets: Vec<Expr>, body: Expr, node_count: u32, max_predicate_depth: u16) -> Self {
        Program { lets, body, node_count, max_predicate_depth }
    }

    /// Check every invariant above against `env`, the `Env` the program was checked
    /// against. The checker guarantees them; this is what tests and the fuzz target
    /// call to hold it to that, and what the engine may call in debug builds.
    pub fn validate<P>(&self, env: &crate::Env<P>) -> Result<(), String> {
        let mut v =
            Validator { env_root: env.root(), funcs: env.func_count(), seen: Vec::new(), node_count: self.node_count };
        v.seen.resize(self.node_count as usize, false);
        for (i, e) in self.lets.iter().enumerate() {
            v.expr(e, i, 0, 1)?;
        }
        v.expr(&self.body, self.lets.len(), 0, 1)?;
        let preds = max_predicates(self.roots());
        if preds > self.max_predicate_depth {
            return Err(format!("predicate depth {preds} exceeds max_predicate_depth {}", self.max_predicate_depth));
        }
        Ok(())
    }

    fn roots(&self) -> impl Iterator<Item = &Expr> {
        self.lets.iter().chain(core::iter::once(&self.body))
    }
}

fn max_predicates<'a>(roots: impl Iterator<Item = &'a Expr>) -> u16 {
    /// Predicate nesting below `e`: a predicate's body is one deeper than its sequence.
    fn go(e: &Expr) -> u16 {
        match &e.kind {
            ExprKind::Predicate { seq, arg, body, .. } => {
                go(seq).max(arg.as_deref().map_or(0, go)).max(body.as_deref().map_or(0, |b| go(b).saturating_add(1)))
            }
            _ => e.children().map(go).max().unwrap_or(0),
        }
    }
    roots.map(go).max().unwrap_or(0)
}

/// Walks a program for [`Program::validate`].
struct Validator {
    env_root: &'static crate::StructDesc,
    funcs: usize,
    seen: Vec<bool>,
    node_count: u32,
}

fn is_bool(t: &Type) -> bool {
    matches!(t, Type::Bool)
}

impl Validator {
    /// `slots`: slots visible here; `preds`: enclosing predicates; `depth`: tree depth.
    fn expr(&mut self, e: &Expr, slots: usize, preds: u32, depth: u32) -> Result<(), String> {
        let at = |what: &str| format!("node {} at {:?}: {what}", e.id.0, e.span);
        if depth > dictum_syntax::MAX_DEPTH {
            return Err(at("deeper than MAX_DEPTH"));
        }
        match self.seen.get_mut(e.id.index()) {
            Some(seen) if !*seen => *seen = true,
            Some(_) => return Err(at("duplicate node id")),
            None => return Err(at(&format!("id not below node_count {}", self.node_count))),
        }
        let bad_type = |what: &str| Err(at(&format!("{what}; node type is {}", e.ty)));
        match &e.kind {
            ExprKind::Const(c) => {
                if !c.inhabits(&e.ty) {
                    return bad_type(&format!("constant {c:?} is not of the node type"));
                }
            }
            ExprKind::Root => {
                if e.ty != Type::Struct(self.env_root) {
                    return bad_type("root is not the schema root struct");
                }
            }
            ExprKind::Slot(s) => {
                if s.index() >= slots {
                    return Err(at(&format!("slot {} used before it is bound", s.index())));
                }
            }
            ExprKind::Pointer(_) => {
                if preds == 0 {
                    return Err(at("pointer outside a predicate"));
                }
            }
            ExprKind::Field { base, field, optional } => {
                let desc = match (base.ty(), optional) {
                    (Type::Struct(d), _) => *d,
                    (Type::Option(o), true) => match o.inner() {
                        Type::Struct(d) => *d,
                        _ => return Err(at("field of a non-struct")),
                    },
                    _ => return Err(at("field of a non-struct")),
                };
                let Some(f) = desc.fields().get(usize::from(field.index())) else {
                    return Err(at("field id outside the struct"));
                };
                let ty = if base.ty().admits_nil() { Type::option(f.ty()) } else { f.ty() };
                if ty != e.ty {
                    return bad_type(&format!("field `{}` has type {ty}", f.name()));
                }
            }
            ExprKind::Logic { operands, .. } => {
                if operands.len() < 2 || !operands.iter().all(|o| is_bool(o.ty())) || !is_bool(&e.ty) {
                    return bad_type("logic operands must be at least two bools");
                }
            }
            ExprKind::Conditional { cond, then, otherwise } => {
                if !is_bool(cond.ty()) || *then.ty() != e.ty || *otherwise.ty() != e.ty {
                    return bad_type("conditional needs a bool and two branches of the node type");
                }
            }
            ExprKind::Compare { .. } | ExprKind::In { .. } => {
                if !is_bool(&e.ty) {
                    return bad_type("comparison is not bool");
                }
            }
            ExprKind::Binary { op, lhs, rhs } => check_binary(*op, lhs.ty(), rhs.ty(), &e.ty).map_err(|m| at(&m))?,
            ExprKind::Unary { op, arg } => {
                let ok = match op {
                    UnaryOp::Not => is_bool(arg.ty()) && is_bool(&e.ty),
                    UnaryOp::NegInt => *arg.ty() == Type::Int && e.ty == Type::Int,
                    UnaryOp::NegFloat => *arg.ty() == Type::Float && e.ty == Type::Float,
                    #[cfg(feature = "decimal")]
                    UnaryOp::NegDecimal => *arg.ty() == Type::Decimal && e.ty == Type::Decimal,
                    UnaryOp::NegDuration => *arg.ty() == Type::Duration && e.ty == Type::Duration,
                    UnaryOp::Dyn(_) => e.ty == Type::Any,
                };
                if !ok {
                    return bad_type(&format!("{op:?} on {}", arg.ty()));
                }
            }
            ExprKind::Convert { op, arg } => {
                let (from, to) = (arg.ty(), &e.ty);
                let ok = match op {
                    Conversion::IntToFloat => *from == Type::Int && *to == Type::Float,
                    #[cfg(feature = "decimal")]
                    Conversion::IntToDecimal => *from == Type::Int && *to == Type::Decimal,
                    Conversion::Some => {
                        matches!(to, Type::Option(o) if o.inner() == from) || (*from == Type::Nil && to.admits_nil())
                    }
                    Conversion::Check => *from == Type::Any && *to != Type::Any,
                    Conversion::Erase => *from != Type::Any && *to == Type::Any,
                };
                if !ok {
                    return bad_type(&format!("{op:?} from {from}"));
                }
            }
            ExprKind::Call { func, .. } => {
                if func.index() >= self.funcs {
                    return Err(at("function id outside the schema"));
                }
            }
            ExprKind::Range { from, to } => {
                if *from.ty() != Type::Int || *to.ty() != Type::Int || e.ty != Type::array(Type::Int) {
                    return bad_type("range needs two ints");
                }
            }
            ExprKind::Predicate { op, arg, body, .. } => {
                let arg_ok = match (op, arg.as_deref()) {
                    (_, None) => true,
                    (PredicateOp::SortBy(_), Some(order)) => *order.ty() == Type::String,
                    (PredicateOp::Reduce, Some(init)) => *init.ty() == e.ty,
                    (_, Some(_)) => false,
                };
                let body_ok = *op != PredicateOp::Reduce || body.as_deref().is_some_and(|b| *b.ty() == e.ty);
                if !arg_ok || !body_ok {
                    return bad_type(&format!("{op:?} with this argument or body"));
                }
            }
            ExprKind::Coalesce { .. }
            | ExprKind::Index { .. }
            | ExprKind::Slice { .. }
            | ExprKind::Array(_)
            | ExprKind::Map(_)
            | ExprKind::Intrinsic { .. } => {}
        }
        // A predicate's body (not its sequence or argument) is one predicate deeper.
        if let ExprKind::Predicate { seq, arg, body, .. } = &e.kind {
            self.expr(seq, slots, preds, depth.saturating_add(1))?;
            if let Some(a) = arg {
                self.expr(a, slots, preds, depth.saturating_add(1))?;
            }
            if let Some(b) = body {
                self.expr(b, slots, preds.saturating_add(1), depth.saturating_add(1))?;
            }
            return Ok(());
        }
        for c in e.children() {
            self.expr(c, slots, preds, depth.saturating_add(1))?;
        }
        Ok(())
    }
}

/// The operand and result types each resolved binary operation requires.
fn check_binary(op: BinaryOp, l: &Type, r: &Type, out: &Type) -> Result<(), String> {
    use Type as T;
    let ok = match op {
        BinaryOp::Int(_) => *l == T::Int && *r == T::Int && *out == T::Int,
        BinaryOp::Float(_) => *l == T::Float && *r == T::Float && *out == T::Float,
        #[cfg(feature = "decimal")]
        BinaryOp::Decimal(_) => *l == T::Decimal && *r == T::Decimal && *out == T::Decimal,
        BinaryOp::Concat => *l == T::String && *r == T::String && *out == T::String,
        BinaryOp::ConcatBytes => *l == T::Bytes && *r == T::Bytes && *out == T::Bytes,
        BinaryOp::ConcatArray => matches!(l, T::Array(_)) && l == r && l == out,
        BinaryOp::TimeSubTime => *l == T::Time && *r == T::Time && *out == T::Duration,
        BinaryOp::TimeAddDuration | BinaryOp::TimeSubDuration => *l == T::Time && *r == T::Duration && *out == T::Time,
        BinaryOp::DurationAdd | BinaryOp::DurationSub => *l == T::Duration && *r == T::Duration && *out == T::Duration,
        BinaryOp::Eq(_) | BinaryOp::Ne(_) => *out == T::Bool,
        BinaryOp::Matches | BinaryOp::Contains | BinaryOp::StartsWith | BinaryOp::EndsWith => {
            *l == T::String && *r == T::String && *out == T::Bool
        }
        BinaryOp::Dyn(_) => *out == T::Any || *out == T::Bool,
    };
    if ok { Ok(()) } else { Err(format!("{op:?} on {l} and {r} giving {out}")) }
}

/// A typed node.
#[derive(Clone, Debug)]
pub struct Expr {
    id: NodeId,
    span: Span,
    ty: Type,
    kind: ExprKind,
}

impl Expr {
    /// Id for side tables.
    pub fn id(&self) -> NodeId {
        self.id
    }

    /// Source position.
    pub fn span(&self) -> Span {
        self.span
    }

    /// Static type of the value this node produces.
    pub fn ty(&self) -> &Type {
        &self.ty
    }

    /// What the node computes.
    pub fn kind(&self) -> &ExprKind {
        &self.kind
    }

    pub(crate) fn new(id: NodeId, span: Span, ty: Type, kind: ExprKind) -> Self {
        Expr { id, span, ty, kind }
    }

    /// Children in evaluation order (for traversals that do not care about kinds).
    pub fn children(&self) -> impl Iterator<Item = &Expr> {
        let mut out: Vec<&Expr> = Vec::new();
        match &self.kind {
            ExprKind::Const(_) | ExprKind::Root | ExprKind::Slot(_) | ExprKind::Pointer(_) => {}
            ExprKind::Field { base, .. } => out.push(base),
            ExprKind::Index { base, index, .. } => out.extend([&**base, &**index]),
            ExprKind::Slice { base, from, to, .. } => {
                out.push(base);
                out.extend(from.as_deref());
                out.extend(to.as_deref());
            }
            ExprKind::Unary { arg, .. } | ExprKind::Convert { arg, .. } => out.push(arg),
            ExprKind::Binary { lhs, rhs, .. } | ExprKind::Coalesce { lhs, rhs } => out.extend([&**lhs, &**rhs]),
            ExprKind::Logic { operands, .. } | ExprKind::Array(operands) => out.extend(operands),
            ExprKind::Compare { first, rest, .. } => {
                out.push(first);
                out.extend(rest.iter().map(|(_, e)| e));
            }
            ExprKind::In { needle, haystack, .. } => out.extend([&**needle, &**haystack]),
            ExprKind::Conditional { cond, then, otherwise } => out.extend([&**cond, &**then, &**otherwise]),
            ExprKind::Map(pairs) => {
                for (k, v) in pairs {
                    out.extend([k, v]);
                }
            }
            ExprKind::Range { from, to } => out.extend([&**from, &**to]),
            ExprKind::Predicate { seq, arg, body, .. } => {
                out.push(seq);
                out.extend(arg.as_deref());
                out.extend(body.as_deref());
            }
            ExprKind::Intrinsic { args, .. } | ExprKind::Call { args, .. } => out.extend(args),
        }
        out.into_iter()
    }

    /// Children, mutably, so the folder can work bottom-up.
    pub fn children_mut(&mut self) -> impl Iterator<Item = &mut Expr> {
        let mut out: Vec<&mut Expr> = Vec::new();
        match &mut self.kind {
            ExprKind::Const(_) | ExprKind::Root | ExprKind::Slot(_) | ExprKind::Pointer(_) => {}
            ExprKind::Field { base, .. } => out.push(base),
            ExprKind::Index { base, index, .. } => out.extend([&mut **base, &mut **index]),
            ExprKind::Slice { base, from, to, .. } => {
                out.push(base);
                out.extend(from.as_deref_mut());
                out.extend(to.as_deref_mut());
            }
            ExprKind::Unary { arg, .. } | ExprKind::Convert { arg, .. } => out.push(arg),
            ExprKind::Binary { lhs, rhs, .. } | ExprKind::Coalesce { lhs, rhs } => {
                out.extend([&mut **lhs, &mut **rhs]);
            }
            ExprKind::Logic { operands, .. } | ExprKind::Array(operands) => out.extend(operands),
            ExprKind::Compare { first, rest, .. } => {
                out.push(first);
                out.extend(rest.iter_mut().map(|(_, e)| e));
            }
            ExprKind::In { needle, haystack, .. } => out.extend([&mut **needle, &mut **haystack]),
            ExprKind::Conditional { cond, then, otherwise } => {
                out.extend([&mut **cond, &mut **then, &mut **otherwise]);
            }
            ExprKind::Map(pairs) => {
                for (k, v) in pairs {
                    out.extend([k, v]);
                }
            }
            ExprKind::Range { from, to } => out.extend([&mut **from, &mut **to]),
            ExprKind::Predicate { seq, arg, body, .. } => {
                out.push(seq);
                out.extend(arg.as_deref_mut());
                out.extend(body.as_deref_mut());
            }
            ExprKind::Intrinsic { args, .. } | ExprKind::Call { args, .. } => out.extend(args),
        }
        out.into_iter()
    }

    /// Replace this node by a constant, keeping id, span and type.
    ///
    /// Returns the constant back if it does not inhabit `self.ty()` (a folder bug);
    /// the folder then leaves the node unfolded rather than corrupting the IR.
    pub fn replace_with_const(&mut self, value: Const) -> Result<(), Const> {
        if !value.inhabits(&self.ty) {
            return Err(value);
        }
        self.kind = ExprKind::Const(value);
        Ok(())
    }
}

/// Node kinds.
#[derive(Clone, Debug)]
pub enum ExprKind {
    /// A literal or a folded constant.
    Const(Const),
    /// `$env`: the schema root. A top-level variable `user` is `Field { base: Root, .. }`.
    Root,
    /// A `let` binding.
    Slot(SlotId),
    /// `#`, `#index`, `#acc` of the innermost predicate.
    Pointer(Pointer),
    /// `base.f` (`optional`: `base?.f`, base is `Option<Struct>`, result is optional).
    Field { base: Box<Expr>, field: FieldId, optional: bool },
    /// `base[index]`. Map lookups produce `Option<V>`; array/string/bytes out of
    /// range is a runtime error; negative indices count from the end.
    Index { base: Box<Expr>, index: Box<Expr>, op: IndexOp, optional: bool },
    /// `base[from:to]`; string bounds must be char boundaries (E0304).
    Slice { base: Box<Expr>, from: Option<Box<Expr>>, to: Option<Box<Expr>>, op: SliceOp, optional: bool },
    /// Prefix operation.
    Unary { op: UnaryOp, arg: Box<Expr> },
    /// Strict binary operation.
    Binary { op: BinaryOp, lhs: Box<Expr>, rhs: Box<Expr> },
    /// Short-circuit, n-ary; operands are `bool`.
    Logic { op: LogicOp, operands: Vec<Expr> },
    /// Comparison chain; all operands promoted to one `kind`; each evaluated once.
    Compare { kind: CompareKind, first: Box<Expr>, rest: Vec<(CompareOp, Expr)> },
    /// `needle in haystack` / `not in`.
    In { op: InOp, needle: Box<Expr>, haystack: Box<Expr>, negated: bool },
    /// `cond ? then : otherwise`; branches already converted to the node's type.
    Conditional { cond: Box<Expr>, then: Box<Expr>, otherwise: Box<Expr> },
    /// `lhs ?? rhs`, lhs optional.
    Coalesce { lhs: Box<Expr>, rhs: Box<Expr> },
    /// Array literal. The folder turns all-constant ones into `Const::Array`.
    Array(Vec<Expr>),
    /// Map literal; keys are `string`. Constant duplicate keys were rejected (E0210).
    Map(Vec<(Expr, Expr)>),
    /// `from..to` (inclusive, Go-compatible), a lazy sequence of `int`.
    Range { from: Box<Expr>, to: Box<Expr> },
    /// `all(seq, body)` etc. `body` is `None` only where the builtin allows it. `arg`
    /// is the optional third argument, evaluated after `seq` and outside the body's
    /// scope: `sortBy`'s order (a string) or `reduce`'s initial `#acc` (of the
    /// node's type); no other predicate has one.
    Predicate { op: PredicateOp, seq: Box<Expr>, arg: Option<Box<Expr>>, body: Option<Box<Expr>> },
    /// A builtin resolved to one monomorphic operation.
    Intrinsic { op: Intrinsic, args: Vec<Expr> },
    /// A schema function.
    Call { func: FuncId, args: Vec<Expr> },
    /// An implicit conversion made explicit.
    Convert { op: Conversion, arg: Box<Expr> },
}

/// Folded constants. Immutable, cheap to clone.
#[derive(Clone, Debug, PartialEq)]
pub enum Const {
    /// `nil`
    Nil,
    /// `bool`
    Bool(bool),
    /// `int`
    Int(i64),
    /// `float`
    Float(f64),
    /// `decimal`
    #[cfg(feature = "decimal")]
    Decimal(rust_decimal::Decimal),
    /// `string`
    Str(Arc<str>),
    /// `bytes`
    Bytes(Arc<[u8]>),
    /// `time` (from a folded `date("...")`)
    Time(jiff::Timestamp),
    /// `duration` (from a folded `duration("...")`)
    Duration(jiff::SignedDuration),
    /// `array`
    Array(Arc<[Const]>),
    /// `map`, insertion order, unique keys.
    Map(Arc<[(Arc<str>, Const)]>),
}

impl Const {
    /// Whether this constant is a value of `ty`.
    pub fn inhabits(&self, ty: &Type) -> bool {
        match (self, ty) {
            (_, Type::Any) | (Const::Nil, Type::Nil | Type::Option(_)) => true,
            (c, Type::Option(o)) => c.inhabits(o.inner()),
            (Const::Bool(_), Type::Bool)
            | (Const::Int(_), Type::Int)
            | (Const::Float(_), Type::Float)
            | (Const::Str(_), Type::String)
            | (Const::Bytes(_), Type::Bytes)
            | (Const::Time(_), Type::Time)
            | (Const::Duration(_), Type::Duration) => true,
            #[cfg(feature = "decimal")]
            (Const::Decimal(_), Type::Decimal) => true,
            (Const::Array(xs), Type::Array(t)) => xs.iter().all(|x| x.inhabits(t)),
            (Const::Map(kvs), Type::Map(t)) => kvs.iter().all(|(_, v)| v.inhabits(t)),
            _ => false,
        }
    }
}

/// Resolved prefix operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    /// `not` on `bool`
    Not,
    /// `-` on `int` (overflow on `i64::MIN`)
    NegInt,
    /// `-` on `float`
    NegFloat,
    /// `-` on `decimal`
    #[cfg(feature = "decimal")]
    NegDecimal,
    /// `-` on `duration`
    NegDuration,
    /// Operand is `any`: resolve at run time with [`crate::resolve::unary`].
    Dyn(dictum_syntax::ast::UnaryOp),
}

/// `int` arithmetic. Division is not here: `int / int` is `float` (operands are
/// converted), and `**` is `float` (Go's `math.Pow`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IntOp {
    /// checked `+`
    Add,
    /// checked `-`
    Sub,
    /// checked `*`
    Mul,
    /// `%`, zero divisor is E0302
    Mod,
}

/// `float` arithmetic (IEEE 754; no errors).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FloatOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `**` via `libm::pow`
    Pow,
}

/// `decimal` arithmetic; overflow and zero division are errors.
#[cfg(feature = "decimal")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DecimalOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `%`
    Mod,
}

/// Resolved strict binary operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    /// `int` ⊕ `int`
    Int(IntOp),
    /// `float` ⊕ `float`
    Float(FloatOp),
    /// `decimal` ⊕ `decimal`
    #[cfg(feature = "decimal")]
    Decimal(DecimalOp),
    /// `string + string`
    Concat,
    /// `bytes + bytes`
    ConcatBytes,
    /// `array<T> + array<T>`
    ConcatArray,
    /// `time - time` → `duration`
    TimeSubTime,
    /// `time + duration` → `time`
    TimeAddDuration,
    /// `time - duration` → `time`
    TimeSubDuration,
    /// `duration + duration`
    DurationAdd,
    /// `duration - duration`
    DurationSub,
    /// `==`
    Eq(EqKind),
    /// `!=`
    Ne(EqKind),
    /// `string matches string` (constant patterns compiled once at compile time)
    Matches,
    /// `string contains string`
    Contains,
    /// `string startsWith string`
    StartsWith,
    /// `string endsWith string`
    EndsWith,
    /// An operand is `any`: resolve at run time with [`crate::resolve::binary`].
    Dyn(dictum_syntax::ast::BinaryOp),
}

/// How `==` compares, chosen from the (promoted) operand types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EqKind {
    /// `bool`
    Bool,
    /// `int`
    Int,
    /// `float` (IEEE: `NaN != NaN`)
    Float,
    /// `decimal`
    #[cfg(feature = "decimal")]
    Decimal,
    /// `string`
    String,
    /// `bytes`
    Bytes,
    /// `time`
    Time,
    /// `duration`
    Duration,
    /// Options, arrays and maps of comparable types: nil-aware, element-wise.
    Structural,
    /// An operand is `any`.
    Dyn,
}

/// How a comparison chain compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompareKind {
    /// `int`
    Int,
    /// `float`
    Float,
    /// `decimal`
    #[cfg(feature = "decimal")]
    Decimal,
    /// `string` (byte-wise, as Go)
    String,
    /// `time`
    Time,
    /// `duration`
    Duration,
    /// An operand is `any`.
    Dyn,
}

/// A bit operation, as Go's operator on `int`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BitOp {
    /// `bitand`: `&`
    And,
    /// `bitor`: `|`
    Or,
    /// `bitxor`: `^`
    Xor,
    /// `bitnand`: `&^` (and not)
    AndNot,
    /// `bitnot`: `^x`
    Not,
    /// `bitshl`: `<<`, `0` from a shift of 64 or more
    Shl,
    /// `bitshr`: `>>` with sign fill
    Shr,
    /// `bitushr`: `>>` with zero fill, as `int(uint(x) >> y)`
    Ushr,
}

/// Membership.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InOp {
    /// `x in array`; constant arrays become a sorted set at compile time.
    Array(EqKind),
    /// `key in map`
    MapKey,
    /// `x in a..b` without materializing the range.
    Range,
    /// An operand is `any`.
    Dyn,
}

/// Indexing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IndexOp {
    /// `array[int]`
    Array,
    /// `map[string]` → `Option<V>`
    Map,
    /// `string[int]` (byte index, one-character string)
    String,
    /// `bytes[int]` → `int`
    Bytes,
    /// Base or index is `any`.
    Dyn,
}

/// Slicing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SliceOp {
    /// `array[a:b]`
    Array,
    /// `string[a:b]` (byte offsets, must be char boundaries)
    String,
    /// `bytes[a:b]`
    Bytes,
    /// Base is `any`.
    Dyn,
}

/// Predicate builtins. Each stops as soon as its result is decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PredicateOp {
    /// `all`
    All,
    /// `any`
    Any,
    /// `none`
    None,
    /// `one`
    One,
    /// `filter`
    Filter,
    /// `map`
    Map,
    /// `count`
    Count,
    /// `find` → `Option<T>`
    Find,
    /// `findIndex` → `int` (`-1` when not found)
    FindIndex,
    /// `findLast` → `Option<T>`, scanning from the end
    FindLast,
    /// `findLastIndex` → `int` (`-1` when not found), scanning from the end
    FindLastIndex,
    /// `groupBy` → `Map<Array<T>>`, keyed by the body's string, groups in first-seen order
    GroupBy,
    /// `sortBy(xs, key[, order])`: every key first, then a stable sort by the keys' ordering
    SortBy(CompareKind),
    /// `reduce(xs, body[, init])` → the last `#acc`; without `init`, the first element
    /// is the initial `#acc` and the body starts at the second
    Reduce,
}

/// Non-predicate builtins after overload resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Intrinsic {
    /// `len(string)`: number of characters (not bytes), as Go.
    LenString,
    /// `len(bytes)`
    LenBytes,
    /// `len(array)`
    LenArray,
    /// `len(map)`
    LenMap,
    /// `trim(s)`: Unicode white space, as Go's `strings.TrimSpace`.
    Trim,
    /// `trim(s, cutset)`: any character in the cutset, as Go's `strings.Trim`.
    TrimCutset,
    /// `trimPrefix(s[, prefix])`: the prefix defaults to one space, as Go.
    TrimPrefix,
    /// `trimSuffix(s[, suffix])`: the suffix defaults to one space, as Go.
    TrimSuffix,
    /// `upper`
    Upper,
    /// `lower`
    Lower,
    /// `split(s, sep[, n])`, or `splitAfter` (keeping each separator) when `after`.
    /// `n` as Go's `strings.SplitN`: below zero all parts, zero none, else at most `n`.
    Split { after: bool },
    /// `replace`
    Replace,
    /// `repeat(s, n)`: a negative `n` is an invalid argument.
    Repeat,
    /// `hasPrefix`
    HasPrefix,
    /// `hasSuffix`
    HasSuffix,
    /// `indexOf`: byte offset or `-1`, so `s[indexOf(s, "x"):]` is valid.
    IndexOf,
    /// `lastIndexOf`: byte offset or `-1`, as `indexOf`.
    LastIndexOf,
    /// `abs(int)`
    AbsInt,
    /// `abs(float)`
    AbsFloat,
    /// `ceil`
    Ceil,
    /// `floor`
    Floor,
    /// `round` (half away from zero, as Go's `math.Round`)
    Round,
    /// `max`/`min` over arguments or one array.
    Extremum { kind: CompareKind, max: bool, of_array: bool },
    /// `mean`/`median` over number arguments or one array of numbers, as a float;
    /// `0.0` when there are none, as Go.
    Center { median: bool, of_array: bool },
    /// `bitand`, `bitnot` and the other bit operations on ints.
    Bit(BitOp),
    /// `first` → `Option<T>`
    First,
    /// `last` → `Option<T>`
    Last,
    /// `sum(array<int>)`
    SumInt,
    /// `sum(array<float>)`
    SumFloat,
    /// `join`
    Join,
    /// `concat` of one or more arrays.
    Concat,
    /// `flatten`: every nested array, to any depth, spliced into one array.
    Flatten,
    /// `uniq`: each element not equal to an earlier one, by the element's equality.
    Uniq(EqKind),
    /// `take(xs, n)`: the first `n` elements, all of them when there are fewer.
    Take,
    /// `reverse`
    Reverse,
    /// `sort(xs[, order])`: stable, by the element's ordering, NaN first.
    Sort(CompareKind),
    /// `keys` (insertion or container order)
    Keys,
    /// `values`
    Values,
    /// `toPairs`: `[key, value]` for each entry, in the map's order.
    ToPairs,
    /// `fromPairs`: a map from `[key, value]` pairs with string keys, the last of
    /// equal keys winning, in key order.
    FromPairs,
    /// `int(x)` from the given source tag
    ToInt(Tag),
    /// `float(x)`
    ToFloat(Tag),
    /// `string(x)`
    ToString(Tag),
    /// `decimal(x)`
    #[cfg(feature = "decimal")]
    ToDecimal(Tag),
    /// `type(x)`
    TypeName,
    /// `toJSON(x)`: Go's `json.MarshalIndent(x, "", "  ")`.
    ToJson,
    /// `fromJSON(s)`: numbers as floats and objects as maps in key order, as Go's
    /// `json.Unmarshal` into `any`.
    FromJson,
    /// `toBase64(s)`: standard base64 with padding.
    ToBase64,
    /// `fromBase64(s)`: Go's `base64.StdEncoding`; the decoded bytes must be UTF-8.
    FromBase64,
    /// `now()`: the evaluation's timestamp from `Ctx`, the same for every call within
    /// one evaluation (and one `RuleSet::eval`). Fails with `ClockUnset` when the
    /// `Ctx` has no time.
    Now,
    /// `duration(string)`: Go duration syntax (`1h30m`, `-1.5h`, `300ms`).
    ParseDuration,
    /// `date(string)`: RFC 3339, or a date / naive date-time read as UTC.
    ParseDate,
    /// `get(array, int)` → `Option<T>`
    GetArray,
    /// `get(map, string)` → `Option<V>`
    GetMap,
    /// An argument is `any`: resolve at run time with [`crate::resolve::builtin`].
    Dyn(dictum_syntax::Builtin),
}

/// Implicit conversions made explicit by the checker.
#[derive(Clone, Debug, PartialEq)]
pub enum Conversion {
    /// `int` → `float`
    IntToFloat,
    /// `int` → `decimal`
    #[cfg(feature = "decimal")]
    IntToDecimal,
    /// `T` → `Option<T>` (no-op at run time; changes only the static type)
    Some,
    /// `any` → the node's type. Scalars check the tag; arrays and maps are checked
    /// element-wise (charged to the step budget) so typed code downstream never sees
    /// an ill-typed value.
    Check,
    /// `T` → `any` (no-op at run time)
    Erase,
}
