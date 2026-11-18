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
pub const MILESTONE: u8 = 0;

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
