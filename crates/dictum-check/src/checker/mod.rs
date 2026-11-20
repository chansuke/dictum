//! The type checker. Private: the crate exposes only `check`.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use dictum_syntax::ast::{self, Callee, MapKey, MemberKey, Pointer};
use dictum_syntax::{Arg, Builtin, Diagnostic, Diagnostics, ErrorCode, Help, Span};

use crate::ir::{
    self, CompareKind, Const, Conversion, EqKind, ExprKind, InOp, IndexOp, Intrinsic, NodeId, PredicateOp, SliceOp,
    SlotId,
};
use crate::resolve::{self, Promote};
use crate::{Env, Tag, Type};

/// The checker's inference result for one node.
///
/// `None` stands where the doc had `Type::Error`: the node is poisoned. It already
/// produced a diagnostic (or depends on one that did), and nothing that depends on
/// it reports again. Keeping it out of `Type` means a poisoned type can never leak
/// into the IR.
type Inferred = Option<ir::Expr>;

/// `let` names visible at a point: `lets[i]` is slot `i`. Later lets shadow earlier
/// ones; rule variables (root fields) are looked up after lets. A poisoned let has
/// no type, and its uses are poisoned silently.
struct LetScope<'src> {
    names: Vec<(&'src str, SlotId, Option<Type>)>,
}

/// The types a predicate body's pointers have.
struct PredicateScope {
    /// `#`
    elem: Type,
    /// `#acc`, in `reduce` only
    acc: Option<Type>,
}

pub(crate) struct Checker<'e, P> {
    env: &'e Env<P>,
    diags: Diagnostics,
    next_id: u32,
    /// Enclosing predicates, innermost last. `#` and `#acc` refer to the last entry.
    predicates: Vec<PredicateScope>,
    max_predicate_depth: u16,
}

/// The type of a value with a fixed scalar tag.
fn scalar(tag: Tag) -> Option<Type> {
    Some(match tag {
        Tag::Nil => Type::Nil,
        Tag::Bool => Type::Bool,
        Tag::Int => Type::Int,
        Tag::Float => Type::Float,
        #[cfg(feature = "decimal")]
        Tag::Decimal => Type::Decimal,
        Tag::String => Type::String,
        Tag::Bytes => Type::Bytes,
        Tag::Time => Type::Time,
        Tag::Duration => Type::Duration,
        Tag::Array | Tag::Map | Tag::Struct => return None,
    })
}

/// The least type both `a` and `b` convert to implicitly, if any.
fn unify(a: &Type, b: &Type) -> Option<Type> {
    if a == b {
        return Some(a.clone());
    }
    match (a, b) {
        (Type::Any, _) | (_, Type::Any) => Some(Type::Any),
        (Type::Nil, t) | (t, Type::Nil) => Some(Type::option(t.clone())),
        (Type::Option(x), t) | (t, Type::Option(x)) => unify(x.inner(), t.non_optional()).map(Type::option),
        (Type::Int, Type::Float) | (Type::Float, Type::Int) => Some(Type::Float),
        #[cfg(feature = "decimal")]
        (Type::Int, Type::Decimal) | (Type::Decimal, Type::Int) => Some(Type::Decimal),
        _ => None,
    }
}

/// How `a == b` compares: nil-aware and element-wise for options, arrays and maps,
/// by [`resolve::equality`] for scalars. `None` if they cannot be compared.
fn equality(a: &Type, b: &Type) -> Option<(EqKind, Promote, Promote)> {
    let structural = Some((EqKind::Structural, Promote::None, Promote::None));
    match (a, b) {
        (Type::Any, _) | (_, Type::Any) => Some((EqKind::Dyn, Promote::None, Promote::None)),
        (Type::Nil, _) | (_, Type::Nil) => structural,
        (Type::Option(x), t) | (t, Type::Option(x)) => {
            equality(x.inner(), t.non_optional()).filter(|(_, l, r)| *l == Promote::None && *r == Promote::None)?;
            structural
        }
        (Type::Array(x), Type::Array(y)) | (Type::Map(x), Type::Map(y)) => {
            equality(x, y).filter(|(_, l, r)| *l == Promote::None && *r == Promote::None)?;
            structural
        }
        _ => resolve::equality(a.tag()?, b.tag()?),
    }
}

/// The element type of `flatten` of an array of `t`: `t` without its array levels,
/// or `any` where a level may or may not be an array.
fn flattened(t: &Type) -> Type {
    match t {
        Type::Array(t) => flattened(t),
        Type::Option(o) if matches!(o.inner(), Type::Array(_)) => Type::Any,
        t => t.clone(),
    }
}

fn op_text(op: ast::BinaryOp) -> &'static str {
    use ast::BinaryOp as B;
    match op {
        B::Add => "+",
        B::Sub => "-",
        B::Mul => "*",
        B::Div => "/",
        B::Mod => "%",
        B::Pow => "**",
        B::Coalesce => "??",
        B::Eq => "==",
        B::Ne => "!=",
        B::In => "in",
        B::NotIn => "not in",
        B::Matches => "matches",
        B::Contains => "contains",
        B::StartsWith => "startsWith",
        B::EndsWith => "endsWith",
        B::Range => "..",
    }
}

fn ty_arg(t: &Type) -> Arg {
    Arg::Type(format!("{t}").into())
}

fn name_arg(s: &str) -> Arg {
    Arg::Name(s.into())
}

impl<'e, P> Checker<'e, P> {
    pub(crate) fn new(env: &'e Env<P>) -> Self {
        Checker { env, diags: Diagnostics::new(), next_id: 0, predicates: Vec::new(), max_predicate_depth: 0 }
    }

