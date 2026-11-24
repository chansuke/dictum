//! Schema functions: Rust closures whose signature is inferred from their types.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;

use dictum_check::{FuncSig, Type};

use crate::engine::value::Value;

/// Whether a function may be folded and shared.
///
/// Every schema function must be free of side effects (the doc's premise). `Pure`
/// additionally promises the same result for the same arguments, so the folder may
/// call it at compile time when its arguments are constant and `RuleSet` may share
/// one call between rules. Chosen by `SchemaBuilder::pure_func` vs `func`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purity {
    Pure,
    Volatile,
}

/// Why a call failed.
pub(crate) enum CallError {
    /// The function returned `Err`.
    Function(Box<dyn core::error::Error + Send + Sync>),
    /// An argument did not convert. The checker typed every argument against the
    /// signature, so this is a dictum bug and becomes `EvalErrorKind::Internal`.
    BadArgument,
}

pub(crate) type Call = dyn for<'a> Fn(&[Value<'a>]) -> Result<Value<'a>, CallError> + Send + Sync;

/// The `P` payload of `dictum_check::Env`: what the evaluator needs to run a call.
pub(crate) struct FuncImpl {
    pub(crate) purity: Purity,
    pub(crate) call: Arc<Call>,
}

/// A Rust function usable as a schema function: `Fn(A1, ..., An) -> R` for
/// `n <= 4`, where each `Ai` is `bool`, `i64`, `f64`, `&str` or `jiff::Timestamp`,
/// and `R` is `bool`, `i64`, `f64` or `String`, or `Result<that, E>` with
/// `E: Error + Send + Sync + 'static`. The parameter and return lists are the
/// `FromArg` and `RetValue` impls below; extend those, not this comment.
///
/// The marker `M` only disambiguates arities; callers never name it.
pub trait IntoFunc<M>: Send + Sync + 'static {
    #[doc(hidden)]
    fn into_func(self) -> ErasedFunc;
}

/// A function with its inferred signature, type-erased. Opaque outside this crate.
pub struct ErasedFunc {
    pub(crate) sig: FuncSig,
    pub(crate) call: Arc<Call>,
}

/// A parameter type. `Of<'s>` is the type the closure receives, which may borrow
/// from the argument (`&str`).
pub trait FromArg {
    /// The parameter as the closure receives it.
    type Of<'s>;
    /// Its dictum type.
    fn ty() -> Type;
    /// Convert; `None` only on a checker bug.
    fn from_value<'s>(v: &'s Value<'_>) -> Option<Self::Of<'s>>;
}

/// An infallible return type.
pub trait RetValue {
    /// Its dictum type.
    fn ty() -> Type;
    /// Into an owned value.
    fn into_value(self) -> Value<'static>;
}

/// A return type: a [`RetValue`] or `Result<RetValue, E>`.
pub trait Ret {
    /// Its dictum type.
    fn ty() -> Type;
    #[doc(hidden)]
    fn into_result(self) -> Result<Value<'static>, Box<dyn core::error::Error + Send + Sync>>;
}

impl<T: RetValue> Ret for T {
    fn ty() -> Type {
        T::ty()
    }
    fn into_result(self) -> Result<Value<'static>, Box<dyn core::error::Error + Send + Sync>> {
        Ok(self.into_value())
    }
}

impl<T: RetValue, E: core::error::Error + Send + Sync + 'static> Ret for Result<T, E> {
    fn ty() -> Type {
        T::ty()
    }
    fn into_result(self) -> Result<Value<'static>, Box<dyn core::error::Error + Send + Sync>> {
        self.map(RetValue::into_value).map_err(|e| Box::new(e) as _)
    }
}

impl FromArg for i64 {
    type Of<'s> = i64;
    fn ty() -> Type {
        Type::Int
    }
    fn from_value(v: &Value<'_>) -> Option<i64> {
        v.as_int()
    }
}

impl FromArg for f64 {
    type Of<'s> = f64;
    fn ty() -> Type {
        Type::Float
    }
    fn from_value(v: &Value<'_>) -> Option<f64> {
        v.as_float()
    }
}

impl FromArg for bool {
    type Of<'s> = bool;
    fn ty() -> Type {
        Type::Bool
    }
    fn from_value(v: &Value<'_>) -> Option<bool> {
        v.as_bool()
    }
}

/// Borrowed string parameter: no allocation per call.
impl FromArg for &str {
    type Of<'s> = &'s str;
    fn ty() -> Type {
        Type::String
    }
    fn from_value<'s>(v: &'s Value<'_>) -> Option<&'s str> {
        v.as_str()
    }
}

impl FromArg for jiff::Timestamp {
    type Of<'s> = jiff::Timestamp;
    fn ty() -> Type {
        Type::Time
    }
    fn from_value(v: &Value<'_>) -> Option<jiff::Timestamp> {
        v.as_time()
    }
}

impl RetValue for bool {
    fn ty() -> Type {
        Type::Bool
    }
    fn into_value(self) -> Value<'static> {
        Value::bool(self)
    }
}

impl RetValue for i64 {
    fn ty() -> Type {
        Type::Int
    }
    fn into_value(self) -> Value<'static> {
        Value::int(self)
    }
}

impl RetValue for f64 {
    fn ty() -> Type {
        Type::Float
    }
    fn into_value(self) -> Value<'static> {
        Value::float(self)
    }
}

impl RetValue for String {
    fn ty() -> Type {
        Type::String
    }
    fn into_value(self) -> Value<'static> {
        Value::string(self)
    }
}

/// Fix a closure's signature to the higher-ranked `Call`, which closure inference
/// does not do on its own.
fn erase(f: impl for<'a> Fn(&[Value<'a>]) -> Result<Value<'a>, CallError> + Send + Sync + 'static) -> Arc<Call> {
    Arc::new(f)
}

// Arity impls. The double bound is what lets `|code: &str| ...` infer: `Fn(A1) -> R`
// fixes `A1 = &'x str` for an inferred `'x`, and `for<'s> Fn(A1::Of<'s>) -> R`
// requires the closure to accept a borrow of any lifetime, which a closure
// annotated with `&str` does.
macro_rules! into_func {
    ($($a:ident $v:ident),*) => {
        impl<Func, R, $($a,)*> IntoFunc<(R, $($a,)*)> for Func
        where
            Func: Fn($($a),*) -> R + for<'s> Fn($($a::Of<'s>),*) -> R + Send + Sync + 'static,
            R: Ret,
            $($a: FromArg,)*
        {
            fn into_func(self) -> ErasedFunc {
                let sig = FuncSig { params: alloc::vec![$($a::ty()),*].into(), ret: R::ty() };
                let call = erase(move |args| {
                    let [$($v),*] = args else { return Err(CallError::BadArgument) };
                    $( let $v = $a::from_value($v).ok_or(CallError::BadArgument)?; )*
                    self($($v),*).into_result().map_err(CallError::Function)
                });
                ErasedFunc { sig, call }
            }
        }
    };
}

into_func!();
into_func!(A1 a1);
into_func!(A1 a1, A2 a2);
into_func!(A1 a1, A2 a2, A3 a3);
into_func!(A1 a1, A2 a2, A3 a3, A4 a4);
