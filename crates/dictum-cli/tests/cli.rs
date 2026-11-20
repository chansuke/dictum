//! The `dictum` command: its schema handling, and the binary's output and exit
//! status (0 a value, 1 a rule that fails, 2 bad input).
// Test code: the workspace's no-panic lints are for evaluation code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use dictum_cli::{infer_schema, root_from_toml};

fn facts_dir() -> PathBuf {
    dictum_testkit::conformance::dir().join("facts")
}

#[test]
fn an_inferred_schema_is_a_draft_that_loads() {
    let json = std::fs::read_to_string(facts_dir().join("sample.json")).unwrap();
    let draft = infer_schema(&json).unwrap();
    for line in [
        "user = \"User\"",
        "nums = \"array<int>\"",
        "Created = \"time\"",
        "Email = \"Option<any>\"",
        "Items = \"array<Item>\"",
        "Price = \"float\"",
        "Notes = \"Notes\"",
    ] {
        assert!(draft.lines().any(|l| l == line), "{line:?} not in\n{draft}");
    }
    root_from_toml(&draft).unwrap();
}

#[test]
fn a_mistaken_schema_file_says_what_is_wrong() {
    let fails = |text: &str| root_from_toml(text).err().unwrap();
    assert_eq!(fails("[types.User]\nName = \"string\"\n"), "schema: needs a `[root]` table");
    assert_eq!(fails("[root]\nn = 1\n"), "schema: `root.n` must be a type written as a string");
    assert_eq!(fails("[root]\n[structs.A]\n"), "schema: unknown section `structs`; use `root` and `types`");
    assert_eq!(fails("[root]\nu = \"Usr\"\n"), "schema: `Facts.u` names an unknown type `Usr`");
}

#[cfg(not(target_os = "wasi"))]
mod binary {
    use std::io::Write;
    use std::process::{Command, Output, Stdio};

    fn dictum(args: &[&str], stdin: &str) -> Output {
        let dir = super::facts_dir();
        let mut child = Command::new(env!("CARGO_BIN_EXE_dictum"))
            .args(args.iter().map(|a| a.replace("{facts}", &dir.display().to_string())))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    }

    const SAMPLE: [&str; 6] =
        ["--schema", "{facts}/sample.schema.toml", "--facts", "{facts}/sample.json", "--now", "2026-01-01T00:00:00Z"];

    fn eval(expr: &str) -> (Option<i32>, String, String) {
        let mut args = vec!["eval"];
        args.extend(SAMPLE);
        args.push(expr);
        let out = dictum(&args, "");
        (out.status.code(), String::from_utf8(out.stdout).unwrap(), String::from_utf8(out.stderr).unwrap())
    }

    #[test]
    fn eval_prints_the_value_as_a_rule_literal() {
        assert_eq!(eval("order.Items | map(.Sku)"), (Some(0), "[\"A-1\", \"B-2\"]\n".into(), String::new()));
        assert_eq!(eval("now() - user.Created").1, "18673h46m40s\n");
    }

    #[test]
    fn a_failing_rule_prints_its_error_and_exits_1() {
        let (code, out, err) = eval("user.Nope");
        assert_eq!((code, out.as_str()), (Some(1), ""));
        assert!(err.starts_with("error[E0204]: `User` has no field `Nope`"), "{err}");
        assert_eq!(eval("7 % 0").0, Some(1));
    }

    #[test]
    fn bad_input_exits_2() {
        let out = dictum(&["eval", "--schema", "{facts}/missing.toml", "--facts", "{facts}/sample.json", "1"], "");
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8(out.stderr).unwrap().starts_with("dictum: "), "names the program");
    }

    #[test]
    fn repl_evaluates_each_line_until_quit() {
        let mut args = vec!["repl"];
        args.extend(SAMPLE);
        let out = dictum(&args, "user.Age\n\nuser.Nope\nlimit * 2\n:quit\nnever\n");
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert_eq!(out.status.code(), Some(0));
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines.first(), Some(&"36"));
        assert!(lines.iter().any(|l| l.starts_with("error[E0204]")), "{stdout}");
        assert_eq!(lines.last(), Some(&"20"));
    }
}
