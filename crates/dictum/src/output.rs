//! Rule result types.

use alloc::string::String;
use alloc::vec::Vec;

use dictum_check::Type;

use crate::Dynamic;
#[cfg(feature = "decimal")]
use crate::engine::value::Repr;
use crate::engine::value::Value;

mod sealed {
    pub trait Sealed {}
    impl Sealed for bool {}
    impl Sealed for i64 {}
    impl Sealed for f64 {}
    impl Sealed for alloc::string::String {}
    impl Sealed for jiff::Timestamp {}
    impl Sealed for jiff::Zoned {}
    impl Sealed for jiff::tz::TimeZone {}
    impl Sealed for jiff::SignedDuration {}
    #[cfg(feature = "decimal")]
    impl Sealed for rust_decimal::Decimal {}
    impl<T: super::Output> Sealed for Option<T> {}
    impl<T: super::Output> Sealed for alloc::vec::Vec<T> {}
    impl Sealed for crate::Dynamic {}
}

/// A type a rule can return: `Rule<T, _>` requires `T: Output`.
///
/// The checker compiles the rule against `T`'s dictum type, inserting the implicit
/// conversions (`int` to `float`, `T` to `Option<T>`, `any` checked), so converting
/// the final value cannot fail. [`Dynamic`] accepts every type.
///
/// Implemented for `bool`, `i64`, `f64`, `String`, `jiff::Timestamp`,
/// `jiff::SignedDuration`, `rust_decimal::Decimal` (`decimal` feature), `Option<T>`,
/// `Vec<T>`, and `Dynamic`. Sealed: the hidden
/// methods name the internal `Type` and `Value`, so an outside impl could only be
/// wrong.
pub trait Output: sealed::Sealed + Sized + 'static {
    #[doc(hidden)]
    fn expected() -> Type;
    #[doc(hidden)]
    fn from_value(v: Value<'_>) -> Option<Self>;
}

impl Output for bool {
    fn expected() -> Type {
        Type::Bool
    }
    fn from_value(v: Value<'_>) -> Option<Self> {
        v.as_bool()
    }
}

impl Output for i64 {
    fn expected() -> Type {
        Type::Int
    }
    fn from_value(v: Value<'_>) -> Option<Self> {
        v.as_int()
    }
}

impl Output for f64 {
    fn expected() -> Type {
        Type::Float
    }
    fn from_value(v: Value<'_>) -> Option<Self> {
        v.as_float()
    }
}

impl Output for String {
    fn expected() -> Type {
        Type::String
    }
    fn from_value(v: Value<'_>) -> Option<Self> {
        v.as_str().map(Into::into)
    }
}

impl Output for jiff::Timestamp {
    fn expected() -> Type {
        Type::Time
    }
    fn from_value(v: Value<'_>) -> Option<Self> {
        v.as_time()
    }
}

impl Output for jiff::Zoned {
    fn expected() -> Type {
        Type::Time
    }
    fn from_value(v: Value<'_>) -> Option<Self> {
        v.as_zoned().map(|(t, z)| t.to_zoned(z.clone()))
    }
}

impl Output for jiff::tz::TimeZone {
    fn expected() -> Type {
        Type::Location
    }
    fn from_value(v: Value<'_>) -> Option<Self> {
        v.as_location().cloned()
    }
}

impl Output for jiff::SignedDuration {
    fn expected() -> Type {
        Type::Duration
    }
    fn from_value(v: Value<'_>) -> Option<Self> {
        v.as_duration()
    }
}

#[cfg(feature = "decimal")]
impl Output for rust_decimal::Decimal {
    fn expected() -> Type {
        Type::Decimal
    }
    fn from_value(v: Value<'_>) -> Option<Self> {
        match v.0 {
            Repr::Decimal(d) => Some(d),
            _ => None,
        }
    }
}

impl<T: Output> Output for Option<T> {
    fn expected() -> Type {
        Type::option(T::expected())
    }
    fn from_value(v: Value<'_>) -> Option<Self> {
        if v.is_nil() { Some(None) } else { T::from_value(v).map(Some) }
    }
}

impl<T: Output> Output for Vec<T> {
    fn expected() -> Type {
        Type::array(T::expected())
    }
    fn from_value(v: Value<'_>) -> Option<Self> {
        v.as_seq()?.iter().map(T::from_value).collect()
    }
}

impl Output for Dynamic {
    fn expected() -> Type {
        Type::Any
    }
    fn from_value(v: Value<'_>) -> Option<Self> {
        // Deep copy; structs become `Dynamic::Map` keyed by field name.
        Some(v.to_dynamic())
    }
}
