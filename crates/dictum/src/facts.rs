//! How Rust data becomes dictum values without copying.
//!
//! What is borrowed (zero-copy) and what is not, decided once here:
//! - borrowed: `String`/`&str`/`Box<str>` contents, nested derived structs, sequences
//!   (`Vec<T>`, `Box<[T]>`, `&[T]`) and maps (`BTreeMap<String, T>`). A sequence is
//!   read through [`SeqView`]: element values are produced on access, so a
//!   `Vec<String>` field is never converted to a `Vec<Value>`.
//! - copied: scalars (`bool`, integers, floats, timestamps, durations), by value.
//! - owned (`Arc`): only values computed during evaluation (literals with dynamic
//!   parts, `filter`/`map` results, concatenations).
//! - rejected at compile time: `HashMap` (iteration order is not deterministic, and
//!   `keys(m)` must be), `u64`/`usize`/`i128` (may not fit `int`), generic structs.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::ops::ControlFlow;

use dictum_check::{FieldId, StructDesc, Type};

use crate::engine::value::{MapRef, Repr, Seq, Str, Value};

/// A type that can be the root of a [`Schema`](crate::Schema): its fields are the
/// variables a rule can name. Implemented by `#[derive(Dictum)]`; every derived
/// struct can be a root or a nested field.
pub trait Dictum: Object {
    /// `Self` with every lifetime parameter replaced by `'static`; the type a
    /// `Schema`/`Rule` is parameterized by. Lets `Facts<'a>` (borrowing its parts)
    /// be evaluated by a `Rule<_, Facts<'static>>`.
    #[doc(hidden)]
    type Static: Dictum<Static = Self::Static> + 'static;

    /// The descriptor of this struct.
    #[doc(hidden)]
    fn __desc() -> &'static StructDesc;
}

/// A derived struct, read field by field. Implemented by `#[derive(Dictum)]` only.
///
/// Contract (upheld by the derive, which generates both from one field list):
/// `field(FieldId(i))` returns a value of type `desc().fields()[i].ty()`. An
/// out-of-range id returns `missing_field()`; the evaluator reports a mismatch as an
/// internal error instead of panicking. Not `Send + Sync`: facts are borrowed by one
/// evaluation on one thread.
pub trait Object {
    /// The descriptor, for explain output and the runtime struct name.
    fn desc(&self) -> &'static StructDesc;
    /// Read one field, borrowing from `self`.
    fn field(&self, id: FieldId) -> Value<'_>;
}

/// A Rust type usable as a field (or nested element) of a derived struct.
///
/// One impl per supported Rust type gives both its dictum type and how to read it,
/// so the two cannot disagree.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be used in a dictum fact",
    note = "supported: bool, i8..i64, u8..u32, f32, f64, String, &str, Box<str>, \
            jiff::Timestamp, jiff::SignedDuration, Option<T>, Vec<T>, Box<[T]>, &[T], \
            Box<T>, &T, BTreeMap<String, T>, dictum::Dynamic, and #[derive(Dictum)] structs",
    note = "HashMap is rejected because its order is not deterministic; use BTreeMap",
    note = "use #[dictum(skip)] to keep the field out of rules"
)]
pub trait Field {
    /// The dictum type of values of `Self`.
    fn ty() -> Type;
    /// Borrow `self` as a value.
    fn read(&self) -> Value<'_>;
}

/// A borrowed sequence. Elements are produced on access.
pub trait SeqView {
    /// Number of elements.
    fn len(&self) -> usize;
    /// `len() == 0`.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Element `i`, or `None` past the end.
    fn get(&self, i: usize) -> Option<Value<'_>>;
}

/// A borrowed string-keyed map, iterated in its container's (deterministic) order.
pub trait MapView {
    /// Number of entries.
    fn len(&self) -> usize;
    /// `len() == 0`.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Lookup.
    fn get(&self, key: &str) -> Option<Value<'_>>;
    /// Visit entries in order until `f` breaks. Internal iteration keeps the trait
    /// object-safe without boxing an iterator.
    fn for_each<'s>(&'s self, f: &mut dyn FnMut(&'s str, Value<'s>) -> ControlFlow<()>);
}

/// Scalars that map to one dictum type, one row per Rust type. Integers widen to
/// `int` (`i64::from`) and `f32` to `float`; `u64`/`usize` are left out because they
/// do not fit in `i64`. The `on_unimplemented` note on [`Field`] lists these rows.
macro_rules! scalar_fields {
    ($($rust:ty => $ty:ident($repr:ident)),* $(,)?) => {$(
        impl Field for $rust {
            fn ty() -> Type {
                Type::$ty
            }
            fn read(&self) -> Value<'_> {
                Value(Repr::$repr(From::from(*self)))
            }
        }
    )*};
}

