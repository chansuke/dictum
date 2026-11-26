//! Schemas described at run time: what a description may say, and how facts that
//! do not match it are reported.
#![cfg(feature = "runtime-schema")]
#![expect(clippy::unwrap_used, reason = "helpers outside #[test] functions: a failure there fails the test")]

use dictum::runtime::{Description, Facts, Root};
use dictum::{Ctx, DynMap, Dynamic, ErrorCode, EvalErrorKind, NodeStatus, Rule, RuleSet, Schema, Span};

fn map(entries: &[(&str, Dynamic)]) -> Dynamic {
    let mut m = DynMap::new();
    for (k, v) in entries {
        m.insert(*k, v.clone());
    }
    Dynamic::Map(m)
}

fn people() -> Root {
    let mut d = Description::new();
    d.structure("Person", [("Name", "string"), ("Born", "time"), ("Boss", "Option<Person>")]);
    d.root([("people", "array<Person>"), ("limits", "map<string, duration>")]).unwrap()
}

fn eval(root: &Root, facts: &Dynamic, src: &str) -> Dynamic {
    let schema = Schema::runtime(root).build().unwrap();
    let rule = Rule::<Dynamic, _>::compile(src, &schema).unwrap();
    let facts = root.facts(facts).unwrap();
    rule.eval(&Ctx::new(&facts)).unwrap()
}

#[test]
fn a_recursive_described_struct_reads_and_type_checks() {
    let boss =
        map(&[("Name", Dynamic::String("Grace".into())), ("Born", Dynamic::String("1906-12-09T00:00:00Z".into()))]);
    let ada = map(&[
        ("Name", Dynamic::String("Ada".into())),
        ("Born", Dynamic::String("1815-12-10T00:00:00+01:00".into())),
        ("Boss", boss),
    ]);
    let facts =
        map(&[("people", Dynamic::Array(vec![ada])), ("limits", map(&[("day", Dynamic::String("24h".into()))]))]);
    let root = people();
    assert_eq!(eval(&root, &facts, "people[0].Boss?.Name"), Dynamic::String("Grace".into()));
    assert_eq!(eval(&root, &facts, "people[0].Born.Year()"), Dynamic::Int(1815));
    assert_eq!(eval(&root, &facts, "limits.day?.Hours()"), Dynamic::Float(24.0));
    let schema = Schema::runtime(&root).build().unwrap();
    let err = Rule::<Dynamic, _>::compile("people[0].Nope", &schema).err().unwrap();
    assert_eq!(err.diagnostics()[0].code().as_str(), "E0204");
}

#[test]
fn a_mistaken_description_says_where() {
    let fails = |fields: &[(&str, &str)]| {
        let mut d = Description::new();
        d.structure("User", [("Name", "string")]);
        d.root(fields.iter().copied()).err().unwrap().to_string()
    };
    assert_eq!(fails(&[("u", "Usr")]), "`Facts.u` names an unknown type `Usr`");
    assert_eq!(fails(&[("u", "array<int")]), "`Facts.u` needs `>` to close a generic type");
    assert_eq!(fails(&[("u", "map<int, int>")]), "`Facts.u` needs `map<string, T>`; map keys are always strings");
    assert_eq!(fails(&[("u", "int int")]), "`Facts.u` has a type with extra text: `int int`");
    assert_eq!(fails(&[("u", "int"), ("u", "int")]), "`Facts.u` is described twice");
    assert_eq!(fails(&[("not", "int")]), "`Facts.not` is not a valid field name");
    let mut d = Description::new();
    d.structure("A", [("x", "int")]).structure("A", [("y", "int")]);
    assert_eq!(d.root([("a", "A")]).err().unwrap().to_string(), "`A` is described twice");
}

#[test]
fn facts_that_do_not_match_say_where() {
    let root = people();
    let person = |born: Dynamic| map(&[("Name", Dynamic::String("Ada".into())), ("Born", born)]);
    let fails = |facts: Dynamic| root.facts(&facts).err().unwrap().to_string();
    let limits = map(&[]);
    assert_eq!(
        fails(map(&[
            ("people", Dynamic::Array(vec![person(Dynamic::String("1815".into()))])),
            ("limits", limits.clone())
        ])),
        "$env.people[0].Born: needs an RFC 3339 time, found `1815`"
    );
    assert_eq!(
        fails(map(&[("people", Dynamic::Array(vec![person(Dynamic::Int(1815))])), ("limits", limits.clone())])),
        "$env.people[0].Born: needs an RFC 3339 time, found an integer"
    );
    assert_eq!(fails(map(&[("limits", limits.clone())])), "$env.people: needs an array, found null");
    assert_eq!(
        fails(map(&[("people", Dynamic::Array(vec![])), ("limits", limits), ("extra", Dynamic::Int(1))])),
        "$env.extra: is not a field of the schema"
    );
    assert_eq!(
        fails(map(&[("people", Dynamic::Array(vec![])), ("limits", map(&[("day", Dynamic::String("1 day".into()))]))])),
        "$env.limits.day: needs a duration such as `1h30m`, found `1 day`"
    );
}

