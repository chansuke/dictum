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
#[cfg(feature = "decimal")]
use crate::ir::DecimalOp;
use crate::ir::{BinaryOp, BitOp, CompareKind, EqKind, FloatOp, InOp, IntOp, Intrinsic, UnaryOp};

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
    use Tag as T;
    let plain = |op, result| Some(Binary { op, lhs: Promote::None, rhs: Promote::None, result });
    match op {
        AstBinary::Eq | AstBinary::Ne => {
            let (kind, l, r) = equality(lhs, rhs)?;
            let op = if op == AstBinary::Eq { BinaryOp::Eq(kind) } else { BinaryOp::Ne(kind) };
            return Some(Binary { op, lhs: l, rhs: r, result: T::Bool });
        }
        AstBinary::Matches | AstBinary::Contains | AstBinary::StartsWith | AstBinary::EndsWith => {
            if (lhs, rhs) != (T::String, T::String) {
                return None;
            }
            let op = match op {
                AstBinary::Matches => BinaryOp::Matches,
                AstBinary::Contains => BinaryOp::Contains,
                AstBinary::StartsWith => BinaryOp::StartsWith,
                _ => BinaryOp::EndsWith,
            };
            return plain(op, T::Bool);
        }
        AstBinary::Coalesce | AstBinary::In | AstBinary::NotIn | AstBinary::Range => return None,
        AstBinary::Add | AstBinary::Sub | AstBinary::Mul | AstBinary::Div | AstBinary::Mod | AstBinary::Pow => {}
    }
    // Non-numeric rows.
    match (op, lhs, rhs) {
        (AstBinary::Add, T::String, T::String) => return plain(BinaryOp::Concat, T::String),
        (AstBinary::Add, T::Bytes, T::Bytes) => return plain(BinaryOp::ConcatBytes, T::Bytes),
        (AstBinary::Add, T::Array, T::Array) => return plain(BinaryOp::ConcatArray, T::Array),
        (AstBinary::Sub, T::Time, T::Time) => return plain(BinaryOp::TimeSubTime, T::Duration),
        (AstBinary::Add, T::Time, T::Duration) => return plain(BinaryOp::TimeAddDuration, T::Time),
        (AstBinary::Sub, T::Time, T::Duration) => return plain(BinaryOp::TimeSubDuration, T::Time),
        (AstBinary::Add, T::Duration, T::Duration) => return plain(BinaryOp::DurationAdd, T::Duration),
        (AstBinary::Sub, T::Duration, T::Duration) => return plain(BinaryOp::DurationSub, T::Duration),
        _ => {}
    }
    let (num, l, r) = numeric(lhs, rhs)?;
    let float = |f: FloatOp, l, r| Some(Binary { op: BinaryOp::Float(f), lhs: l, rhs: r, result: T::Float });
    let promote_both = |p: Promote| if p == Promote::None { Promote::IntToFloat } else { p };
    match (num, op) {
        (T::Int, AstBinary::Add) => plain(BinaryOp::Int(IntOp::Add), T::Int),
        (T::Int, AstBinary::Sub) => plain(BinaryOp::Int(IntOp::Sub), T::Int),
        (T::Int, AstBinary::Mul) => plain(BinaryOp::Int(IntOp::Mul), T::Int),
        (T::Int, AstBinary::Mod) => plain(BinaryOp::Int(IntOp::Mod), T::Int),
        // `int / int` and `int ** int` are float, as in Go.
        (T::Int, AstBinary::Div) => float(FloatOp::Div, promote_both(l), promote_both(r)),
        (T::Int, AstBinary::Pow) => float(FloatOp::Pow, promote_both(l), promote_both(r)),
        (T::Float, AstBinary::Add) => float(FloatOp::Add, l, r),
        (T::Float, AstBinary::Sub) => float(FloatOp::Sub, l, r),
        (T::Float, AstBinary::Mul) => float(FloatOp::Mul, l, r),
        (T::Float, AstBinary::Div) => float(FloatOp::Div, l, r),
        (T::Float, AstBinary::Pow) => float(FloatOp::Pow, l, r),
        #[cfg(feature = "decimal")]
        (T::Decimal, op) => {
            let op = match op {
                AstBinary::Add => DecimalOp::Add,
                AstBinary::Sub => DecimalOp::Sub,
                AstBinary::Mul => DecimalOp::Mul,
                AstBinary::Div => DecimalOp::Div,
                AstBinary::Mod => DecimalOp::Mod,
                _ => return None,
            };
            Some(Binary { op: BinaryOp::Decimal(op), lhs: l, rhs: r, result: T::Decimal })
        }
        _ => None,
    }
}

