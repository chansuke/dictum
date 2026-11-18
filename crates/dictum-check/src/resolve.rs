//! Overload tables: the one place that says which operator or builtin applies to
//! which operand types.
//!
//! The checker calls these with tags derived from static types. The evaluator calls
//! the same functions with tags of runtime values when an operand is `any`
//! (`BinaryOp::Dyn`, `Intrinsic::Dyn`, ...). Static and dynamic typing therefore
//! cannot disagree: there is no second copy of the rules.

use dictum_syntax::Builtin;
use dictum_syntax::ast::{BinaryOp as AstBinary, UnaryOp as AstUnary};

use crate::Tag;
use crate::ir::{BinaryOp, CompareKind, EqKind, InOp, Intrinsic, UnaryOp};

/// Numeric promotion applied to an operand before the operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Promote {
    /// Use as is.
    None,
    /// `int` → `float`
    IntToFloat,
    /// `int` → `decimal`
    #[cfg(feature = "decimal")]
    IntToDecimal,
}

/// Result of resolving a strict binary operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binary {
    /// The monomorphic operation; never `BinaryOp::Dyn`.
    pub op: BinaryOp,
    /// Promotion of the left operand.
    pub lhs: Promote,
    /// Promotion of the right operand.
    pub rhs: Promote,
    /// Tag of the result (`Array` for `ConcatArray`; the checker computes the element
    /// type itself).
    pub result: Tag,
}

/// Resolve `lhs op rhs` for the strict operators (`+ - * / % ** == != matches
/// contains startsWith endsWith`). `??`, `in`, `not in` and `..` have their own
/// entry points or node kinds.
///
/// Key rows (the full table is the body):
/// - `int ⊕ int` for `+ - * %` → `int`; `/` and `**` → both promoted, `float`
/// - `int ⊕ float`, `float ⊕ int` → promote the `int`, `float`
/// - `decimal ⊕ int` → promote the `int`, `decimal`; `decimal ⊕ float` → `None`
///   (the checker reports E0212 for exactly this pair)
/// - `string + string` → `Concat`; `time - time` → `duration`; `time ± duration` → `time`
/// - `==`/`!=` defer to [`equality`]
pub fn binary(op: AstBinary, lhs: Tag, rhs: Tag) -> Option<Binary> {
    todo!()
}

/// Resolve a prefix operator.
pub fn unary(op: AstUnary, arg: Tag) -> Option<(UnaryOp, Tag)> {
    todo!()
}

/// Which equality applies to `lhs == rhs`, with promotions. `None` means the types
/// cannot be compared (E0207), which is a compile error in Go expr too.
pub fn equality(lhs: Tag, rhs: Tag) -> Option<(EqKind, Promote, Promote)> {
    todo!()
}

/// Which ordering applies to `lhs < rhs`.
pub fn ordering(lhs: Tag, rhs: Tag) -> Option<(CompareKind, Promote, Promote)> {
    todo!()
}

/// Which membership test applies to `needle in haystack`.
pub fn membership(needle: Tag, haystack: Tag) -> Option<InOp> {
    todo!()
}

/// Resolve a non-predicate builtin from its argument tags. Predicate builtins and
/// `now` have fixed shapes and are typed directly by the checker.
pub fn builtin(b: Builtin, args: &[Tag]) -> Option<(Intrinsic, Tag)> {
    // TODO: one arm per Builtin; exhaustive match (no `_ =>`), so a new Builtin
    // variant fails the build here until it is typed.
    todo!()
}