scalar_fields! {
    bool => Bool(Bool),
    i8 => Int(Int), i16 => Int(Int), i32 => Int(Int), i64 => Int(Int),
    u8 => Int(Int), u16 => Int(Int), u32 => Int(Int),
    f32 => Float(Float), f64 => Float(Float),
    jiff::Timestamp => Time(Time),
    jiff::SignedDuration => Duration(Duration),
}

impl Field for String {
    fn ty() -> Type {
        Type::String
    }
    fn read(&self) -> Value<'_> {
        Value(Repr::Str(Str::Borrowed(self)))
    }
}

impl Field for &str {
    fn ty() -> Type {
        Type::String
    }
    fn read(&self) -> Value<'_> {
        Value(Repr::Str(Str::Borrowed(self)))
    }
}

impl Field for Box<str> {
    fn ty() -> Type {
        Type::String
    }
    fn read(&self) -> Value<'_> {
        Value(Repr::Str(Str::Borrowed(self)))
    }
}

impl<T: Field> Field for Option<T> {
    fn ty() -> Type {
        Type::option(T::ty())
    }
    fn read(&self) -> Value<'_> {
        self.as_ref().map_or(Value::NIL, Field::read)
    }
}

impl<T: Field> Field for Vec<T> {
    fn ty() -> Type {
        Type::array(T::ty())
    }
    fn read(&self) -> Value<'_> {
        Value(Repr::Array(Seq::Borrowed(self)))
    }
}

impl<T: Field> Field for &[T] {
    fn ty() -> Type {
        Type::array(T::ty())
    }
    fn read(&self) -> Value<'_> {
        Value(Repr::Array(Seq::Borrowed(self)))
    }
}

impl<T: Field> Field for Box<[T]> {
    fn ty() -> Type {
        Type::array(T::ty())
    }
    fn read(&self) -> Value<'_> {
        Value(Repr::Array(Seq::Borrowed(self)))
    }
}

/// `&T` for sized `T`: the root borrowing its parts (`Facts<'a> { user: &'a User }`).
impl<T: Field> Field for &T {
    fn ty() -> Type {
        T::ty()
    }
    fn read(&self) -> Value<'_> {
        T::read(self)
    }
}

impl<T: Field> Field for Box<T> {
    fn ty() -> Type {
        T::ty()
    }
    fn read(&self) -> Value<'_> {
        T::read(self)
    }
}

impl<T: Field> Field for BTreeMap<String, T> {
    fn ty() -> Type {
        Type::map(T::ty())
    }
    fn read(&self) -> Value<'_> {
        Value(Repr::Map(MapRef::Borrowed(self)))
    }
}

impl<T: Field> SeqView for Vec<T> {
    fn len(&self) -> usize {
        Vec::len(self)
    }
    fn get(&self, i: usize) -> Option<Value<'_>> {
        self.as_slice().get(i).map(Field::read)
    }
}

impl<T: Field> SeqView for &[T] {
    fn len(&self) -> usize {
        <[T]>::len(self)
    }
    fn get(&self, i: usize) -> Option<Value<'_>> {
        <[T]>::get(self, i).map(Field::read)
    }
}

impl<T: Field> SeqView for Box<[T]> {
    fn len(&self) -> usize {
        <[T]>::len(self)
    }
    fn get(&self, i: usize) -> Option<Value<'_>> {
        <[T]>::get(self, i).map(Field::read)
    }
}

