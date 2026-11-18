//! An owned, dynamically typed value.

use alloc::string::String;
use alloc::vec::Vec;
use core::ops::ControlFlow;

use dictum_check::Type;

use crate::engine::value::{Bytes, MapRef, Repr, Seq, Str, Value};
use crate::facts::{Field, MapView, SeqView};

/// An owned value whose type is known only at run time.
///
/// Three roles, one type: a fact field of type `any` (e.g. parsed JSON attributes),
/// the result of a `Rule<Dynamic, _>` (any result type, for a REPL or CLI), and the
/// values recorded by [`Report`](crate::Report). Read as a field it is borrowed,
/// not copied.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Dynamic {
    /// `nil`
    Nil,
    /// `bool`
    Bool(bool),
    /// `int`
    Int(i64),
    /// `float`
    Float(f64),
    /// `decimal`
    #[cfg(feature = "decimal")]
    Decimal(rust_decimal::Decimal),
    /// `string`
    String(String),
    /// `bytes`
    Bytes(Vec<u8>),
    /// `time`
    Time(jiff::Timestamp),
    /// `duration`
    Duration(jiff::SignedDuration),
    /// `array`
    Array(Vec<Dynamic>),
    /// `map` (and structs, keyed by field name)
    Map(DynMap),
}

/// String-keyed map in insertion order with unique keys.
///
/// The `Vec` is private so uniqueness holds by construction; lookups are linear,
/// which suits attribute bags. Swapping in an index later is invisible to callers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DynMap {
    entries: Vec<(String, Dynamic)>,
}

impl DynMap {
    /// Empty map.
    pub const fn new() -> Self {
        DynMap { entries: Vec::new() }
    }

    /// Insert or replace (keeping the original position). Returns the old value.
    pub fn insert(&mut self, key: impl Into<String>, value: Dynamic) -> Option<Dynamic> {
        let key = key.into();
        match self.entries.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => Some(core::mem::replace(v, value)),
            None => {
                self.entries.push((key, value));
                None
            }
        }
    }

    /// Lookup.
    pub fn get(&self, key: &str) -> Option<&Dynamic> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Entries in order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Dynamic)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Field for Dynamic {
    fn ty() -> Type {
        Type::Any
    }
    fn read(&self) -> Value<'_> {
        Value(match self {
            Dynamic::Nil => Repr::Nil,
            Dynamic::Bool(b) => Repr::Bool(*b),
            Dynamic::Int(n) => Repr::Int(*n),
            Dynamic::Float(f) => Repr::Float(*f),
            #[cfg(feature = "decimal")]
            Dynamic::Decimal(d) => Repr::Decimal(*d),
            Dynamic::String(s) => Repr::Str(Str::Borrowed(s)),
            Dynamic::Bytes(b) => Repr::Bytes(Bytes::Borrowed(b)),
            Dynamic::Time(t) => Repr::Time(*t),
            Dynamic::Duration(d) => Repr::Duration(*d),
            Dynamic::Array(v) => Repr::Array(Seq::Borrowed(v)),
            Dynamic::Map(m) => Repr::Map(MapRef::Borrowed(m)),
        })
    }
}

impl MapView for DynMap {
    fn len(&self) -> usize {
        self.entries.len()
    }
    fn get(&self, key: &str) -> Option<Value<'_>> {
        DynMap::get(self, key).map(Field::read)
    }
    fn for_each<'s>(&'s self, f: &mut dyn FnMut(&'s str, Value<'s>) -> ControlFlow<()>) {
        for (k, v) in &self.entries {
            if f(k, v.read()).is_break() {
                break;
            }
        }
    }
}

// `Vec<Dynamic>` is a `SeqView` through the generic `impl<T: Field> SeqView for Vec<T>`.
const _: fn() = || {
    fn assert_seq<T: SeqView>() {}
    assert_seq::<Vec<Dynamic>>();
};
