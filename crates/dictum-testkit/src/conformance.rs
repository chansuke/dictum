//! Conformance cases: `tests/conformance/*.toml` at the repository root.
//!
//! ```toml
//! [[case]]
//! name = "not binds tighter than =="
//! expr = "not true == false"
//! expect = true                 # the value, as TOML (int, float, string, bool, array, table)
//!
//! [[case]]
//! name = "nil result"
//! expr = "user.Email"
//! nil = true                    # the value is nil (TOML has no null)
//!
//! [[case]]
//! name = "== does not chain"
//! expr = "1 == 1 == true"
//! error = "E0106"               # compile-time (E00xx-E02xx) or runtime (E03xx) code
//!
//! [[case]]
//! name = "byte indices"
//! expr = "\"héllo\"[2:]"
//! error = "E0304"
//! deviation = "Go slices inside the character; dictum reports CharBoundary"
//! ```
//!
//! Exactly one of `expect`, `nil` and `error` is set. `deviation` records an
//! intentional difference from expr-lang (Go) and is required wherever the expected
//! result differs from Go's. Cases are evaluated against
//! [`sample`](crate::fixture::sample) at [`sample_now`](crate::fixture::sample_now).

use std::fs;
use std::path::{Path, PathBuf};

use dictum::{DynMap, Dynamic};
use serde::Deserialize;

/// What a case expects.
#[derive(Clone, Debug, PartialEq)]
pub enum Expect {
    /// Evaluates to this value.
    Value(Dynamic),
    /// Fails with this code, at compile time or at run time (the code's range says
    /// which).
    Error(String),
}

impl Expect {
    /// The earliest stage that can decide this case.
    pub fn stage(&self) -> Stage {
        match self {
            Expect::Error(code) if code.starts_with("E01") => Stage::Parse,
            // An invalid constant pattern is found when `Rule::compile` builds the
            // regex, after type checking (`dictum-check` has no regex engine).
            Expect::Error(code) if code == "E0211" => Stage::Eval,
            Expect::Error(code) if code.starts_with("E02") => Stage::Check,
            _ => Stage::Eval,
        }
    }
}

/// Pipeline stages, in milestone order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// Nothing implemented yet.
    None,
    /// M1: `dictum_syntax::parse`.
    Parse,
    /// M2: `dictum_check::check`.
    Check,
    /// M3: evaluation through the public `Rule` API.
    Eval,
}

/// One case.
#[derive(Clone, Debug)]
pub struct Case {
    /// `file.toml: name`, unique across the suite.
    pub id: String,
    pub expr: String,
    pub expect: Expect,
    pub deviation: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    case: Vec<RawCase>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCase {
    name: String,
    expr: String,
    expect: Option<toml::Value>,
    nil: Option<bool>,
    error: Option<String>,
    deviation: Option<String>,
}

/// The repository's `tests/conformance` directory.
pub fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance")
}

/// Load every case under `dir`, sorted by file name. Any malformed file or case is an
/// error, so a typo in a case file cannot silently drop cases.
pub fn load(dir: &Path) -> Result<Vec<Case>, String> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    paths.sort();
    let mut cases = Vec::new();
    for path in paths {
        let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let text = fs::read_to_string(&path).map_err(|e| format!("{file_name}: {e}"))?;
        let file: File = toml::from_str(&text).map_err(|e| format!("{file_name}: {e}"))?;
        for raw in file.case {
            let id = format!("{file_name}: {}", raw.name);
            let expect = match (raw.expect, raw.nil, raw.error) {
                (Some(v), None, None) => Expect::Value(to_dynamic(v).map_err(|e| format!("{id}: {e}"))?),
                (None, Some(true), None) => Expect::Value(Dynamic::Nil),
                (None, None, Some(code)) if is_code(&code) => Expect::Error(code),
                (None, None, Some(code)) => return Err(format!("{id}: `{code}` is not an error code like E0201")),
                _ => return Err(format!("{id}: set exactly one of `expect`, `nil = true`, `error`")),
            };
            if cases.iter().any(|c: &Case| c.id == id) {
                return Err(format!("{id}: duplicate case name"));
            }
            cases.push(Case { id, expr: raw.expr, expect, deviation: raw.deviation });
        }
    }
    Ok(cases)
}

fn is_code(s: &str) -> bool {
    s.len() == 5 && s.starts_with('E') && s[1..].bytes().all(|b| b.is_ascii_digit())
}

fn to_dynamic(v: toml::Value) -> Result<Dynamic, String> {
    Ok(match v {
        toml::Value::Boolean(b) => Dynamic::Bool(b),
        toml::Value::Integer(i) => Dynamic::Int(i),
        toml::Value::Float(f) => Dynamic::Float(f),
        toml::Value::String(s) => Dynamic::String(s),
        toml::Value::Array(xs) => Dynamic::Array(xs.into_iter().map(to_dynamic).collect::<Result<_, _>>()?),
        toml::Value::Table(t) => {
            let mut m = DynMap::new();
            for (k, v) in t {
                m.insert(k, to_dynamic(v)?);
            }
            Dynamic::Map(m)
        }
        toml::Value::Datetime(d) => return Err(format!("datetime `{d}`: write times as strings via date(..)")),
    })
}
