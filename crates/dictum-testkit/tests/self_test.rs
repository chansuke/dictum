//! Tests of the test tooling itself: a broken generator or loader would make every
//! property and conformance run meaningless while still passing.
#![expect(
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    reason = "helpers outside #[test] functions: a failure there fails the test"
)]

use dictum_testkit::conformance::{self, Expect, Stage};
use dictum_testkit::generate;
use proptest::prelude::*;

fn balanced(src: &str) -> bool {
    let mut depth = 0i32;
    let mut in_str = false;
    for c in src.chars() {
        match c {
            '"' => in_str = !in_str,
            '(' if !in_str => depth += 1,
            ')' if !in_str => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0 && !in_str
}

proptest! {
    #[test]
    fn generated_rules_are_balanced((_, src) in generate::any_rule(4)) {
        prop_assert!(balanced(&src), "{src}");
    }
}

#[test]
fn repository_cases_load() {
    let cases = conformance::load(&conformance::dir()).expect("tests/conformance must load");
    assert!(!cases.is_empty());
    for c in &cases {
        assert!(c.expect.stage() > Stage::None, "{}", c.id);
    }
}

fn load_str(text: &str) -> Result<Vec<conformance::Case>, String> {
    // One directory per call: tests run in parallel.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // Under cargo's per-target scratch directory, which a WASI runner can map; WASI
    // has no process id, and its single process runs the tests one at a time.
    let process = if cfg!(target_os = "wasi") { 0 } else { std::process::id() };
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("dictum-testkit-{process}-{n}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.toml"), text).unwrap();
    let out = conformance::load(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    out
}

#[test]
fn loader_rejects_ambiguous_and_malformed_cases() {
    let both = "[[case]]\nname = \"x\"\nexpr = \"1\"\nexpect = 1\nerror = \"E0201\"\n";
    assert!(load_str(both).unwrap_err().contains("exactly one"));
    let none = "[[case]]\nname = \"x\"\nexpr = \"1\"\n";
    assert!(load_str(none).unwrap_err().contains("exactly one"));
    let bad_code = "[[case]]\nname = \"x\"\nexpr = \"1\"\nerror = \"oops\"\n";
    assert!(load_str(bad_code).unwrap_err().contains("not an error code"));
    let unknown_key = "[[case]]\nname = \"x\"\nexpr = \"1\"\nexpect = 1\nexpected = 2\n";
    assert!(load_str(unknown_key).is_err());
    let dup = "[[case]]\nname = \"x\"\nexpr = \"1\"\nexpect = 1\n[[case]]\nname = \"x\"\nexpr = \"2\"\nexpect = 2\n";
    assert!(load_str(dup).unwrap_err().contains("duplicate"));
    let feature = "[[case]]\nname = \"x\"\nexpr = \"1\"\nexpect = 1\nfeature = \"decimals\"\n";
    assert!(load_str(feature).unwrap_err().contains("unknown feature `decimals`"));
}

#[test]
fn loader_maps_toml_values_and_stages() {
    let text = "[[case]]\nname = \"i\"\nexpr = \"1\"\nexpect = 1\n\
                [[case]]\nname = \"f\"\nexpr = \"1.0\"\nexpect = 1.0\n\
                [[case]]\nname = \"n\"\nexpr = \"nil\"\nnil = true\n\
                [[case]]\nname = \"p\"\nexpr = \"1 ==\"\nerror = \"E0101\"\n\
                [[case]]\nname = \"t\"\nexpr = \"1 + true\"\nerror = \"E0208\"\n";
    let cases = load_str(text).unwrap();
    assert_eq!(cases[0].expect, Expect::Value(dictum::Dynamic::Int(1)));
    assert_eq!(cases[1].expect, Expect::Value(dictum::Dynamic::Float(1.0)));
    assert_eq!(cases[2].expect, Expect::Value(dictum::Dynamic::Nil));
    assert_eq!(cases[3].expect.stage(), Stage::Parse);
    assert_eq!(cases[4].expect.stage(), Stage::Check);
    assert_eq!(cases[0].expect.stage(), Stage::Eval);
}