    pub(crate) fn program(mut self, program: &ast::Program<'_>, expected: &Type) -> Result<ir::Program, Diagnostics> {
        let mut scope = LetScope { names: Vec::new() };
        let mut lets = Vec::new();
        for (i, l) in program.lets.iter().enumerate() {
            let value = self.expr(&l.value, &scope);
            let slot = SlotId::new(u16::try_from(i).unwrap_or(u16::MAX));
            scope.names.push((l.name.name, slot, value.as_ref().map(|v| v.ty().clone())));
            lets.extend(value);
        }
        let body = self.expr(&program.body, &scope);
        let body = body.and_then(|b| match self.try_assign(b, expected) {
            Ok(b) => Some(b),
            Err(b) => {
                let args = vec![ty_arg(b.ty()), ty_arg(expected)];
                self.error(ErrorCode::ResultType, b.span(), args);
                None
            }
        });
        match body {
            Some(body) if self.diags.is_empty() => {
                Ok(ir::Program::new(lets, body, self.next_id, self.max_predicate_depth))
            }
            _ => Err(self.diags),
        }
    }

    // ---- helpers --------------------------------------------------------------

    fn error(&mut self, code: ErrorCode, span: Span, args: Vec<Arg>) {
        self.diags.push(Diagnostic::new(code, span, args));
    }

    fn mk(&mut self, span: Span, ty: Type, kind: ExprKind) -> ir::Expr {
        let id = self.fresh_id();
        ir::Expr::new(id, span, ty, kind)
    }

    fn convert(&mut self, e: ir::Expr, op: Conversion, to: Type) -> ir::Expr {
        let span = e.span();
        self.mk(span, to, ExprKind::Convert { op, arg: Box::new(e) })
    }

    fn promote(&mut self, e: ir::Expr, p: Promote) -> ir::Expr {
        match p {
            Promote::None => e,
            Promote::IntToFloat => self.convert(e, Conversion::IntToFloat, Type::Float),
            #[cfg(feature = "decimal")]
            Promote::IntToDecimal => self.convert(e, Conversion::IntToDecimal, Type::Decimal),
        }
    }

    /// `e` converted to `to`, or `e` back if no implicit conversion applies.
    fn try_assign(&mut self, e: ir::Expr, to: &Type) -> Result<ir::Expr, ir::Expr> {
        let from = e.ty().clone();
        if from == *to {
            return Ok(e);
        }
        match (&from, to) {
            (_, Type::Any) => Ok(self.convert(e, Conversion::Erase, Type::Any)),
            (Type::Any, _) => Ok(self.convert(e, Conversion::Check, to.clone())),
            (Type::Int, Type::Float) => Ok(self.convert(e, Conversion::IntToFloat, Type::Float)),
            #[cfg(feature = "decimal")]
            (Type::Int, Type::Decimal) => Ok(self.convert(e, Conversion::IntToDecimal, Type::Decimal)),
            (Type::Nil, Type::Option(_)) => Ok(self.convert(e, Conversion::Some, to.clone())),
            (Type::Option(_), Type::Option(_)) => Err(e),
            (_, Type::Option(o)) => {
                let inner = self.try_assign(e, o.inner())?;
                Ok(self.convert(inner, Conversion::Some, to.clone()))
            }
            _ => Err(e),
        }
    }

    /// Make `e` acceptable where `to` is expected, inserting `Convert` nodes:
    /// identical → as is; int→float; int→decimal; T→Option<T> (`Some`); nil→Option<T>;
    /// any→T (`Check`); T→any (`Erase`); array<int>→array<float> is NOT implicit.
    /// Otherwise E0201 at `e.span()` and poisoned.
    fn assign(&mut self, e: ir::Expr, to: &Type) -> Inferred {
        match self.try_assign(e, to) {
            Ok(e) => Some(e),
            Err(e) => {
                self.error(ErrorCode::TypeMismatch, e.span(), vec![ty_arg(to), ty_arg(e.ty())]);
                None
            }
        }
    }

