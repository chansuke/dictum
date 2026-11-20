//! The runtime value. Its representation is private to this crate.
//!
//! `Value` is nameable from outside only through `__private` (the derive's generated
//! `Object::field` returns one), and it has no public constructors besides what the
//! `Field` impls use. Changing the representation touches this file and the engine,
//! nothing else.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ops::ControlFlow;

use dictum_check::FieldId;
use dictum_check::ir::Const;

use crate::facts::{MapView, Object, SeqView};
use crate::{DynMap, Dynamic};

/// A value borrowing from facts for `'a`. Cheap to clone (scalars, borrows, `Arc`s).
#[derive(Clone)]
pub struct Value<'a>(pub(crate) Repr<'a>);

/// Returned by a derived `Object::field` for an id it does not know. The evaluator
/// treats it like any other type mismatch at a field read (`Internal`, not a panic).
pub fn missing_field<'a>() -> Value<'a> {
    Value(Repr::Nil)
}

/// How deeply `reduce` may nest computed containers. Twice the syntax's depth, so
/// that a `fromJSON` value (at most [`dictum_syntax::MAX_DEPTH`]) fits inside an
/// accumulator, while every recursive walk over a value stays well within the stack.
pub(crate) const MAX_VALUE_DEPTH: u16 = 256;

/// The most elements a folded constant holds (as the folder's allocation budget).
const MAX_CONST_ELEMENTS: usize = 1 << 16;

/// The depth of a container of `children`.
fn nesting<'v, 'a: 'v>(children: impl Iterator<Item = &'v Value<'a>>) -> u16 {
    children.map(Value::depth).max().unwrap_or(0).saturating_add(1)
}

// Every evaluation step moves values; a time carries its zone without growing them.
#[cfg(target_pointer_width = "64")]
const _: () = assert!(core::mem::size_of::<Value<'static>>() == 32);

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
    /// An instant and the zone it is shown in; comparisons use the instant only.
    Time(jiff::Timestamp, jiff::tz::TimeZone),
    Duration(jiff::SignedDuration),
    Location(jiff::tz::TimeZone),
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

impl Bytes<'_> {
    pub(crate) fn as_slice(&self) -> &[u8] {
        match self {
            Bytes::Borrowed(b) => b,
            Bytes::Shared(b) => b,
        }
    }
}

