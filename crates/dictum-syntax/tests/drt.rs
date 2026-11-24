//! Differential test: the parser against the Lean reference parser
//! (`formal/lean/Dictum/Syntax.lean`, run as `dictum-oracle`).
//!
//! Both parsers get the same generated sources and must agree on which ones parse
//! and on the tree (as s-expressions). The Lean parser is written in the shape of
//! Go expr-lang's `parseExpression`, independently of the Pratt parser here, so a
//! disagreement is a bug in one of them or a gap in the precedence table.
//!
//! Ignored until M1, and when the oracle is not built (see `dictum_testkit::oracle`).
//! One trial runs before M1: it checks that the generated sources are not mostly
//! syntax errors for the Lean parser, since agreeing on rejections alone would
//! say little.
//! `just drt` builds it and runs this with `PROPTEST_CASES` cases (default 4096).
#![expect(
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    reason = "helpers outside #[test] functions: a failure there fails the test"
)]

use std::cell::RefCell;
use std::path::PathBuf;

use dictum_testkit::generate::{operator_soup, operator_source};
use dictum_testkit::oracle::{self, Oracle};
use dictum_testkit::syntax::program;
use libtest_mimic::{Arguments, Failed, Trial};
use proptest::strategy::{BoxedStrategy, Strategy, ValueTree};
use proptest::test_runner::{Config, TestCaseError, TestRunner};

fn main() {
    let args = Arguments::from_args();
    let oracle = Oracle::locate();
    let ignored = oracle.is_err();
    let trial = |name: &str, strategy: fn() -> BoxedStrategy<String>| {
        let oracle = oracle.clone();
        Trial::test(name, move || agree(oracle?, strategy())).with_ignored_flag(ignored)
    };
    let mut trials = vec![
        trial("parser_agrees_with_lean_on_operator_sources", || operator_source(4)),
        trial("parser_agrees_with_lean_on_token_soup", operator_soup),
    ];
    let generated = oracle.clone();
    trials.push(
        Trial::test("operator_sources_are_mostly_well_formed", move || well_formed_share(generated?))
            .with_ignored_flag(oracle.is_err()),
    );
    libtest_mimic::run(&args, trials).exit();
}

fn agree(path: PathBuf, sources: BoxedStrategy<String>) -> Result<(), Failed> {
    let oracle = RefCell::new(Oracle::spawn(&path));
    let mut runner = TestRunner::new(Config { cases: oracle::cases(), failure_persistence: None, ..Config::default() });
    runner
        .run(&sources, |src| {
            let rust = dictum_syntax::parse(&src).map(|p| program(&p)).map_err(|d| {
                let (diags, _) = d.finish();
                diags.first().map(|d| d.code().as_str().to_owned()).unwrap_or_default()
            });
            let lean = oracle.borrow_mut().parse(&src);
            match (&rust, &lean) {
                (Ok(a), Ok(b)) if a == b => Ok(()),
                (Err(_), Err(_)) => Ok(()),
                _ => Err(TestCaseError::fail(format!("{src}\n  rust: {rust:?}\n  lean: {lean:?}"))),
            }
        })
        .map_err(|e| e.to_string().into())
}

/// At least a third of [`operator_source`]'s sources parse, by the Lean parser.
fn well_formed_share(path: PathBuf) -> Result<(), Failed> {
    let mut oracle = Oracle::spawn(&path);
    let mut runner = TestRunner::deterministic();
    let strategy = operator_source(4);
    let total = 1000;
    let parsed = (0..total)
        .filter(|_| {
            let src = strategy.new_tree(&mut runner).expect("a generated source").current();
            oracle.parse(&src).is_ok()
        })
        .count();
    if parsed * 3 >= total {
        Ok(())
    } else {
        Err(format!("only {parsed} of {total} generated sources parse; the comparison would be mostly about errors")
            .into())
    }
}
