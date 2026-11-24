//! An owned, dynamically typed value.

use alloc::string::String;
use alloc::vec::Vec;
use core::ops::ControlFlow;

use dictum_check::{FieldId, Type};

use crate::engine::value::{Bytes, MapRef, Repr, Seq, Str, Value, missing_field};
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
    /// `time`, in the zone it is shown in
    Time(jiff::Zoned),
    /// `duration`
    Duration(jiff::SignedDuration),
    /// `location`
    Location(jiff::tz::TimeZone),
    /// `array`
    Array(Vec<Dynamic>),
    /// `map` (and structs, keyed by field name)
    Map(DynMap),
}

impl Dynamic {
    /// A deep, owned copy of an evaluated value. Structs become maps keyed by field
    /// name.
    pub(crate) fn copy_of(v: &Value<'_>) -> Dynamic {
        match &v.0 {
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
            Repr::Array(seq) => Dynamic::Array(seq.iter().map(|x| Dynamic::copy_of(&x)).collect()),
            Repr::Map(m) => {
                let mut out = DynMap::new();
                m.for_each(&mut |k, v| {
                    out.insert(k, Dynamic::copy_of(&v));
                    ControlFlow::Continue(())
                });
                Dynamic::Map(out)
            }
            Repr::Object(o) => {
                let mut out = DynMap::new();
                for (i, f) in o.desc().fields().iter().enumerate() {
                    let id = u16::try_from(i).map_or_else(|_| missing_field(), |i| o.field(FieldId::new(i)));
                    out.insert(f.name(), Dynamic::copy_of(&id));
                }
                Dynamic::Map(out)
            }
        }
    }
}

/// The value as a rule literal would write it: `"Ada"`, `[1, 2]`, `{gift: "yes"}`,
/// `nil`, a duration as `1h30m0s`, a float as Go prints it. Used by `explain` and
/// the CLI.
impl core::fmt::Display for Dynamic {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut out = String::new();
        write_value(self, &mut out);
        f.write_str(&out)
    }
}

/// A value as a rule literal would write it.
fn write_value(v: &Dynamic, out: &mut String) {
    use core::fmt::Write;
    match v {
        Dynamic::Nil => out.push_str("nil"),
        Dynamic::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Dynamic::Int(n) => {
            let _ = write!(out, "{n}");
        }
        Dynamic::Float(x) => crate::engine::builtins::go_float(*x, out),
        #[cfg(feature = "decimal")]
        Dynamic::Decimal(d) => {
            let _ = write!(out, "{d}");
        }
        Dynamic::String(s) => {
            let _ = write!(out, "{s:?}");
        }
        Dynamic::Bytes(b) => {
            let _ = write!(out, "{b:?}");
        }
        Dynamic::Time(t) if t.time_zone() == &jiff::tz::TimeZone::UTC => {
            let _ = write!(out, "{}", t.timestamp());
        }
        Dynamic::Time(t) => {
            let _ = write!(out, "{t}");
        }
        Dynamic::Location(z) => out.push_str(crate::engine::builtins::location_name(z)),
        Dynamic::Duration(d) => crate::engine::builtins::go_duration(*d, out),
        Dynamic::Array(xs) => {
            out.push('[');
            for (i, x) in xs.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_value(x, out);
            }
            out.push(']');
        }
        Dynamic::Map(m) => {
            out.push('{');
            for (i, (k, x)) in m.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                // A key a rule could not write bare is quoted, as in a map literal.
                if dictum_syntax::is_identifier(k) {
                    let _ = write!(out, "{k}: ");
                } else {
                    let _ = write!(out, "{k:?}: ");
                }
                write_value(x, out);
            }
            out.push('}');
        }
    }
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
            Dynamic::Time(t) => Repr::Time(t.timestamp(), t.time_zone().clone()),
            Dynamic::Duration(d) => Repr::Duration(*d),
            Dynamic::Location(z) => Repr::Location(z.clone()),
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
