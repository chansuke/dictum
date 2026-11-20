//! The `dictum` command: rules over JSON facts whose types a TOML schema describes.
//!
//! The commands live here rather than in `main.rs`, so the conformance cases run
//! through the same code without starting a process (and on WASI).

use dictum::jiff::Timestamp;
use dictum::runtime::{Description, Facts, Root};
use dictum::{Ctx, DynMap, Dynamic, Limits, Rule, Schema};

/// A schema and the facts to evaluate rules against.
pub struct Session {
    schema: Schema<Facts>,
    facts: Facts,
    now: Timestamp,
    limits: Limits,
}

impl Session {
    /// Read a TOML schema and JSON facts.
    pub fn new(schema_toml: &str, facts_json: &str, now: Timestamp) -> Result<Session, String> {
        let root = root_from_toml(schema_toml)?;
        let schema = Schema::runtime(&root).build().map_err(|e| format!("schema: {e:?}"))?;
        let facts = facts_from_json(&root, facts_json)?;
        Ok(Session { schema, facts, now, limits: Limits::default() })
    }

    /// Replace the evaluation limits.
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Compile and evaluate `expr`. The error is shown as the library shows it, with
    /// a caret under the failing part.
    pub fn eval(&self, expr: &str) -> Result<Dynamic, Failure> {
        let rule = Rule::<Dynamic, Facts>::compile(expr, &self.schema).map_err(Failure::Compile)?;
        let ctx = Ctx::new(&self.facts).with_now(self.now).with_limits(self.limits);
        rule.eval(&ctx).map_err(Failure::Eval)
    }
}

/// Why an expression gave no value.
#[derive(Debug)]
pub enum Failure {
    /// It does not compile.
    Compile(dictum::CompileError),
    /// It failed while evaluating.
    Eval(dictum::EvalError),
}

impl Failure {
    /// The first diagnostic's code (`E0201`, `E0302`, ...).
    pub fn code(&self) -> &'static str {
        match self {
            Failure::Compile(e) => e.diagnostics().first().map_or("E0900", |d| d.code().as_str()),
            Failure::Eval(e) => e.code().as_str(),
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Compile(e) => e.fmt(f),
            Failure::Eval(e) => e.fmt(f),
        }
    }
}

/// `[root]` names the variables, `[types.Name]` the structs, each field's type as a
/// string (`"array<int>"`, `"Option<User>"`). Fields keep the order they are written.
pub fn root_from_toml(text: &str) -> Result<Root, String> {
    let table: toml::Table = text.parse().map_err(|e| format!("schema: {e}"))?;
    let fields = |at: &str, value: &toml::Value| -> Result<Vec<(String, String)>, String> {
        let toml::Value::Table(t) = value else { return Err(format!("schema: `{at}` must be a table")) };
        t.iter()
            .map(|(k, v)| match v {
                toml::Value::String(ty) => Ok((k.clone(), ty.clone())),
                _ => Err(format!("schema: `{at}.{k}` must be a type written as a string")),
            })
            .collect()
    };
    let mut description = Description::new();
    for (key, value) in &table {
        match key.as_str() {
            "root" => {}
            "types" => {
                let toml::Value::Table(types) = value else { return Err("schema: `types` must be a table".into()) };
                for (name, value) in types {
                    description.structure(name, fields(&format!("types.{name}"), value)?);
                }
            }
            other => return Err(format!("schema: unknown section `{other}`; use `root` and `types`")),
        }
    }
    let root = table.get("root").ok_or("schema: needs a `[root]` table")?;
    description.root(fields("root", root)?).map_err(|e| format!("schema: {e}"))
}

/// JSON facts, read as the root's types.
pub fn facts_from_json(root: &Root, text: &str) -> Result<Facts, String> {
    let json: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("facts: {e}"))?;
    root.facts(&dynamic(&json)).map_err(|e| format!("facts: {e}"))
}

/// A JSON value as a dynamic one: an integral number that fits is an `int`.
pub fn dynamic(json: &serde_json::Value) -> Dynamic {
    match json {
        serde_json::Value::Null => Dynamic::Nil,
        serde_json::Value::Bool(b) => Dynamic::Bool(*b),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => Dynamic::Int(i),
            None => Dynamic::Float(n.as_f64().unwrap_or(f64::NAN)),
        },
        serde_json::Value::String(s) => Dynamic::String(s.clone()),
        serde_json::Value::Array(xs) => Dynamic::Array(xs.iter().map(dynamic).collect()),
        serde_json::Value::Object(m) => {
            let mut out = DynMap::new();
            for (k, v) in m {
                out.insert(k, dynamic(v));
            }
            Dynamic::Map(out)
        }
    }
}

/// A first draft of a schema for `facts_json`, to edit: types come from the values
/// (an integral number is `int`, `null` is `Option<any>`, an empty array is
/// `array<any>`, a string that reads as RFC 3339 is `time`), and every object
/// becomes a struct named after its key (`user` is `User`, `items` is `Item`).
pub fn infer_schema(facts_json: &str) -> Result<String, String> {
    let json: serde_json::Value = serde_json::from_str(facts_json).map_err(|e| format!("facts: {e}"))?;
    let serde_json::Value::Object(root) = &json else { return Err("facts: the top level must be an object".into()) };
    let mut types: Vec<(String, Vec<(String, String)>)> = Vec::new();
    let root_fields: Vec<(String, String)> = root.iter().map(|(k, v)| (k.clone(), infer(k, v, &mut types))).collect();
    let mut out = String::from("[root]\n");
    write_fields(&mut out, &root_fields);
    for (name, fields) in &types {
        out.push_str(&format!("\n[types.{name}]\n"));
        write_fields(&mut out, fields);
    }
    Ok(out)
}

fn write_fields(out: &mut String, fields: &[(String, String)]) {
    for (name, ty) in fields {
        let key = if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') && !name.is_empty() {
            name.clone()
        } else {
            format!("{name:?}")
        };
        out.push_str(&format!("{key} = {ty:?}\n"));
    }
}

fn infer(key: &str, v: &serde_json::Value, types: &mut Vec<(String, Vec<(String, String)>)>) -> String {
    match v {
        serde_json::Value::Null => "Option<any>".into(),
        serde_json::Value::Bool(_) => "bool".into(),
        serde_json::Value::Number(n) if n.is_i64() => "int".into(),
        serde_json::Value::Number(_) => "float".into(),
        serde_json::Value::String(s) if s.parse::<Timestamp>().is_ok() => "time".into(),
        serde_json::Value::String(_) => "string".into(),
        serde_json::Value::Array(xs) => match xs.first() {
            Some(first) => format!("array<{}>", infer(key.strip_suffix('s').unwrap_or(key), first, types)),
            None => "array<any>".into(),
        },
        serde_json::Value::Object(m) => {
            let name = type_name(key);
            if !types.iter().any(|(n, _)| *n == name) {
                types.push((name.clone(), Vec::new()));
                let fields = m.iter().map(|(k, v)| (k.clone(), infer(k, v, types))).collect();
                if let Some(entry) = types.iter_mut().find(|(n, _)| *n == name) {
                    entry.1 = fields;
                }
            }
            name
        }
    }
}

/// `user` is `User`, `line_item` is `LineItem`.
fn type_name(key: &str) -> String {
    let name: String = key
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut cs = w.chars();
            cs.next().map(|c| c.to_ascii_uppercase().to_string() + cs.as_str()).unwrap_or_default()
        })
        .collect();
    if name.starts_with(|c: char| c.is_ascii_alphabetic()) { name } else { format!("T{name}") }
}
