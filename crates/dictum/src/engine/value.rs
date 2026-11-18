//! The runtime value. Its representation is private to this crate.
//!
//! `Value` is nameable from outside only through `__private` (the derive's generated
//! `Object::field` returns one), and it has no public constructors besides what the
//! `Field` impls use. Changing the representation touches this file and the engine,
//! nothing else.

use alloc::sync::Arc;

use crate::facts::{MapView, Object, SeqView};

/// A value borrowing from facts for `'a`. Cheap to clone (scalars, borrows, `Arc`s).
#[derive(Clone)]
pub struct Value<'a>(pub(crate) Repr<'a>);

/// Returned by a derived `Object::field` for an id it does not know. The evaluator
/// treats it like any other type mismatch at a field read (`Internal`, not a panic).
pub fn missing_field<'a>() -> Value<'a> {
    Value(Repr::Nil)
}

#[derive(Clone)]
pub(crate) enum Repr<'a> {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    #[cfg(feature = "decimal")]
    Decimal(rust_decimal::Decimal),
    Str(Str<'a>),
    Bytes(Bytes<'a>),
    Time(jiff::Timestamp),
    Duration(jiff::SignedDuration),
    Array(Seq<'a>),
    Map(MapRef<'a>),
    Object(&'a dyn Object),
}

/// A string: borrowed from facts, or shared (literals, computed strings).
#[derive(Clone)]
pub(crate) enum Str<'a> {
    Borrowed(&'a str),
    Shared(Arc<str>),
}

impl Str<'_> {
    pub(crate) fn as_str(&self) -> &str {
        match self {
            Str::Borrowed(s) => s,
            Str::Shared(s) => s,
        }
    }
}

#[derive(Clone)]
pub(crate) enum Bytes<'a> {
    Borrowed(&'a [u8]),
    Shared(Arc<[u8]>),
}

/// A sequence.
#[derive(Clone)]
pub(crate) enum Seq<'a> {
    /// A fact field; elements are read on access.
    Borrowed(&'a dyn SeqView),
    /// Built during evaluation.
    Owned(Arc<[Value<'a>]>),
    /// `from..to` (inclusive), never materialized; iteration is charged per element.
    Range { from: i64, to: i64 },
}

/// A string-keyed map.
#[derive(Clone)]
pub(crate) enum MapRef<'a> {
    /// A fact field.
    Borrowed(&'a dyn MapView),
    /// A map literal: insertion order, unique keys (checked when built). Literals
    /// are small, so lookup is a linear scan; no hasher is needed in `no_std`.
    Owned(Arc<[(Arc<str>, Value<'a>)]>),
}

impl<'a> Value<'a> {
    pub(crate) const NIL: Value<'static> = Value(Repr::Nil);

    /// Borrow a derived struct.
    pub fn object(o: &'a dyn Object) -> Self {
        Value(Repr::Object(o))
    }

    /// Runtime tag, for `any` dispatch through `dictum_check::resolve`.
    pub(crate) fn tag(&self) -> dictum_check::Tag {
        use dictum_check::Tag;
        match &self.0 {
            Repr::Nil => Tag::Nil,
            Repr::Bool(_) => Tag::Bool,
            Repr::Int(_) => Tag::Int,
            Repr::Float(_) => Tag::Float,
            #[cfg(feature = "decimal")]
            Repr::Decimal(_) => Tag::Decimal,
            Repr::Str(_) => Tag::String,
            Repr::Bytes(_) => Tag::Bytes,
            Repr::Time(_) => Tag::Time,
            Repr::Duration(_) => Tag::Duration,
            Repr::Array(_) => Tag::Array,
            Repr::Map(_) => Tag::Map,
            Repr::Object(_) => Tag::Struct,
        }
    }
}
