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
    /// Index into a side table of length `Program::node_count`.
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// A `let` slot: slot `i` holds `Program::lets()[i]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SlotId(u16);

impl SlotId {
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

    /// Children in evaluation order (for traversals that do not care about kinds).
    pub fn children(&self) -> impl Iterator<Item = &Expr> {
        // TODO: match on kind; yield boxed and Vec children in evaluation order.
        todo!();
        #[allow(unreachable_code)]
        core::iter::empty()
    }

    /// Children, mutably, so the folder can work bottom-up.
    pub fn children_mut(&mut self) -> impl Iterator<Item = &mut Expr> {
        todo!();
        #[allow(unreachable_code)]
        core::iter::empty()
    }

    /// Replace this node by a constant, keeping id, span and type.
    ///
    /// Returns the constant back if it does not inhabit `self.ty()` (a folder bug);
    /// the folder then leaves the node unfolded rather than corrupting the IR.
    pub fn replace_with_const(&mut self, value: Const) -> Result<(), Const> {
        // TODO: if !value.inhabits(&self.ty) { return Err(value) }
        //       self.kind = ExprKind::Const(value)
        todo!()
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
    /// `all(seq, body)` etc. `body` is `None` only where the builtin allows it.
    Predicate { op: PredicateOp, seq: Box<Expr>, body: Option<Box<Expr>> },
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
        todo!()
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
    /// `trim`
    Trim,
    /// `upper`
    Upper,
    /// `lower`
    Lower,
    /// `split`
    Split,
    /// `replace`
    Replace,
    /// `hasPrefix`
    HasPrefix,
    /// `hasSuffix`
    HasSuffix,
    /// `indexOf`: byte offset or `-1`, so `s[indexOf(s, "x"):]` is valid.
    IndexOf,
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
    /// `keys` (insertion or container order)
    Keys,
    /// `values`
    Values,
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
    /// `now()`: the evaluation's timestamp from `Ctx`, the same for every call within
    /// one evaluation (and one `RuleSet::eval`).
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
