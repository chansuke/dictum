//! End-to-end behavior of the public API that a conformance case cannot express:
//! contexts, rule sets, result types, explain reports, limits and threads.
#![expect(clippy::unwrap_used, reason = "helpers outside #[test] functions: a failure there fails the test")]

use std::sync::Arc;

use dictum::{Ctx, DynMap, Dynamic, ErrorCode, EvalErrorKind, Limits, NodeStatus, Rule, RuleSet, Schema};
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
fn post_optimize_constants_are_refolded_at_compile_time() {
    // `level` is `u8`, so `level < 256` simplifies to `true` in `optimize`, making the
    // surrounding expression constant; post-optimization re-folding folds it before
    // compilation so `eval` spends zero runtime steps/allocs on it.
    let rule =
        Rule::<bool, Facts>::compile("count(1..(level < 256 ? 100 : 200), # % 2 == 0) == 50", &schema()).unwrap();
    let facts = sample();
    let tight = Ctx::new(&facts).with_limits(Limits::default().with_max_steps(10));
    assert!(rule.eval(&tight).unwrap());
    assert!(matches!(rule.explain(&tight).result().as_ref().unwrap_err().kind(), EvalErrorKind::StepLimit));

    let concat = Rule::<String, Facts>::compile(r#"(level < 256 ? "a" : "b") + "c""#, &schema()).unwrap();
    let zero_alloc = Ctx::new(&facts).with_limits(Limits::default().with_max_alloc(0));
    assert_eq!(concat.eval(&zero_alloc).unwrap(), "ac");
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

#[test]
fn to_json_precharges_alloc_incrementally_and_matches_exact_boundary() {
    let rule = Rule::<String, Facts>::compile("toJSON(user.Name)", &schema()).unwrap();
    let check = |facts: &Facts, max_alloc: u64| {
        let ctx = Ctx::new(facts).with_limits(Limits::default().with_max_alloc(max_alloc));
        let fast = rule.eval(&ctx).map_err(|e| e.code());
        let spec = rule.explain(&ctx).into_result().map_err(|e| e.code());
        assert_eq!(fast, spec);
        fast
    };

    // 6x HTML-escaped expansion and plain long strings fail on tight budgets.
    let mut escaped_huge = sample();
    escaped_huge.user.name = "<".repeat(20_000);
    assert_eq!(check(&escaped_huge, 10), Err(ErrorCode::AllocLimit));

    let mut plain_huge = sample();
    plain_huge.user.name = "a".repeat(20_000);
    assert_eq!(check(&plain_huge, 10), Err(ErrorCode::AllocLimit));

    // Exact boundary for HTML-escaped text: `"\"\\u003ca\\u0026b\\u003e\""` is 22 bytes.
    let mut escaped = sample();
    escaped.user.name = "<a&b>".into();
    assert_eq!(check(&escaped, 21), Err(ErrorCode::AllocLimit));
    assert_eq!(check(&escaped, 22), Ok("\"\\u003ca\\u0026b\\u003e\"".into()));

    // Exact boundary for base64-encoded bytes: 30 bytes -> 40 base64 chars + 2 quotes = 42 bytes.
    let bytes_rule = Rule::<String, Facts>::compile("toJSON(user.Attrs)", &schema()).unwrap();
    let mut with_bytes = sample();
    with_bytes.user.attrs = Dynamic::Bytes(vec![0u8; 30]);
    let ctx_fail = Ctx::new(&with_bytes).with_limits(Limits::default().with_max_alloc(41));
    let ctx_ok = Ctx::new(&with_bytes).with_limits(Limits::default().with_max_alloc(42));
    assert_eq!(bytes_rule.eval(&ctx_fail).unwrap_err().code(), ErrorCode::AllocLimit);
    assert_eq!(bytes_rule.explain(&ctx_fail).into_result().unwrap_err().code(), ErrorCode::AllocLimit);
    assert_eq!(bytes_rule.eval(&ctx_ok).unwrap().len(), 42);
    assert_eq!(bytes_rule.explain(&ctx_ok).into_result().unwrap().len(), 42);
}

#[test]
fn from_json_precharges_strings_and_object_entries() {
    let str_rule = Rule::<Dynamic, Facts>::compile("fromJSON(user.Name)", &schema()).unwrap();
    let check_dyn = |facts: &Facts, max_alloc: u64| {
        let ctx = Ctx::new(facts).with_limits(Limits::default().with_max_alloc(max_alloc));
        let fast = str_rule.eval(&ctx).map_err(|e| e.code());
        let spec = str_rule.explain(&ctx).into_result().map_err(|e| e.code());
        assert_eq!(fast, spec);
        fast
    };

    // Large raw and escaped JSON strings fail under tight budgets.
    let mut raw_huge = sample();
    raw_huge.user.name = format!("\"{}\"", "x".repeat(20_000));
    assert_eq!(check_dyn(&raw_huge, 10), Err(ErrorCode::AllocLimit));

    let mut esc_huge = sample();
    esc_huge.user.name = format!("\"{}\"", "\\u0041".repeat(5_000));
    assert_eq!(check_dyn(&esc_huge, 10), Err(ErrorCode::AllocLimit));

    // Exact boundary for mixed raw + escape + multi-byte UTF-8: `"a\nbé"` is 5 UTF-8 bytes.
    let mut mixed = sample();
    mixed.user.name = r#""a\nb\u00e9""#.into();
    assert_eq!(check_dyn(&mixed, 4), Err(ErrorCode::AllocLimit));
    assert_eq!(check_dyn(&mixed, 5), Ok(Dynamic::String("a\nbé".into())));

    // Object with duplicate key: keys "a", "a", "b" (3 bytes) + 2 unique map entries = 5 alloc.
    let len_rule = Rule::<i64, Facts>::compile("len(fromJSON(user.Name))", &schema()).unwrap();
    let mut obj = sample();
    obj.user.name = r#"{"a":1,"a":2,"b":3}"#.into();
    let ctx_fail = Ctx::new(&obj).with_limits(Limits::default().with_max_alloc(4));
    let ctx_ok = Ctx::new(&obj).with_limits(Limits::default().with_max_alloc(5));
    assert_eq!(len_rule.eval(&ctx_fail).unwrap_err().code(), ErrorCode::AllocLimit);
    assert_eq!(len_rule.explain(&ctx_fail).into_result().unwrap_err().code(), ErrorCode::AllocLimit);
    assert_eq!(len_rule.eval(&ctx_ok).unwrap(), 2);
    assert_eq!(len_rule.explain(&ctx_ok).into_result().unwrap(), 2);
}

#[test]
fn string_builtins_precharge_alloc_on_tight_budgets() {
    let b64_rule = Rule::<String, Facts>::compile("fromBase64(user.Name)", &schema()).unwrap();
    let mut huge_b64 = sample();
    huge_b64.user.name = "AAAA".repeat(5_000);
    let tight = Ctx::new(&huge_b64).with_limits(Limits::default().with_max_alloc(10));
    assert_eq!(b64_rule.eval(&tight).unwrap_err().code(), ErrorCode::AllocLimit);
    assert_eq!(b64_rule.explain(&tight).into_result().unwrap_err().code(), ErrorCode::AllocLimit);

    let mut small_b64 = sample();
    small_b64.user.name = "YWJjZA==".into();
    let fail = Ctx::new(&small_b64).with_limits(Limits::default().with_max_alloc(3));
    let ok = Ctx::new(&small_b64).with_limits(Limits::default().with_max_alloc(4));
    assert_eq!(b64_rule.eval(&fail).unwrap_err().code(), ErrorCode::AllocLimit);
    assert_eq!(b64_rule.eval(&ok).unwrap(), "abcd");

    let join_rule = Rule::<String, Facts>::compile("join(user.Tags, user.Name)", &schema()).unwrap();
    let mut huge_sep = sample();
    huge_sep.user.name = "-".repeat(10_000);
    let tight_join = Ctx::new(&huge_sep).with_limits(Limits::default().with_max_alloc(10));
    assert_eq!(join_rule.eval(&tight_join).unwrap_err().code(), ErrorCode::AllocLimit);
    assert_eq!(join_rule.explain(&tight_join).into_result().unwrap_err().code(), ErrorCode::AllocLimit);

    let upper_rule = Rule::<String, Facts>::compile("upper(user.Name)", &schema()).unwrap();
    let mut huge_str = sample();
    huge_str.user.name = "a".repeat(10_000);
    let tight_upper = Ctx::new(&huge_str).with_limits(Limits::default().with_max_alloc(10));
    assert_eq!(upper_rule.eval(&tight_upper).unwrap_err().code(), ErrorCode::AllocLimit);
    assert_eq!(upper_rule.explain(&tight_upper).into_result().unwrap_err().code(), ErrorCode::AllocLimit);
}

#[test]
fn folded_constant_containers_are_charged_when_copied_out() {
    // `c` folds at compile time into a `Seq::Const` of 1,000 ints; embedding it 70 times
    // into a dynamic array builds 71 outer elements and copies 71 + 70 * 1,000 = 70,071
    // elements out into `Dynamic` (70,142 total alloc for `eval`).
    let copies = vec!["c"; 70].join(", ");
    let src = format!("let c = map(1..1000, #); [user.Age, {copies}]");
    let rule = Rule::<Dynamic, Facts>::compile(&src, &schema()).unwrap();
    let facts = sample();
    let tight = Ctx::new(&facts).with_limits(Limits::default().with_max_alloc(70_141));
    let err = rule.eval(&tight).unwrap_err();
    assert!(matches!(err.kind(), EvalErrorKind::AllocLimit), "{err:?}");
    let spec_err = rule.explain(&tight).into_result().unwrap_err();
    assert!(matches!(spec_err.kind(), EvalErrorKind::AllocLimit), "{spec_err:?}");

    let exact = Ctx::new(&facts).with_limits(Limits::default().with_max_alloc(70_142));
    let out = rule.eval(&exact).unwrap();
    assert!(matches!(out, Dynamic::Array(xs) if xs.len() == 71));

    // Same for folded constant maps (`MapRef::Const`): 4 * 5 entries + 5 outer = 25 copied,
    // plus 5 for the outer array literal = 30 total alloc.
    let map_rule =
        Rule::<Dynamic, Facts>::compile("let m = {a: 1, b: 2, c: 3, d: 4, e: 5}; [user.Age, m, m, m, m]", &schema())
            .unwrap();
    let fail = Ctx::new(&facts).with_limits(Limits::default().with_max_alloc(29));
    let ok = Ctx::new(&facts).with_limits(Limits::default().with_max_alloc(30));
    assert_eq!(map_rule.eval(&fail).unwrap_err().code(), ErrorCode::AllocLimit);
    assert!(map_rule.eval(&ok).is_ok());
}

#[test]
fn to_json_enforces_depth_limit_on_deeply_nested_dynamic() {
    let rule = Rule::<String, Facts>::compile("toJSON(user.Attrs)", &schema()).unwrap();
    let nest_array = |levels: usize| {
        let mut v = Dynamic::Int(0);
        for _ in 0..levels {
            v = Dynamic::Array(vec![v]);
        }
        v
    };
    let nest_map = |levels: usize| {
        let mut v = Dynamic::Int(0);
        for _ in 0..levels {
            let mut m = DynMap::new();
            m.insert("a", v);
            v = Dynamic::Map(m);
        }
        v
    };

    // Depth 256 (MAX_VALUE_DEPTH) succeeds with enough allocation budget for indentation.
    let mut at_limit = sample();
    at_limit.user.attrs = nest_array(256);
    let big = Limits::default().with_max_alloc(1 << 20);
    assert!(rule.eval(&Ctx::new(&at_limit).with_limits(big)).is_ok());
    assert!(rule.explain(&Ctx::new(&at_limit).with_limits(big)).into_result().is_ok());

    // Depth 257 and 500 fail cleanly with DepthLimit (E0312) for both arrays and maps.
    for value in [nest_array(257), nest_array(500), nest_map(257), nest_map(500)] {
        let mut facts = sample();
        facts.user.attrs = value;
        let ctx = Ctx::new(&facts).with_limits(big);
        let fast = rule.eval(&ctx).unwrap_err();
        assert!(matches!(fast.kind(), EvalErrorKind::DepthLimit), "{fast:?}");
        assert_eq!(fast.code(), ErrorCode::DepthLimit);
        assert_eq!(fast.code().as_str(), "E0312");
        let spec = rule.explain(&ctx).into_result().unwrap_err();
        assert_eq!(spec.code(), ErrorCode::DepthLimit);
    }
}

#[test]
fn schema_functions_arities_and_extended_types() {
    use dictum::jiff::{SignedDuration, Timestamp};

    let schema = Schema::<Facts>::builder()
        .pure_func("const42", || 42i64)
        .pure_func("inc", |a: i64| a + 1)
        .pure_func("add2", |a: i64, b: i64| a + b)
        .pure_func("sum3", |a: i64, b: i64, c: i64| a + b + c)
        .pure_func("sum4", |a: i64, b: i64, c: i64, d: i64| a + b + c + d)
        .pure_func("shift", |t: Timestamp, d: SignedDuration| t + d)
        .pure_func("halfDur", |d: SignedDuration| SignedDuration::from_secs(d.as_secs() / 2))
        .pure_func("greet", |name: Option<&str>| name.unwrap_or("anon").to_owned())
        .pure_func("posOrNil", |x: i64| (x > 0).then_some(x))
        .pure_func("optDouble", |x: Option<i64>| x.map(|n| n * 2))
        .build()
        .unwrap();

    let facts = sample();
    let ctx = Ctx::new(&facts).with_now(sample_now());

    // Arities 0..=4 (both compiled fast path and explain interpreter).
    for (src, expected) in [
        ("const42()", 42),
        ("inc(user.Age)", 37),
        ("add2(user.Age, limit)", 46),
        ("sum3(user.Age, limit, level)", 246),
        ("sum4(user.Age, limit, level, delta)", 146),
    ] {
        let rule = Rule::<i64, Facts>::compile(src, &schema).unwrap();
        assert_eq!(rule.eval(&ctx).unwrap(), expected, "{src}");
        assert_eq!(rule.explain(&ctx).into_result().unwrap(), expected, "{src}");
    }

    // Timestamp & SignedDuration arguments and returns.
    let shift = Rule::<Timestamp, Facts>::compile(r#"shift(user.Created, duration("1h"))"#, &schema).unwrap();
    let want_ts = facts.user.created + SignedDuration::from_hours(1);
    assert_eq!(shift.eval(&ctx).unwrap(), want_ts);
    assert_eq!(shift.explain(&ctx).into_result().unwrap(), want_ts);

    let half = Rule::<SignedDuration, Facts>::compile(r#"halfDur(duration("10s"))"#, &schema).unwrap();
    assert_eq!(half.eval(&ctx).unwrap(), SignedDuration::from_secs(5));
    assert_eq!(half.explain(&ctx).into_result().unwrap(), SignedDuration::from_secs(5));

    // Option<&str> parameter: None from optional field and Some via promotion.
    let greet_none = Rule::<String, Facts>::compile("greet(user.Email)", &schema).unwrap();
    assert_eq!(greet_none.eval(&ctx).unwrap(), "anon");
    let greet_some = Rule::<String, Facts>::compile("greet(user.Name)", &schema).unwrap();
    assert_eq!(greet_some.eval(&ctx).unwrap(), "Ada");
    let mut with_email = sample();
    with_email.user.email = Some("ada@example.com".into());
    assert_eq!(greet_none.eval(&Ctx::new(&with_email)).unwrap(), "ada@example.com");

    // Option<T> return & chaining into Option<T> parameter.
    let pos = Rule::<Option<i64>, Facts>::compile("posOrNil(delta)", &schema).unwrap();
    assert_eq!(pos.eval(&ctx).unwrap(), None);
    let pos_ok = Rule::<Option<i64>, Facts>::compile("posOrNil(user.Age)", &schema).unwrap();
    assert_eq!(pos_ok.eval(&ctx).unwrap(), Some(36));

    let chained_nil = Rule::<Option<i64>, Facts>::compile("optDouble(posOrNil(delta))", &schema).unwrap();
    assert_eq!(chained_nil.eval(&ctx).unwrap(), None);
    let chained_some = Rule::<Option<i64>, Facts>::compile("optDouble(posOrNil(user.Age))", &schema).unwrap();
    assert_eq!(chained_some.eval(&ctx).unwrap(), Some(72));
}

#[cfg(feature = "decimal")]
#[test]
fn schema_functions_support_decimal_and_optional_decimal() {
    use dictum::rust_decimal::Decimal;

    let schema = Schema::<Facts>::builder()
        .pure_func("tax", |amount: Decimal, rate: Option<Decimal>| rate.map(|r| amount * r).unwrap_or(Decimal::ZERO))
        .pure_func("maybeDec", |positive: bool| positive.then_some(Decimal::new(125, 2)))
        .build()
        .unwrap();

    let facts = sample();
    let ctx = Ctx::new(&facts);

    let with_rate = Rule::<Decimal, Facts>::compile(r#"tax(decimal("100"), decimal("0.1"))"#, &schema).unwrap();
    assert_eq!(with_rate.eval(&ctx).unwrap(), Decimal::new(100, 1));
    assert_eq!(with_rate.explain(&ctx).into_result().unwrap(), Decimal::new(100, 1));

    let no_rate = Rule::<Decimal, Facts>::compile(r#"tax(decimal("100"), nil)"#, &schema).unwrap();
    assert_eq!(no_rate.eval(&ctx).unwrap(), Decimal::ZERO);

    let some_dec = Rule::<Option<Decimal>, Facts>::compile("maybeDec(user.Active)", &schema).unwrap();
    assert_eq!(some_dec.eval(&ctx).unwrap(), Some(Decimal::new(125, 2)));
    let none_dec = Rule::<Option<Decimal>, Facts>::compile("maybeDec(!user.Active)", &schema).unwrap();
    assert_eq!(none_dec.eval(&ctx).unwrap(), None);
}
