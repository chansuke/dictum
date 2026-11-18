//! dictum: an expr-lang dialect for rule engines.
//!
//! Compile a rule once against a [`Schema`] (built from one `#[derive(Dictum)]` root
//! struct), then evaluate it many times against borrowed facts:
//!
//! ```ignore
//! use dictum::jiff::Timestamp;
//! use dictum::{Ctx, Dictum, Rule, Schema};
//!
//! #[derive(Dictum)]
//! #[dictum(rename_all = "PascalCase")]
//! struct User { age: i64, group: String }
//!
//! #[derive(Dictum)]
//! struct Facts<'a> { user: &'a User }
//!
//! let schema = Schema::<Facts<'static>>::builder().build()?;
//! let rule: Rule<bool, Facts<'static>> =
//!     Rule::compile(r#"user.Group in ["admin"] && user.Age >= 18"#, &schema)?;
//!
//! let user = User { age: 20, group: "admin".into() };
//! let ok = rule.eval(&Ctx::new(&Facts { user: &user }).with_now(Timestamp::now()))?;
//! ```
//!
//! # Public surface
//!
//! This file is the complete list of public items. Everything under `engine` is
//! private to this crate; `__private` exists only for code `#[derive(Dictum)]`
//! generates and carries no stability promise.
#![no_std]
#![allow(dead_code, unused_variables)] // skeleton: bodies are `todo!()`; remove in Phase D.

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

// Lets `#[derive(Dictum)]` output (which names `::dictum::...`) work inside this crate's tests.
extern crate self as dictum;

mod ctx;
mod dynamic;
mod engine;
mod error;
mod explain;
mod facts;
mod func;
mod output;
mod rule;
mod rule_set;
mod schema;

pub use ctx::{Ctx, Limits};
pub use dictum_check::SchemaError;
pub use dictum_derive::Dictum;
pub use dictum_syntax::{Arg, Diagnostic, ErrorCode, Help, Span};
pub use dynamic::{DynMap, Dynamic};
pub use error::{CompileError, EvalError, EvalErrorKind};
pub use explain::{ExplainNode, NodeStatus, Report};
pub use facts::Dictum;
pub use func::IntoFunc;
pub use output::Output;
pub use rule::Rule;
pub use rule_set::{Outcomes, RuleSet, RuleSetBuilder};
pub use schema::{Schema, SchemaBuilder};

/// The time types in the public API (`Ctx::with_now` takes a `jiff::Timestamp`).
pub use jiff;
/// The decimal type (`decimal` feature).
#[cfg(feature = "decimal")]
pub use rust_decimal;

/// Items named by `#[derive(Dictum)]` output and by the bounds of [`IntoFunc`].
/// Not part of the API: no semver promise, may change in any release.
#[doc(hidden)]
pub mod __private {
    pub use crate::engine::value::{Value, missing_field};
    pub use crate::facts::{Field, MapView, Object, SeqView};
    pub use crate::func::{ErasedFunc, FromArg, Ret, RetValue};
    pub use dictum_check::{FieldDesc, FieldId, StructDesc, Type};
}
