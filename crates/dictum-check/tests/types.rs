//! `Type` invariants that hold before the checker exists: `Option` normalization
//! (no `Option<Option<T>>`, no `Option<Nil>`, no `Option<Any>`).

use dictum_check::Type;
use proptest::prelude::*;

fn ty() -> impl Strategy<Value = Type> {
    let leaf = prop_oneof![
        Just(Type::Any),
        Just(Type::Nil),
        Just(Type::Bool),
        Just(Type::Int),
        Just(Type::Float),
        Just(Type::String),
        Just(Type::Bytes),
        Just(Type::Time),
        Just(Type::Duration),
    ];
    leaf.prop_recursive(4, 16, 1, |inner| {
        prop_oneof![
            inner.clone().prop_map(Type::array),
            inner.clone().prop_map(Type::map),
            inner.prop_map(Type::option)
        ]
    })
}

/// No `Option` anywhere in `t` wraps `Option`, `Nil` or `Any`.
fn normalized(t: &Type) -> bool {
    match t {
        Type::Option(o) => !matches!(o.inner(), Type::Option(_) | Type::Nil | Type::Any) && normalized(o.inner()),
        Type::Array(e) | Type::Map(e) => normalized(e),
        _ => true,
    }
}

proptest! {
    #[test]
    fn option_is_idempotent(t in ty()) {
        let once = Type::option(t);
        prop_assert_eq!(Type::option(once.clone()), once);
    }

    #[test]
    fn every_constructed_type_is_normalized(t in ty()) {
        prop_assert!(normalized(&t), "{t:?}");
    }

    #[test]
    fn option_of_a_non_optional_type_wraps_it(t in ty()) {
        let o = Type::option(t.clone());
        match t {
            Type::Option(_) | Type::Nil | Type::Any => prop_assert_eq!(o, t),
            _ => prop_assert!(matches!(&o, Type::Option(inner) if *inner.inner() == t)),
        }
    }
}
