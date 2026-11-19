//! End-to-end behavior of the public API that a conformance case cannot express:
//! contexts, rule sets, result types, explain reports, limits and threads.
// Test code: the workspace's no-panic lints are for evaluation code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::sync::Arc;

use dictum::{Ctx, Dynamic, EvalErrorKind, Limits, NodeStatus, Rule, RuleSet, Schema};
use dictum_testkit::{Facts, sample, sample_now};

fn schema() -> Schema<Facts> {
    Schema::<Facts>::builder().build().unwrap()
}

#[test]
fn now_without_a_time_is_clock_unset() {
    let rule = Rule::<bool, Facts>::compile("now() > user.Created", &schema()).unwrap();
    let facts = sample();
    let err = rule.eval(&Ctx::new(&facts)).unwrap_err();
    assert!(matches!(err.kind(), EvalErrorKind::ClockUnset));
    assert_eq!(err.code().as_str(), "E0311");
    assert!(rule.eval(&Ctx::new(&facts).with_now(sample_now())).unwrap());
}

#[test]
fn rules_that_do_not_call_now_need_no_time() {
    let rule = Rule::<bool, Facts>::compile("user.Age > 18", &schema()).unwrap();
    assert!(rule.eval(&Ctx::new(&sample())).unwrap());
}

#[test]
fn result_type_is_checked_at_compile_time() {
    let err = Rule::<bool, Facts>::compile("user.Age", &schema()).err().unwrap();
    assert_eq!(err.diagnostics()[0].code().as_str(), "E0209");
}

#[test]
fn int_result_is_promoted_to_float() {
    assert!(Rule::<f64, Facts>::compile("user.Age", &schema()).is_ok());
}

#[test]
fn compile_errors_are_collected_and_capped() {
    let src = (0..30).map(|i| format!("x{i}")).collect::<Vec<_>>().join(" + ");
    let err = Rule::<Dynamic, Facts>::compile(&src, &schema()).err().unwrap();
    assert_eq!(err.diagnostics().len(), dictum_syntax::MAX_DIAGNOSTICS);
    let starts: Vec<u32> = err.diagnostics().iter().map(|d| d.span().start).collect();
    assert!(starts.windows(2).all(|w| w[0] <= w[1]), "sorted by position");
}

#[test]
#[ignore = "pending M5: RuleSet"]
fn rule_set_stops_at_the_first_failing_rule() {
    let schema = schema();
    let mut b = RuleSet::<bool, Facts>::builder(&schema);
    b.insert("adult", "user.Age >= 18").unwrap();
    b.insert("broken", "7 % 0 > 0").unwrap();
    b.insert("staff", r#""admin" in user.Tags"#).unwrap();
    let set = b.build();
    let facts = sample();
    let err = set.eval(&Ctx::new(&facts)).err().unwrap();
    assert_eq!(err.rule(), Some("broken"));
    assert_eq!(err.code().as_str(), "E0302");
}

#[test]
#[ignore = "pending M5: RuleSet"]
fn rule_set_reports_matches_in_insertion_order() {
    let schema = schema();
    let mut b = RuleSet::<bool, Facts>::builder(&schema);
    b.insert("staff", r#""admin" in user.Tags"#).unwrap();
    b.insert("minor", "user.Age < 18").unwrap();
    b.insert("adult", "user.Age >= 18").unwrap();
    let set = b.build();
    let facts = sample();
    let outcomes = set.eval(&Ctx::new(&facts)).unwrap();
    assert_eq!(outcomes.matched().collect::<Vec<_>>(), ["staff", "adult"]);
    assert_eq!(set.names().collect::<Vec<_>>(), ["staff", "minor", "adult"]);
}

#[test]
fn duplicate_rule_names_are_rejected() {
    let schema = schema();
    let mut b = RuleSet::<bool, Facts>::builder(&schema);
    b.insert("a", "true").unwrap();
    let err = b.insert("a", "false").unwrap_err();
    assert_eq!(err.diagnostics()[0].code().as_str(), "E0001");
    assert_eq!(err.rule(), Some("a"));
}

#[test]
fn explain_marks_skipped_and_failed_nodes() {
    let rule = Rule::<bool, Facts>::compile("user.Age < 18 && 7 % 0 > 0", &schema()).unwrap();
    let report = rule.explain(&Ctx::new(&sample()));
    assert!(!*report.result().as_ref().unwrap());
    let children = report.root().children();
    assert_eq!(children.len(), 2);
    assert!(matches!(children[1].status(), NodeStatus::Skipped));

    let failing = Rule::<bool, Facts>::compile("user.Age > 18 && 7 % 0 > 0", &schema()).unwrap();
    let report = failing.explain(&Ctx::new(&sample()));
    assert!(report.result().is_err());
    assert!(matches!(report.root().children()[1].status(), NodeStatus::Failed(_)));
}

#[test]
fn step_limit_is_enforced() {
    let rule = Rule::<i64, Facts>::compile("count(nums, # > 0)", &schema()).unwrap();
    let facts = sample();
    let tight = Ctx::new(&facts).with_limits(Limits::default().with_max_steps(3));
    assert!(matches!(rule.eval(&tight).unwrap_err().kind(), EvalErrorKind::StepLimit));
    assert_eq!(rule.eval(&Ctx::new(&facts)).unwrap(), 8);
}

#[test]
fn compiled_rules_are_shared_across_threads() {
    let rule = Arc::new(Rule::<bool, Facts>::compile("user.Age >= 18", &schema()).unwrap());
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let rule = Arc::clone(&rule);
            std::thread::spawn(move || rule.eval(&Ctx::new(&sample())).unwrap())
        })
        .collect();
    assert!(handles.into_iter().all(|h| h.join().unwrap()));
}

#[test]
fn schema_functions_pure_and_volatile() {
    let schema = Schema::<Facts>::builder()
        .pure_func("double", |x: i64| x * 2)
        .func("fail", |_: i64| -> Result<i64, std::fmt::Error> { Err(std::fmt::Error) })
        .build()
        .unwrap();
    let ok = Rule::<i64, Facts>::compile("double(user.Age)", &schema).unwrap();
    assert_eq!(ok.eval(&Ctx::new(&sample())).unwrap(), 72);
    let err = Rule::<i64, Facts>::compile("fail(1)", &schema).unwrap().eval(&Ctx::new(&sample())).unwrap_err();
    assert!(matches!(err.kind(), EvalErrorKind::Function { .. }));
    assert_eq!(err.code().as_str(), "E0308");
}

#[test]
fn explain_elides_large_values_without_changing_the_result() {
    let rule = Rule::<i64, Facts>::compile("count(1..20000, # > 0)", &schema()).unwrap();
    let report = rule.explain(&Ctx::new(&sample()));
    assert_eq!(*report.result().as_ref().unwrap(), 20000);
    let range = &report.root().children()[0];
    assert!(matches!(range.status(), NodeStatus::Elided), "{:?}", range.status());
}