/// The common numeric tag of two operands and how each is promoted to it.
fn numeric(lhs: Tag, rhs: Tag) -> Option<(Tag, Promote, Promote)> {
    use Tag as T;
    match (lhs, rhs) {
        (T::Int, T::Int) => Some((T::Int, Promote::None, Promote::None)),
        (T::Float, T::Float) => Some((T::Float, Promote::None, Promote::None)),
        (T::Int, T::Float) => Some((T::Float, Promote::IntToFloat, Promote::None)),
        (T::Float, T::Int) => Some((T::Float, Promote::None, Promote::IntToFloat)),
        #[cfg(feature = "decimal")]
        (T::Decimal, T::Decimal) => Some((T::Decimal, Promote::None, Promote::None)),
        #[cfg(feature = "decimal")]
        (T::Int, T::Decimal) => Some((T::Decimal, Promote::IntToDecimal, Promote::None)),
        #[cfg(feature = "decimal")]
        (T::Decimal, T::Int) => Some((T::Decimal, Promote::None, Promote::IntToDecimal)),
        _ => None,
    }
}

/// Resolve a prefix operator. `+x` is `x` for a number, so it resolves to no
/// operation; the checker drops it.
pub fn unary(op: AstUnary, arg: Tag) -> Option<(UnaryOp, Tag)> {
    use Tag as T;
    match (op, arg) {
        (AstUnary::Not, T::Bool) => Some((UnaryOp::Not, T::Bool)),
        (AstUnary::Neg, T::Int) => Some((UnaryOp::NegInt, T::Int)),
        (AstUnary::Neg, T::Float) => Some((UnaryOp::NegFloat, T::Float)),
        #[cfg(feature = "decimal")]
        (AstUnary::Neg, T::Decimal) => Some((UnaryOp::NegDecimal, T::Decimal)),
        (AstUnary::Neg, T::Duration) => Some((UnaryOp::NegDuration, T::Duration)),
        _ => None,
    }
}

/// Which equality applies to `lhs == rhs`, with promotions. `None` means the types
/// cannot be compared (E0207), which is a compile error in Go expr too.
///
/// `nil` compares with anything (it is equal only to `nil`). Arrays and maps compare
/// structurally; the checker also requires their elements to be comparable.
pub fn equality(lhs: Tag, rhs: Tag) -> Option<(EqKind, Promote, Promote)> {
    use Tag as T;
    let same = |k| Some((k, Promote::None, Promote::None));
    match (lhs, rhs) {
        (T::Nil, _) | (_, T::Nil) => same(EqKind::Structural),
        (T::Bool, T::Bool) => same(EqKind::Bool),
        (T::String, T::String) => same(EqKind::String),
        (T::Bytes, T::Bytes) => same(EqKind::Bytes),
        (T::Time, T::Time) => same(EqKind::Time),
        (T::Duration, T::Duration) => same(EqKind::Duration),
        (T::Array, T::Array) | (T::Map, T::Map) => same(EqKind::Structural),
        _ => {
            let (num, l, r) = numeric(lhs, rhs)?;
            let kind = match num {
                T::Int => EqKind::Int,
                #[cfg(feature = "decimal")]
                T::Decimal => EqKind::Decimal,
                _ => EqKind::Float,
            };
            Some((kind, l, r))
        }
    }
}

/// Which ordering applies to `lhs < rhs`.
pub fn ordering(lhs: Tag, rhs: Tag) -> Option<(CompareKind, Promote, Promote)> {
    use Tag as T;
    let same = |k| Some((k, Promote::None, Promote::None));
    match (lhs, rhs) {
        (T::String, T::String) => same(CompareKind::String),
        (T::Time, T::Time) => same(CompareKind::Time),
        (T::Duration, T::Duration) => same(CompareKind::Duration),
        _ => {
            let (num, l, r) = numeric(lhs, rhs)?;
            let kind = match num {
                T::Int => CompareKind::Int,
                #[cfg(feature = "decimal")]
                T::Decimal => CompareKind::Decimal,
                _ => CompareKind::Float,
            };
            Some((kind, l, r))
        }
    }
}

/// Which membership test applies to `needle in haystack`. For arrays the element
/// comparison is decided by the caller ([`equality`] on the element type), so this
/// returns `InOp::Array(EqKind::Dyn)` as a placeholder the checker replaces.
pub fn membership(needle: Tag, haystack: Tag) -> Option<InOp> {
    match (needle, haystack) {
        (_, Tag::Array) => Some(InOp::Array(EqKind::Dyn)),
        (Tag::String, Tag::Map) => Some(InOp::MapKey),
        _ => None,
    }
}