#[test]
fn mismatched_runtime_root_is_rejected_with_schema_mismatch() {
    // Two distinct roots with identical field names and types: without a descriptor
    // identity check, field reads by index would silently read from the wrong root.
    let root_a = Description::new().root([("x", "int")]).unwrap();
    let root_b = Description::new().root([("x", "int")]).unwrap();
    let root_c = Description::new().root([("y", "string")]).unwrap();

    let facts_a = root_a.facts(&map(&[("x", Dynamic::Int(10))])).unwrap();
    let facts_b = root_b.facts(&map(&[("x", Dynamic::Int(42))])).unwrap();
    let facts_c = root_c.facts(&map(&[("y", Dynamic::String("hi".into()))])).unwrap();

    let schema_a = Schema::runtime(&root_a).build().unwrap();
    let rule = Rule::<i64, _>::compile("x + 1", &schema_a).unwrap();
    assert_eq!(rule.eval(&Ctx::new(&facts_a)).unwrap(), 11);

    for bad in [&facts_b, &facts_c] {
        let err = rule.eval(&Ctx::new(bad)).unwrap_err();
        assert!(matches!(err.kind(), EvalErrorKind::SchemaMismatch));
        assert_eq!(err.code(), ErrorCode::SchemaMismatch);
        assert_eq!(err.code().as_str(), "E0313");
        assert_eq!(err.span(), Span { start: 0, end: 5 });
        assert_eq!(err.rule(), None);
        assert_eq!(
            err.to_string(),
            "error[E0313]: facts do not match the schema this rule was compiled against\n \
             --> rule:1:1\n  \
             |\n\
             1 | x + 1\n  \
             | ^^^^^"
        );
    }

    let report = rule.explain(&Ctx::new(&facts_b));
    let err = report.result().as_ref().unwrap_err();
    assert!(matches!(err.kind(), EvalErrorKind::SchemaMismatch));
    assert_eq!(err.code(), ErrorCode::SchemaMismatch);
    assert_eq!(err.span(), Span { start: 0, end: 5 });
    assert_eq!(report.root().status(), &NodeStatus::Failed(ErrorCode::SchemaMismatch));
    assert!(report.to_string().contains("failed E0313"));

    // Body span is reported even when the rule has `let` bindings.
    let with_let = Rule::<i64, _>::compile("let y = x;\ny + 1", &schema_a).unwrap();
    let err = with_let.eval(&Ctx::new(&facts_b)).unwrap_err();
    assert!(matches!(err.kind(), EvalErrorKind::SchemaMismatch));
    assert_eq!(err.span(), Span { start: 11, end: 16 });

    // RuleSet::eval and RuleSet::explain check the descriptor too.
    let mut builder = RuleSet::<bool, _>::builder(&schema_a);
    builder.insert("positive", "x > 0").unwrap();
    builder.insert("small", "x < 100").unwrap();
    let set = builder.build();

    let matched: Vec<_> = set.eval(&Ctx::new(&facts_a)).unwrap().matched().collect();
    assert_eq!(matched, vec!["positive", "small"]);

    let err = set.eval(&Ctx::new(&facts_b)).err().unwrap();
    assert!(matches!(err.kind(), EvalErrorKind::SchemaMismatch));
    assert_eq!(err.code(), ErrorCode::SchemaMismatch);
    assert_eq!(err.rule(), Some("positive"));
    assert_eq!(err.span(), Span { start: 0, end: 5 });
    assert!(err.to_string().contains("--> positive:1:1"));

    let report = set.explain("small", &Ctx::new(&facts_b)).unwrap();
    let err = report.result().as_ref().unwrap_err();
    assert!(matches!(err.kind(), EvalErrorKind::SchemaMismatch));
    assert_eq!(err.code(), ErrorCode::SchemaMismatch);
    assert_eq!(err.rule(), Some("small"));
    assert_eq!(err.span(), Span { start: 0, end: 7 });
    assert_eq!(report.root().status(), &NodeStatus::Failed(ErrorCode::SchemaMismatch));
}

#[test]
fn schema_builder_for_runtime_facts_rejects_facts_from_a_root() {
    let schema = Schema::<Facts>::builder().build().unwrap();
    let rule = Rule::<i64, _>::compile("1 + 1", &schema).unwrap();
    let root = Description::new().root([("x", "int")]).unwrap();
    let facts = root.facts(&map(&[("x", Dynamic::Int(1))])).unwrap();

    let err = rule.eval(&Ctx::new(&facts)).unwrap_err();
    assert!(matches!(err.kind(), EvalErrorKind::SchemaMismatch));
    assert_eq!(err.code(), ErrorCode::SchemaMismatch);

    let report = rule.explain(&Ctx::new(&facts));
    assert!(matches!(report.result().as_ref().unwrap_err().kind(), EvalErrorKind::SchemaMismatch));
    assert_eq!(report.root().status(), &NodeStatus::Failed(ErrorCode::SchemaMismatch));
}

