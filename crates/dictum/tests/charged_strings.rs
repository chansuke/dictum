//! A string a rule computes is built with `engine::frame::Text`, which charges
//! `max_alloc` before each write. Building a `String` first and charging it after
//! (the removed `built` helper, `Value::string`) let `string([s, s, ...])` allocate
//! gigabytes under a one-megabyte limit, and the same mistake had been fixed one
//! builtin at a time before (sort, then `toJSON`, `join`, `upper` and the slices).
//! This test reads the crate's source and fails on any `Str::Shared` built outside
//! the files that own it.

use std::path::{Path, PathBuf};
use syn::visit::Visit;

/// The files that may build `Str::Shared` directly, and why.
const OWNERS: &[(&str, &str)] = &[
    ("engine/value.rs", "defines Str and shares strings that already exist"),
    ("engine/frame.rs", "Text, the charged builder"),
    ("func.rs", "a host function's String: the host allocated it, so there is nothing to charge first"),
];

#[test]
fn computed_strings_are_built_with_text() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect(&src, &mut files);
    assert!(
        files.iter().any(|f| f.ends_with("engine/frame.rs")),
        "found no src/engine/frame.rs under {}; update this test to the new layout",
        src.display()
    );
    let mut found = Vec::new();
    for file in &files {
        let shown = file.strip_prefix(&src).unwrap_or(file).display().to_string();
        if OWNERS.iter().any(|(owner, _)| shown == *owner) {
            continue;
        }
        match std::fs::read_to_string(file).map_err(|e| e.to_string()).and_then(|t| shared_strings(&t)) {
            Ok(lines) => found.extend(lines.into_iter().map(|line| format!("crates/dictum/src/{shown}:{line}"))),
            Err(e) => found.push(format!("crates/dictum/src/{shown}: cannot read or parse: {e}")),
        }
    }
    assert!(
        found.is_empty(),
        "These lines build a Str::Shared directly:\n  {}\n\n\
         Build the string with `Text` (crates/dictum/src/engine/frame.rs): `Text::copy(f, s, span)` \
         for a copy, `push_str`/`push` for a string built piece by piece, and `reserve` then \
         `write_reserved` for a writer in a lower layer whose length you know first. Each charges \
         max_alloc before the buffer grows. Only {} may build Str::Shared.",
        found.join("\n  "),
        OWNERS.iter().map(|(f, why)| format!("{f} ({why})")).collect::<Vec<_>>().join(", ")
    );
}

#[test]
fn the_check_reports_a_built_str_and_ignores_patterns() {
    let text = r#"
        fn built(s: String) -> Value<'static> { Value(Repr::Str(Str::Shared(s.into()))) }
        fn read(v: &Value<'_>) -> bool { matches!(v.0, Repr::Str(Str::Shared(_))) }
        fn parts(xs: Vec<Arc<str>>) -> Vec<Str<'static>> { xs.into_iter().map(Str::Shared).collect() }
    "#;
    assert_eq!(shared_strings(text), Ok(vec![2, 4]));
}

/// The line of each expression that names `Str::Shared` (a call or a function value).
fn shared_strings(text: &str) -> Result<Vec<usize>, String> {
    let file = syn::parse_file(text).map_err(|e| e.to_string())?;
    let mut visitor = SharedStr { lines: Vec::new() };
    visitor.visit_file(&file);
    Ok(visitor.lines)
}

struct SharedStr {
    lines: Vec<usize>,
}

impl<'ast> Visit<'ast> for SharedStr {
    fn visit_expr_path(&mut self, e: &'ast syn::ExprPath) {
        let names: Vec<String> = e.path.segments.iter().map(|s| s.ident.to_string()).collect();
        if names.ends_with(&["Str".to_owned(), "Shared".to_owned()]) {
            use syn::spanned::Spanned;
            self.lines.push(e.span().start().line);
        }
        syn::visit::visit_expr_path(self, e);
    }
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
    files.sort();
}