/// Resolve `receiver.name(args)`. There are no user methods, so a receiver type that
/// has no method of that name is E0214; nothing resolves yet, because no type has
/// methods until the time builtins land.
///
/// The checker calls this with the receiver's static type, and the evaluator with the
/// tag of the actual value when the receiver is `any` ([`Intrinsic::DynMethod`]).
pub fn method(receiver: Tag, name: &str, args: &[Tag]) -> Option<(Intrinsic, Tag)> {
    let _ = (receiver, name, args);
    None
}

/// How many arguments a builtin takes, as an inclusive range, counting a piped value.
pub const fn arity(b: Builtin) -> (usize, usize) {
    match b {
        Builtin::Now => (0, 0),
        Builtin::Upper
        | Builtin::Lower
        | Builtin::Abs
        | Builtin::Ceil
        | Builtin::Floor
        | Builtin::Round
        | Builtin::BitNot
        | Builtin::First
        | Builtin::Last
        | Builtin::Sum
        | Builtin::Flatten
        | Builtin::Uniq
        | Builtin::Reverse
        | Builtin::Len
        | Builtin::Keys
        | Builtin::Values
        | Builtin::ToPairs
        | Builtin::FromPairs
        | Builtin::Int
        | Builtin::Float
        | Builtin::String
        | Builtin::Type
        | Builtin::Decimal
        | Builtin::ToJson
        | Builtin::FromJson
        | Builtin::ToBase64
        | Builtin::FromBase64
        | Builtin::Duration
        | Builtin::Date => (1, 1),
        Builtin::Repeat
        | Builtin::HasPrefix
        | Builtin::HasSuffix
        | Builtin::IndexOf
        | Builtin::LastIndexOf
        | Builtin::Take
        | Builtin::BitAnd
        | Builtin::BitOr
        | Builtin::BitXor
        | Builtin::BitNand
        | Builtin::BitShl
        | Builtin::BitShr
        | Builtin::BitUshr
        | Builtin::Get
        | Builtin::All
        | Builtin::Any
        | Builtin::None
        | Builtin::One
        | Builtin::Filter
        | Builtin::Map
        | Builtin::Find
        | Builtin::FindIndex
        | Builtin::FindLast
        | Builtin::FindLastIndex
        | Builtin::GroupBy => (2, 2),
        Builtin::Trim | Builtin::TrimPrefix | Builtin::TrimSuffix | Builtin::Count | Builtin::Join | Builtin::Sort => {
            (1, 2)
        }
        Builtin::Split | Builtin::SplitAfter | Builtin::SortBy | Builtin::Reduce => (2, 3),
        Builtin::Replace => (3, 3),
        Builtin::Max | Builtin::Min | Builtin::Mean | Builtin::Median | Builtin::Concat => (1, usize::MAX),
    }
}

