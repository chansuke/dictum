//! Many named rules evaluated together, sharing common subexpressions.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::marker::PhantomData;

use dictum_syntax::{Arg, ErrorCode};

use crate::engine::frame::Frame;
use crate::engine::{self, Checked, PreparedSet};
use crate::rule::TypeMarker;
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
    _types: TypeMarker<T, F>,
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

    /// Evaluate every rule against one `ctx`, in insertion order.
    ///
    /// Stops at the first error: the result is either every rule's value or the error
    /// of the first rule that failed (`EvalError::rule` names it). A pure
    /// subexpression that occurs in several rules is computed once per call; `now()`
    /// is the same for all rules. Each rule has its own budget (`ctx`'s limits);
    /// shared work is charged to the rule that computes it first.
    pub fn eval<G: Dictum<Static = F>>(&self, ctx: &Ctx<'_, G>) -> Result<Outcomes<'_, T>, EvalError> {
        let rules = self.inner.rules();
        let mut frame = Frame::new(ctx.facts, ctx.now, &ctx.limits, self.inner.cache_slots());
        let (mut names, mut values) = (Vec::with_capacity(rules.len()), Vec::with_capacity(rules.len()));
        for (i, rule) in rules.iter().enumerate() {
            if i > 0 {
                // A fresh budget; the cache keeps what earlier rules computed.
                frame.next_rule(&ctx.limits, 0);
            }
            let value = rule.run(&mut frame).map_err(|f| rule.error(f))?;
            values.push(T::from_value(value).map_err(|kind| rule.output_error(kind))?);
            names.push(rule.name().unwrap_or_default());
        }
        Ok(Outcomes { names, values })
    }

    /// Explain one rule by name, as [`Rule::explain`](crate::Rule::explain) would
    /// (without sharing). `None` if no rule has that name.
    pub fn explain<G: Dictum<Static = F>>(&self, name: &str, ctx: &Ctx<'_, G>) -> Option<Report<T>> {
        self.inner.get(name).map(|rule| crate::rule::explain(rule, ctx))
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
        if self.rules.iter().any(|r| r.name() == Some(name)) {
            let end = u32::try_from(src.len()).unwrap_or(u32::MAX);
            let span = dictum_syntax::Span { start: 0, end };
            let d = dictum_syntax::Diagnostic::new(ErrorCode::DuplicateRule, span, alloc::vec![Arg::Name(name.into())]);
            let mut diags = dictum_syntax::Diagnostics::new();
            diags.push(d);
            return Err(engine::compile_error(src, Some(name), diags));
        }
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

/// Every rule's value from one successful [`RuleSet::eval`], in insertion order.
pub struct Outcomes<'s, T> {
    names: Vec<&'s str>,
    values: Vec<T>,
}

impl<'s, T> Outcomes<'s, T> {
    /// Every rule with its value.
    pub fn iter(&self) -> impl Iterator<Item = (&'s str, &T)> {
        self.names.iter().copied().zip(self.values.iter())
    }
}

impl<'s> Outcomes<'s, bool> {
    /// Rules that evaluated to `true`.
    pub fn matched(&self) -> impl Iterator<Item = &'s str> {
        self.iter().filter_map(|(n, v)| v.then_some(n))
    }
}

const _: () = {
    const fn assert_send_sync<X: Send + Sync>() {}
    type NotSync = core::cell::Cell<u8>;
    assert_send_sync::<RuleSet<bool, NotSync>>();
};
