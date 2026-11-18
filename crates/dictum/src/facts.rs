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

use crate::engine::value::Value;

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
    /// Element `i`, or `None` past the end.
    fn get(&self, i: usize) -> Option<Value<'_>>;
}

/// A borrowed string-keyed map, iterated in its container's (deterministic) order.
pub trait MapView {
    /// Number of entries.
    fn len(&self) -> usize;
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
    ($($rust:ty => $ty:ident),* $(,)?) => {$(
        impl Field for $rust {
            fn ty() -> Type {
                Type::$ty
            }
            fn read(&self) -> Value<'_> {
                todo!()
            }
        }
    )*};
}

scalar_fields! {
    bool => Bool,
    i8 => Int, i16 => Int, i32 => Int, i64 => Int,
    u8 => Int, u16 => Int, u32 => Int,
    f32 => Float, f64 => Float,
    jiff::Timestamp => Time,
    jiff::SignedDuration => Duration,
}

impl Field for String {
    fn ty() -> Type {
        Type::String
    }
    fn read(&self) -> Value<'_> {
        todo!()
    }
}

impl Field for &str {
    fn ty() -> Type {
        Type::String
    }
    fn read(&self) -> Value<'_> {
        todo!()
    }
}

impl Field for Box<str> {
    fn ty() -> Type {
        Type::String
    }
    fn read(&self) -> Value<'_> {
        todo!()
    }
}

impl<T: Field> Field for Option<T> {
    fn ty() -> Type {
        Type::option(T::ty())
    }
    fn read(&self) -> Value<'_> {
        todo!()
    }
}

impl<T: Field> Field for Vec<T> {
    fn ty() -> Type {
        Type::array(T::ty())
    }
    fn read(&self) -> Value<'_> {
        // Value::seq(self as &dyn SeqView)
        todo!()
    }
}

impl<T: Field> Field for &[T] {
    fn ty() -> Type {
        Type::array(T::ty())
    }
    fn read(&self) -> Value<'_> {
        // `&&[T]` coerces to `&dyn SeqView` (a slice itself cannot).
        todo!()
    }
}

impl<T: Field> Field for Box<[T]> {
    fn ty() -> Type {
        Type::array(T::ty())
    }
    fn read(&self) -> Value<'_> {
        todo!()
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
        todo!()
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