/// Resolve a non-predicate builtin from its argument tags. Predicate builtins and
/// `now` have fixed shapes and are typed directly by the checker.
///
/// The returned tag is the result's when it is fixed. Where the result depends on
/// an element type (`first`, `sum`, `max` of an array, `sort` and the other array
/// builtins, `get`, `keys`, `values`, `toPairs`, `fromPairs`), it is the container's
/// tag and the checker computes the full type. `fromJSON` returns `any`, which has no
/// tag; its tag here is the argument's.
pub fn builtin(b: Builtin, args: &[Tag]) -> Option<(Intrinsic, Tag)> {
    use Tag as T;
    let ok = |i, t| Some((i, t));
    match (b, args) {
        (Builtin::Trim, [T::String]) => ok(Intrinsic::Trim, T::String),
        (Builtin::Trim, [T::String, T::String]) => ok(Intrinsic::TrimCutset, T::String),
        (Builtin::TrimPrefix, [T::String] | [T::String, T::String]) => ok(Intrinsic::TrimPrefix, T::String),
        (Builtin::TrimSuffix, [T::String] | [T::String, T::String]) => ok(Intrinsic::TrimSuffix, T::String),
        (Builtin::Upper, [T::String]) => ok(Intrinsic::Upper, T::String),
        (Builtin::Lower, [T::String]) => ok(Intrinsic::Lower, T::String),
        (Builtin::Split | Builtin::SplitAfter, [T::String, T::String] | [T::String, T::String, T::Int]) => {
            ok(Intrinsic::Split { after: b == Builtin::SplitAfter }, T::Array)
        }
        (Builtin::Repeat, [T::String, T::Int]) => ok(Intrinsic::Repeat, T::String),
        (Builtin::Replace, [T::String, T::String, T::String]) => ok(Intrinsic::Replace, T::String),
        (Builtin::HasPrefix, [T::String, T::String]) => ok(Intrinsic::HasPrefix, T::Bool),
        (Builtin::HasSuffix, [T::String, T::String]) => ok(Intrinsic::HasSuffix, T::Bool),
        (Builtin::IndexOf, [T::String, T::String]) => ok(Intrinsic::IndexOf, T::Int),
        (Builtin::LastIndexOf, [T::String, T::String]) => ok(Intrinsic::LastIndexOf, T::Int),
        (Builtin::Abs, [T::Int]) => ok(Intrinsic::AbsInt, T::Int),
        (Builtin::Abs, [T::Float]) => ok(Intrinsic::AbsFloat, T::Float),
        (Builtin::Ceil, [T::Float]) => ok(Intrinsic::Ceil, T::Float),
        (Builtin::Floor, [T::Float]) => ok(Intrinsic::Floor, T::Float),
        (Builtin::Round, [T::Float]) => ok(Intrinsic::Round, T::Float),
        (Builtin::Max | Builtin::Min, [T::Array]) => {
            ok(Intrinsic::Extremum { kind: CompareKind::Dyn, max: b == Builtin::Max, of_array: true }, T::Array)
        }
        (Builtin::Max | Builtin::Min, args)
            if !args.is_empty() && args.iter().all(|t| matches!(t, T::Int | T::Float)) =>
        {
            let float = args.contains(&T::Float);
            let (kind, result) = if float { (CompareKind::Float, T::Float) } else { (CompareKind::Int, T::Int) };
            ok(Intrinsic::Extremum { kind, max: b == Builtin::Max, of_array: false }, result)
        }
        (Builtin::Mean | Builtin::Median, [T::Array]) => {
            ok(Intrinsic::Center { median: b == Builtin::Median, of_array: true }, T::Array)
        }
        (Builtin::Mean | Builtin::Median, args)
            if !args.is_empty() && args.iter().all(|t| matches!(t, T::Int | T::Float)) =>
        {
            ok(Intrinsic::Center { median: b == Builtin::Median, of_array: false }, T::Float)
        }
        (Builtin::BitNot, [T::Int]) => ok(Intrinsic::Bit(BitOp::Not), T::Int),
        (Builtin::BitAnd, [T::Int, T::Int]) => ok(Intrinsic::Bit(BitOp::And), T::Int),
        (Builtin::BitOr, [T::Int, T::Int]) => ok(Intrinsic::Bit(BitOp::Or), T::Int),
        (Builtin::BitXor, [T::Int, T::Int]) => ok(Intrinsic::Bit(BitOp::Xor), T::Int),
        (Builtin::BitNand, [T::Int, T::Int]) => ok(Intrinsic::Bit(BitOp::AndNot), T::Int),
        (Builtin::BitShl, [T::Int, T::Int]) => ok(Intrinsic::Bit(BitOp::Shl), T::Int),
        (Builtin::BitShr, [T::Int, T::Int]) => ok(Intrinsic::Bit(BitOp::Shr), T::Int),
        (Builtin::BitUshr, [T::Int, T::Int]) => ok(Intrinsic::Bit(BitOp::Ushr), T::Int),
        (Builtin::First, [T::Array]) => ok(Intrinsic::First, T::Array),
        (Builtin::Last, [T::Array]) => ok(Intrinsic::Last, T::Array),
        (Builtin::Sum, [T::Array]) => ok(Intrinsic::SumInt, T::Array),
        (Builtin::Len, [T::String]) => ok(Intrinsic::LenString, T::Int),
        (Builtin::Len, [T::Bytes]) => ok(Intrinsic::LenBytes, T::Int),
        (Builtin::Len, [T::Array]) => ok(Intrinsic::LenArray, T::Int),
        (Builtin::Len, [T::Map]) => ok(Intrinsic::LenMap, T::Int),
        (Builtin::Join, [T::Array] | [T::Array, T::String]) => ok(Intrinsic::Join, T::String),
        (Builtin::Concat, args) if !args.is_empty() && args.iter().all(|t| *t == T::Array) => {
            ok(Intrinsic::Concat, T::Array)
        }
        (Builtin::Flatten, [T::Array]) => ok(Intrinsic::Flatten, T::Array),
        // Structural equality compares values of any types, as Go's `runtime.Equal`;
        // the checker narrows it from the element type.
        (Builtin::Uniq, [T::Array]) => ok(Intrinsic::Uniq(EqKind::Structural), T::Array),
        (Builtin::Take, [T::Array, T::Int]) => ok(Intrinsic::Take, T::Array),
        (Builtin::Reverse, [T::Array]) => ok(Intrinsic::Reverse, T::Array),
        (Builtin::Sort, [T::Array] | [T::Array, T::String]) => ok(Intrinsic::Sort(CompareKind::Dyn), T::Array),
        (Builtin::Keys, [T::Map]) => ok(Intrinsic::Keys, T::Array),
        (Builtin::Values, [T::Map]) => ok(Intrinsic::Values, T::Array),
        (Builtin::ToPairs, [T::Map]) => ok(Intrinsic::ToPairs, T::Array),
        (Builtin::FromPairs, [T::Array]) => ok(Intrinsic::FromPairs, T::Map),
        (Builtin::Int, [t @ (T::Int | T::Float | T::String)]) => ok(Intrinsic::ToInt(*t), T::Int),
        (Builtin::Float, [t @ (T::Int | T::Float | T::String)]) => ok(Intrinsic::ToFloat(*t), T::Float),
        (Builtin::String, [t]) => ok(Intrinsic::ToString(*t), T::String),
        (Builtin::Type, [_]) => ok(Intrinsic::TypeName, T::String),
        (Builtin::ToJson, [_]) => ok(Intrinsic::ToJson, T::String),
        (Builtin::FromJson, [T::String]) => ok(Intrinsic::FromJson, T::String),
        (Builtin::ToBase64, [T::String]) => ok(Intrinsic::ToBase64, T::String),
        (Builtin::FromBase64, [T::String]) => ok(Intrinsic::FromBase64, T::String),
        #[cfg(feature = "decimal")]
        (Builtin::Decimal, [t @ (T::Int | T::Float | T::String | T::Decimal)]) => {
            ok(Intrinsic::ToDecimal(*t), T::Decimal)
        }
        (Builtin::Now, []) => ok(Intrinsic::Now, T::Time),
        (Builtin::Duration, [T::String]) => ok(Intrinsic::ParseDuration, T::Duration),
        (Builtin::Date, [T::String]) => ok(Intrinsic::ParseDate, T::Time),
        (Builtin::Get, [T::Array, T::Int]) => ok(Intrinsic::GetArray, T::Array),
        (Builtin::Get, [T::Map, T::String]) => ok(Intrinsic::GetMap, T::Map),
        // Typed by the checker (predicates), or no matching overload.
        (
            Builtin::All
            | Builtin::Any
            | Builtin::None
            | Builtin::One
            | Builtin::Filter
            | Builtin::Map
            | Builtin::Count
            | Builtin::Find
            | Builtin::FindIndex
            | Builtin::FindLast
            | Builtin::FindLastIndex
            | Builtin::GroupBy
            | Builtin::SortBy
            | Builtin::Reduce
            | Builtin::Trim
            | Builtin::TrimPrefix
            | Builtin::TrimSuffix
            | Builtin::Upper
            | Builtin::Lower
            | Builtin::Split
            | Builtin::SplitAfter
            | Builtin::Replace
            | Builtin::Repeat
            | Builtin::HasPrefix
            | Builtin::HasSuffix
            | Builtin::IndexOf
            | Builtin::LastIndexOf
            | Builtin::Max
            | Builtin::Min
            | Builtin::Mean
            | Builtin::Median
            | Builtin::BitAnd
            | Builtin::BitOr
            | Builtin::BitXor
            | Builtin::BitNand
            | Builtin::BitNot
            | Builtin::BitShl
            | Builtin::BitShr
            | Builtin::BitUshr
            | Builtin::Abs
            | Builtin::Ceil
            | Builtin::Floor
            | Builtin::Round
            | Builtin::First
            | Builtin::Last
            | Builtin::Sum
            | Builtin::Len
            | Builtin::Join
            | Builtin::Concat
            | Builtin::Flatten
            | Builtin::Uniq
            | Builtin::Take
            | Builtin::Reverse
            | Builtin::Sort
            | Builtin::Keys
            | Builtin::Values
            | Builtin::ToPairs
            | Builtin::FromPairs
            | Builtin::Int
            | Builtin::Float
            | Builtin::String
            | Builtin::Type
            | Builtin::Decimal
            | Builtin::ToJson
            | Builtin::FromJson
            | Builtin::ToBase64
            | Builtin::FromBase64
            | Builtin::Now
            | Builtin::Duration
            | Builtin::Date
            | Builtin::Get,
            _,
        ) => None,
    }
}