impl<T: Field> MapView for BTreeMap<String, T> {
    fn len(&self) -> usize {
        BTreeMap::len(self)
    }
    fn get(&self, key: &str) -> Option<Value<'_>> {
        BTreeMap::get(self, key).map(Field::read)
    }
    fn for_each<'s>(&'s self, f: &mut dyn FnMut(&'s str, Value<'s>) -> ControlFlow<()>) {
        for (k, v) in self {
            if f(k, v.read()).is_break() {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use alloc::boxed::Box;
    use alloc::collections::BTreeMap;
    use alloc::string::String;
    use alloc::vec::Vec;
    use core::ops::ControlFlow;

    use dictum_check::{FieldId, Tag, Type};
    use jiff::{SignedDuration, Timestamp};
    use proptest::prelude::*;

    use super::{Field, Object};
    use crate::Dynamic;
    use crate::engine::value::{MapRef, Repr, Seq, Value};

    #[derive(crate::Dictum, Clone, Debug)]
    struct Inner {
        n: u16,
        tags: Vec<String>,
    }

    /// Every supported field type once, nested and optional ones included.
    #[derive(crate::Dictum, Clone, Debug)]
    struct All {
        b: bool,
        i_8: i8,
        i_16: i16,
        i_32: i32,
        i_64: i64,
        u_8: u8,
        u_16: u16,
        u_32: u32,
        f_32: f32,
        f_64: f64,
        s: String,
        bs: Box<str>,
        t: Timestamp,
        d: SignedDuration,
        any: Dynamic,
        opt: Option<i64>,
        opt_inner: Option<Box<Inner>>,
        xs: Vec<i32>,
        fixed: Box<[f64]>,
        map: BTreeMap<String, Option<String>>,
        inner: Inner,
    }

    fn inner() -> impl Strategy<Value = Inner> {
        (any::<u16>(), proptest::collection::vec("[a-z]{0,3}", 0..3)).prop_map(|(n, tags)| Inner { n, tags })
    }

    fn dynamic() -> impl Strategy<Value = Dynamic> {
        prop_oneof![
            Just(Dynamic::Nil),
            any::<bool>().prop_map(Dynamic::Bool),
            any::<i64>().prop_map(Dynamic::Int),
            "[a-z]{0,3}".prop_map(Dynamic::String),
            proptest::collection::vec(any::<i64>().prop_map(Dynamic::Int), 0..3).prop_map(Dynamic::Array),
        ]
    }

    prop_compose! {
        fn all()(
            (b, i_8, i_16, i_32, i_64, u_8, u_16, u_32) in
                (any::<bool>(), any::<i8>(), any::<i16>(), any::<i32>(), any::<i64>(), any::<u8>(), any::<u16>(), any::<u32>()),
            (f_32, f_64, s, bs, secs, nanos) in
                (any::<f32>(), any::<f64>(), ".{0,4}", ".{0,4}", -1_000_000_000i64..1_000_000_000, any::<i32>()),
            (any_, opt, opt_inner, xs, fixed) in (
                dynamic(),
                proptest::option::of(any::<i64>()),
                proptest::option::of(inner()),
                proptest::collection::vec(any::<i32>(), 0..4),
                proptest::collection::vec(any::<f64>(), 0..4),
            ),
            (map, inner) in (proptest::collection::btree_map("[a-z]{1,3}", proptest::option::of("[a-z]{0,3}"), 0..3), inner()),
        ) -> All {
            All {
                b, i_8, i_16, i_32, i_64, u_8, u_16, u_32, f_32, f_64,
                s, bs: bs.into(),
                t: Timestamp::from_second(secs).unwrap(),
                d: SignedDuration::from_secs(i64::from(nanos)),
                any: any_, opt, opt_inner: opt_inner.map(Box::new), xs, fixed: fixed.into(), map, inner,
            }
        }
    }

    /// `v` is a value of `ty`, all the way down.
    fn conforms(ty: &Type, v: &Value<'_>) -> bool {
        match (ty, &v.0) {
            (Type::Any, _) => true,
            (Type::Option(_), Repr::Nil) => true,
            (Type::Option(o), _) => conforms(o.inner(), v),
            (Type::Array(t), Repr::Array(Seq::Borrowed(seq))) => {
                (0..seq.len()).all(|i| conforms(t, &seq.get(i).unwrap()))
            }
            (Type::Map(t), Repr::Map(MapRef::Borrowed(map))) => {
                let mut ok = true;
                map.for_each(&mut |_, x| {
                    ok &= conforms(t, &x);
                    ControlFlow::Continue(())
                });
                ok
            }
            (Type::Struct(d), Repr::Object(o)) => {
                core::ptr::eq(*d, o.desc())
                    && d.fields()
                        .iter()
                        .enumerate()
                        .all(|(i, f)| conforms(&f.ty(), &o.field(FieldId::new(u16::try_from(i).unwrap()))))
            }
            (t, _) => t.tag() == Some(v.tag()) && !matches!(v.tag(), Tag::Array | Tag::Map | Tag::Struct),
        }
    }

    proptest! {
        /// The derive emits the descriptor and the field reader from one field list,
        /// so `field(i)` is always a value of `fields()[i].ty()`.
        #[test]
        fn derived_fields_are_values_of_their_declared_types(x in all()) {
            prop_assert!(conforms(&All::ty(), &x.read()));
        }
    }

    #[test]
    fn unknown_field_ids_read_as_missing() {
        let x = Inner { n: 1, tags: Vec::new() };
        assert_eq!(x.field(FieldId::new(99)).tag(), Tag::Nil);
    }
}
