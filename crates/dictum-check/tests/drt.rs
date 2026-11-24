//! Differential test: the checker against the Lean type checker (`Dictum.infer` in
//! `formal/lean/Dictum/Check.lean`, run as `dictum-oracle`).
//!
//! `infer_iff` proves that `infer` accepts exactly the rules `HasType` types, and
//! `soundness` is stated over `HasType`. Agreeing with `infer` on generated core
//! rules, well typed or not, is what makes those theorems about this checker.
//!
//! The comparison waits for M2; one trial runs before that and checks that the
//! generated rules are a mix of well-typed and ill-typed ones.
#![expect(
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    reason = "helpers outside #[test] functions: a failure there fails the test"
)]

use std::cell::RefCell;
use std::path::PathBuf;

use dictum_check::ir::{Conversion, Expr, ExprKind};
use dictum_check::{Env, Type};
use dictum_testkit::Facts;
use dictum_testkit::generate::{core_types, core_untyped};
use dictum_testkit::oracle::{self, CoreTy, Oracle};
use libtest_mimic::{Arguments, Failed, Trial};
use proptest::strategy::{Strategy, ValueTree};
use proptest::test_runner::{Config, TestCaseError, TestRunner};

fn main() {
    let args = Arguments::from_args();
    let oracle = Oracle::locate();
    let compared = oracle.clone();
    let generated = oracle.clone();
    let trials = vec![
        Trial::test("checker_agrees_with_lean", move || agree(compared?)).with_ignored_flag(oracle.is_err()),
        Trial::test("generated_rules_are_a_mix_of_typed_and_ill_typed", move || mixed(generated?))
            .with_ignored_flag(oracle.is_err()),
    ];
    libtest_mimic::run(&args, trials).exit();
}

fn unerased(e: &Expr) -> &Type {
    match e.kind() {
        ExprKind::Convert { op: Conversion::Erase, arg } => arg.ty(),
        _ => e.ty(),
    }
}

fn agree(path: PathBuf) -> Result<(), Failed> {
    let oracle = RefCell::new(Oracle::spawn(&path));
    let env = Env::<()>::new(<Facts as dictum::Dictum>::__desc(), Vec::new()).map_err(|e| format!("{e:?}"))?;
    let mut runner = TestRunner::new(Config { cases: oracle::cases(), failure_persistence: None, ..Config::default() });
    runner
        .run(&core_untyped(4), |(src, core)| {
            let ast = dictum_syntax::parse(&src).map_err(|_| TestCaseError::fail(format!("{src}: does not parse")))?;
            let rust = match dictum_check::check(&ast, &env, &Type::Any) {
                // Asked for `any`, the body is erased to it; compare the type before that.
                Ok(p) => match unerased(p.body()) {
                    Type::Int => Some(CoreTy::Int),
                    Type::Bool => Some(CoreTy::Bool),
                    other => return Err(TestCaseError::fail(format!("{src}: unexpected type {other}"))),
                },
                Err(_) => None,
            };
            let lean = oracle.borrow_mut().check(&core_types(), &core);
            if rust == lean {
                Ok(())
            } else {
                Err(TestCaseError::fail(format!("{src}\n  core: {core}\n  rust: {rust:?}\n  lean: {lean:?}")))
            }
        })
        .map_err(|e| e.to_string().into())
}

/// Between a fifth and four fifths of the generated rules are well typed, so both
/// acceptance and rejection are compared.
fn mixed(path: PathBuf) -> Result<(), Failed> {
    let mut oracle = Oracle::spawn(&path);
    let mut runner = TestRunner::deterministic();
    let strategy = core_untyped(4);
    let total = 2000;
    let typed = (0..total)
        .filter(|_| {
            let (_, core) = strategy.new_tree(&mut runner).expect("a generated rule").current();
            oracle.check(&core_types(), &core).is_some()
        })
        .count();
    if typed * 5 >= total && typed * 5 <= total * 4 {
        Ok(())
    } else {
        Err(format!("{typed} of {total} generated rules are well typed").into())
    }
}
