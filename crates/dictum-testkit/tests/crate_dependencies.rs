//! Which workspace crates each crate may depend on, from the manifests. The table
//! is the one in docs/architecture.md ("Crates and what each may see").
//!
//! Dev-dependency cycles (a crate's tests depend on a crate that depends on it) are
//! listed, not forbidden: the test links a second copy of the crate. The list must
//! match exactly, so a new cycle fails and a removed one must be taken off.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// For each crate, the workspace crates its `[dependencies]` may name.
const ALLOWED: &[(&str, &[&str])] = &[
    ("dictum-syntax", &[]),
    ("dictum-check", &["dictum-syntax"]),
    ("dictum-derive", &["dictum-syntax"]),
    ("dictum", &["dictum-syntax", "dictum-check", "dictum-derive"]),
    ("dictum-cli", &["dictum"]),
    ("dictum-testkit", &["dictum", "dictum-syntax"]),
    ("dictum-fuzz", &["dictum", "dictum-check", "dictum-syntax", "dictum-testkit"]),
];

/// `(crate, dev-dependency)` pairs where the dev-dependency depends on the crate.
const DEV_CYCLES: &[(&str, &str)] = &[
    ("dictum", "dictum-testkit"),
    ("dictum-check", "dictum"),
    ("dictum-check", "dictum-testkit"),
    ("dictum-syntax", "dictum-testkit"),
];

#[test]
fn crates_depend_only_on_the_crates_the_table_allows() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifests = match workspace_manifests(&root) {
        Ok(m) => m,
        Err(e) => panic!("{e}"),
    };
    let allowed: BTreeMap<&str, &[&str]> = ALLOWED.iter().copied().collect();
    let mut problems = Vec::new();
    for (name, m) in &manifests {
        let Some(may) = allowed.get(name.as_str()) else {
            problems.push(format!(
                "`{name}` is not in ALLOWED in crates/dictum-testkit/tests/crate_dependencies.rs. \
                 Add it with the workspace crates it may depend on, and add it to the table in docs/architecture.md."
            ));
            continue;
        };
        for dep in m.normal.iter().filter(|d| !may.contains(&d.as_str())) {
            problems.push(format!(
                "`{name}` depends on `{dep}`, which the table in docs/architecture.md does not allow. \
                 Move the item `{name}` needs into a crate it may depend on (for dictum-derive and \
                 dictum-check, that is dictum-syntax), or pass it in from the caller."
            ));
        }
    }
    let found = dev_cycles(&manifests);
    let listed: BTreeSet<(String, String)> =
        DEV_CYCLES.iter().map(|(a, b)| ((*a).to_owned(), (*b).to_owned())).collect();
    for (krate, dev) in found.difference(&listed) {
        problems.push(format!(
            "`{krate}` dev-depends on `{dev}`, which depends on `{krate}`: its tests link two copies of \
             `{krate}`, whose types do not match. Test `{krate}` through its own API, or put the test in `{dev}`."
        ));
    }
    for (krate, dev) in listed.difference(&found) {
        problems.push(format!("the dev-dependency cycle `{krate}` -> `{dev}` is gone; remove it from DEV_CYCLES"));
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[derive(Default)]
struct Manifest {
    /// Workspace crates in `[dependencies]` and `[build-dependencies]`.
    normal: BTreeSet<String>,
    /// Workspace crates in `[dev-dependencies]`.
    dev: BTreeSet<String>,
}

/// The workspace crates and the fuzz crate, by package name.
fn workspace_manifests(root: &Path) -> Result<BTreeMap<String, Manifest>, String> {
    let mut paths: Vec<_> = std::fs::read_dir(root.join("crates"))
        .map_err(|e| format!("{}: {e}", root.display()))?
        .flatten()
        .map(|e| e.path().join("Cargo.toml"))
        .filter(|p| p.is_file())
        .collect();
    paths.push(root.join("fuzz/Cargo.toml"));
    let mut raw = BTreeMap::new();
    for path in paths {
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let table: toml::Table = text.parse().map_err(|e| format!("{}: {e}", path.display()))?;
        let name = table
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .ok_or_else(|| format!("{}: no package.name", path.display()))?
            .to_owned();
        raw.insert(name, table);
    }
    let names: BTreeSet<String> = raw.keys().cloned().collect();
    let deps = |table: &toml::Table, section: &str| -> BTreeSet<String> {
        let Some(section) = table.get(section).and_then(|s| s.as_table()) else {
            return BTreeSet::new();
        };
        section
            .iter()
            .map(|(key, spec)| spec.get("package").and_then(|p| p.as_str()).unwrap_or(key).to_owned())
            .filter(|dep| names.contains(dep))
            .collect()
    };
    Ok(raw
        .iter()
        .map(|(name, table)| {
            let mut normal = deps(table, "dependencies");
            normal.extend(deps(table, "build-dependencies"));
            (name.clone(), Manifest { normal, dev: deps(table, "dev-dependencies") })
        })
        .collect())
}

/// `(crate, dev)` where `dev` is a dev-dependency of `crate` that reaches `crate`
/// through normal dependencies.
fn dev_cycles(manifests: &BTreeMap<String, Manifest>) -> BTreeSet<(String, String)> {
    let reaches = |from: &str, to: &str| {
        let mut seen = BTreeSet::new();
        let mut stack = vec![from.to_owned()];
        while let Some(next) = stack.pop() {
            if next == to {
                return true;
            }
            if seen.insert(next.clone()) {
                stack.extend(manifests.get(&next).into_iter().flat_map(|m| m.normal.iter().cloned()));
            }
        }
        false
    };
    manifests
        .iter()
        .flat_map(|(name, m)| m.dev.iter().filter(|dev| reaches(dev, name)).map(|dev| (name.clone(), dev.clone())))
        .collect()
}
