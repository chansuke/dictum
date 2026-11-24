//! End-to-end behavior of the public API that a conformance case cannot express:
//! contexts, rule sets, result types, explain reports, limits and threads.
#![expect(clippy::unwrap_used, reason = "helpers outside #[test] functions: a failure there fails the test")]

use std::sync::Arc;

use dictum::{Ctx, Dynamic, ErrorCode, EvalErrorKind, Limits, NodeStatus, Rule, RuleSet, Schema};
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
fn json_conversions_are_charged_to_the_budget() {
    let facts = sample();
    let run = |src: &str, limits: Limits| {
        let rule = Rule::<i64, Facts>::compile(src, &schema()).unwrap();
        let ctx = Ctx::new(&facts).with_limits(limits);
        let (fast, spec) =
            (rule.eval(&ctx).map_err(|e| e.code()), rule.explain(&ctx).into_result().map_err(|e| e.code()));
        assert_eq!(fast, spec, "{src}");
        fast
    };
    let min = |src: &str, limits: fn(u64) -> Limits| (0..1000).find(|&n| run(src, limits(n)).is_ok()).unwrap();
    let steps = |n| Limits::default().with_max_steps(n);
    let alloc = |n| Limits::default().with_max_alloc(n);
    // toJSON: one step per value (the array and its 8 ints), one alloc per byte of text.
    // fromJSON: one more step per value read, and one alloc per element built.
    let encode = "len(toJSON(nums))";
    let decode = "len(fromJSON(toJSON(nums)))";
    assert_eq!(run(encode, Limits::default()), Ok(42));
    assert_eq!(min(encode, steps), 9);
    assert_eq!(min(encode, alloc), 42);
    assert_eq!(min(decode, steps), min(encode, steps) + 9);
    assert_eq!(min(decode, alloc), min(encode, alloc) + 8);
    assert_eq!(run(encode, alloc(min(encode, alloc) - 1)), Err(ErrorCode::AllocLimit));
    assert_eq!(run(encode, steps(min(encode, steps) - 1)), Err(ErrorCode::StepLimit));
    assert_eq!(run(decode, steps(min(decode, steps) - 1)), Err(ErrorCode::StepLimit));
    assert_eq!(run(decode, alloc(min(decode, alloc) - 1)), Err(ErrorCode::AllocLimit));
}

#[test]
#[cfg_attr(target_os = "wasi", ignore = "wasm32-wasip1 has no threads")]
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

#[test]
fn constant_subtrees_are_folded_at_compile_time() {
    // `count(1..100, ...)` is constant, so `eval` does not iterate it; `explain` runs
    // the rule as written and does.
    let rule = Rule::<bool, Facts>::compile("count(1..100, # % 2 == 0) == 50 && user.Age > 0", &schema()).unwrap();
    let facts = sample();
    let tight = Ctx::new(&facts).with_limits(Limits::default().with_max_steps(10));
    assert!(rule.eval(&tight).unwrap());
    assert!(matches!(rule.explain(&tight).result().as_ref().unwrap_err().kind(), EvalErrorKind::StepLimit));
}

#[test]
fn constants_too_large_to_fold_run_under_the_callers_limits() {
    let rule = Rule::<i64, Facts>::compile("count(1..20000, # > 0)", &schema()).unwrap();
    let facts = sample();
    assert_eq!(rule.eval(&Ctx::new(&facts)).unwrap(), 20000);
    let tight = Ctx::new(&facts).with_limits(Limits::default().with_max_steps(100));
    assert!(matches!(rule.eval(&tight).unwrap_err().kind(), EvalErrorKind::StepLimit));
}

