//! Evaluation benchmarks on the shared fixture. CI only builds them
//! (`cargo bench --no-run`); `just bench` runs them and the same rules on Go's expr
//! (`go/bench_test.go`). The results are in docs/benchmarks.md.
#![expect(clippy::unwrap_used, reason = "benchmark setup on fixed rules and facts")]

use criterion::{Criterion, criterion_group, criterion_main};
use dictum::{Ctx, Rule, RuleSet, Schema};
use dictum_testkit::{Facts, sample, sample_now};

/// Representative rules: a field comparison, membership, a predicate over a
/// sequence, string work, and arithmetic over structs.
const RULES: &[(&str, &str)] = &[
    ("compare", "user.Age >= 18 && user.Active"),
    ("membership", r#"user.Name in ["Ada", "Grace", "Barbara"] || "admin" in user.Tags"#),
    ("predicate", "all(nums, # > 0) && count(nums, # % 2 == 0) > 2"),
    ("strings", r#"upper(user.Name) startsWith "A" && len(join(user.Tags, ",")) > 3"#),
    ("arithmetic", "sum(map(order.Items, .Price * .Qty)) * (1 - (order.Discount ?? 0.0)) > 100"),
];

fn single_rules(c: &mut Criterion) {
    let schema = Schema::<Facts>::builder().build().unwrap();
    let facts = sample();
    let ctx = Ctx::new(&facts).with_now(sample_now());
    let mut group = c.benchmark_group("eval");
    for (name, src) in RULES {
        let rule = Rule::<bool, Facts>::compile(src, &schema).unwrap();
        // The Go side checks the same result.
        assert_eq!(rule.eval(&ctx).ok(), Some(true), "{name}");
        group.bench_function(*name, |b| b.iter(|| rule.eval(&ctx)));
    }
    group.finish();
}

fn rule_set(c: &mut Criterion) {
    let schema = Schema::<Facts>::builder().build().unwrap();
    let mut builder = RuleSet::<bool, Facts>::builder(&schema);
    for (name, src) in RULES {
        builder.insert(name, src).unwrap();
    }
    let set = builder.build();
    let facts = sample();
    let ctx = Ctx::new(&facts).with_now(sample_now());
    c.bench_function("rule_set", |b| b.iter(|| set.eval(&ctx)));
}

/// The longest rule above: compile cost is dominated by parsing and checking.
const COMPILE: &str = "sum(map(order.Items, .Price * .Qty)) * (1 - (order.Discount ?? 0.0)) > 100";

fn compile(c: &mut Criterion) {
    let schema = Schema::<Facts>::builder().build().unwrap();
    c.bench_function("compile", |b| b.iter(|| Rule::<bool, Facts>::compile(COMPILE, &schema)));
}

criterion_group!(benches, single_rules, rule_set, compile);
criterion_main!(benches);
