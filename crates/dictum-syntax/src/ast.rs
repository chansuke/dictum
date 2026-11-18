//! The untyped AST. Produced by [`crate::parse`], consumed once by `dictum-check`.
//!
//! The AST borrows identifiers from the source (`&'src str`) instead of interning
//! them: it lives for one compile call, and the typed IR replaces every name with an
//! index (`FieldId`, `SlotId`, `FuncId`), so a symbol table would buy nothing.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::{Builtin, Span};

/// `{ "let" ident "=" expr ";" } expr`. `let` exists only at the top of a program,
/// so it is not an expression kind.
#[derive(Debug)]
pub struct Program<'src> {
    /// Bindings in source order. Binding `i` may refer to bindings `0..i`.
    pub lets: Vec<Let<'src>>,
    /// The rule's result expression.
    pub body: Expr<'src>,
}

/// `let name = value;`
#[derive(Debug)]
pub struct Let<'src> {
    /// The bound name.
    pub name: Ident<'src>,
    /// The bound expression, evaluated once, eagerly, in source order.
    pub value: Expr<'src>,
}

/// An identifier with its position.
#[derive(Clone, Copy, Debug)]
pub struct Ident<'src> {
    /// The identifier text, borrowed from the source.
    pub name: &'src str,
    /// Where it appears.
    pub span: Span,
}

/// One expression node.
#[derive(Debug)]
pub struct Expr<'src> {
    /// What the node is.
    pub kind: ExprKind<'src>,
    /// Where it is.
    pub span: Span,
}

/// Expression kinds after desugaring (see the crate docs for what was desugared).
#[derive(Debug)]
pub enum ExprKind<'src> {
    /// `nil`
    Nil,
    /// `true` / `false`
    Bool(bool),
    /// Integer literal; out-of-range literals are a syntax error (E0104).
    Int(i64),
    /// Float literal.
    Float(f64),
    /// String literal; borrowed when it has no escapes.
    Str(Cow<'src, str>),
    /// `b"..."`
    Bytes(Cow<'src, [u8]>),
    /// `[a, b]`
    Array(Vec<Expr<'src>>),
    /// `{k: v}` in source order.
    Map(Vec<(MapKey<'src>, Expr<'src>)>),
    /// A variable: a `let` binding or a field of the schema root.
    Ident(&'src str),
    /// `$env`: the schema root itself.
    Env,
    /// `#`, `#index`, `#acc` (only inside a predicate).
    Pointer(Pointer),
    /// `not x`, `!x`, `-x`, `+x`
    Unary { op: UnaryOp, expr: Box<Expr<'src>> },
    /// Strict binary operators (both sides always evaluated).
    Binary { op: BinaryOp, lhs: Box<Expr<'src>>, rhs: Box<Expr<'src>> },
    /// Short-circuit `&&`/`||`, flattened. Invariant: `operands.len() >= 2`.
    Logic { op: LogicOp, operands: Vec<Expr<'src>> },
    /// `a < b <= c`. Invariant: `rest.len() >= 1`. Each middle operand is evaluated
    /// once; evaluation stops at the first false comparison.
    Compare { first: Box<Expr<'src>>, rest: Vec<(CompareOp, Expr<'src>)> },
    /// `x.f`, `x?.f`, `x[i]`, `x?.[i]`. `x.f` and `x["f"]` are the same node.
    Member { obj: Box<Expr<'src>>, key: MemberKey<'src>, optional: bool },
    /// `x[a:b]`, `x?.[a:b]`
    Slice { obj: Box<Expr<'src>>, from: Option<Box<Expr<'src>>>, to: Option<Box<Expr<'src>>>, optional: bool },
    /// `name(args)`. Pipes are already folded in as the first argument.
    Call { callee: Callee<'src>, callee_span: Span, args: Vec<Expr<'src>> },
    /// A predicate argument (`{ ... }` or the bare form). Only the parser creates it,
    /// and only at the argument position [`Builtin::predicate_arg`] names.
    Predicate(Box<Expr<'src>>),
    /// `c ? a : b` and `if c { a } else { b }`
    Conditional { cond: Box<Expr<'src>>, then: Box<Expr<'src>>, otherwise: Box<Expr<'src>> },
    /// Placeholder for a region that failed to parse. Never reaches the checker.
    Error,
}

/// Map literal key.
#[derive(Debug)]
pub enum MapKey<'src> {
    /// `ident:` or `"string":`
    Name(Cow<'src, str>, Span),
    /// `(expr):` — must type-check as `String`.
    Computed(Expr<'src>),
}

/// Member key.
#[derive(Debug)]
pub enum MemberKey<'src> {
    /// `.name`
    Field(Ident<'src>),
    /// `[expr]`
    Index(Box<Expr<'src>>),
}

/// Who is being called. Builtins are resolved by the parser because it must know
/// where predicate arguments are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Callee<'src> {
    /// A language builtin.
    Builtin(Builtin),
    /// A schema function; resolved (or rejected) by the checker.
    Func(&'src str),
}

/// Predicate pointers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pointer {
    /// `#`
    Current,
    /// `#index`
    Index,
    /// `#acc` (only inside `reduce`)
    Acc,
}

/// Prefix operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    /// `not` / `!`
    Not,
    /// `-`
    Neg,
    /// `+`
    Plus,
}

/// Strict binary operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/` (always `float`, or `decimal`)
    Div,
    /// `%` (`int` only)
    Mod,
    /// `**` / `^`
    Pow,
    /// `??`
    Coalesce,
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `in`
    In,
    /// `not in`
    NotIn,
    /// `matches`
    Matches,
    /// `contains`
    Contains,
    /// `startsWith`
    StartsWith,
    /// `endsWith`
    EndsWith,
    /// `..`
    Range,
}

/// Short-circuit operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LogicOp {
    /// `and` / `&&`
    And,
    /// `or` / `||`
    Or,
}

/// Chainable comparisons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompareOp {
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
}
