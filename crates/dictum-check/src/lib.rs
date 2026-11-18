//! Internal crate of dictum: AST + schema environment to typed IR.
//!
//! Public items are the contract with the `dictum` crate: [`Type`] and the struct
//! descriptors that `#[derive(Dictum)]` emits, the [`Env`] the checker resolves names
//! against, the typed [`ir`], and the overload tables in [`resolve`] that the
//! evaluator reuses for `any` operands. The checker itself is private.
//!
//! This crate knows types, never values: it has no notion of how a field is read or
//! how `+` is computed. Function implementations ride along as an opaque payload `P`
//! in [`Env<P>`], which is how the schema keeps signature and implementation in one
//! entry without this crate depending on the evaluator.
#![no_std]
#![allow(dead_code, unused_variables)] // skeleton: bodies are `todo!()`; remove in Phase D.

extern crate alloc;

mod checker;
mod env;
pub mod ir;
pub mod resolve;
mod types;

pub use env::{Env, FuncId, FuncSig, SchemaError};
pub use types::{FieldDesc, FieldId, OptionType, StructDesc, Tag, Type};

use dictum_syntax::{Diagnostics, ast};

/// Type-check one parsed rule against `env`.
///
/// `expected` is the result type the caller will convert to (`Type::Any` accepts
/// every type). A result that is assignable but not identical is converted in the IR
/// (`int` to `float`, `T` to `Option<T>`, `any` to `T` with a runtime check).
///
/// On `Ok`, the returned program satisfies every invariant documented on
/// [`ir::Program`]. On `Err`, at most `MAX_DIAGNOSTICS` diagnostics; a type error
/// poisons its node so dependent nodes report nothing further.
pub fn check<P>(program: &ast::Program<'_>, env: &Env<P>, expected: &Type) -> Result<ir::Program, Diagnostics> {
    checker::Checker::new(env).program(program, expected)
}