/// A sequence.
#[derive(Clone)]
pub(crate) enum Seq<'a> {
    /// A fact field; elements are read on access.
    Borrowed(&'a dyn SeqView),
    /// Built during evaluation, with its nesting depth ([`Value::depth`]).
    Owned(Arc<[Value<'a>]>, u16),
    /// A constant of the rule; elements are converted on access, so using one costs
    /// no allocation.
    Const(Arc<[Const]>),
    /// `from..to` (inclusive), never materialized; iteration is charged per element.
    Range { from: i64, to: i64 },
}

impl<'a> Seq<'a> {
    pub(crate) fn len(&self) -> usize {
        match self {
            Seq::Borrowed(v) => v.len(),
            Seq::Owned(xs, _) => xs.len(),
            Seq::Const(xs) => xs.len(),
            Seq::Range { from, to } => {
                let n = i128::from(*to).saturating_sub(i128::from(*from)).saturating_add(1).max(0);
                usize::try_from(n).unwrap_or(usize::MAX)
            }
        }
    }

    pub(crate) fn get(&self, i: usize) -> Option<Value<'a>> {
        match self {
            Seq::Borrowed(v) => v.get(i),
            Seq::Owned(xs, _) => xs.get(i).cloned(),
            Seq::Const(xs) => xs.get(i).map(Value::from_const),
            Seq::Range { from, to } => {
                let n = from.checked_add(i64::try_from(i).ok()?)?;
                (n <= *to).then_some(Value::int(n))
            }
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = Value<'a>> + '_ {
        (0..self.len()).map_while(|i| self.get(i))
    }
}

/// A string-keyed map.
#[derive(Clone)]
pub(crate) enum MapRef<'a> {
    /// A fact field.
    Borrowed(&'a dyn MapView),
    /// A map literal: insertion order, unique keys (checked when built). Literals
    /// are small, so lookup is a linear scan; no hasher is needed in `no_std`.
    Owned(Arc<[(Arc<str>, Value<'a>)]>, u16),
    /// A constant of the rule, as `Seq::Const`.
    Const(Arc<[(Arc<str>, Const)]>),
}

impl<'a> MapRef<'a> {
    pub(crate) fn len(&self) -> usize {
        match self {
            MapRef::Borrowed(m) => m.len(),
            MapRef::Owned(kvs, _) => kvs.len(),
            MapRef::Const(kvs) => kvs.len(),
        }
    }

    pub(crate) fn get(&self, key: &str) -> Option<Value<'a>> {
        match self {
            MapRef::Borrowed(m) => m.get(key),
            MapRef::Owned(kvs, _) => kvs.iter().find(|(k, _)| &**k == key).map(|(_, v)| v.clone()),
            MapRef::Const(kvs) => kvs.iter().find(|(k, _)| &**k == key).map(|(_, v)| Value::from_const(v)),
        }
    }

    /// Visit entries in order until `f` breaks.
    pub(crate) fn for_each(&self, f: &mut dyn FnMut(&str, Value<'a>) -> ControlFlow<()>) {
        match self {
            MapRef::Borrowed(m) => m.for_each(&mut |k, v| f(k, v)),
            MapRef::Owned(kvs, _) => {
                for (k, v) in kvs.iter() {
                    if f(k, v.clone()).is_break() {
                        break;
                    }
                }
            }
            MapRef::Const(kvs) => {
                for (k, v) in kvs.iter() {
                    if f(k, Value::from_const(v)).is_break() {
                        break;
                    }
                }
            }
        }
    }
}

impl<'a> Value<'a> {
    pub(crate) const NIL: Value<'static> = Value(Repr::Nil);

    pub(crate) const fn bool(b: bool) -> Self {
        Value(Repr::Bool(b))
    }

    pub(crate) const fn int(n: i64) -> Self {
        Value(Repr::Int(n))
    }

    pub(crate) const fn float(x: f64) -> Self {
        Value(Repr::Float(x))
    }

    /// A computed string.
    pub(crate) fn string(s: String) -> Self {
        Value(Repr::Str(Str::Shared(Arc::from(s))))
    }

    /// A computed array.
    pub(crate) fn array(xs: Vec<Value<'a>>) -> Self {
        let depth = nesting(xs.iter());
        Value(Repr::Array(Seq::Owned(Arc::from(xs), depth)))
    }

    /// A map built during evaluation: insertion order, unique keys.
    pub(crate) fn map(kvs: Vec<(Arc<str>, Value<'a>)>) -> Self {
        let depth = nesting(kvs.iter().map(|(_, v)| v));
        Value(Repr::Map(MapRef::Owned(Arc::from(kvs), depth)))
    }

    /// How deeply containers built during evaluation nest inside this value: 0 for a
    /// scalar, 1 for a container of facts or constants (their depth is bounded by
    /// the schema and the rule's syntax). Only `reduce` can nest values without a
    /// bound, so it checks this against [`MAX_VALUE_DEPTH`].
    pub(crate) fn depth(&self) -> u16 {
        match &self.0 {
            Repr::Array(Seq::Owned(_, d)) | Repr::Map(MapRef::Owned(_, d)) => *d,
            Repr::Array(_) | Repr::Map(_) => 1,
            _ => 0,
        }
    }

    /// A folded constant or literal. Constants are part of the rule, so building one
    /// is not charged to the budget.
    pub(crate) fn from_const(c: &Const) -> Value<'static> {
        Value(match c {
            Const::Nil => Repr::Nil,
            Const::Bool(b) => Repr::Bool(*b),
            Const::Int(n) => Repr::Int(*n),
            Const::Float(x) => Repr::Float(*x),
            #[cfg(feature = "decimal")]
            Const::Decimal(d) => Repr::Decimal(*d),
            Const::Str(s) => Repr::Str(Str::Shared(s.clone())),
            Const::Bytes(b) => Repr::Bytes(Bytes::Shared(b.clone())),
            Const::Time(t, z) => Repr::Time(*t, z.clone()),
            Const::Duration(d) => Repr::Duration(*d),
            Const::Location(z) => Repr::Location(z.clone()),
            Const::Array(xs) => Repr::Array(Seq::Const(xs.clone())),
            Const::Map(kvs) => Repr::Map(MapRef::Const(kvs.clone())),
        })
    }

    /// The value as a constant, for the folder. `None` for what a constant cannot
    /// hold: a struct, a range (kept lazy), anything borrowed from facts, or more
    /// than [`MAX_CONST_ELEMENTS`] elements. A constant is a tree, so a value shared
    /// several times counts once per reference.
    pub(crate) fn to_const(&self) -> Option<Const> {
        self.to_const_within(&mut MAX_CONST_ELEMENTS.clone())
    }

    fn to_const_within(&self, budget: &mut usize) -> Option<Const> {
        *budget = budget.checked_sub(1)?;
        Some(match &self.0 {
            Repr::Nil => Const::Nil,
            Repr::Bool(b) => Const::Bool(*b),
            Repr::Int(n) => Const::Int(*n),
            Repr::Float(x) => Const::Float(*x),
            #[cfg(feature = "decimal")]
            Repr::Decimal(d) => Const::Decimal(*d),
            Repr::Str(Str::Shared(s)) => Const::Str(s.clone()),
            Repr::Bytes(Bytes::Shared(b)) => Const::Bytes(b.clone()),
            Repr::Time(t, z) => Const::Time(*t, z.clone()),
            Repr::Duration(d) => Const::Duration(*d),
            Repr::Location(z) => Const::Location(z.clone()),
            Repr::Array(Seq::Const(xs)) => Const::Array(xs.clone()),
            Repr::Array(Seq::Owned(xs, _)) => {
                Const::Array(xs.iter().map(|x| x.to_const_within(budget)).collect::<Option<_>>()?)
            }
            Repr::Map(MapRef::Const(kvs)) => Const::Map(kvs.clone()),
            Repr::Map(MapRef::Owned(kvs, _)) => Const::Map(
                kvs.iter().map(|(k, v)| Some((k.clone(), v.to_const_within(budget)?))).collect::<Option<_>>()?,
            ),
            Repr::Str(Str::Borrowed(_))
            | Repr::Bytes(Bytes::Borrowed(_))
            | Repr::Array(Seq::Borrowed(_) | Seq::Range { .. })
            | Repr::Map(MapRef::Borrowed(_))
            | Repr::Object(_) => return None,
        })
    }

    pub(crate) fn as_bool(&self) -> Option<bool> {
        match self.0 {
            Repr::Bool(b) => Some(b),
            _ => None,
        }
    }

    pub(crate) fn as_int(&self) -> Option<i64> {
        match self.0 {
            Repr::Int(n) => Some(n),
            _ => None,
        }
    }

    pub(crate) fn as_float(&self) -> Option<f64> {
        match self.0 {
            Repr::Float(x) => Some(x),
            _ => None,
        }
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match &self.0 {
            Repr::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub(crate) fn as_bytes(&self) -> Option<&[u8]> {
        match &self.0 {
            Repr::Bytes(b) => Some(b.as_slice()),
            _ => None,
        }
    }

    /// The instant of a time, without its zone.
    pub(crate) fn as_time(&self) -> Option<jiff::Timestamp> {
        match self.0 {
            Repr::Time(t, _) => Some(t),
            _ => None,
        }
    }

    pub(crate) fn as_zoned(&self) -> Option<(jiff::Timestamp, &jiff::tz::TimeZone)> {
        match &self.0 {
            Repr::Time(t, z) => Some((*t, z)),
            _ => None,
        }
    }

    pub(crate) fn as_location(&self) -> Option<&jiff::tz::TimeZone> {
        match &self.0 {
            Repr::Location(z) => Some(z),
            _ => None,
        }
    }

    pub(crate) fn as_duration(&self) -> Option<jiff::SignedDuration> {
        match self.0 {
            Repr::Duration(d) => Some(d),
            _ => None,
        }
    }

    pub(crate) fn as_seq(&self) -> Option<&Seq<'a>> {
        match &self.0 {
            Repr::Array(s) => Some(s),
            _ => None,
        }
    }

    pub(crate) fn as_map(&self) -> Option<&MapRef<'a>> {
        match &self.0 {
            Repr::Map(m) => Some(m),
            _ => None,
        }
    }

    pub(crate) fn is_nil(&self) -> bool {
        matches!(self.0, Repr::Nil)
    }

    /// Elements that copying this value out (to a `Dynamic` result) would build
    /// beyond what is in memory: a range's elements, and a computed container once
    /// per reference, since the copy is a tree. Counting stops past `limit`.
    pub(crate) fn copied_elements(&self, limit: u64) -> u64 {
        let mut n = 0u64;
        self.count_copied(limit, &mut n);
        n
    }

    fn count_copied(&self, limit: u64, n: &mut u64) {
        let add = |n: &mut u64, k: usize| *n = n.saturating_add(u64::try_from(k).unwrap_or(u64::MAX));
        match &self.0 {
            Repr::Array(s @ Seq::Range { .. }) => add(n, s.len()),
            Repr::Array(Seq::Owned(xs, _)) => {
                add(n, xs.len());
                for x in xs.iter() {
                    if *n > limit {
                        return;
                    }
                    x.count_copied(limit, n);
                }
            }
            Repr::Map(MapRef::Owned(kvs, _)) => {
                add(n, kvs.len());
                for (_, x) in kvs.iter() {
                    if *n > limit {
                        return;
                    }
                    x.count_copied(limit, n);
                }
            }
            _ => {}
        }
    }

    /// A deep, owned copy. Structs become maps keyed by field name.
    pub(crate) fn to_dynamic(&self) -> Dynamic {
        match &self.0 {
            Repr::Nil => Dynamic::Nil,
            Repr::Bool(b) => Dynamic::Bool(*b),
            Repr::Int(n) => Dynamic::Int(*n),
            Repr::Float(x) => Dynamic::Float(*x),
            #[cfg(feature = "decimal")]
            Repr::Decimal(d) => Dynamic::Decimal(*d),
            Repr::Str(s) => Dynamic::String(s.as_str().into()),
            Repr::Bytes(b) => Dynamic::Bytes(b.as_slice().into()),
            Repr::Time(t, z) => Dynamic::Time(t.to_zoned(z.clone())),
            Repr::Duration(d) => Dynamic::Duration(*d),
            Repr::Location(z) => Dynamic::Location(z.clone()),
            Repr::Array(seq) => Dynamic::Array(seq.iter().map(|x| x.to_dynamic()).collect()),
            Repr::Map(m) => {
                let mut out = DynMap::new();
                m.for_each(&mut |k, v| {
                    out.insert(k, v.to_dynamic());
                    ControlFlow::Continue(())
                });
                Dynamic::Map(out)
            }
            Repr::Object(o) => {
                let mut out = DynMap::new();
                for (i, f) in o.desc().fields().iter().enumerate() {
                    let id = u16::try_from(i).map_or_else(|_| missing_field(), |i| o.field(FieldId::new(i)));
                    out.insert(f.name(), id.to_dynamic());
                }
                Dynamic::Map(out)
            }
        }
    }

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
            Repr::Time(..) => Tag::Time,
            Repr::Duration(_) => Tag::Duration,
            Repr::Location(_) => Tag::Location,
            Repr::Array(_) => Tag::Array,
            Repr::Map(_) => Tag::Map,
            Repr::Object(_) => Tag::Struct,
        }
    }
}
