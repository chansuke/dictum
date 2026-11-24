//! Every `match` on a `Builtin` or an `ErrorCode` lists each variant.
//!
//! docs/architecture.md ("At boundaries") promises that a new builtin fails the
//! build until it is typed in `dictum-check` and implemented in `dictum`. A `_ =>`
//! arm (or a catch-all binding such as `other =>`) breaks that promise: the new
//! variant falls into the wildcard and compiles. This test reads the source of
//! every workspace crate and reports such arms.
//!
//! Only matches whose arms name the enum directly (`Builtin::Len =>`) are checked.
//! A match on a tuple such as `(b, tags)` is an overload table: a missing row is a
//! type error the user sees, not a silent default.

use std::path::{Path, PathBuf};
use syn::visit::Visit;

/// The enums whose matches must be exhaustive.
const ENUMS: &[&str] = &["Builtin", "ErrorCode"];

#[test]
fn builtin_and_error_code_matches_have_no_catch_all_arm() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let files = rust_files(&crates);
    assert!(
        files.iter().any(|f| f.ends_with("dictum-syntax/src/builtin.rs")),
        "found no crates/*/src/**/*.rs next to {}; update rust_files in this test to the new layout",
        crates.display()
    );
    let mut found = Vec::new();
    for file in files {
        let shown = file.strip_prefix(&crates).unwrap_or(&file).display().to_string();
        let arms = std::fs::read_to_string(&file).map_err(|e| e.to_string()).and_then(|text| catch_all_arms(&text));
        match arms {
            Ok(arms) => {
                found.extend(arms.into_iter().map(|(name, line)| format!("crates/{shown}:{line} (match on {name})")))
            }
            Err(e) => found.push(format!("crates/{shown}: cannot read or parse: {e}")),
        }
    }
    assert!(
        found.is_empty(),
        "These matches on Builtin or ErrorCode end in a catch-all arm:\n  {}\n\n\
         List every variant instead of `_ =>` or a binding such as `other =>`. Group the \
         variants that share a result with `|`. A new builtin must fail the build until it \
         is typed and implemented (docs/architecture.md, \"At boundaries\"). If the match \
         is a lookup that should answer for only some variants, move it into a function \
         that matches every variant, as `typing` in dictum-check/src/checker/mod.rs does.",
        found.join("\n  ")
    );
}

#[test]
fn the_check_reports_a_catch_all_on_builtin_and_ignores_tuples() {
    let text = r#"
        fn wild(b: Builtin) -> u8 { match b { Builtin::Len => 1, _ => 0 } }
        fn bound(c: ErrorCode) -> u8 { match c { ErrorCode::ArgCount => 1, other => 0 } }
        fn full(b: Builtin) -> u8 { match b { Builtin::Len => 1, Builtin::Trim | Builtin::Upper => 0 } }
        fn table(b: Builtin, t: u8) -> u8 { match (b, t) { (Builtin::Len, 0) => 1, _ => 0 } }
        fn other(x: Option<u8>) -> u8 { match x { Some(_) => 1, _ => 0 } }
    "#;
    assert_eq!(catch_all_arms(text), Ok(vec![("Builtin", 2), ("ErrorCode", 3)]));
}

/// The enum name and line of each match on one of [`ENUMS`] with a catch-all arm.
fn catch_all_arms(text: &str) -> Result<Vec<(&'static str, usize)>, String> {
    let file = syn::parse_file(text).map_err(|e| e.to_string())?;
    let mut visitor = Matches { found: Vec::new() };
    visitor.visit_file(&file);
    Ok(visitor.found)
}

struct Matches {
    found: Vec<(&'static str, usize)>,
}

impl<'ast> Visit<'ast> for Matches {
    fn visit_expr_match(&mut self, m: &'ast syn::ExprMatch) {
        let named = m.arms.iter().find_map(|arm| enum_named(&arm.pat));
        if let Some(name) = named {
            if let Some(arm) = m.arms.iter().find(|arm| is_catch_all(&arm.pat)) {
                self.found.push((name, line_of(arm)));
            }
        }
        syn::visit::visit_expr_match(self, m);
    }
}

/// The enum of [`ENUMS`] a top-level pattern names, as in `Builtin::Len` or
/// `Builtin::Len | Builtin::Trim`.
fn enum_named(pat: &syn::Pat) -> Option<&'static str> {
    let path = match pat {
        syn::Pat::Path(p) => &p.path,
        syn::Pat::TupleStruct(p) => &p.path,
        syn::Pat::Struct(p) => &p.path,
        syn::Pat::Or(p) => return p.cases.iter().find_map(enum_named),
        _ => return None,
    };
    let enum_segment = path.segments.iter().rev().nth(1)?;
    ENUMS.iter().copied().find(|name| enum_segment.ident == name)
}

/// `_`, or a binding without a sub-pattern, which matches anything.
fn is_catch_all(pat: &syn::Pat) -> bool {
    match pat {
        syn::Pat::Wild(_) => true,
        syn::Pat::Ident(p) => p.subpat.is_none(),
        syn::Pat::Or(p) => p.cases.iter().any(is_catch_all),
        _ => false,
    }
}

fn line_of(arm: &syn::Arm) -> usize {
    use syn::spanned::Spanned;
    arm.pat.span().start().line
}

/// Every `.rs` file under each crate's `src/`.
fn rust_files(crates: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(crates) else {
        return files;
    };
    for entry in entries.flatten() {
        collect(&entry.path().join("src"), &mut files);
    }
    files.sort();
    files
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, files);
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
}
