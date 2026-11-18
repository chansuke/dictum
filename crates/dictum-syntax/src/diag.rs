//! Diagnostics shared by every stage.
//!
//! `ErrorCode` is the single list of every error dictum can report, compile-time and
//! runtime. Each code carries its English message template; a translation keys on
//! `(code, args)` and never parses the English text.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;

use crate::{MAX_DIAGNOSTICS, Span};

macro_rules! error_codes {
    ($( $variant:ident = $code:literal, $template:literal; )*) => {
        /// Every error dictum reports. E00xx API, E01xx syntax, E02xx types,
        /// E03xx evaluation, E09xx internal.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum ErrorCode {
            $(
                #[doc = concat!("`", $code, "`: ", $template)]
                $variant,
            )*
        }

        impl ErrorCode {
            /// `"E0201"` etc.
            pub const fn as_str(self) -> &'static str {
                match self { $(ErrorCode::$variant => $code,)* }
            }

            /// English template; `{0}`, `{1}` are replaced by `Diagnostic::args`.
            pub const fn template(self) -> &'static str {
                match self { $(ErrorCode::$variant => $template,)* }
            }
        }
    };
}

error_codes! {
    // E00xx: API misuse that is reported like a diagnostic.
    DuplicateRule = "E0001", "a rule named `{0}` is already in this set";
    // E01xx: syntax.
    SourceTooLarge = "E0100", "rule source is larger than 4 GiB";
    UnexpectedToken = "E0101", "expected {0}, found {1}";
    UnterminatedString = "E0102", "unterminated string literal";
    InvalidEscape = "E0103", "invalid escape `{0}`";
    IntegerTooLarge = "E0104", "integer literal does not fit in int";
    TooDeep = "E0105", "expression is nested more than {0} levels deep";
    ChainedEquality = "E0106", "`{0}` cannot be chained; use `&&`";
    MixedCoalesce = "E0107", "`??` cannot be mixed with `{0}` without parentheses";
    NotCallable = "E0108", "only named functions can be called";
    PointerOutsidePredicate = "E0109", "`{0}` is only valid inside a predicate";
    InvalidNumber = "E0110", "invalid number literal";
    // E02xx: types and names.
    TypeMismatch = "E0201", "expected {0}, found {1}";
    UnknownVariable = "E0202", "unknown variable `{0}`";
    UnknownFunction = "E0203", "unknown function `{0}`";
    UnknownField = "E0204", "`{0}` has no field `{1}`";
    ArgCount = "E0205", "`{0}` takes {1} arguments, found {2}";
    OptionalAccess = "E0206", "`{0}` may be nil";
    NotComparable = "E0207", "{0} and {1} cannot be compared";
    NoOperator = "E0208", "no operator `{0}` for {1} and {2}";
    ResultType = "E0209", "rule returns {0}, expected {1}";
    DuplicateKey = "E0210", "duplicate map key `{0}`";
    InvalidPattern = "E0211", "invalid regular expression: {0}";
    DecimalFloatMix = "E0212", "decimal and float cannot be mixed; use decimal() or float()";
    DynamicFieldKey = "E0213", "a struct field must be named by a constant";
    // E03xx: evaluation.
    Overflow = "E0301", "integer overflow";
    DivisionByZero = "E0302", "division by zero";
    IndexOutOfBounds = "E0303", "index {0} is out of bounds for length {1}";
    CharBoundary = "E0304", "byte index {0} is not a character boundary";
    StepLimit = "E0305", "step limit exceeded";
    AllocLimit = "E0306", "allocation limit exceeded";
    RuntimeType = "E0307", "expected {0}, found {1}";
    FunctionFailed = "E0308", "function `{0}` failed: {1}";
    InvalidArgument = "E0309", "invalid {0}: `{1}`";
    RuntimeDuplicateKey = "E0310", "duplicate map key `{0}`";
    ClockUnset = "E0311", "`now()` needs a time; pass one with `Ctx::with_now`";
    // E09xx: a dictum bug. Never expected; reported instead of panicking.
    Internal = "E0900", "internal error: {0}";
}

/// A message argument, kept structured so messages can be translated.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Arg {
    /// A rendered type such as `Option<int>`.
    Type(Box<str>),
    /// An identifier or token as written.
    Name(Box<str>),
    /// A count or index.
    Int(i64),
    /// Free text from an external source (regex errors, function errors).
    Text(Box<str>),
}

/// A follow-up hint, also translatable by code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Help {
    /// "use `?.` to access a field of a value that may be nil"
    UseSafeNavigation,
    /// "add parentheses"
    AddParentheses,
    /// "string indices are byte offsets; `len` counts characters, so `len(s)` is not
    /// a valid index into non-ASCII text — use `indexOf` for byte positions"
    ByteIndices,
    /// "write `x == y && y == z`"
    SplitComparison,
}

/// One located message.
#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    code: ErrorCode,
    span: Span,
    args: Vec<Arg>,
    help: Option<Help>,
}

impl Diagnostic {
    /// A new diagnostic; `args` fill the code's template in order.
    pub fn new(code: ErrorCode, span: Span, args: Vec<Arg>) -> Self {
        Diagnostic { code, span, args, help: None }
    }

    /// Attach a hint.
    pub fn with_help(mut self, help: Help) -> Self {
        self.help = Some(help);
        self
    }

    /// The code.
    pub fn code(&self) -> ErrorCode {
        self.code
    }

    /// Where.
    pub fn span(&self) -> Span {
        self.span
    }

    /// Template arguments.
    pub fn args(&self) -> &[Arg] {
        &self.args
    }

    /// The hint, if any.
    pub fn help(&self) -> Option<Help> {
        self.help
    }
}

/// The per-rule diagnostic collector used by the parser and the checker.
///
/// It owns the "at most [`MAX_DIAGNOSTICS`], sorted by position" policy so no stage
/// re-implements it.
#[derive(Debug, Default)]
pub struct Diagnostics {
    kept: Vec<Diagnostic>,
    dropped: usize,
}

impl Diagnostics {
    /// An empty collector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one; beyond the limit only the count grows.
    pub fn push(&mut self, d: Diagnostic) {
        if self.kept.len() < MAX_DIAGNOSTICS {
            self.kept.push(d);
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    /// Whether nothing was recorded.
    pub fn is_empty(&self) -> bool {
        self.kept.is_empty()
    }

    /// The kept diagnostics sorted by span start, plus how many were dropped.
    pub fn finish(mut self) -> (Vec<Diagnostic>, usize) {
        self.kept.sort_by_key(|d| (d.span.start, d.span.end));
        (self.kept, self.dropped)
    }
}

/// Write `d` in the Go-like caret format:
///
/// ```text
/// error[E0201]: expected int, found string
///  --> rule:1:12
///   |
/// 1 | user.Age + "x"
///   |            ^^^
/// ```
///
/// `origin` is the rule name (or `"rule"`). Used by `Display` of both compile and
/// runtime errors so the format has one implementation.
pub fn render(
    f: &mut fmt::Formatter<'_>,
    src: &str,
    origin: &str,
    code: ErrorCode,
    span: Span,
    args: &[Arg],
    help: Option<Help>,
) -> fmt::Result {
    // TODO: substitute `{n}` in `code.template()` with `args[n]`; compute line/column
    // by counting '\n' before `span.start` (column in chars, 1-based); print the line
    // and `^` under the span, clamped to that line; then `= help: ...` if any.
    todo!()
}