#[test]
fn a_folded_pattern_is_compiled_once_or_fails_at_run_time() {
    let facts = sample();
    let ctx = Ctx::new(&facts);
    let ok = Rule::<bool, Facts>::compile(r#"user.Name matches ("^A" + "d")"#, &schema()).unwrap();
    assert!(ok.eval(&ctx).unwrap());
    // Not a literal, so not rejected at compile time (E0211); invalid once folded.
    let bad = Rule::<bool, Facts>::compile(r#"user.Name matches ("(" + "")"#, &schema()).unwrap();
    let (fast, spec) = (bad.eval(&ctx).unwrap_err(), bad.explain(&ctx).into_result().unwrap_err());
    assert!(matches!(fast.kind(), EvalErrorKind::InvalidArgument { .. }), "{fast}");
    assert_eq!((fast.code(), fast.span()), (spec.code(), spec.span()));
}

#[test]
fn only_pure_functions_are_called_at_compile_time() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static PURE: AtomicUsize = AtomicUsize::new(0);
    static VOLATILE: AtomicUsize = AtomicUsize::new(0);
    let schema = Schema::<Facts>::builder()
        .pure_func("pure", |x: i64| {
            PURE.fetch_add(1, Ordering::SeqCst);
            x
        })
        .func("volatile", |x: i64| {
            VOLATILE.fetch_add(1, Ordering::SeqCst);
            x
        })
        .build()
        .unwrap();
    let rule = Rule::<i64, Facts>::compile("pure(1) + volatile(2)", &schema).unwrap();
    assert_eq!((PURE.load(Ordering::SeqCst), VOLATILE.load(Ordering::SeqCst)), (1, 0));
    assert_eq!(rule.eval(&Ctx::new(&sample())).unwrap(), 3);
    assert_eq!((PURE.load(Ordering::SeqCst), VOLATILE.load(Ordering::SeqCst)), (1, 1));
}

#[test]
fn a_rule_set_computes_a_shared_subexpression_once_per_eval() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    let schema = Schema::<Facts>::builder()
        .pure_func("lookup", |x: i64| {
            CALLS.fetch_add(1, Ordering::SeqCst);
            x
        })
        .build()
        .unwrap();
    let mut b = RuleSet::<bool, Facts>::builder(&schema);
    // `+` is commutative for sharing, so the two calls are the same subexpression.
    b.insert("a", "lookup(user.Age + limit) > 40").unwrap();
    b.insert("b", "lookup(limit + user.Age) < 50").unwrap();
    let set = b.build();
    let facts = sample();
    let outcomes = set.eval(&Ctx::new(&facts)).unwrap();
    assert_eq!(outcomes.matched().collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    set.eval(&Ctx::new(&facts)).unwrap();
    assert_eq!(CALLS.load(Ordering::SeqCst), 2, "the cache lives for one eval");
}

#[test]
fn rule_set_explains_one_rule_by_name() {
    let schema = schema();
    let mut b = RuleSet::<bool, Facts>::builder(&schema);
    b.insert("adult", "user.Age >= 18").unwrap();
    let set = b.build();
    let facts = sample();
    let report = set.explain("adult", &Ctx::new(&facts)).unwrap();
    assert!(*report.result().as_ref().unwrap());
    assert!(set.explain("missing", &Ctx::new(&facts)).is_none());
}

#[test]
fn a_value_shared_many_times_is_walked_once_per_reference_budget() {
    // Each step doubles the references to one value; walking it per reference would
    // take 2^40 visits when the folder makes it a constant or explain sizes it.
    let src = r#"len(reduce(1..40, [#acc, #acc], fromJSON("0")))"#;
    let rule = Rule::<i64, Facts>::compile(src, &schema()).unwrap();
    let facts = sample();
    let ctx = Ctx::new(&facts);
    assert_eq!(rule.eval(&ctx).unwrap(), 2);
    assert_eq!(rule.explain(&ctx).into_result().unwrap(), 2);
}

#[test]
fn copying_out_a_shared_value_is_charged_per_reference() {
    // The `Dynamic` result is a tree: 2^40 elements, so the copy must hit the limit.
    let src = r#"reduce(1..40, [#acc, #acc], fromJSON("0"))"#;
    let rule = Rule::<Dynamic, Facts>::compile(src, &schema()).unwrap();
    let facts = sample();
    let err = rule.eval(&Ctx::new(&facts)).unwrap_err();
    assert!(matches!(err.kind(), EvalErrorKind::AllocLimit), "{err:?}");
}
