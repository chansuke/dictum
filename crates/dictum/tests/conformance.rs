//! End-to-end conformance: every case in `tests/conformance/*.toml`, one trial each.
//!
//! A case is checked at every stage `dictum_testkit::MILESTONE` reaches:
//!
//! - parse (M1): an E01xx case fails `dictum_syntax::parse` with that code first;
//!   every other case parses.
//! - check (M2): an E02xx case fails `dictum_check::check` with that code first;
//!   every other case type-checks.
//! - eval (M3): through the public API only. `Rule::compile` fails with the
//!   expected compile-time code, or succeeds and then `eval` gives the expected value
//!   or runtime code, and `explain(ctx).into_result()` gives the same outcome.
//!
//! Cases beyond the reached stage are reported as ignored with the milestone they
//! wait for, so `cargo test` output shows what is still pending.
// Test code: the workspace's no-panic lints are for evaluation code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use dictum::{Ctx, Dynamic, ErrorCode, Rule, Schema};
use dictum_check::{Env, Type};
use dictum_testkit::conformance::{self, Case, Expect, Stage};
use dictum_testkit::{Facts, sample, sample_now};
use libtest_mimic::{Arguments, Failed, Trial};

fn main() {
    let args = Arguments::from_args();
    let cases = conformance::load(&conformance::dir()).unwrap_or_else(|e| panic!("loading cases: {e}"));
    let reached = dictum_testkit::stage();
    let trials = cases
        .into_iter()
        .map(|case| {
            let pending = reached == Stage::None;
            Trial::test(case.id.clone(), move || run(&case, reached)).with_ignored_flag(pending)
        })
        .collect();
    libtest_mimic::run(&args, trials).exit();
}

fn run(case: &Case, reached: Stage) -> Result<(), Failed> {
    let ctx = |msg: String| -> Failed {
        let deviation = case.deviation.as_deref().map(|d| format!("\n  deviation: {d}")).unwrap_or_default();
        format!("{msg}\n  expr: {}\n  expected: {:?}{deviation}", case.expr, case.expect).into()
    };
    let expected_code = match &case.expect {
        Expect::Error(code) => Some(code.as_str()),
        Expect::Value(_) => None,
    };

    // Parse.
    let ast = match (dictum_syntax::parse(&case.expr), case.expect.stage()) {
        (Err(diags), Stage::Parse) => return first_code_is(&diags.finish().0, expected_code).map_err(ctx),
        (Err(diags), _) => return Err(ctx(format!("parse failed: {:?}", codes(&diags.finish().0)))),
        (Ok(_), Stage::Parse) => return Err(ctx("parsed, expected a syntax error".into())),
        (Ok(ast), _) => ast,
    };
    if reached < Stage::Check {
        return Ok(());
    }

    // Check.
    let env = Env::<()>::new(<Facts as dictum::Dictum>::__desc(), Vec::new()).map_err(|e| ctx(format!("{e:?}")))?;
    match (dictum_check::check(&ast, &env, &Type::Any), case.expect.stage()) {
        (Err(diags), Stage::Check) => return first_code_is(&diags.finish().0, expected_code).map_err(ctx),
        (Err(diags), _) => return Err(ctx(format!("check failed: {:?}", codes(&diags.finish().0)))),
        (Ok(_), Stage::Check) => return Err(ctx("type-checked, expected a type error".into())),
        (Ok(_), _) => {}
    }
    if reached < Stage::Eval {
        return Ok(());
    }

    // Evaluate through the public API.
    let schema = Schema::<Facts>::builder().build().map_err(|e| ctx(format!("{e:?}")))?;
    let rule = match Rule::<Dynamic, Facts>::compile(&case.expr, &schema) {
        Err(e) => return first_code_is(e.diagnostics(), expected_code).map_err(ctx),
        Ok(rule) => rule,
    };
    let facts = sample();
    let ctx_ = Ctx::new(&facts).with_now(sample_now());
    let fast = rule.eval(&ctx_);
    let spec = rule.explain(&ctx_).into_result();
    let outcome = |r: &Result<Dynamic, dictum::EvalError>| match r {
        Ok(v) => Expect::Value(v.clone()),
        Err(e) => Expect::Error(e.code().as_str().to_owned()),
    };
    let (fast, spec) = (outcome(&fast), outcome(&spec));
    if fast != spec {
        return Err(ctx(format!("eval and explain disagree: eval {fast:?}, explain {spec:?}")));
    }
    if fast != case.expect {
        return Err(ctx(format!("got {fast:?}")));
    }
    Ok(())
}

fn codes(diags: &[dictum::Diagnostic]) -> Vec<&'static str> {
    diags.iter().map(|d| d.code().as_str()).collect()
}

fn first_code_is(diags: &[dictum::Diagnostic], expected: Option<&str>) -> Result<(), String> {
    let first: Option<ErrorCode> = diags.first().map(|d| d.code());
    match (first.map(ErrorCode::as_str), expected) {
        (Some(got), Some(want)) if got == want => Ok(()),
        (got, want) => Err(format!("first diagnostic {got:?}, expected {want:?}; all: {:?}", codes(diags))),
    }
}
