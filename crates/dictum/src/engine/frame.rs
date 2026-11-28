//! Per-evaluation mutable state. A `Frame` is created per `eval` call (per
//! `RuleSet::eval` call for a set) and dropped at its end, so compiled rules hold no
//! mutable state and stay `Send + Sync`.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use dictum_syntax::Span;

use crate::EvalErrorKind;
use crate::ctx::Limits;
use crate::engine::value::{Object, Repr, Str, Value};

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
    pub(crate) const fn alloc_left(&self) -> u64 {
        self.alloc
    }

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
        let cache = (0..cache_slots).map(|_| None).collect();
        Frame { root, now, slots: Vec::new(), scopes: Vec::new(), budget: Budget::new(limits), cache }
    }

    /// Prepare for the next rule of a set: fresh budget, slots and scopes; the cache
    /// is kept.
    pub(crate) fn next_rule(&mut self, limits: &Limits, slot_count: usize) {
        self.budget = Budget::new(limits);
        self.slots.clear();
        self.slots.reserve(slot_count);
        self.scopes.clear();
    }

    /// A fault at `span`.
    pub(crate) fn fault(kind: EvalErrorKind, span: Span) -> Fault {
        Fault { kind, span }
    }
}

pub(super) type Res<'a> = Result<Value<'a>, Fault>;

pub(super) fn internal(what: &'static str, span: Span) -> Fault {
    Frame::fault(EvalErrorKind::Internal(what), span)
}

/// Attach `span` to an `ops` error.
pub(super) fn at(span: Span) -> impl Fn(EvalErrorKind) -> Fault {
    move |kind| Frame::fault(kind, span)
}

pub(super) fn steps(f: &mut Frame<'_>, n: usize, span: Span) -> Result<(), Fault> {
    f.budget.step(u64::try_from(n).unwrap_or(u64::MAX)).map_err(at(span))
}

pub(super) fn alloc(f: &mut Frame<'_>, n: usize, span: Span) -> Result<(), Fault> {
    f.budget.alloc(u64::try_from(n).unwrap_or(u64::MAX)).map_err(at(span))
}

/// A string a rule computes. It is the only way to make a computed string value:
/// every write charges `max_alloc` one per byte before the buffer grows, so a
/// builtin cannot build a string first and charge it after (which let
/// `string([s, s, ...])` allocate gigabytes under a one-megabyte limit).
/// `crates/dictum/tests/charged_strings.rs` fails on any other construction of
/// `Str::Shared`.
pub(crate) struct Text {
    buf: String,
    /// Bytes already charged; `buf.len() <= paid`.
    paid: usize,
}

impl Text {
    pub(super) const fn new() -> Self {
        Text { buf: String::new(), paid: 0 }
    }

    /// A computed copy of `s`.
    pub(super) fn copy(f: &mut Frame<'_>, s: &str, span: Span) -> Res<'static> {
        let mut text = Text::new();
        text.push_str(f, s, span)?;
        Ok(text.into_value())
    }

    /// Charge for `n` more bytes now, so the writes that fill them charge nothing.
    pub(super) fn reserve(&mut self, f: &mut Frame<'_>, n: usize, span: Span) -> Result<(), Fault> {
        let want = self.buf.len().saturating_add(n);
        if want > self.paid {
            alloc(f, want.saturating_sub(self.paid), span)?;
            self.paid = want;
            self.buf.reserve(want.saturating_sub(self.buf.len()));
        }
        Ok(())
    }

    pub(super) fn push_str(&mut self, f: &mut Frame<'_>, s: &str, span: Span) -> Result<(), Fault> {
        self.reserve(f, s.len(), span)?;
        self.buf.push_str(s);
        Ok(())
    }

    pub(super) fn push(&mut self, f: &mut Frame<'_>, c: char, span: Span) -> Result<(), Fault> {
        self.reserve(f, c.len_utf8(), span)?;
        self.buf.push(c);
        Ok(())
    }

    /// Let a writer from a lower layer (`builtins`, `layout`) append into bytes
    /// already paid for with [`Text::reserve`]. Writing past them is an engine bug,
    /// reported as `Internal`, so the reservation must be an upper bound.
    pub(super) fn write_reserved(&mut self, span: Span, write: impl FnOnce(&mut String)) -> Result<(), Fault> {
        write(&mut self.buf);
        if self.buf.len() > self.paid {
            return Err(internal("a writer passed the bytes reserved for it", span));
        }
        Ok(())
    }

    pub(super) fn as_str(&self) -> &str {
        &self.buf
    }

    pub(super) fn into_shared(self) -> Arc<str> {
        Arc::from(self.buf)
    }

    pub(super) fn into_value(self) -> Value<'static> {
        Value(Repr::Str(Str::Shared(self.into_shared())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_charges_before_the_work_and_stops_at_the_limit() {
        let mut b = Budget::new(&Limits::default().with_max_steps(3).with_max_alloc(10));
        assert!(b.step(2).is_ok());
        assert!(b.step(1).is_ok());
        assert!(matches!(b.step(1), Err(EvalErrorKind::StepLimit)));
        assert!(b.alloc(10).is_ok());
        assert!(matches!(b.alloc(1), Err(EvalErrorKind::AllocLimit)));
    }

    #[test]
    fn an_oversized_request_fails_without_partial_charge() {
        let mut b = Budget::new(&Limits::default().with_max_alloc(10));
        assert!(matches!(b.alloc(u64::MAX), Err(EvalErrorKind::AllocLimit)));
        assert!(b.alloc(10).is_ok(), "the failed request charged nothing");
    }
}
