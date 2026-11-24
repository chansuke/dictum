//! The Lean names the code and docs cite for each optimization exist in
//! `formal/lean`. docs/verification.md ("Optimizations, and the theorem that allows
//! each") lets the engine apply only an optimization with a theorem; this test keeps
//! a renamed or removed theorem from leaving the table, or `engine/optimize.rs`,
//! pointing at nothing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[test]
fn every_cited_lean_name_is_declared() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let decls = match declarations(&root.join("formal/lean")) {
        Ok(d) if !d.is_empty() => d,
        Ok(_) => panic!("found no Lean declarations under formal/lean"),
        Err(e) => panic!("{e}"),
    };
    let read = |p: &str| std::fs::read_to_string(root.join(p)).map_err(|e| format!("{p}: {e}"));
    let mut missing = Vec::new();
    match read("crates/dictum/src/engine/optimize.rs") {
        Ok(text) => {
            for (line, name) in optimize_citations(&text) {
                if !decls.contains_key(&name) {
                    missing.push(format!(
                        "crates/dictum/src/engine/optimize.rs:{line}: Lean `{name}` is not a def or theorem in formal/lean"
                    ));
                }
            }
        }
        Err(e) => missing.push(e),
    }
    match read("docs/verification.md") {
        Ok(text) => {
            for (name, file) in table_theorems(&text) {
                let found = decls.get(&name).filter(|d| d.iter().any(|(kind, _)| kind == "theorem"));
                let in_file = |f: &str| found.is_some_and(|d| d.iter().any(|(_, at)| at.ends_with(f)));
                match (&file, found) {
                    (_, None) => {
                        missing.push(format!("docs/verification.md: `{name}` is not a theorem in formal/lean"))
                    }
                    (Some(f), Some(_)) if !in_file(f) => {
                        missing.push(format!("docs/verification.md: `{name}` is not a theorem in {f}"))
                    }
                    _ => {}
                }
            }
        }
        Err(e) => missing.push(e),
    }
    assert!(
        missing.is_empty(),
        "{}\n\nAn optimization may only be applied with a theorem that allows it (docs/verification.md, \
         \"Optimizations\"). Prove it in formal/lean first, or fix the name where it is cited; \
         do not remove the citation to make this pass.",
        missing.join("\n")
    );
}

#[test]
fn the_parsers_find_citations_and_table_cells() {
    let rust = "/// Lean `opRange`: x\n/// Lean `and_swap`/`or_swap`: y\n//! - `a`: `b`\n";
    assert_eq!(
        optimize_citations(rust),
        [(1, "opRange".to_owned()), (2, "and_swap".to_owned()), (2, "or_swap".to_owned())]
    );
    let md = "### Optimizations, and the theorem that allows each\n\n| a | b | theorem |\n| --- | --- | --- |\n\
              | x | `c` | `p`, `q` (`Opt/S.lean`) |\n| y | z | `r` |\n\nThe end `s`.\n";
    assert_eq!(
        table_theorems(md),
        [
            ("p".to_owned(), Some("Opt/S.lean".to_owned())),
            ("q".to_owned(), Some("Opt/S.lean".to_owned())),
            ("r".to_owned(), None),
        ]
    );
}

/// Each declared name with its kind and file.
fn declarations(dir: &Path) -> Result<BTreeMap<String, Vec<(String, String)>>, String> {
    let mut files = Vec::new();
    lean_files(dir, &mut files)?;
    let mut decls: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for file in files {
        let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let shown = file.display().to_string();
        for line in text.lines() {
            let mut words = line.split_whitespace().skip_while(|w| w.starts_with("@[") || w.ends_with(']'));
            let mut kind = words.next();
            while matches!(kind, Some("private" | "protected" | "noncomputable" | "partial")) {
                kind = words.next();
            }
            if let (Some(kind @ ("theorem" | "lemma" | "def" | "abbrev")), Some(name)) = (kind, words.next()) {
                let short = name.rsplit('.').next().unwrap_or(name);
                let kind = if kind == "lemma" { "theorem" } else { kind };
                decls.entry(short.to_owned()).or_default().push((kind.to_owned(), shown.clone()));
            }
        }
    }
    Ok(decls)
}

fn lean_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|n| n == ".lake") {
            continue;
        }
        if path.is_dir() {
            lean_files(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "lean") {
            out.push(path);
        }
    }
    Ok(())
}

/// Names in doc comments of the form ``Lean `name` `` or ``Lean `a`/`b` ``.
fn optimize_citations(text: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for (number, line) in (1..).zip(text.lines()) {
        let Some((_, mut rest)) = line.split_once("/// Lean ") else { continue };
        while let Some(body) = rest.strip_prefix('`') {
            let Some((name, after)) = body.split_once('`') else { break };
            found.push((number, name.to_owned()));
            rest = after.strip_prefix('/').unwrap_or("");
        }
    }
    found
}

/// The theorem column of the optimizations table: each name, with the file the
/// cell names after it, if any.
fn table_theorems(text: &str) -> Vec<(String, Option<String>)> {
    let Some(start) = text.find("### Optimizations, and the theorem that allows each") else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let rows =
        text.get(start..).unwrap_or("").lines().skip_while(|l| !l.starts_with('|')).take_while(|l| l.starts_with('|'));
    for row in rows.skip(2) {
        let cell = row.trim_end().trim_end_matches('|').rsplit('|').next().unwrap_or("");
        let mut names = Vec::new();
        let mut file = None;
        for (i, token) in cell.split('`').enumerate() {
            if i % 2 == 1 {
                if token.ends_with(".lean") {
                    file = Some(token.to_owned());
                } else {
                    names.push(token.to_owned());
                }
            }
        }
        found.extend(names.into_iter().map(|n| (n, file.clone())));
    }
    found
}
