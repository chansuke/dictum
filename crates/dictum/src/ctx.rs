//! Per-evaluation input.

use jiff::Timestamp;

use crate::Dictum;

/// Everything one evaluation reads: the facts, the time `now()` returns, and the
/// resource limits.
///
/// There is no clock object and no default time. `now` is a value the caller reads
/// once, so the same `Ctx` always gives the same result (given `pure` functions), a
/// test fixes time by passing a constant, and `now()` is identical across every rule
/// of one `RuleSet::eval`, which lets common-subexpression sharing include it.
/// A `no_std` caller without a clock passes any fixed timestamp explicitly.
pub struct Ctx<'f, F> {
    pub(crate) facts: &'f F,
    pub(crate) now: Timestamp,
    pub(crate) limits: Limits,
}

// Manual impls: `Ctx` is `Copy` whatever `F` is (it only borrows the facts).
impl<F> Clone for Ctx<'_, F> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<F> Copy for Ctx<'_, F> {}

impl<'f, F: Dictum> Ctx<'f, F> {
    /// Evaluate against `facts` at time `now`, with default [`Limits`].
    pub fn new(facts: &'f F, now: Timestamp) -> Self {
        Ctx { facts, now, limits: Limits::default() }
    }

    /// Replace the limits.
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }
}

/// Resource limits for one rule evaluation. In a `RuleSet`, each rule gets its own
/// budget; work shared between rules is charged to the rule that computes it first.
///
/// Counting happens only where work is proportional to data: each element visited
/// by a predicate, `in`, `join`, `sum`, ...; and each element or byte allocated for
/// an array, map or string. A range is charged when iterated, not when created.
/// There is no depth limit here: nesting is bounded statically by the parser
/// (`dictum_syntax::MAX_DEPTH`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Elements visited. Exceeding it is `EvalErrorKind::StepLimit`.
    pub max_steps: u64,
    /// Elements and bytes allocated. Exceeding it is `EvalErrorKind::AllocLimit`.
    pub max_alloc: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_steps: 1_000_000, max_alloc: 16 * 1024 * 1024 }
    }
}

impl Limits {
    /// Set `max_steps`.
    pub const fn with_max_steps(mut self, n: u64) -> Self {
        self.max_steps = n;
        self
    }

    /// Set `max_alloc`.
    pub const fn with_max_alloc(mut self, n: u64) -> Self {
        self.max_alloc = n;
        self
    }
}
