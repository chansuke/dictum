//! The type checker. Private: the crate exposes only `check`.

use alloc::vec::Vec;

use dictum_syntax::ast;
use dictum_syntax::{Diagnostics, Span};

use crate::ir::{self, NodeId, SlotId};
use crate::{Env, Type};

/// The checker's inference result for one node.
///
/// `Poisoned` stands where the doc had `Type::Error`: the node already produced a
/// diagnostic, and nothing that depends on it reports again. Keeping it out of
/// `Type` means a poisoned type can never leak into the IR.
enum Inferred {
    Ok(ir::Expr),
    Poisoned,
}

/// `let` names visible at a point: `lets[i]` is slot `i`. Later lets shadow earlier
/// ones; rule variables (root fields) are looked up after lets.
struct LetScope<'src> {
    names: Vec<(&'src str, SlotId, Type)>,
}

pub(crate) struct Checker<'e, P> {
    env: &'e Env<P>,
    diags: Diagnostics,
    next_id: u32,
    /// Element types of enclosing predicates, innermost last. `#` is the last entry.
    predicates: Vec<Type>,
    max_predicate_depth: u16,
}

impl<'e, P> Checker<'e, P> {
    pub(crate) fn new(env: &'e Env<P>) -> Self {
        Checker { env, diags: Diagnostics::new(), next_id: 0, predicates: Vec::new(), max_predicate_depth: 0 }
    }

    pub(crate) fn program(self, program: &ast::Program<'_>, expected: &Type) -> Result<ir::Program, Diagnostics> {
        // TODO:
        // for each let: infer value with the lets seen so far; push (name, slot, ty)
        // infer body; then `self.assign(body, expected)` (E0209 on failure)
        // if !self.diags.is_empty() -> Err(self.diags)
        todo!()
    }

    /// Infer one expression.
    ///
    /// TODO, per kind (the rules are the design doc's typing table):
    /// - Ident: let scope, else `env.root().field(name)` as `Field { base: Root }`,
    ///   else E0202.
    /// - Member on Struct with a constant key: `StructDesc::field` (E0204); a
    ///   non-constant key on a struct is E0213. On `Option<Struct>` without `?.`:
    ///   E0206 + Help::UseSafeNavigation. `?.` on a non-optional base behaves as `.`.
    ///   Member on Map: `Index { op: Map }` → `Option<V>`.
    /// - Binary/Unary: tags from operand types → `resolve::binary/unary`; operands of
    ///   type `any` → `BinaryOp::Dyn` with result `any`. Insert `Convert` for promotions.
    ///   decimal⊕float → E0212; otherwise no resolution → E0208.
    /// - Compare chain: unify all operands with `resolve::ordering` pairwise; one kind.
    /// - `??`: lhs must be optional (or `any`); result is the unification of the
    ///   inner type and rhs.
    /// - Conditional: cond bool; branches unified (T|T→T, T|nil→Option<T>,
    ///   int|float→float, else E0201).
    /// - Call(Builtin) with predicate: infer seq (array<T> or a Range), push T onto
    ///   `predicates`, infer body, pop. Body type per op (bool for all/any/..., any T
    ///   for map). Other builtins: `resolve::builtin`.
    /// - Call(Func): `env.func(name)` (E0203), arity (E0205), assign each arg.
    /// - Pointer: `predicates.last()` (the parser already guaranteed we are inside one).
    fn expr(&mut self, e: &ast::Expr<'_>, lets: &LetScope<'_>) -> Inferred {
        todo!()
    }

    /// Make `e` acceptable where `to` is expected, inserting `Convert` nodes:
    /// identical → as is; int→float; int→decimal; T→Option<T> (`Some`); nil→Option<T>;
    /// any→T (`Check`); T→any (`Erase`); array<int>→array<float> is NOT implicit.
    /// Otherwise E0201 at `e.span()` and `Poisoned`.
    fn assign(&mut self, e: ir::Expr, to: &Type, at: Span) -> Inferred {
        todo!()
    }

    fn fresh_id(&mut self) -> NodeId {
        todo!()
    }
}
