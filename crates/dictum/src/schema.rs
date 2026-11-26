//! What rules may refer to.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::marker::PhantomData;

use dictum_check::{FuncSig, SchemaError, StructDesc};

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
    ///
    /// For a schema described at run time (`runtime::Facts`), use `Schema::runtime`
    /// instead; `Schema::<runtime::Facts>::builder()` uses an empty root descriptor
    /// that rejects facts read by any `runtime::Root` with
    /// [`EvalErrorKind::SchemaMismatch`](crate::EvalErrorKind::SchemaMismatch).
    pub fn builder() -> SchemaBuilder<F> {
        SchemaBuilder { root: F::__desc(), funcs: Vec::new(), _facts: PhantomData }
    }
}

#[cfg(feature = "runtime-schema")]
impl Schema<crate::runtime::Facts> {
    /// Start a schema whose variables are the fields of a root described at run time.
    /// Evaluate its rules with facts read by the same [`Root`](crate::runtime::Root).
    pub fn runtime(root: &crate::runtime::Root) -> SchemaBuilder<crate::runtime::Facts> {
        SchemaBuilder { root: root.desc(), funcs: Vec::new(), _facts: PhantomData }
    }
}

/// Registers schema functions. Validation (names, duplicates, builtin shadowing)
/// happens once, in [`build`](SchemaBuilder::build).
pub struct SchemaBuilder<F> {
    root: &'static StructDesc,
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
        let env = SchemaCore::new(self.root, self.funcs)?;
        Ok(Schema { core: Arc::new(env), _facts: PhantomData })
    }
}
