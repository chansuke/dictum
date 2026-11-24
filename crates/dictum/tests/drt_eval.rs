//! Differential test: evaluation against the Lean definition (`Dictum.eval` in
//! `formal/lean/Dictum/Core.lean`, run as `dictum-oracle`).
//!
//! The Lean proofs (type soundness, folding, the simplifications and skipped
//! overflow checks in `Dictum.Opt`) are about `Dictum.eval`. This test is what ties
//! them to the engine: generated well-typed rules of the core fragment are
//! evaluated by `Rule::eval` and by the oracle on the same facts, and must give the
//! same value or the same error.
//!
//! The comparison waits for M3; one trial runs before that and checks the generator:
//! the oracle never answers `internal` (every rule is well typed), and the rules
//! produce values and both runtime errors.
#![expect(
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    reason = "helpers outside #[test] functions: a failure there fails the test"
)]

use std::cell::RefCell;
use std::path::PathBuf;

use dictum::{Ctx, Dynamic, EvalErrorKind, Rule, Schema};
use dictum_testkit::generate::{Ty, core_env, core_expr};
use dictum_testkit::oracle::{self, CoreResult, CoreVal, Oracle};
use dictum_testkit::{Facts, sample};
use libtest_mimic::{Arguments, Failed, Trial};
use proptest::strategy::{BoxedStrategy, Strategy, ValueTree};
use proptest::test_runner::{Config, TestCaseError, TestRunner};

fn main() {
    let args = Arguments::from_args();
    let oracle = Oracle::locate();
    let compared = oracle.clone();
    let generated = oracle.clone();
    let trials = vec![
        Trial::test("eval_agrees_with_lean", move || agree(compared?)).with_ignored_flag(oracle.is_err()),
        Trial::test("core_rules_are_well_typed_and_varied", move || varied(generated?))
            .with_ignored_flag(oracle.is_err()),
    ];
    libtest_mimic::run(&args, trials).exit();
}

fn rules() -> BoxedStrategy<(Ty, (String, String))> {
    proptest::sample::select(&[Ty::Int, Ty::Bool][..])
        .prop_flat_map(|t| core_expr(t, 4).prop_map(move |r| (t, r)))
        .boxed()
}

fn agree(path: PathBuf) -> Result<(), Failed> {
    let oracle = RefCell::new(Oracle::spawn(&path));
    let schema = Schema::<Facts>::builder().build().map_err(|e| format!("{e:?}"))?;
    let facts = sample();
    let env = core_env(&facts);
    let mut runner = TestRunner::new(Config { cases: oracle::cases(), failure_persistence: None, ..Config::default() });
    runner
        .run(&rules(), |(_, (src, core))| {
            let rule = Rule::<Dynamic, Facts>::compile(&src, &schema)
                .map_err(|e| TestCaseError::fail(format!("{src}: does not compile: {e}")))?;
            let rust = match rule.eval(&Ctx::new(&facts)) {
                Ok(Dynamic::Int(n)) => CoreResult::Ok(CoreVal::Int(n)),
                Ok(Dynamic::Bool(b)) => CoreResult::Ok(CoreVal::Bool(b)),
                Ok(other) => return Err(TestCaseError::fail(format!("{src}: unexpected value {other:?}"))),
                Err(e) => match e.kind() {
                    EvalErrorKind::Overflow => CoreResult::Overflow,
                    EvalErrorKind::DivisionByZero => CoreResult::DivisionByZero,
                    other => return Err(TestCaseError::fail(format!("{src}: unexpected error {other:?}"))),
                },
            };
            let lean = oracle.borrow_mut().eval(&env, &core);
            if rust == lean {
                Ok(())
            } else {
                Err(TestCaseError::fail(format!("{src}\n  core: {core}\n  rust: {rust:?}\n  lean: {lean:?}")))
            }
        })
        .map_err(|e| e.to_string().into())
}

fn varied(path: PathBuf) -> Result<(), Failed> {
    let mut oracle = Oracle::spawn(&path);
    let env = core_env(&sample());
    let mut runner = TestRunner::deterministic();
    let strategy = rules();
    let (mut values, mut overflows, mut zeros) = (0, 0, 0);
    for _ in 0..2000 {
        let (_, (src, core)) = strategy.new_tree(&mut runner).expect("a generated rule").current();
        match oracle.eval(&env, &core) {
            CoreResult::Ok(_) => values += 1,
            CoreResult::Overflow => overflows += 1,
            CoreResult::DivisionByZero => zeros += 1,
            CoreResult::Internal => return Err(format!("not well typed: {src}\n  core: {core}").into()),
        }
    }
    if values > 1000 && overflows > 20 && zeros > 20 {
        Ok(())
    } else {
        Err(format!("values {values}, overflows {overflows}, divisions by zero {zeros}").into())
    }
}
