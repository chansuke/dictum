//! What rules may refer to.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::marker::PhantomData;

use dictum_check::{FuncSig, SchemaError};

use crate::Dictum;
use crate::engine::SchemaCore;
use crate::func::{FuncImpl, IntoFunc, Purity};

/// The names a rule can use: the fields of the root struct `F` (its variables) and
/// the registered functions. Built once, shared by every rule (`Clone` is an `Arc`
/// clone).
///
/// `F` is the root's `'static` form (`Facts<'static>`); rules compiled against a
/// `Schema<F>` can only be evaluated with an `F` (or `F` with shorter lifetimes),
/// which the type system checks.
pub struct Schema<F> {
    pub(crate) core: Arc<SchemaCore>,
    _facts: PhantomData<fn(&F)>,
}

impl<F> Clone for Schema<F> {
    fn clone(&self) -> Self {
        Schema { core: self.core.clone(), _facts: PhantomData }
    }
}

impl<F: Dictum<Static = F>> Schema<F> {
    /// Start a schema whose variables are `F`'s fields.
    pub fn builder() -> SchemaBuilder<F> {
        SchemaBuilder { funcs: Vec::new(), _facts: PhantomData }
    }
}

/// Registers schema functions. Validation (names, duplicates, builtin shadowing)
/// happens once, in [`build`](SchemaBuilder::build).
pub struct SchemaBuilder<F> {
    funcs: Vec<(Box<str>, FuncSig, FuncImpl)>,
    _facts: PhantomData<fn(&F)>,
}

impl<F: Dictum<Static = F>> SchemaBuilder<F> {
    /// Register a function that may return different results for the same arguments
    /// (reads configuration, rates, ...). It is called every time it is evaluated:
    /// never folded at compile time, never shared between rules.
    pub fn func<M>(self, name: &str, f: impl IntoFunc<M>) -> Self {
        self.register(name, f, Purity::Volatile)
    }

    /// Register a function that always returns the same result for the same
    /// arguments. With constant arguments it is called once at compile time; in a
    /// `RuleSet` one call is shared by every rule that makes it.
    pub fn pure_func<M>(self, name: &str, f: impl IntoFunc<M>) -> Self {
        self.register(name, f, Purity::Pure)
    }

    fn register<M>(mut self, name: &str, f: impl IntoFunc<M>, purity: Purity) -> Self {
        let erased = f.into_func();
        self.funcs.push((name.into(), erased.sig, FuncImpl { purity, call: erased.call }));
        self
    }

    /// Validate and freeze.
    pub fn build(self) -> Result<Schema<F>, SchemaError> {
        let env = SchemaCore::new(F::__desc(), self.funcs)?;
        Ok(Schema { core: Arc::new(env), _facts: PhantomData })
    }
}
