//! The single lists this crate owns (error codes, builtins, reserved words) and the
//! small pieces implemented before M1. Every other stage trusts these.
#![expect(
    clippy::arithmetic_side_effects,
    reason = "helpers outside #[test] functions: a failure there fails the test"
)]

use std::collections::BTreeSet;

use dictum_syntax::{Builtin, ErrorCode, Span, is_identifier, is_reserved};
use proptest::prelude::*;

#[test]
fn error_codes_are_unique_and_well_formed() {
    let mut seen = BTreeSet::new();
    for &code in ErrorCode::ALL {
        let s = code.as_str();
        assert!(seen.insert(s), "duplicate code {s}");
        assert!(s.len() == 5 && s.starts_with('E') && s[1..].bytes().all(|b| b.is_ascii_digit()), "{s}");
        let class = &s[1..3];
        assert!(["00", "01", "02", "03", "09"].contains(&class), "{s}: unknown class E{class}xx");
    }
}

/// `{n}` placeholders are numbered from 0 without gaps, so a translation can rely on
/// `args[n]` existing for every placeholder.
#[test]
fn error_templates_number_placeholders_densely() {
    for &code in ErrorCode::ALL {
        let t = code.template();
        let mut used = BTreeSet::new();
        let mut rest = t;
        while let Some(i) = rest.find('{') {
            let close = rest[i..].find('}').unwrap_or_else(|| panic!("{}: unclosed `{{`", code.as_str())) + i;
            let n: usize = rest[i + 1..close].parse().unwrap_or_else(|_| panic!("{}: `{t}`", code.as_str()));
            used.insert(n);
            rest = &rest[close + 1..];
        }
        let expected: BTreeSet<usize> = (0..used.len()).collect();
        assert_eq!(used, expected, "{}: `{t}`", code.as_str());
    }
}

#[test]
fn builtins_are_unique_callable_names() {
    let mut seen = BTreeSet::new();
    for &b in Builtin::ALL {
        let name = b.name();
        assert!(seen.insert(name), "duplicate builtin {name}");
        assert!(is_identifier(name), "{name} must be callable");
        assert_eq!(Builtin::from_name(name), Some(b));
    }
    assert_eq!(Builtin::from_name("nope"), None);
}

#[test]
fn reserved_words_are_not_identifiers() {
    for w in ["true", "false", "nil", "let", "if", "else", "not", "and", "or", "in", "matches", "contains"] {
        assert!(is_reserved(w), "{w}");
        assert!(!is_identifier(w), "{w}");
    }
    for ok in ["user", "_x", "Age2", "startsWithX"] {
        assert!(is_identifier(ok), "{ok}");
    }
    for bad in ["", "2x", "a-b", "é", "a b", "$env"] {
        assert!(!is_identifier(bad), "{bad:?}");
    }
}

fn span() -> impl Strategy<Value = Span> {
    (0u32..1000, 0u32..1000).prop_map(|(a, b)| Span { start: a.min(b), end: a.max(b) })
}

proptest! {
    #[test]
    fn span_to_is_the_smallest_cover(a in span(), b in span()) {
        let c = a.to(b);
        prop_assert_eq!(c, b.to(a));
        prop_assert!(c.start <= a.start && c.start <= b.start && c.end >= a.end && c.end >= b.end);
        prop_assert!(c.start == a.start || c.start == b.start);
        prop_assert!(c.end == a.end || c.end == b.end);
    }

    #[test]
    fn span_text_never_panics(src in "\\PC{0,16}", a in span()) {
        if let Some(t) = a.text(&src) {
            prop_assert_eq!(t.len(), (a.end - a.start) as usize);
        }
    }

    #[test]
    fn identifiers_match_the_lexical_rule(s in "[A-Za-z_][A-Za-z0-9_]{0,8}") {
        prop_assert_eq!(is_identifier(&s), !is_reserved(&s));
    }
}
