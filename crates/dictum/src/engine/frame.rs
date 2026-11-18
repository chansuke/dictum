//! Per-evaluation mutable state. A `Frame` is created per `eval` call (per
//! `RuleSet::eval` call for a set) and dropped at its end, so compiled rules hold no
//! mutable state and stay `Send + Sync`.

use alloc::vec::Vec;

use dictum_syntax::Span;

use crate::EvalErrorKind;
use crate::ctx::Limits;
use crate::engine::value::Value;
use crate::facts::Object;

/// An error inside the engine: kind + span. Converted to `EvalError` (which adds the
/// source and rule name) at the public boundary.
pub(crate) struct Fault {
    pub(crate) kind: EvalErrorKind,
    pub(crate) span: Span,
}

/// Remaining budget for the current rule.
pub(crate) struct Budget {
    steps: u64,
    alloc: u64,
}

impl Budget {
    pub(crate) fn new(limits: &Limits) -> Self {
        Budget { steps: limits.max_steps, alloc: limits.max_alloc }
    }

    /// Charge `n` visited elements *before* doing the work.
    pub(crate) fn step(&mut self, n: u64) -> Result<(), EvalErrorKind> {
        self.steps = self.steps.checked_sub(n).ok_or(EvalErrorKind::StepLimit)?;
        Ok(())
    }

    /// Charge `n` elements/bytes *before* allocating them, so an oversized request
    /// (`repeat("x", 1 << 40)`) fails without attempting the allocation.
    pub(crate) fn alloc(&mut self, n: u64) -> Result<(), EvalErrorKind> {
        self.alloc = self.alloc.checked_sub(n).ok_or(EvalErrorKind::AllocLimit)?;
        Ok(())
    }
}

/// The innermost predicate's pointers.
pub(crate) struct Scope<'a> {
    /// `#`
    pub(crate) current: Value<'a>,
    /// `#index`
    pub(crate) index: i64,
    /// `#acc` (reduce only)
    pub(crate) acc: Option<Value<'a>>,
}

pub(crate) struct Frame<'a> {
    /// The root fact; top-level variables are its fields.
    pub(crate) root: &'a dyn Object,
    /// What `now()` returns for this whole evaluation; `None` makes `now()` fail
    /// with `ClockUnset`.
    pub(crate) now: Option<jiff::Timestamp>,
    /// `let` values; filled eagerly in order before the body runs.
    pub(crate) slots: Vec<Value<'a>>,
    /// Predicate scopes, innermost last. Capacity `max_predicate_depth`.
    pub(crate) scopes: Vec<Scope<'a>>,
    pub(crate) budget: Budget,
    /// RuleSet shared-subexpression cache, one entry per `CacheSlot`; empty for a
    /// single `Rule`. A slot is written at most once per frame (first successful
    /// computation) and read by later rules; errors are never stored.
    pub(crate) cache: Vec<Option<Value<'a>>>,
}

impl<'a> Frame<'a> {
    pub(crate) fn new(root: &'a dyn Object, now: Option<jiff::Timestamp>, limits: &Limits, cache_slots: u32) -> Self {
        todo!()
    }

    /// Prepare for the next rule of a set: fresh budget, slots and scopes; the cache
    /// is kept.
    pub(crate) fn next_rule(&mut self, limits: &Limits, slot_count: usize) {
        todo!()
    }
}