    fn fresh_id(&mut self) -> NodeId {
        let id = NodeId::new(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    /// Infer all of `es`, reporting every error, poisoned if any is.
    fn all(&mut self, es: &[ast::Expr<'_>], lets: &LetScope<'_>) -> Option<Vec<ir::Expr>> {
        let out: Vec<Inferred> = es.iter().map(|e| self.expr(e, lets)).collect();
        out.into_iter().collect()
    }

    // ---- expressions ----------------------------------------------------------

    /// Infer one expression (the rules are the design doc's typing table).
    fn expr(&mut self, e: &ast::Expr<'_>, lets: &LetScope<'_>) -> Inferred {
        use ast::ExprKind as K;
        let span = e.span;
        match &e.kind {
            K::Nil => Some(self.mk(span, Type::Nil, ExprKind::Const(Const::Nil))),
            K::Bool(b) => Some(self.mk(span, Type::Bool, ExprKind::Const(Const::Bool(*b)))),
            K::Int(n) => Some(self.mk(span, Type::Int, ExprKind::Const(Const::Int(*n)))),
            K::Float(f) => Some(self.mk(span, Type::Float, ExprKind::Const(Const::Float(*f)))),
            K::Str(s) => Some(self.mk(span, Type::String, ExprKind::Const(Const::Str(Arc::from(&**s))))),
            K::Bytes(b) => Some(self.mk(span, Type::Bytes, ExprKind::Const(Const::Bytes(Arc::from(&**b))))),
            K::Array(xs) => {
                let items = self.all(xs, lets)?;
                self.array(span, items)
            }
            K::Map(pairs) => self.map(span, pairs, lets),
            K::Ident(name) => self.ident(span, name, lets),
            K::Env => {
                let root = self.env.root();
                Some(self.mk(span, Type::Struct(root), ExprKind::Root))
            }
            K::Pointer(p) => self.pointer(span, *p),
            K::Unary { op, expr } => self.unary(span, *op, expr, lets),
            K::Binary { op, lhs, rhs } => self.binary(span, *op, lhs, rhs, lets),
            K::Logic { op, operands } => {
                let operands = self.all(operands, lets)?;
                let operands: Option<Vec<_>> = operands.into_iter().map(|o| self.assign(o, &Type::Bool)).collect();
                Some(self.mk(span, Type::Bool, ExprKind::Logic { op: *op, operands: operands? }))
            }
            K::Compare { first, rest } => self.compare(span, first, rest, lets),
            K::Member { obj, key, optional } => self.member(span, obj, key, *optional, lets),
            K::Slice { obj, from, to, optional } => {
                self.slice(span, obj, from.as_deref(), to.as_deref(), *optional, lets)
            }
            K::Call { callee: Callee::Builtin(b), callee_span, args } => {
                self.builtin(span, *b, *callee_span, args, lets)
            }
            K::Call { callee: Callee::Func(name), callee_span, args } => {
                self.func(span, name, *callee_span, args, lets)
            }
            // Only the parser creates predicates, as builtin arguments; `builtin` unwraps
            // them, so this is reached only for a body on its own.
            K::Predicate(body) => self.expr(body, lets),
            K::Conditional { cond, then, otherwise } => self.conditional(span, cond, then, otherwise, lets),
            // The parser reported it, and returns `Err` whenever one exists.
            K::Error => None,
        }
    }

    /// Elements unified to one type; `array<any>` if they have none in common.
    fn array(&mut self, span: Span, items: Vec<ir::Expr>) -> Inferred {
        let elem = items
            .iter()
            .map(|i| i.ty().clone())
            .try_fold(None, |acc: Option<Type>, t| {
                Some(Some(match acc {
                    None => t,
                    Some(a) => unify(&a, &t)?,
                }))
            })
            .map_or(Type::Any, |t| t.unwrap_or(Type::Any));
        let items: Vec<ir::Expr> = items
            .into_iter()
            .map(|i| match self.try_assign(i, &elem) {
                Ok(i) | Err(i) => i,
            })
            .collect();
        Some(self.mk(span, Type::array(elem), ExprKind::Array(items)))
    }

    fn map(&mut self, span: Span, pairs: &[(MapKey<'_>, ast::Expr<'_>)], lets: &LetScope<'_>) -> Inferred {
        let mut seen: Vec<&str> = Vec::new();
        let mut keys = Vec::new();
        let mut values = Vec::new();
        let mut poisoned = false;
        for (k, v) in pairs {
            let key = match k {
                MapKey::Name(name, kspan) => {
                    if seen.contains(&&**name) {
                        self.error(ErrorCode::DuplicateKey, *kspan, vec![name_arg(name)]);
                        poisoned = true;
                    }
                    seen.push(name);
                    Some(self.mk(*kspan, Type::String, ExprKind::Const(Const::Str(Arc::from(&**name)))))
                }
                MapKey::Computed(k) => self.expr(k, lets).and_then(|k| self.assign(k, &Type::String)),
            };
            let value = self.expr(v, lets);
            match (key, value) {
                (Some(k), Some(v)) => {
                    keys.push(k);
                    values.push(v);
                }
                _ => poisoned = true,
            }
        }
        if poisoned {
            return None;
        }
        let elem = values
            .iter()
            .map(|v| v.ty().clone())
            .try_fold(None, |acc: Option<Type>, t| {
                Some(Some(match acc {
                    None => t,
                    Some(a) => unify(&a, &t)?,
                }))
            })
            .map_or(Type::Any, |t| t.unwrap_or(Type::Any));
        let pairs = keys
            .into_iter()
            .zip(values)
            .map(|(k, v)| match self.try_assign(v, &elem) {
                Ok(v) | Err(v) => (k, v),
            })
            .collect();
        Some(self.mk(span, Type::map(elem), ExprKind::Map(pairs)))
    }

    fn ident(&mut self, span: Span, name: &str, lets: &LetScope<'_>) -> Inferred {
        if let Some((_, slot, ty)) = lets.names.iter().rev().find(|(n, _, _)| *n == name) {
            let ty = ty.clone()?;
            return Some(self.mk(span, ty, ExprKind::Slot(*slot)));
        }
        let root = self.env.root();
        let Some((field, desc)) = root.field(name) else {
            self.error(ErrorCode::UnknownVariable, span, vec![name_arg(name)]);
            return None;
        };
        let base = self.mk(span, Type::Struct(root), ExprKind::Root);
        Some(self.mk(span, desc.ty(), ExprKind::Field { base: Box::new(base), field, optional: false }))
    }

    fn pointer(&mut self, span: Span, p: Pointer) -> Inferred {
        let scope = self.predicates.last();
        let ty = match (p, scope) {
            (Pointer::Current, Some(s)) => s.elem.clone(),
            (Pointer::Index, Some(_)) => Type::Int,
            (Pointer::Acc, Some(PredicateScope { acc: Some(t), .. })) => t.clone(),
            (Pointer::Acc, _) | (_, None) => {
                let name = match p {
                    Pointer::Current => "#",
                    Pointer::Index => "#index",
                    Pointer::Acc => "#acc",
                };
                self.error(ErrorCode::PointerOutsidePredicate, span, vec![name_arg(name)]);
                return None;
            }
        };
        Some(self.mk(span, ty, ExprKind::Pointer(p)))
    }

    fn unary(&mut self, span: Span, op: ast::UnaryOp, arg: &ast::Expr<'_>, lets: &LetScope<'_>) -> Inferred {
        let arg = self.expr(arg, lets)?;
        if *arg.ty() == Type::Any {
            return Some(self.mk(span, Type::Any, ExprKind::Unary { op: ir::UnaryOp::Dyn(op), arg: Box::new(arg) }));
        }
        let tag = arg.ty().tag();
        if op == ast::UnaryOp::Plus {
            // `+x` is `x` for every type `-x` accepts.
            if tag.is_some_and(|t| resolve::unary(ast::UnaryOp::Neg, t).is_some()) {
                return Some(arg);
            }
        } else if let Some((rop, result)) = tag.and_then(|t| resolve::unary(op, t)) {
            let ty = scalar(result)?;
            return Some(self.mk(span, ty, ExprKind::Unary { op: rop, arg: Box::new(arg) }));
        }
        let expected = if op == ast::UnaryOp::Not { "bool" } else { "a number" };
        self.error(ErrorCode::TypeMismatch, arg.span(), vec![name_arg(expected), ty_arg(arg.ty())]);
        None
    }

    fn binary(
        &mut self,
        span: Span,
        op: ast::BinaryOp,
        lhs: &ast::Expr<'_>,
        rhs: &ast::Expr<'_>,
        lets: &LetScope<'_>,
    ) -> Inferred {
        use ast::BinaryOp as B;
        let l = self.expr(lhs, lets);
        let r = self.expr(rhs, lets);
        let (l, r) = (l?, r?);
        match op {
            B::Coalesce => return self.coalesce(span, l, r),
            B::In | B::NotIn => return self.membership(span, l, r, op == B::NotIn),
            B::Range => {
                let from = self.assign(l, &Type::Int);
                let to = self.assign(r, &Type::Int);
                let kind = ExprKind::Range { from: Box::new(from?), to: Box::new(to?) };
                return Some(self.mk(span, Type::array(Type::Int), kind));
            }
            _ => {}
        }
        let is_test = matches!(op, B::Eq | B::Ne | B::Matches | B::Contains | B::StartsWith | B::EndsWith);
        if *l.ty() == Type::Any || *r.ty() == Type::Any {
            let ty = if is_test { Type::Bool } else { Type::Any };
            let kind = ExprKind::Binary { op: ir::BinaryOp::Dyn(op), lhs: Box::new(l), rhs: Box::new(r) };
            return Some(self.mk(span, ty, kind));
        }
        if matches!(op, B::Eq | B::Ne) {
            let Some((kind, pl, pr)) = equality(l.ty(), r.ty()) else {
                self.error(ErrorCode::NotComparable, span, vec![ty_arg(l.ty()), ty_arg(r.ty())]);
                return None;
            };
            let (l, r) = (self.promote(l, pl), self.promote(r, pr));
            let op = if op == B::Eq { ir::BinaryOp::Eq(kind) } else { ir::BinaryOp::Ne(kind) };
            return Some(self.mk(span, Type::Bool, ExprKind::Binary { op, lhs: Box::new(l), rhs: Box::new(r) }));
        }
        let resolved = match (l.ty().tag(), r.ty().tag()) {
            (Some(lt), Some(rt)) => resolve::binary(op, lt, rt),
            _ => None,
        };
        let Some(b) = resolved else {
            #[cfg(feature = "decimal")]
            {
                let pair = (l.ty(), r.ty());
                if matches!(pair, (Type::Decimal, Type::Float) | (Type::Float, Type::Decimal)) {
                    self.error(ErrorCode::DecimalFloatMix, span, Vec::new());
                    return None;
                }
            }
            let args = vec![name_arg(op_text(op)), ty_arg(l.ty()), ty_arg(r.ty())];
            self.error(ErrorCode::NoOperator, span, args);
            return None;
        };
        let ty = match b.op {
            ir::BinaryOp::ConcatArray => {
                if l.ty() != r.ty() {
                    let args = vec![name_arg(op_text(op)), ty_arg(l.ty()), ty_arg(r.ty())];
                    self.error(ErrorCode::NoOperator, span, args);
                    return None;
                }
                l.ty().clone()
            }
            _ => scalar(b.result)?,
        };
        let (l, r) = (self.promote(l, b.lhs), self.promote(r, b.rhs));
        Some(self.mk(span, ty, ExprKind::Binary { op: b.op, lhs: Box::new(l), rhs: Box::new(r) }))
    }

    /// `a ?? b`: `a`'s non-optional type, optional again only if `b` may be nil.
    fn coalesce(&mut self, span: Span, l: ir::Expr, r: ir::Expr) -> Inferred {
        let ty = match l.ty() {
            Type::Any => Type::Any,
            Type::Nil => r.ty().clone(),
            t => {
                let inner = t.non_optional().clone();
                if r.ty().admits_nil() && *r.ty() != Type::Any { Type::option(inner) } else { inner }
            }
        };
        let r = self.assign(r, &ty)?;
        Some(self.mk(span, ty, ExprKind::Coalesce { lhs: Box::new(l), rhs: Box::new(r) }))
    }

    fn membership(&mut self, span: Span, needle: ir::Expr, haystack: ir::Expr, negated: bool) -> Inferred {
        let op = match haystack.ty() {
            _ if matches!(haystack.kind(), ExprKind::Range { .. }) => {
                let needle = self.assign(needle, &Type::Int)?;
                return self.in_node(span, InOp::Range, needle, haystack, negated);
            }
            Type::Any => InOp::Dyn,
            Type::Array(elem) if **elem == Type::Any || *needle.ty() == Type::Any => InOp::Dyn,
            Type::Array(elem) => {
                let elem = (**elem).clone();
                match equality(needle.ty(), &elem) {
                    Some((kind, p, Promote::None)) => {
                        let needle = self.promote(needle, p);
                        return self.in_node(span, InOp::Array(kind), needle, haystack, negated);
                    }
                    _ => {
                        self.error(ErrorCode::NotComparable, span, vec![ty_arg(needle.ty()), ty_arg(&elem)]);
                        return None;
                    }
                }
            }
            Type::Map(_) => {
                let needle = self.assign(needle, &Type::String)?;
                return self.in_node(span, InOp::MapKey, needle, haystack, negated);
            }
            _ => {
                let args =
                    vec![name_arg(if negated { "not in" } else { "in" }), ty_arg(needle.ty()), ty_arg(haystack.ty())];
                self.error(ErrorCode::NoOperator, span, args);
                return None;
            }
        };
        self.in_node(span, op, needle, haystack, negated)
    }

    fn in_node(&mut self, span: Span, op: InOp, needle: ir::Expr, haystack: ir::Expr, negated: bool) -> Inferred {
        let kind = ExprKind::In { op, needle: Box::new(needle), haystack: Box::new(haystack), negated };
        Some(self.mk(span, Type::Bool, kind))
    }

    /// A comparison chain: every adjacent pair must be ordered, and all operands are
    /// promoted to one kind.
    fn compare(
        &mut self,
        span: Span,
        first: &ast::Expr<'_>,
        rest: &[(ast::CompareOp, ast::Expr<'_>)],
        lets: &LetScope<'_>,
    ) -> Inferred {
        let first = self.expr(first, lets);
        let others: Vec<Inferred> = rest.iter().map(|(_, e)| self.expr(e, lets)).collect();
        let mut operands = vec![first?];
        for o in others {
            operands.push(o?);
        }
        let dynamic = operands.iter().any(|o| *o.ty() == Type::Any);
        let mut kind = None;
        if !dynamic {
            for pair in operands.windows(2) {
                let [a, b] = pair else { continue };
                let k = match (a.ty().tag(), b.ty().tag()) {
                    (Some(x), Some(y)) => resolve::ordering(x, y).map(|(k, _, _)| k),
                    _ => None,
                };
                let Some(k) = k else {
                    self.error(ErrorCode::NotComparable, span, vec![ty_arg(a.ty()), ty_arg(b.ty())]);
                    return None;
                };
                kind = Some(match (kind, k) {
                    (Some(CompareKind::Float), _) | (_, CompareKind::Float) => CompareKind::Float,
                    #[cfg(feature = "decimal")]
                    (Some(CompareKind::Decimal), _) | (_, CompareKind::Decimal) => CompareKind::Decimal,
                    _ => k,
                });
            }
        }
        let kind = kind.unwrap_or(CompareKind::Dyn);
        let target = match kind {
            CompareKind::Float => Some(Type::Float),
            #[cfg(feature = "decimal")]
            CompareKind::Decimal => Some(Type::Decimal),
            _ => None,
        };
        let mut operands = operands.into_iter().map(|o| match &target {
            Some(t) => match self.try_assign(o, t) {
                Ok(o) | Err(o) => o,
            },
            None => o,
        });
        let first = operands.next()?;
        let rest = rest.iter().map(|(op, _)| *op).zip(operands).collect();
        Some(self.mk(span, Type::Bool, ExprKind::Compare { kind, first: Box::new(first), rest }))
    }

    fn member(
        &mut self,
        span: Span,
        obj: &ast::Expr<'_>,
        key: &MemberKey<'_>,
        optional: bool,
        lets: &LetScope<'_>,
    ) -> Inferred {
        let base = self.expr(obj, lets);
        let index = match key {
            MemberKey::Index(k) => Some(self.expr(k, lets)),
            MemberKey::Field(_) => None,
        };
        let base = base?;
        let (inner, through) = match base.ty() {
            Type::Option(o) if optional => (o.inner().clone(), true),
            Type::Option(_) => {
                let d = Diagnostic::new(ErrorCode::OptionalAccess, obj.span, vec![ty_arg(base.ty())]);
                self.diags.push(d.with_help(Help::UseSafeNavigation));
                return None;
            }
            t => (t.clone(), false),
        };
        let wrap = |t: Type| if through { Type::option(t) } else { t };
        // A constant string key names a field, as `.name` does.
        let index = match index {
            Some(Some(i)) => match i.kind() {
                ExprKind::Const(Const::Str(s)) if matches!(inner, Type::Struct(_) | Type::Any) => {
                    Err((s.clone(), i.span()))
                }
                _ => Ok(i),
            },
            Some(None) => return None,
            None => match key {
                MemberKey::Field(f) => Err((Arc::from(f.name), f.span)),
                MemberKey::Index(_) => return None,
            },
        };
        match (&inner, index) {
            (Type::Struct(d), Err((name, nspan))) => {
                let Some((field, desc)) = d.field(&name) else {
                    self.error(ErrorCode::UnknownField, nspan, vec![name_arg(d.name()), name_arg(&name)]);
                    return None;
                };
                let kind = ExprKind::Field { base: Box::new(base), field, optional: through };
                Some(self.mk(span, wrap(desc.ty()), kind))
            }
            (Type::Struct(_), Ok(i)) => {
                self.error(ErrorCode::DynamicFieldKey, i.span(), Vec::new());
                None
            }
            (Type::Any, key) => {
                let index = match key {
                    Ok(i) => i,
                    Err((name, nspan)) => self.mk(nspan, Type::String, ExprKind::Const(Const::Str(name))),
                };
                let kind = ExprKind::Index { base: Box::new(base), index: Box::new(index), op: IndexOp::Dyn, optional };
                Some(self.mk(span, Type::Any, kind))
            }
            (Type::Map(v), key) => {
                let index = match key {
                    Ok(i) => self.assign(i, &Type::String)?,
                    Err((name, nspan)) => self.mk(nspan, Type::String, ExprKind::Const(Const::Str(name))),
                };
                let ty = Type::option((**v).clone());
                let kind = ExprKind::Index {
                    base: Box::new(base),
                    index: Box::new(index),
                    op: IndexOp::Map,
                    optional: through,
                };
                Some(self.mk(span, ty, kind))
            }
            (Type::Array(_) | Type::String | Type::Bytes, Ok(i)) => {
                let index = self.assign(i, &Type::Int)?;
                let (op, ty) = match &inner {
                    Type::Array(t) => (IndexOp::Array, (**t).clone()),
                    Type::String => (IndexOp::String, Type::String),
                    _ => (IndexOp::Bytes, Type::Int),
                };
                let kind = ExprKind::Index { base: Box::new(base), index: Box::new(index), op, optional: through };
                Some(self.mk(span, wrap(ty), kind))
            }
            (_, Err((name, nspan))) => {
                self.error(ErrorCode::UnknownField, nspan, vec![name_arg(&format!("{inner}")), name_arg(&name)]);
                None
            }
            (_, Ok(_)) => {
                let args = vec![name_arg("an array, map, string or bytes"), ty_arg(&inner)];
                self.error(ErrorCode::TypeMismatch, obj.span, args);
                None
            }
        }
    }

    fn slice(
        &mut self,
        span: Span,
        obj: &ast::Expr<'_>,
        from: Option<&ast::Expr<'_>>,
        to: Option<&ast::Expr<'_>>,
        optional: bool,
        lets: &LetScope<'_>,
    ) -> Inferred {
        let base = self.expr(obj, lets);
        let from = from.map(|f| self.expr(f, lets).and_then(|f| self.assign(f, &Type::Int)));
        let to = to.map(|t| self.expr(t, lets).and_then(|t| self.assign(t, &Type::Int)));
        let base = base?;
        let from = from.map_or(Some(None), |f| f.map(|f| Some(Box::new(f))))?;
        let to = to.map_or(Some(None), |t| t.map(|t| Some(Box::new(t))))?;
        let (inner, through) = match base.ty() {
            Type::Option(o) if optional => (o.inner().clone(), true),
            t => (t.clone(), false),
        };
        let op = match inner {
            Type::Array(_) => SliceOp::Array,
            Type::String => SliceOp::String,
            Type::Bytes => SliceOp::Bytes,
            Type::Any => SliceOp::Dyn,
            _ => {
                let args = vec![name_arg("an array, string or bytes"), ty_arg(base.ty())];
                self.error(ErrorCode::TypeMismatch, obj.span, args);
                return None;
            }
        };
        let ty = if through { Type::option(inner) } else { inner };
        let kind = ExprKind::Slice { base: Box::new(base), from, to, op, optional: through };
        Some(self.mk(span, ty, kind))
    }

    fn conditional(
        &mut self,
        span: Span,
        cond: &ast::Expr<'_>,
        then: &ast::Expr<'_>,
        otherwise: &ast::Expr<'_>,
        lets: &LetScope<'_>,
    ) -> Inferred {
        let cond = self.expr(cond, lets).and_then(|c| self.assign(c, &Type::Bool));
        let then = self.expr(then, lets);
        let otherwise_span = otherwise.span;
        let otherwise = self.expr(otherwise, lets);
        let (cond, then, otherwise) = (cond?, then?, otherwise?);
        let Some(ty) = unify(then.ty(), otherwise.ty()) else {
            self.error(ErrorCode::TypeMismatch, otherwise_span, vec![ty_arg(then.ty()), ty_arg(otherwise.ty())]);
            return None;
        };
        let then = self.assign(then, &ty)?;
        let otherwise = self.assign(otherwise, &ty)?;
        let kind = ExprKind::Conditional { cond: Box::new(cond), then: Box::new(then), otherwise: Box::new(otherwise) };
        Some(self.mk(span, ty, kind))
    }

    fn arity_error(&mut self, span: Span, name: &str, (min, max): (usize, usize), found: usize) {
        let takes = if min == max {
            format!("{min}")
        } else if max == usize::MAX {
            format!("at least {min}")
        } else {
            format!("{min} or {max}")
        };
        let found = i64::try_from(found).unwrap_or(i64::MAX);
        self.error(ErrorCode::ArgCount, span, vec![name_arg(name), name_arg(&takes), Arg::Int(found)]);
    }

    fn builtin(
        &mut self,
        span: Span,
        b: Builtin,
        callee_span: Span,
        args: &[ast::Expr<'_>],
        lets: &LetScope<'_>,
    ) -> Inferred {
        let (min, max) = resolve::arity(b);
        if args.len() < min || args.len() > max {
            self.arity_error(callee_span, b.name(), (min, max), args.len());
            return None;
        }
        if b.predicate_arg().is_some() {
            return self.predicate(span, b, args, lets);
        }
        let args = self.all(args, lets)?;
        // `ceil`, `floor` and `round` of an int: the int as a float, as Go does.
        let args: Vec<ir::Expr> = if matches!(b, Builtin::Ceil | Builtin::Floor | Builtin::Round) {
            args.into_iter().map(|a| self.promote_int(a)).collect()
        } else {
            args
        };
        // Defined on every type, options and `any` included.
        let any_type = match b {
            Builtin::Type => Some(Intrinsic::TypeName),
            Builtin::ToJson => Some(Intrinsic::ToJson),
            _ => None,
        };
        if let Some(op) = any_type {
            return Some(self.mk(span, Type::String, ExprKind::Intrinsic { op, args }));
        }
        if args.iter().any(|a| *a.ty() == Type::Any) {
            return Some(self.mk(span, Type::Any, ExprKind::Intrinsic { op: Intrinsic::Dyn(b), args }));
        }
        let tags: Option<Vec<Tag>> = args.iter().map(|a| a.ty().tag()).collect();
        let Some((op, result)) = tags.as_deref().and_then(|t| resolve::builtin(b, t)) else {
            let found: Vec<String> = args.iter().map(|a| format!("{}", a.ty())).collect();
            let args = vec![name_arg(&format!("arguments of `{}`", b.name())), name_arg(&found.join(", "))];
            self.error(ErrorCode::TypeMismatch, span, args);
            return None;
        };
        let elem = |a: &ir::Expr| match a.ty() {
            Type::Array(t) | Type::Map(t) => (**t).clone(),
            _ => Type::Any,
        };
        let (op, ty, args) = match op {
            Intrinsic::Split { .. } => (op, Type::array(Type::String), args),
            Intrinsic::First | Intrinsic::Last | Intrinsic::GetArray | Intrinsic::GetMap => {
                let t = args.first().map(elem).unwrap_or(Type::Any);
                (op, Type::option(t), args)
            }
            Intrinsic::Keys => (op, Type::array(Type::String), args),
            Intrinsic::ToPairs => (op, Type::array(Type::array(Type::Any)), args),
            Intrinsic::FromPairs => {
                let pair = args.first().map(elem).unwrap_or(Type::Any);
                let key = match &pair {
                    Type::Array(t) => t.tag(),
                    t if t.tag().is_some() => return self.element_error(span, "an array of pairs", &args),
                    _ => None,
                };
                if key.is_some_and(|k| k != Tag::String) {
                    return self.element_error(span, "an array of pairs with string keys", &args);
                }
                (op, Type::map(Type::Any), args)
            }
            Intrinsic::FromJson => (op, Type::Any, args),
            Intrinsic::Values => {
                let t = args.first().map(elem).unwrap_or(Type::Any);
                (op, Type::array(t), args)
            }
            Intrinsic::SumInt => match args.first().map(elem) {
                Some(Type::Int) => (Intrinsic::SumInt, Type::Int, args),
                Some(Type::Float) => (Intrinsic::SumFloat, Type::Float, args),
                Some(Type::Any) => (Intrinsic::Dyn(b), Type::Any, args),
                _ => return self.element_error(span, "an array of numbers", &args),
            },
            Intrinsic::Join => match args.first().map(elem) {
                Some(Type::String) => (op, Type::String, args),
                Some(Type::Any) => (Intrinsic::Dyn(b), Type::Any, args),
                _ => return self.element_error(span, "an array of strings", &args),
            },
            Intrinsic::Extremum { max, of_array: true, .. } => {
                let t = args.first().map(elem).unwrap_or(Type::Any);
                let kind = match t {
                    Type::Int => CompareKind::Int,
                    Type::Float => CompareKind::Float,
                    Type::String => CompareKind::String,
                    Type::Time => CompareKind::Time,
                    Type::Duration => CompareKind::Duration,
                    Type::Any => CompareKind::Dyn,
                    _ => return self.element_error(span, "an array of ordered values", &args),
                };
                (Intrinsic::Extremum { kind, max, of_array: true }, Type::option(t), args)
            }
            Intrinsic::Center { of_array: true, .. } => match args.first().map(elem) {
                Some(Type::Int | Type::Float | Type::Any) => (op, Type::Float, args),
                _ => return self.element_error(span, "an array of numbers", &args),
            },
            Intrinsic::Concat => {
                let first = args.first().map(elem).unwrap_or(Type::Any);
                let t = if args.iter().all(|a| elem(a) == first) { first } else { Type::Any };
                (op, Type::array(t), args)
            }
            Intrinsic::Flatten => {
                let t = args.first().map(elem).map_or(Type::Any, |t| flattened(&t));
                (op, Type::array(t), args)
            }
            Intrinsic::Take | Intrinsic::Reverse => {
                let ty = args.first().map_or(Type::Any, |a| a.ty().clone());
                (op, ty, args)
            }
            Intrinsic::Uniq(_) => {
                let t = args.first().map(elem).unwrap_or(Type::Any);
                let Some((kind, ..)) = equality(&t, &t) else {
                    self.error(ErrorCode::NotComparable, span, vec![ty_arg(&t), ty_arg(&t)]);
                    return None;
                };
                let kind = if kind == EqKind::Dyn { EqKind::Structural } else { kind };
                (Intrinsic::Uniq(kind), Type::array(t), args)
            }
            Intrinsic::Sort(_) => {
                let t = args.first().map(elem).unwrap_or(Type::Any);
                let kind = match &t {
                    Type::Any => CompareKind::Dyn,
                    t => match t.tag().and_then(|tag| resolve::ordering(tag, tag)) {
                        Some((kind, ..)) => kind,
                        None => return self.element_error(span, "an array of ordered values", &args),
                    },
                };
                (Intrinsic::Sort(kind), Type::array(t), args)
            }
            Intrinsic::Extremum { kind: CompareKind::Float, of_array: false, .. } => {
                let args = args.into_iter().map(|a| self.promote_int(a)).collect();
                (op, Type::Float, args)
            }
            _ => (op, scalar(result)?, args),
        };
        Some(self.mk(span, ty, ExprKind::Intrinsic { op, args }))
    }

    fn promote_int(&mut self, a: ir::Expr) -> ir::Expr {
        if *a.ty() == Type::Int { self.promote(a, Promote::IntToFloat) } else { a }
    }

    fn element_error(&mut self, span: Span, expected: &str, args: &[ir::Expr]) -> Inferred {
        let found = args.first().map_or_else(|| "nothing".into(), |a| format!("{}", a.ty()));
        self.error(ErrorCode::TypeMismatch, span, vec![name_arg(expected), name_arg(&found)]);
        None
    }

    /// `all(xs, p)` and the other predicate builtins: `#` is an element of `xs`.
    fn predicate(&mut self, span: Span, b: Builtin, args: &[ast::Expr<'_>], lets: &LetScope<'_>) -> Inferred {
        let [seq, rest @ ..] = args else { return None };
        let seq = self.expr(seq, lets)?;
        let elem = match seq.ty() {
            Type::Array(t) => (**t).clone(),
            Type::Any => Type::Any,
            other => {
                self.error(ErrorCode::TypeMismatch, seq.span(), vec![name_arg("an array"), ty_arg(other)]);
                return None;
            }
        };
        let op = match b {
            Builtin::All => PredicateOp::All,
            Builtin::Any => PredicateOp::Any,
            Builtin::None => PredicateOp::None,
            Builtin::One => PredicateOp::One,
            Builtin::Filter => PredicateOp::Filter,
            Builtin::Map => PredicateOp::Map,
            Builtin::Count => PredicateOp::Count,
            Builtin::Find => PredicateOp::Find,
            Builtin::FindLast => PredicateOp::FindLast,
            Builtin::FindLastIndex => PredicateOp::FindLastIndex,
            Builtin::GroupBy => PredicateOp::GroupBy,
            // The key's ordering is set once the body is typed.
            Builtin::SortBy => PredicateOp::SortBy(CompareKind::Dyn),
            Builtin::Reduce => PredicateOp::Reduce,
            _ => PredicateOp::FindIndex,
        };
        // The third argument is outside the body's scope: `#` there is the enclosing
        // predicate's.
        let arg = match rest.get(1) {
            Some(a) => {
                let a = self.expr(a, lets)?;
                Some(if op == PredicateOp::Reduce { a } else { self.assign(a, &Type::String)? })
            }
            None => None,
        };
        // `#acc` starts as `init`, or as the first element without one, and every
        // step's body must keep that type.
        let acc = (op == PredicateOp::Reduce).then(|| arg.as_ref().map_or_else(|| elem.clone(), |a| a.ty().clone()));
        let body = match rest.first() {
            Some(arg) => {
                let body = match &arg.kind {
                    ast::ExprKind::Predicate(body) => body,
                    _ => arg,
                };
                self.predicates.push(PredicateScope { elem: elem.clone(), acc: acc.clone() });
                let depth = u16::try_from(self.predicates.len()).unwrap_or(u16::MAX);
                self.max_predicate_depth = self.max_predicate_depth.max(depth);
                let body = self.expr(body, lets);
                self.predicates.pop();
                let body = body?;
                Some(match (op, &acc) {
                    (PredicateOp::Map | PredicateOp::SortBy(_), _) => body,
                    (PredicateOp::GroupBy, _) => self.assign(body, &Type::String)?,
                    (PredicateOp::Reduce, Some(acc)) => self.assign(body, acc)?,
                    _ => self.assign(body, &Type::Bool)?,
                })
            }
            // `count(xs)` counts the `true` elements.
            None => {
                if elem != Type::Bool && elem != Type::Any {
                    self.error(ErrorCode::TypeMismatch, seq.span(), vec![name_arg("array<bool>"), ty_arg(seq.ty())]);
                    return None;
                }
                None
            }
        };
        let op = match (op, &body) {
            (PredicateOp::SortBy(_), Some(key)) => match key.ty() {
                Type::Any => PredicateOp::SortBy(CompareKind::Dyn),
                t => match t.tag().and_then(|tag| resolve::ordering(tag, tag)) {
                    Some((kind, ..)) => PredicateOp::SortBy(kind),
                    None => {
                        self.error(ErrorCode::TypeMismatch, key.span(), vec![name_arg("an ordered key"), ty_arg(t)]);
                        return None;
                    }
                },
            },
            (op, _) => op,
        };
        let ty = match op {
            PredicateOp::All | PredicateOp::Any | PredicateOp::None | PredicateOp::One => Type::Bool,
            PredicateOp::Filter | PredicateOp::SortBy(_) => Type::array(elem),
            PredicateOp::Map => Type::array(body.as_ref().map_or(Type::Any, |b| b.ty().clone())),
            PredicateOp::Count | PredicateOp::FindIndex | PredicateOp::FindLastIndex => Type::Int,
            PredicateOp::Find | PredicateOp::FindLast => Type::option(elem),
            PredicateOp::GroupBy => Type::map(Type::array(elem)),
            PredicateOp::Reduce => acc.unwrap_or(Type::Any),
        };
        let kind = ExprKind::Predicate { op, seq: Box::new(seq), arg: arg.map(Box::new), body: body.map(Box::new) };
        Some(self.mk(span, ty, kind))
    }

    fn func(
        &mut self,
        span: Span,
        name: &str,
        callee_span: Span,
        args: &[ast::Expr<'_>],
        lets: &LetScope<'_>,
    ) -> Inferred {
        let args = self.all(args, lets);
        let env = self.env;
        let Some((func, sig)) = env.func(name) else {
            self.error(ErrorCode::UnknownFunction, callee_span, vec![name_arg(name)]);
            return None;
        };
        let args = args?;
        if args.len() != sig.params.len() {
            self.arity_error(callee_span, name, (sig.params.len(), sig.params.len()), args.len());
            return None;
        }
        let args: Option<Vec<ir::Expr>> =
            args.into_iter().zip(sig.params.iter()).map(|(a, p)| self.assign(a, p)).collect();
        Some(self.mk(span, sig.ret.clone(), ExprKind::Call { func, args: args? }))
    }
}
