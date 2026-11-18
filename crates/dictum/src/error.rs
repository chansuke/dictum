//! Errors. One type per operation: compiling returns [`CompileError`], evaluating
//! returns [`EvalError`]. There is no umbrella enum; both implement
//! `core::error::Error`, so `?` into a caller's own error type works.

use alloc::boxed::Box;
use alloc::sync::Arc;
use core::fmt;

use dictum_syntax::{Diagnostic, ErrorCode, Span};

/// A rule failed to compile: one or more diagnostics (at most 20), by position.
///
/// Holds the source so `Display` can point at it without the caller keeping it.
#[derive(Debug)]
pub struct CompileError {
    pub(crate) source: Arc<str>,
    pub(crate) rule: Option<Arc<str>>,
    /// Invariant: non-empty, sorted by span start.
    pub(crate) diagnostics: Box<[Diagnostic]>,
    /// Diagnostics beyond the per-rule limit, counted but not kept.
    pub(crate) dropped: usize,
}

impl CompileError {
    /// The diagnostics, by position. Never empty.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// The rule name when the rule was inserted into a `RuleSetBuilder`.
    pub fn rule(&self) -> Option<&str> {
        self.rule.as_deref()
    }

    /// The rule source.
    pub fn source_text(&self) -> &str {
        &self.source
    }
}

impl fmt::Display for CompileError {
    /// Every diagnostic in caret format, then "and N more" if any were dropped.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let origin = self.rule.as_deref().unwrap_or("rule");
        for (i, d) in self.diagnostics.iter().enumerate() {
            if i > 0 {
                f.write_str("\n\n")?;
            }
            dictum_syntax::render(f, &self.source, origin, d.code(), d.span(), d.args(), d.help())?;
        }
        if self.dropped > 0 {
            let s = if self.dropped == 1 { "" } else { "s" };
            write!(f, "\n\nand {} more error{s}", self.dropped)?;
        }
        Ok(())
    }
}

impl core::error::Error for CompileError {}

/// A rule failed while evaluating. Evaluation stops at the first error of a rule.
#[derive(Debug)]
pub struct EvalError {
    pub(crate) kind: EvalErrorKind,
    pub(crate) span: Span,
    pub(crate) source: Arc<str>,
    pub(crate) rule: Option<Arc<str>>,
}

impl EvalError {
    /// What went wrong.
    pub fn kind(&self) -> &EvalErrorKind {
        &self.kind
    }

    /// Where, in the rule source.
    pub fn span(&self) -> Span {
        self.span
    }

    /// The E03xx code.
    pub fn code(&self) -> ErrorCode {
        self.kind.code()
    }

    /// The rule name, for rules in a `RuleSet`.
    pub fn rule(&self) -> Option<&str> {
        self.rule.as_deref()
    }
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl core::error::Error for EvalError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match &self.kind {
            EvalErrorKind::Function { error, .. } => Some(error.as_ref()),
            _ => None,
        }
    }
}

/// Runtime error kinds.
#[derive(Debug)]
#[non_exhaustive]
pub enum EvalErrorKind {
    /// `int` or `decimal` arithmetic overflowed.
    Overflow,
    /// `x % 0`, decimal `/ 0`.
    DivisionByZero,
    /// An index outside `-len..len`.
    IndexOutOfBounds {
        /// The index as written (negative counts from the end).
        index: i64,
        /// The length indexed into.
        len: u64,
    },
    /// A string index or slice bound inside a multi-byte character. Indices are byte
    /// offsets while `len` counts characters; the diagnostic carries `Help::ByteIndices`.
    CharBoundary {
        /// The offending byte offset.
        index: i64,
    },
    /// `Limits::max_steps` exceeded.
    StepLimit,
    /// `Limits::max_alloc` exceeded.
    AllocLimit,
    /// A value of type `any` did not have the type its use requires.
    TypeMismatch {
        /// What the operation needed.
        expected: &'static str,
        /// What the value was.
        found: &'static str,
    },
    /// A builtin rejected its argument (`duration("1x")`, `int("abc")`, a dynamic
    /// regex that does not compile).
    InvalidArgument {
        /// What was being parsed (`"duration"`, `"regex"`, ...).
        what: &'static str,
    },
    /// A map literal with computed keys produced the same key twice.
    DuplicateKey(Box<str>),
    /// `now()` was called on a `Ctx` built without [`Ctx::with_now`](crate::Ctx::with_now).
    ClockUnset,
    /// A schema function returned `Err`.
    Function {
        /// The function's name.
        name: Box<str>,
        /// What it returned.
        error: Box<dyn core::error::Error + Send + Sync>,
    },
    /// A dictum bug (an IR invariant did not hold). Reported instead of panicking.
    Internal(&'static str),
}

impl EvalErrorKind {
    /// The diagnostic code of this kind.
    pub fn code(&self) -> ErrorCode {
        match self {
            EvalErrorKind::Overflow => ErrorCode::Overflow,
            EvalErrorKind::DivisionByZero => ErrorCode::DivisionByZero,
            EvalErrorKind::IndexOutOfBounds { .. } => ErrorCode::IndexOutOfBounds,
            EvalErrorKind::CharBoundary { .. } => ErrorCode::CharBoundary,
            EvalErrorKind::StepLimit => ErrorCode::StepLimit,
            EvalErrorKind::AllocLimit => ErrorCode::AllocLimit,
            EvalErrorKind::TypeMismatch { .. } => ErrorCode::RuntimeType,
            EvalErrorKind::InvalidArgument { .. } => ErrorCode::InvalidArgument,
            EvalErrorKind::DuplicateKey(_) => ErrorCode::RuntimeDuplicateKey,
            EvalErrorKind::ClockUnset => ErrorCode::ClockUnset,
            EvalErrorKind::Function { .. } => ErrorCode::FunctionFailed,
            EvalErrorKind::Internal(_) => ErrorCode::Internal,
        }
    }
}
