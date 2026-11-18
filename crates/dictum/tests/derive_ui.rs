//! Compile-time guarantees, checked by compiling small programs: what derives, what
//! the derive rejects, and what the type system rejects at the API.
//!
//! The expected compiler output lives next to each case (`ui/fail/*.stderr`). It
//! depends on the compiler version, so this runs only on the pinned toolchain
//! (`rust-toolchain.toml`); regenerate with `TRYBUILD=overwrite` after a toolchain
//! bump and review the diff.

#[test]
fn derive_and_api_compile_time_checks() {
    let t = trybuild::TestCases::new();
    t.pass("tests/ui/pass/*.rs");
    t.compile_fail("tests/ui/fail/*.rs");
}
