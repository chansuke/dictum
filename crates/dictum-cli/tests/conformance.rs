//! Every case in `tests/conformance/*.toml` through the CLI's code path: the TOML
//! schema and JSON facts in `tests/conformance/facts/` instead of the derived
//! `dictum_testkit::Facts`. Same expectations, so a case passes here exactly when
//! the described schema types and reads the facts as the derive does.
#![expect(clippy::panic, reason = "helpers outside #[test] functions: a failure there fails the test")]

use dictum::{Ctx, DynMap, Dynamic, Rule, Schema};
use dictum_cli::Session;
use dictum_testkit::conformance::{self, Case, Expect};
use dictum_testkit::{Facts, sample, sample_now};
use libtest_mimic::{Arguments, Failed, Trial};

fn main() {
    let args = Arguments::from_args();
    let dir = conformance::dir();
    let read =
        |name: &str| std::fs::read_to_string(dir.join("facts").join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let session = Session::new(&read("sample.schema.toml"), &read("sample.json"), sample_now())
        .unwrap_or_else(|e| panic!("sample facts: {e}"));
    let session: &'static Session = Box::leak(Box::new(session));
    let cases = conformance::load(&dir).unwrap_or_else(|e| panic!("loading cases: {e}"));
    let mut trials: Vec<Trial> = cases
        .into_iter()
        .map(|case| {
            let missing = case.feature.as_deref().is_some_and(|f| !(f == "decimal" && cfg!(feature = "decimal")));
            Trial::test(case.id.clone(), move || run(session, &case)).with_ignored_flag(missing)
        })
        .collect();
    trials.push(Trial::test("sample.json matches the derived sample facts", move || same_facts(session)));
    libtest_mimic::run(&args, trials).exit();
}

fn run(session: &Session, case: &Case) -> Result<(), Failed> {
    let got = match session.eval(&case.expr) {
        Ok(v) => Expect::Value(v),
        Err(e) => Expect::Error(e.code().to_owned()),
    };
    if unordered(&got) == unordered(&case.expect) {
        Ok(())
    } else {
        Err(format!("expr: {}\n  expected: {:?}\n  got: {got:?}", case.expr, case.expect).into())
    }
}

/// `toJSON($env)` is the same for the JSON facts and the derived sample: every
/// field, in order, with the same value.
fn same_facts(session: &Session) -> Result<(), Failed> {
    let schema = Schema::<Facts>::builder().build().map_err(|e| format!("{e:?}"))?;
    let rule = Rule::<Dynamic, Facts>::compile("toJSON($env)", &schema).map_err(|e| e.to_string())?;
    let facts = sample();
    let derived = rule.eval(&Ctx::new(&facts)).map_err(|e| e.to_string())?;
    let described = session.eval("toJSON($env)").map_err(|e| e.to_string())?;
    if derived == described { Ok(()) } else { Err(format!("derived: {derived}\n  described: {described}").into()) }
}

/// `e` with every map's entries sorted by key, as the library's conformance runner
/// compares: a map's entry order is not part of a case's result.
fn unordered(e: &Expect) -> Expect {
    fn sort(v: &Dynamic) -> Dynamic {
        match v {
            Dynamic::Array(xs) => Dynamic::Array(xs.iter().map(sort).collect()),
            Dynamic::Map(m) => {
                let mut entries: Vec<(&str, Dynamic)> = m.iter().map(|(k, v)| (k, sort(v))).collect();
                entries.sort_by(|a, b| a.0.cmp(b.0));
                let mut out = DynMap::new();
                for (k, v) in entries {
                    out.insert(k, v);
                }
                Dynamic::Map(out)
            }
            v => v.clone(),
        }
    }
    match e {
        Expect::Value(v) => Expect::Value(sort(v)),
        Expect::Error(code) => Expect::Error(code.clone()),
    }
}
