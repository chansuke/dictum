//! Many named rules evaluated together, sharing common subexpressions.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::marker::PhantomData;

use crate::engine::{self, Checked, PreparedSet};
use crate::{CompileError, Ctx, Dictum, EvalError, Output, Report, Schema};

/// An immutable set of named rules with result type `T`.
///
/// Built by [`RuleSetBuilder`]; never modified afterwards. Hot reload builds a new
/// set and swaps the `Arc` (e.g. with `arc-swap`); evaluations in flight finish on
/// the old one. Immutability is what lets one `eval` share work across rules
/// without locks: the shared-subexpression slots are fixed at `build` time and the
/// cache lives in the per-evaluation frame.
pub struct RuleSet<T, F> {
    inner: Arc<PreparedSet>,
    _types: PhantomData<(fn() -> T, fn(&F))>,
}

impl<T, F> Clone for RuleSet<T, F> {
    fn clone(&self) -> Self {
        RuleSet { inner: self.inner.clone(), _types: PhantomData }
    }
}

impl<T: Output, F: Dictum<Static = F>> RuleSet<T, F> {
    /// Start a set.
    pub fn builder(schema: &Schema<F>) -> RuleSetBuilder<T, F> {
        RuleSetBuilder { schema: schema.clone(), rules: Vec::new(), _types: PhantomData }
    }

    /// Evaluate every rule against one `ctx`.
    ///
    /// Each rule's result is independent: a rule that fails (its evaluation stops at
    /// its first error) does not stop the others. A pure subexpression that occurs in
    /// several rules is computed once per call; `now()` is the same for all rules.
    /// Each rule has its own budget (`ctx`'s limits); shared work is charged to the
    /// rule that computes it first (rules run in insertion order).
    pub fn eval<G: Dictum<Static = F>>(&self, ctx: &Ctx<'_, G>) -> Outcomes<'_, T> {
        // TODO: one Frame for the whole call with cache = vec![None; inner.cache_slots()];
        // for each rule in order: reset budget, slots and scopes; run; on Ok cache keeps
        // the values it filled, on Err nothing is cached (each rule recomputes with its
        // own closure tree, so errors carry that rule's own spans).
        todo!()
    }

    /// Explain one rule by name, as [`Rule::explain`](crate::Rule::explain) would
    /// (without sharing). `None` if no rule has that name.
    pub fn explain<G: Dictum<Static = F>>(&self, name: &str, ctx: &Ctx<'_, G>) -> Option<Report<T>> {
        todo!()
    }

    /// Rule names in insertion order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.inner.names()
    }
}

/// Collects rules, then [`build`](RuleSetBuilder::build)s the set once.
///
/// `insert` parses and type-checks immediately, so errors are reported per rule as
/// they are added; closure compilation and cross-rule sharing happen in `build`,
/// once, when the full set is known.
pub struct RuleSetBuilder<T, F> {
    schema: Schema<F>,
    rules: Vec<Checked>,
    _types: PhantomData<fn() -> T>,
}

impl<T: Output, F: Dictum<Static = F>> RuleSetBuilder<T, F> {
    /// Add a rule. On error the builder is unchanged. A name already in the set is
    /// rejected with `E0001`, so a duplicated rule in a source of rules is loud.
    ///
    /// Loading many rules is a loop over `insert`, collecting the errors you want.
    pub fn insert(&mut self, name: &str, src: &str) -> Result<(), CompileError> {
        // TODO: if self.rules.iter().any(|r| r.name() == Some(name)) -> CompileError
        //       with one Diagnostic(DuplicateRule, span = whole source, [Arg::Name(name)])
        let checked = engine::check(src, Some(name), &self.schema.core, &T::expected())?;
        self.rules.push(checked);
        Ok(())
    }

    /// Compile every rule's closure tree with shared-subexpression slots.
    ///
    /// Infallible except for an internal compiler error, which cannot be blamed on
    /// any rule's source and so is reported as an `EvalError::Internal` at the first
    /// evaluation of the affected rule rather than here.
    pub fn build(self) -> RuleSet<T, F> {
        let inner = PreparedSet::new(self.rules, self.schema.core.clone());
        RuleSet { inner: Arc::new(inner), _types: PhantomData }
    }
}

/// Per-rule results of one [`RuleSet::eval`], in insertion order.
pub struct Outcomes<'s, T> {
    names: Vec<&'s str>,
    results: Vec<Result<T, EvalError>>,
}

impl<'s, T> Outcomes<'s, T> {
    /// Every rule with its result.
    pub fn iter(&self) -> impl Iterator<Item = (&'s str, &Result<T, EvalError>)> {
        self.names.iter().copied().zip(self.results.iter())
    }

    /// Rules that failed.
    pub fn errors(&self) -> impl Iterator<Item = (&'s str, &EvalError)> {
        self.iter().filter_map(|(n, r)| r.as_ref().err().map(|e| (n, e)))
    }
}

impl<'s> Outcomes<'s, bool> {
    /// Rules that evaluated to `true`.
    pub fn matched(&self) -> impl Iterator<Item = &'s str> {
        self.iter().filter_map(|(n, r)| matches!(r, Ok(true)).then_some(n))
    }
}

const _: () = {
    const fn assert_send_sync<X: Send + Sync>() {}
    struct NotSync(core::cell::Cell<u8>);
    assert_send_sync::<RuleSet<bool, NotSync>>();
};
