//! Test support for dictum: the fixture every test compiles against, generators of
//! well-typed rules, the conformance case loader, and the client for the Lean oracle.
//!
//! One fixture for unit, property, conformance, fuzz and benchmark code means a rule
//! that fails in one of them can be pasted into any other.
// Test support, not evaluation code: the workspace's no-panic lints do not apply.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

pub mod conformance;
pub mod fixture;
pub mod generate;
pub mod oracle;
pub mod syntax;

pub use fixture::{Facts, sample, sample_now};

/// The last milestone whose behavior is implemented (0 = none; see
/// docs/dictum-design.md, "マイルストーン"). Raise it in the change that completes a
/// milestone; tests gated on a later milestone are reported as ignored, not passed.
pub const MILESTONE: u8 = 4;

/// The conformance stage `MILESTONE` reaches. M2 (types) and M3 (reference
/// evaluation) are swapped relative to the stage order on purpose: see the
/// milestone list.
pub const fn stage() -> conformance::Stage {
    use conformance::Stage;
    match MILESTONE {
        0 => Stage::None,
        1 => Stage::Parse,
        2 => Stage::Check,
        _ => Stage::Eval,
    }
}

/// Whether two results are the same value, for differential tests: as `==`, except
/// that NaN is the same as NaN (`0/0` gives NaN on both sides, and `NaN != NaN`).
pub fn same_value(a: &dictum::Dynamic, b: &dictum::Dynamic) -> bool {
    use dictum::Dynamic;
    match (a, b) {
        (Dynamic::Float(x), Dynamic::Float(y)) => x == y || (x.is_nan() && y.is_nan()),
        (Dynamic::Array(xs), Dynamic::Array(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| same_value(x, y))
        }
        (Dynamic::Map(xs), Dynamic::Map(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys.iter()).all(|((k, x), (l, y))| k == l && same_value(x, y))
        }
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use dictum::Dynamic;

    use super::same_value;

    #[test]
    fn nan_is_the_same_value_as_nan() {
        assert!(same_value(&Dynamic::Float(f64::NAN), &Dynamic::Float(f64::NAN)));
        assert!(same_value(
            &Dynamic::Array(vec![Dynamic::Float(f64::NAN)]),
            &Dynamic::Array(vec![Dynamic::Float(f64::NAN)])
        ));
        assert!(!same_value(&Dynamic::Float(0.0), &Dynamic::Float(f64::NAN)));
        assert!(!same_value(&Dynamic::Int(1), &Dynamic::Float(1.0)));
    }
}
