//! Internal crate of dictum: source text to AST.
//!
//! Public items are the contract with `dictum-check` (the AST) and with every
//! later stage (`Span`, `Diagnostic`, `ErrorCode`, `Builtin`). The lexer, tokens
//! and parser state are private to this crate.
//!
//! Invariants this crate establishes for every `Ok(Program)`:
//! - every node's `Span` lies inside the source and on UTF-8 boundaries;
//! - pipes are desugared (`a | f(b)` is `Call f(a, b)`), so the AST has no pipe node;
//! - `.Field` inside a predicate is desugared to `#.Field`;
//! - `a ?: b` is desugared to `a || b` (it is `a ? a : b` with `a: bool`);
//! - `&&`/`||` chains are flattened into one n-ary `Logic` node;
//! - `<`/`>`/`<=`/`>=` chains are one `Compare` node (`a == b == c` is rejected);
//! - `#`, `#index`, `#acc` and `.Field` appear only inside a predicate argument;
//! - `(` follows only a plain identifier (no method or computed calls);
//! - tree depth is at most [`MAX_DEPTH`], so every later recursive pass is bounded.
#![no_std]

extern crate alloc;

pub mod ast;
mod builtin;
mod diag;
mod keyword;
mod lexer;
mod parser;
mod span;

pub use builtin::Builtin;
pub use diag::{Arg, Diagnostic, Diagnostics, ErrorCode, Help, render};
pub use keyword::{is_identifier, is_reserved};
pub use span::Span;

/// Maximum AST depth. Bounds recursion in the parser, checker, closure compiler and
/// the evaluator's native stack. Flattened `&&`/`||` chains count as depth 1, so long
/// conjunctions of conditions never reach it.
pub const MAX_DEPTH: u32 = 128;

/// Maximum diagnostics kept per rule. Later ones are counted, not stored.
pub const MAX_DIAGNOSTICS: usize = 20;

/// Parse one rule.
///
/// Recovers at the sync points `,` `)` `]` `}` `;` and keeps going, so one call
/// reports up to [`MAX_DIAGNOSTICS`] syntax errors. Returns `Err` if any error was
/// found; the checker never sees an AST that contains `ExprKind::Error`.
///
/// A source longer than `u32::MAX` bytes is rejected with `E0100` because spans are
/// `u32` offsets.
pub fn parse(src: &str) -> Result<ast::Program<'_>, Diagnostics> {
    parser::Parser::new(src).program()
}
