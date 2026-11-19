//! Property tests over generated well-typed rules (`dictum_testkit::generate`).
//!
//! - equivalence: the closure tree (`eval`) agrees with the spec interpreter
//!   (`explain`), which runs on the unfolded IR, so folding and closure
//!   specialization are both under test;
//! - sharing: a `RuleSet` gives what its rules give standalone;
//! - robustness: no input makes compilation panic.
//!
//! Each test names the milestone that makes it meaningful. Until then it is ignored,
//! never weakened: the assertions are the specification. Case counts come from
//! `PROPTEST_CASES` (default 256).
// Test code: the workspace's no-panic lints are for evaluation code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use dictum::{Ctx, Dynamic, EvalError, EvalErrorKind, Limits, Rule, RuleSet, Schema};
use dictum_testkit::generate::{self, Ty};
use dictum_testkit::{Facts, sample, sample_now};
use proptest::prelude::*;

fn schema() -> Schema<Facts> {
    Schema::<Facts>::builder().build().unwrap()
}

/// Comparable outcome: a value, or an error code with its span. Budget errors are
/// `None`: a `RuleSet` charges shared work to the first rule that computes it, so
/// only non-budget outcomes must match exactly.
fn outcome(r: Result<Dynamic, EvalError>) -> Option<Result<Dynamic, (&'static str, dictum::Span)>> {
    match r {
        Ok(v) => Some(Ok(v)),
        Err(e) if matches!(e.kind(), EvalErrorKind::StepLimit | EvalErrorKind::AllocLimit) => None,
        Err(e) => Some(Err((e.code().as_str(), e.span()))),
    }
}

