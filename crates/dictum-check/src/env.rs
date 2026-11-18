//! The name environment a rule is checked against.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::fmt;

use crate::{StructDesc, Type};

/// Index of a schema function in its `Env`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FuncId(u32);

impl FuncId {
    /// Position in `Env::funcs`.
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// Monomorphic signature of a schema function, inferred from its Rust types.
#[derive(Clone, Debug, PartialEq)]
pub struct FuncSig {
    /// Parameter types; arguments are converted with the same assignability rules as
    /// the rule result (`int` to `float`, `T` to `Option<T>`, `any` checked).
    pub params: Box<[Type]>,
    /// Return type.
    pub ret: Type,
}

struct FuncEntry<P> {
    name: Box<str>,
    sig: FuncSig,
    payload: P,
}

/// Root struct plus schema functions.
///
/// Variables are exactly the fields of `root`; `$env` is `root` itself. There is no
/// separate variable list to keep in sync with the facts type.
///
/// `P` is the evaluator's implementation of a function. This crate stores it next to
/// the signature and never looks inside, so signature and implementation can't drift.
pub struct Env<P> {
    root: &'static StructDesc,
    funcs: Vec<FuncEntry<P>>,
    by_name: BTreeMap<Box<str>, FuncId>,
}

impl<P> Env<P> {
    /// Validate and index the functions.
    ///
    /// Fails if a name is not a valid identifier, is a builtin name
    /// (`Builtin::from_name`), or is registered twice. Field names need no check here:
    /// the derive already rejected invalid ones at compile time.
    pub fn new(root: &'static StructDesc, funcs: Vec<(Box<str>, FuncSig, P)>) -> Result<Self, SchemaError> {
        let mut by_name = BTreeMap::new();
        let mut entries = Vec::with_capacity(funcs.len());
        for (i, (name, sig, payload)) in funcs.into_iter().enumerate() {
            if !dictum_syntax::is_identifier(&name) {
                return Err(SchemaError::InvalidName(name));
            }
            if dictum_syntax::Builtin::from_name(&name).is_some() {
                return Err(SchemaError::ShadowsBuiltin(name));
            }
            // More than u32::MAX functions cannot be registered in practice.
            let id = FuncId(u32::try_from(i).unwrap_or(u32::MAX));
            if by_name.insert(name.clone(), id).is_some() {
                return Err(SchemaError::DuplicateFunction(name));
            }
            entries.push(FuncEntry { name, sig, payload });
        }
        Ok(Env { root, funcs: entries, by_name })
    }

    /// The root struct.
    pub fn root(&self) -> &'static StructDesc {
        self.root
    }

    /// Resolve a function name.
    pub fn func(&self, name: &str) -> Option<(FuncId, &FuncSig)> {
        let id = *self.by_name.get(name)?;
        Some((id, &self.funcs.get(id.index())?.sig))
    }

    /// The implementation stored for `id`.
    ///
    /// `id` came from `func` on this same `Env` (the IR is checked against one `Env`
    /// and kept together with it), so the lookup cannot miss.
    pub fn payload(&self, id: FuncId) -> Option<&P> {
        self.funcs.get(id.index()).map(|e| &e.payload)
    }
}

/// Why a schema could not be built.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SchemaError {
    /// Two functions share a name.
    DuplicateFunction(Box<str>),
    /// A function uses a builtin's name.
    ShadowsBuiltin(Box<str>),
    /// Not `[A-Za-z_][A-Za-z0-9_]*`, or a reserved word.
    InvalidName(Box<str>),
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SchemaError::DuplicateFunction(n) => write!(f, "function `{n}` is registered twice"),
            SchemaError::ShadowsBuiltin(n) => write!(f, "function `{n}` would shadow a builtin"),
            SchemaError::InvalidName(n) => write!(f, "`{n}` is not a valid function name"),
        }
    }
}

impl core::error::Error for SchemaError {}
