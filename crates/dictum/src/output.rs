//! Rule result types.

use alloc::string::String;
use alloc::vec::Vec;

use dictum_check::Type;

use crate::Dynamic;
use crate::engine::value::Value;

mod sealed {
    // Reachable only as a supertrait, which is the point of sealing.
    #[allow(unreachable_pub)]
    pub trait Sealed {}
    impl Sealed for bool {}
    impl Sealed for i64 {}
    impl Sealed for f64 {}
    impl Sealed for alloc::string::String {}
    impl Sealed for jiff::Timestamp {}
    impl Sealed for jiff::SignedDuration {}
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
/// `jiff::SignedDuration`, `Option<T>`, `Vec<T>`, and `Dynamic`. Sealed: the hidden
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
        todo!()
    }
}

impl Output for i64 {
    fn expected() -> Type {
        Type::Int
    }
    fn from_value(v: Value<'_>) -> Option<Self> {
        todo!()
    }
}

impl Output for f64 {
    fn expected() -> Type {
        Type::Float
    }
    fn from_value(v: Value<'_>) -> Option<Self> {
        todo!()
    }
}

impl Output for String {
    fn expected() -> Type {
        Type::String
    }
    fn from_value(v: Value<'_>) -> Option<Self> {
        todo!()
    }
}

impl Output for jiff::Timestamp {
    fn expected() -> Type {
        Type::Time
    }
    fn from_value(v: Value<'_>) -> Option<Self> {
        todo!()
    }
}

impl Output for jiff::SignedDuration {
    fn expected() -> Type {
        Type::Duration
    }
    fn from_value(v: Value<'_>) -> Option<Self> {
        todo!()
    }
}

impl<T: Output> Output for Option<T> {
    fn expected() -> Type {
        Type::option(T::expected())
    }
    fn from_value(v: Value<'_>) -> Option<Self> {
        todo!()
    }
}

impl<T: Output> Output for Vec<T> {
    fn expected() -> Type {
        Type::array(T::expected())
    }
    fn from_value(v: Value<'_>) -> Option<Self> {
        todo!()
    }
}

impl Output for Dynamic {
    fn expected() -> Type {
        Type::Any
    }
    fn from_value(v: Value<'_>) -> Option<Self> {
        // Deep copy; structs become `Dynamic::Map` keyed by field name.
        todo!()
    }
}