proptest! {
    /// M4 completion criterion: 100k cases, run by the scheduled workflow
    /// (`PROPTEST_CASES=100000` in .github/workflows/deep.yml). PRs run the
    /// default 256.
    #[test]
    #[ignore = "pending M4: closure compiler"]
    fn eval_agrees_with_explain((ty, src) in generate::any_rule(4)) {
        let schema = schema();
        let rule = Rule::<Dynamic, Facts>::compile(&src, &schema)
            .unwrap_or_else(|e| panic!("generated rule must compile ({}): {src}\n{e}", ty.name()));
        let facts = sample();
        let ctx = Ctx::new(&facts).with_now(sample_now());
        let fast = outcome(rule.eval(&ctx));
        let spec = outcome(rule.explain(&ctx).into_result());
        if let (Some(fast), Some(spec)) = (fast, spec) {
            let same = match (&fast, &spec) {
                (Ok(a), Ok(b)) => dictum_testkit::same_value(a, b),
                _ => fast == spec,
            };
            prop_assert!(same, "{}: eval {:?}, explain {:?}", src, fast, spec);
        }
    }

    /// The generator only produces well-typed rules; a type error means the generator
    /// or the checker is wrong.
    #[test]
    fn generated_rules_type_check((ty, src) in generate::any_rule(4)) {
        let ast = dictum_syntax::parse(&src).map_err(|d| TestCaseError::fail(format!("{src}: {:?}", d.finish().0)))?;
        let env = dictum_check::Env::<()>::new(<Facts as dictum::Dictum>::__desc(), Vec::new()).unwrap();
        let expected = match ty {
            generate::Ty::Bool => dictum_check::Type::Bool,
            generate::Ty::Int => dictum_check::Type::Int,
            generate::Ty::Float => dictum_check::Type::Float,
            generate::Ty::Str => dictum_check::Type::String,
        };
        // Checked against its own type, the body needs no conversion.
        let program = dictum_check::check(&ast, &env, &expected)
            .map_err(|d| TestCaseError::fail(format!("{src}: {:?}", d.finish().0)))?;
        prop_assert_eq!(program.body().ty(), &expected, "{}", src);
        prop_assert!(program.validate(&env).is_ok(), "{}: {:?}", src, program.validate(&env));
    }

    /// M5: budgets aside, sharing changes nothing. Each rule's *meaning* is its
    /// standalone result with no budget. On success every value equals the
    /// meaning; on a runtime error the error is that of the first rule whose
    /// meaning is an error, with that rule's own span; and if every rule succeeds
    /// alone within the budget, the set succeeds.
    ///
    /// Comparing with the budgeted standalone result instead is wrong: a rule that
    /// runs out of budget alone can finish in the set (another rule paid for
    /// shared work) and then report a runtime error it never reached alone. The
    /// TLA+ model formal/tla/RuleSetEval.tla found this case.
    #[test]
    #[ignore = "pending M5: RuleSet"]
    fn rule_set_agrees_with_standalone_rules(srcs in proptest::collection::vec(generate::rule(Ty::Bool, 3), 1..6)) {
        let schema = schema();
        let facts = sample();
        let ctx = Ctx::new(&facts).with_now(sample_now());
        let unbounded = ctx.with_limits(Limits::default().with_max_steps(u64::MAX).with_max_alloc(u64::MAX));
        let mut builder = RuleSet::<bool, Facts>::builder(&schema);
        let (mut meaning, mut alone) = (Vec::new(), Vec::new());
        for (i, src) in srcs.iter().enumerate() {
            builder.insert(&format!("r{i}"), src).unwrap();
            let rule = Rule::<bool, Facts>::compile(src, &schema).unwrap();
            meaning.push(rule.eval(&unbounded));
            alone.push(rule.eval(&ctx));
        }
        let set = builder.build();
        let budget = |e: &EvalError| matches!(e.kind(), EvalErrorKind::StepLimit | EvalErrorKind::AllocLimit);
        match set.eval(&ctx) {
            Ok(outcomes) => {
                for ((name, got), want) in outcomes.iter().zip(&meaning) {
                    prop_assert_eq!(Ok(*got), want.as_ref().map(|v| *v).map_err(|e| e.code()), "{}", name);
                }
            }
            Err(err) if budget(&err) => {}
            Err(err) => {
                prop_assert!(alone.iter().any(|r| r.is_err()), "every rule succeeds alone but the set fails: {err}");
                let i = meaning.iter().position(|r| r.is_err()).expect("set failed but every rule means a value");
                let want = meaning[i].as_ref().unwrap_err();
                let name = format!("r{i}");
                prop_assert_eq!(err.rule(), Some(name.as_str()));
                prop_assert_eq!((err.code(), err.span()), (want.code(), want.span()));
            }
        }
    }

    #[test]
    #[ignore = "pending M4: closure compiler (compiling a valid rule folds and builds closures)"]
    fn compile_never_panics_on_token_soup(src in generate::token_soup()) {
        let _ = Rule::<Dynamic, Facts>::compile(&src, &schema());
    }

    #[test]
    #[ignore = "pending M4: closure compiler (compiling a valid rule folds and builds closures)"]
    fn compile_never_panics_on_arbitrary_text(src in "\\PC{0,64}") {
        let _ = Rule::<Dynamic, Facts>::compile(&src, &schema());
    }
}

/// CSE matches `==` with swapped operands but must not match `&&`: `false && x`
/// succeeds where `x && false` fails. (Proved in formal/lean, Dictum/Commute.lean.)
#[test]
#[ignore = "pending M5: RuleSet"]
fn short_circuit_operands_are_not_shared_across_orders() {
    let schema = schema();
    let facts = sample();
    let ctx = Ctx::new(&facts).with_now(sample_now());
    let mut b = RuleSet::<bool, Facts>::builder(&schema);
    b.insert("safe", "false && 7 % 0 > 0").unwrap();
    b.insert("unsafe", "7 % 0 > 0 && false").unwrap();
    let err = b.build().eval(&ctx).err().expect("the second rule divides by zero");
    assert_eq!(err.rule(), Some("unsafe"));
    assert_eq!(err.code().as_str(), "E0302");
}
