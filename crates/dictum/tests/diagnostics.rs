//! Snapshots of rendered errors (`Display` of `CompileError` and `EvalError`): the
//! caret format, line/column, multi-byte characters, help lines and the
//! "and N more" tail. Review snapshot changes with `cargo insta review`.
// Test code: the workspace's no-panic lints are for evaluation code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use dictum::{Ctx, Dynamic, Rule, Schema};
use dictum_testkit::{Facts, sample};

fn compile_error(src: &str) -> String {
    let schema = Schema::<Facts>::builder().build().unwrap();
    Rule::<Dynamic, Facts>::compile(src, &schema).err().expect("must not compile").to_string()
}

#[test]
fn syntax_errors() {
    insta::assert_snapshot!("unexpected_token", compile_error("user.Age >= "));
    insta::assert_snapshot!("unterminated_string", compile_error("user.Name == \"Ada"));
    insta::assert_snapshot!("chained_equality", compile_error("1 == 1 == true"));
    insta::assert_snapshot!("recovery_reports_several", compile_error("[1, , 2] + (3 + )"));
}

#[test]
fn type_errors() {
    insta::assert_snapshot!("type_mismatch", compile_error(r#"user.Age + "x""#));
    insta::assert_snapshot!("optional_access_help", compile_error("user.Manager.Name"));
    insta::assert_snapshot!("unknown_field_after_multibyte", compile_error(r#""héllo" + user.Nickname"#));
    insta::assert_snapshot!("multi_line", compile_error("let a = 1;\nlet b = \"x\";\na + b"));
    let many = (0..25).map(|i| format!("x{i}")).collect::<Vec<_>>().join(" + ");
    insta::assert_snapshot!("capped_with_tail", compile_error(&many));
}

#[test]
fn runtime_errors() {
    let schema = Schema::<Facts>::builder().build().unwrap();
    let facts = sample();
    let eval_error = |src: &str| {
        let rule = Rule::<Dynamic, Facts>::compile(src, &schema).unwrap();
        rule.eval(&Ctx::new(&facts)).expect_err("must fail").to_string()
    };
    insta::assert_snapshot!("char_boundary_help", eval_error(r#""héllo"[2:]"#));
    insta::assert_snapshot!("clock_unset", eval_error("now()"));
    insta::assert_snapshot!("overflow", eval_error("user.Age + 9223372036854775807"));
}

#[test]
fn explain_tree() {
    let schema = Schema::<Facts>::builder().build().unwrap();
    let facts = sample();
    let explain = |src: &str| {
        let rule = Rule::<Dynamic, Facts>::compile(src, &schema).unwrap();
        rule.explain(&Ctx::new(&facts)).to_string()
    };
    insta::assert_snapshot!("explain_and", explain(r#""admin" in user.Tags && user.Age >= 18"#));
    insta::assert_snapshot!("explain_short_circuit", explain("user.Age < 18 && 7 % 0 > 0"));
    insta::assert_snapshot!("explain_failure", explain("user.Age > 18 && 7 % 0 > 0"));
    insta::assert_snapshot!("explain_predicate", explain("any(nums, # > 4)"));
    insta::assert_snapshot!("explain_literal_array", explain(r#"user.Name in ["Ada", "Grace"] && user.Age >= 18"#));
}
