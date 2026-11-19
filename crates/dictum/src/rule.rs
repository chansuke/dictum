//! A single compiled rule.

use alloc::sync::Arc;
use core::marker::PhantomData;

use crate::engine::frame::Frame;
use crate::engine::{self, Prepared};
use crate::{CompileError, Ctx, Dictum, EvalError, Output, Report, Schema};

/// A rule compiled against `Schema<F>` whose result is a `T`.
///
/// Immutable after `compile`: `Send + Sync` and cheap to clone (an `Arc`), so share
/// it across threads or swap a newer one in with `arc-swap`.
pub struct Rule<T, F> {
    pub(crate) prepared: Arc<Prepared>,
    _types: TypeMarker<T, F>,
}

/// Ties a compiled rule to its result type `T` and facts type `F` without owning
/// either, so it is `Send + Sync` whatever `T` and `F` are.
pub(crate) type TypeMarker<T, F> = PhantomData<(fn() -> T, fn(&F))>;

impl<T, F> Clone for Rule<T, F> {
    fn clone(&self) -> Self {
        Rule { prepared: self.prepared.clone(), _types: PhantomData }
    }
}

impl<T: Output, F: Dictum<Static = F>> Rule<T, F> {
    /// Parse, type-check against `schema` with result type `T`, fold constants and
    /// build the closure tree.
    ///
    /// All syntax errors, or else all type errors, are reported together (up to 20).
    pub fn compile(src: &str, schema: &Schema<F>) -> Result<Self, CompileError> {
        let checked = engine::check(src, None, &schema.core, &T::expected())?;
        let prepared = Prepared::new(checked, schema.core.clone())?;
        Ok(Rule { prepared: Arc::new(prepared), _types: PhantomData })
    }

    /// Evaluate. Stops at the first error. Never panics; bounded by `ctx`'s limits.
    pub fn eval<G: Dictum<Static = F>>(&self, ctx: &Ctx<'_, G>) -> Result<T, EvalError> {
        let mut frame = Frame::new(ctx.facts, ctx.now, &ctx.limits, 0);
        let value = self.prepared.run(&mut frame).map_err(|f| self.prepared.error(f))?;
        T::from_value(value).ok_or_else(|| self.prepared.internal("output conversion"))
    }

    /// Evaluate with the spec interpreter, recording every node, and return the
    /// explanation. Never fails; see [`Report`]. Slower than `eval`, and `eval` pays
    /// nothing for it.
    pub fn explain<G: Dictum<Static = F>>(&self, ctx: &Ctx<'_, G>) -> Report<T> {
        explain(&self.prepared, ctx)
    }
}

/// `prepared`'s explanation on `ctx`, for `Rule::explain` and `RuleSet::explain`.
pub(crate) fn explain<T: Output, G: Dictum>(prepared: &Prepared, ctx: &Ctx<'_, G>) -> Report<T> {
    let mut frame = Frame::new(ctx.facts, ctx.now, &ctx.limits, 0);
    let (result, root) = prepared.explain(&mut frame);
    let result = result
        .map_err(|f| prepared.error(f))
        .and_then(|v| T::from_value(v).ok_or_else(|| prepared.internal("output conversion")));
    Report { result, root, source: prepared.source().into() }
}

impl<T, F> Rule<T, F> {
    /// The source this rule was compiled from.
    pub fn source(&self) -> &str {
        self.prepared.source()
    }
}

const _: () = {
    const fn assert_send_sync<X: Send + Sync>() {}
    type NotSync = core::cell::Cell<u8>;
    // Holds for any `F`, even one that is neither `Send` nor `Sync`.
    assert_send_sync::<Rule<bool, NotSync>>();
};
