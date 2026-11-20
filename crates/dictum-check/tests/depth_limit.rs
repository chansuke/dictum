//! The IR depth limit. The parser bounds the AST's height by `MAX_DEPTH`,
//! but checking adds conversions the parser cannot foresee (operand
//! promotions, the result's `Erase`), and each one is an IR level — so a
//! rule the parser accepts can still come out of the checker deeper than
//! `MAX_DEPTH`, breaking the invariant `Program::validate` asserts.
//! Checking must reject those with E0105 instead. Regression tests for
//! the crash the Deep workflow's `check` fuzz target found on 2026-11-19
//! (run 44517413081).
// Test code: the workspace's no-panic lints are for evaluation code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use dictum_check::{Env, Type};
use dictum_syntax::ErrorCode;
use dictum_testkit::Facts;

fn env() -> Env<()> {
    Env::new(<Facts as dictum::Dictum>::__desc(), Vec::new()).expect("fixture schema")
}

fn check(src: &str) -> Result<dictum_check::ir::Program, dictum_syntax::Diagnostics> {
    let ast = dictum_syntax::parse(src).expect("the inputs here parse");
    dictum_check::check(&ast, &env(), &Type::Any)
}

fn assert_too_deep(src: &str) {
    let Err(diags) = check(src) else { panic!("{src:.60}... must be rejected as too deep") };
    let (kept, _) = diags.finish();
    assert!(kept.iter().any(|d| d.code() == ErrorCode::TooDeep), "expected E0105, got {kept:?}");
}

fn assert_within_limit(src: &str) {
    let program = check(src).unwrap_or_else(|d| panic!("{src:.60}... must check: {d:?}"));
    program.validate(&env()).expect("a checked program holds its invariants");
}

#[test]
fn chains_at_the_boundary() {
    // 127 ones joined by `+`: the IR is exactly MAX_DEPTH deep once the
    // result's `Erase` conversion is counted. Accepted, and validates.
    assert_within_limit(&vec!["1"; 127].join("+"));
    // 128 ones: the AST's height is exactly MAX_DEPTH, so the parser
    // accepts it — but the IR would be one level deeper. E0105.
    assert_too_deep(&vec!["1"; 128].join("+"));
    // Division promotes the leftmost operand Int -> Float, one more IR
    // level than `+`: the boundary is one term earlier.
    assert_within_limit(&vec!["8"; 126].join("/"));
    assert_too_deep(&vec!["8"; 127].join("/"));
}

#[test]
fn fuzz_crash_input_is_rejected_not_broken() {
    // The exact input the fuzzer found: the checker accepted it and the
    // resulting program failed validation ("node 1 ... deeper than
    // MAX_DEPTH"). It must now be a plain E0105 rejection.
    let src = "2+1/2+2++2/2+7++2+22/2+2/2+2++2+2++7++2/2+7++2++2/2+7++3+22+2++722+2++72+2+1++3++2++2++7++2++7++2/272+++++7++2+22+2++722+2++72+2+1++7++2++2+2+7/6+7++2+22/2+7++2++7++2/2+7+++2+22/2+2/2+2++2+2++7++2/2+7++2++2/2+7++3+22+2++722+2++72+2+1++3++2++2++7++2++7++2/272+++++7++2+22+2++722+2++72+2+1++7++2++2+2+7/6+7++2+22/2+7++2++7++2/2+7++2++2/2+7+2++2++7++2++7++2/272+++++7++2+22+2++722+2++72+22+1++7++2++2+2+7";
    assert_too_deep(src);
}
