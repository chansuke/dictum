//! Schemas described at run time, for facts whose shape is known only then (the
//! CLI reads a TOML schema and JSON facts).
//!
//! ```
//! use dictum::runtime::Description;
//! use dictum::{Ctx, DynMap, Dynamic, Rule, Schema};
//!
//! let mut d = Description::new();
//! d.structure("User", [("Name", "string"), ("Age", "int"), ("Manager", "Option<User>")]);
//! let root = d.root([("user", "User"), ("limit", "int")])?;
//! let schema = Schema::runtime(&root).build()?;
//! let rule = Rule::<bool, _>::compile("user.Age >= limit && user.Manager == nil", &schema)?;
//!
//! let mut user = DynMap::new();
//! user.insert("Name", Dynamic::String("Ada".into()));
//! user.insert("Age", Dynamic::Int(36));
//! let mut facts = DynMap::new();
//! facts.insert("user", Dynamic::Map(user));
//! facts.insert("limit", Dynamic::Int(18));
//! let facts = root.facts(&Dynamic::Map(facts))?;
//! assert!(rule.eval(&Ctx::new(&facts))?);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Every descriptor a [`Description`] builds is leaked: type identity is descriptor
//! identity (`&'static StructDesc`), as for derived structs. Build a schema once and
//! keep it; do not build one per request.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;
use std::sync::OnceLock;

use dictum_check::{FieldDesc, FieldId, StructDesc, Type};

use crate::Dynamic;
use crate::engine::builtins;
use crate::engine::layout;
use crate::engine::value::Object;
use crate::engine::value::Value;
use crate::facts::{Dictum, Field};

/// Named structs and their fields, each field's type written as in diagnostics:
/// `bool` `int` `float` `string` `bytes` `time` `duration` `location` `any`,
/// `array<T>`, `map<string, T>`, `Option<T>`, or a struct's name.
#[derive(Clone, Debug, Default)]
pub struct Description {
    structs: Vec<(String, Vec<(String, String)>)>,
}

impl Description {
    /// No structs yet.
    pub fn new() -> Self {
        Description::default()
    }

    /// Add the struct `name` with `fields` in order (the order of their ids).
    pub fn structure<N: Into<String>, T: Into<String>>(
        &mut self,
        name: &str,
        fields: impl IntoIterator<Item = (N, T)>,
    ) -> &mut Self {
        let fields = fields.into_iter().map(|(n, t)| (n.into(), t.into())).collect();
        self.structs.push((name.into(), fields));
        self
    }

    /// The root struct's fields (the rule's variables), checked and turned into
    /// descriptors together with every struct added so far.
    pub fn root<N: Into<String>, T: Into<String>>(
        &self,
        fields: impl IntoIterator<Item = (N, T)>,
    ) -> Result<Root, DescribeError> {
        let root: Vec<(String, String)> = fields.into_iter().map(|(n, t)| (n.into(), t.into())).collect();
        let mut all: Vec<(&str, &[(String, String)])> = Vec::with_capacity(self.structs.len().saturating_add(1));
        all.push(("Facts", &root));
        for (name, fields) in &self.structs {
            if !dictum_syntax::is_identifier(name) {
                return Err(DescribeError::new(name, "is not a valid struct name"));
            }
            if all.iter().any(|(n, _)| n == name) {
                return Err(DescribeError::new(name, "is described twice"));
            }
            all.push((name, fields));
        }
        // Every descriptor first, then the field types, which may name any of them.
        let mut cells: Vec<Vec<&'static OnceLock<Type>>> = Vec::with_capacity(all.len());
        let mut descs: Vec<(&str, &'static StructDesc)> = Vec::with_capacity(all.len());
        for (name, fields) in &all {
            let mut field_descs = Vec::with_capacity(fields.len());
            let mut field_cells = Vec::with_capacity(fields.len());
            for (i, (field, _)) in fields.iter().enumerate() {
                let path = alloc::format!("{name}.{field}");
                if !dictum_syntax::is_identifier(field) {
                    return Err(DescribeError::new(&path, "is not a valid field name"));
                }
                if fields.get(..i).is_some_and(|before| before.iter().any(|(f, _)| f == field)) {
                    return Err(DescribeError::new(&path, "is described twice"));
                }
                let cell: &'static OnceLock<Type> = Box::leak(Box::new(OnceLock::new()));
                let ty: &'static (dyn Fn() -> Type + Send + Sync) =
                    Box::leak(Box::new(move || cell.get().cloned().unwrap_or(Type::Any)));
                field_descs.push(FieldDesc::described(leak(field), ty));
                field_cells.push(cell);
            }
            let desc: &'static StructDesc =
                Box::leak(Box::new(StructDesc::new(leak(name), Box::leak(field_descs.into()))));
            descs.push((name, desc));
            cells.push(field_cells);
        }
        for ((name, fields), field_cells) in all.iter().zip(&cells) {
            for ((field, text), cell) in fields.iter().zip(field_cells) {
                let ty = parse_type(text, &descs)
                    .map_err(|why| DescribeError::new(&alloc::format!("{name}.{field}"), &why))?;
                let _ = cell.set(ty);
            }
        }
        let desc = descs.first().map(|(_, d)| *d).ok_or_else(|| DescribeError::new("Facts", "has no descriptor"))?;
        Ok(Root { desc })
    }
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.into())
}

/// `ty` from the description language; `structs` are the named descriptors.
fn parse_type(text: &str, structs: &[(&str, &'static StructDesc)]) -> Result<Type, String> {
    let mut p = TypeParser { rest: text, structs };
    let ty = p.ty()?;
    if !p.rest.trim().is_empty() {
        return Err(alloc::format!("has a type with extra text: `{text}`"));
    }
    Ok(ty)
}

struct TypeParser<'t, 's> {
    rest: &'t str,
    structs: &'s [(&'s str, &'static StructDesc)],
}

impl<'t> TypeParser<'t, '_> {
    fn eat(&mut self, token: &str) -> bool {
        self.rest = self.rest.trim_start();
        match self.rest.strip_prefix(token) {
            Some(rest) => {
                self.rest = rest;
                true
            }
            None => false,
        }
    }

    fn word(&mut self) -> &'t str {
        self.rest = self.rest.trim_start();
        let n = self.rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(self.rest.len());
        let (word, rest) = self.rest.split_at(n);
        self.rest = rest;
        word
    }

    fn inner(&mut self) -> Result<Type, String> {
        if !self.eat("<") {
            return Err("needs `<` after a generic type".into());
        }
        let ty = self.ty()?;
        if !self.eat(">") {
            return Err("needs `>` to close a generic type".into());
        }
        Ok(ty)
    }

    fn ty(&mut self) -> Result<Type, String> {
        let word = self.word();
        Ok(match word {
            "bool" => Type::Bool,
            "int" => Type::Int,
            "float" => Type::Float,
            "string" => Type::String,
            "bytes" => Type::Bytes,
            "time" => Type::Time,
            "duration" => Type::Duration,
            "location" => Type::Location,
            "any" => Type::Any,
            #[cfg(feature = "decimal")]
            "decimal" => Type::Decimal,
            "array" => Type::array(self.inner()?),
            "Option" => Type::option(self.inner()?),
            "map" => {
                if !(self.eat("<") && self.eat("string") && self.eat(",")) {
                    return Err("needs `map<string, T>`; map keys are always strings".into());
                }
                let value = self.ty()?;
                if !self.eat(">") {
                    return Err("needs `>` to close a generic type".into());
                }
                Type::map(value)
            }
            "" => return Err("needs a type".into()),
            name => match self.structs.iter().find(|(n, _)| *n == name) {
                Some((_, desc)) => Type::Struct(desc),
                None => return Err(alloc::format!("names an unknown type `{name}`")),
            },
        })
    }
}

/// A mistake in a [`Description`]: where, and what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DescribeError {
    at: String,
    what: String,
}

impl DescribeError {
    fn new(at: &str, what: &str) -> Self {
        DescribeError { at: at.into(), what: what.into() }
    }
}

impl fmt::Display for DescribeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}` {}", self.at, self.what)
    }
}

impl core::error::Error for DescribeError {}

/// The root of a described schema: build a [`Schema`](crate::Schema) with
/// [`Schema::runtime`](crate::Schema::runtime) and read facts with [`Root::facts`].
#[derive(Clone, Copy, Debug)]
pub struct Root {
    desc: &'static StructDesc,
}

impl Root {
    pub(crate) fn desc(&self) -> &'static StructDesc {
        self.desc
    }

    /// Read `value` (a map of the root's fields, such as parsed JSON) as facts of
    /// this root. Each field must have its declared type: `time` from an RFC 3339
    /// string, `duration` from a Go duration string, `location` from a zone name, a
    /// float from any number. A missing or `nil` field is allowed only for `Option`
    /// types; a key the struct does not declare is an error.
    pub fn facts(&self, value: &Dynamic) -> Result<Facts, FactsError> {
        match read_struct(self.desc, value, "$env")? {
            Fact::Struct(record) => Ok(Facts(*record)),
            _ => Err(FactsError::new("$env", "is not a struct")),
        }
    }
}

/// A fact that does not match its declared type: the path to it, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FactsError {
    at: String,
    what: String,
}

impl FactsError {
    fn new(at: &str, what: &str) -> Self {
        FactsError { at: at.into(), what: what.into() }
    }
}

impl fmt::Display for FactsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.at, self.what)
    }
}

impl core::error::Error for FactsError {}

/// Facts read by a [`Root`], to evaluate rules compiled against its schema.
#[derive(Clone, Debug)]
pub struct Facts(Record);

/// A struct's fields, in the order of its descriptor.
#[derive(Clone, Debug)]
struct Record {
    desc: &'static StructDesc,
    fields: Vec<Fact>,
}

/// A value of a declared type, readable as a field.
#[derive(Clone, Debug)]
enum Fact {
    Value(Dynamic),
    Array(Vec<Fact>),
    Map(BTreeMap<String, Fact>),
    Struct(Box<Record>),
}

impl Object for Record {
    fn desc(&self) -> &'static StructDesc {
        self.desc
    }

    fn field(&self, id: FieldId) -> Value<'_> {
        self.fields.get(usize::from(id.index())).map_or_else(crate::engine::value::missing_field, Field::read)
    }
}

impl Object for Facts {
    fn desc(&self) -> &'static StructDesc {
        self.0.desc
    }

    fn field(&self, id: FieldId) -> Value<'_> {
        self.0.field(id)
    }
}

/// The root type of a described schema's rules. Its descriptor comes from the
/// [`Root`] the schema was built with, not from this type.
impl Dictum for Facts {
    type Static = Facts;

    fn __desc() -> &'static StructDesc {
        static NONE: StructDesc = StructDesc::new("Facts", &[]);
        &NONE
    }
}

impl Field for Fact {
    fn ty() -> Type {
        Type::Any
    }

    fn read(&self) -> Value<'_> {
        match self {
            Fact::Value(v) => v.read(),
            Fact::Array(xs) => xs.read(),
            Fact::Map(m) => m.read(),
            Fact::Struct(r) => Value::object(&**r),
        }
    }
}

fn read_struct(desc: &'static StructDesc, value: &Dynamic, path: &str) -> Result<Fact, FactsError> {
    let Dynamic::Map(map) = value else { return Err(FactsError::new(path, "needs an object")) };
    for (key, _) in map.iter() {
        if desc.field(key).is_none() {
            return Err(FactsError::new(&alloc::format!("{path}.{key}"), "is not a field of the schema"));
        }
    }
    let mut fields = Vec::with_capacity(desc.fields().len());
    for f in desc.fields() {
        let at = alloc::format!("{path}.{}", f.name());
        let v = map.get(f.name()).unwrap_or(&Dynamic::Nil);
        fields.push(read(&f.ty(), v, &at)?);
    }
    Ok(Fact::Struct(Box::new(Record { desc, fields })))
}

fn read(ty: &Type, v: &Dynamic, path: &str) -> Result<Fact, FactsError> {
    let mismatch = |want: &str| FactsError::new(path, &alloc::format!("needs {want}, found {}", kind(v)));
    let value = |d: Dynamic| Ok(Fact::Value(d));
    match (ty, v) {
        (Type::Any, _) => value(v.clone()),
        (Type::Option(_), Dynamic::Nil) => value(Dynamic::Nil),
        (Type::Option(o), _) => read(o.inner(), v, path),
        (Type::Bool, Dynamic::Bool(_)) | (Type::Int, Dynamic::Int(_)) | (Type::Float, Dynamic::Float(_)) => {
            value(v.clone())
        }
        (Type::Float, Dynamic::Int(n)) => value(Dynamic::Float(*n as f64)),
        (Type::String, Dynamic::String(_)) | (Type::Bytes, Dynamic::Bytes(_)) => value(v.clone()),
        (Type::Bytes, Dynamic::String(s)) => value(Dynamic::Bytes(s.as_bytes().to_vec())),
        (Type::Time, Dynamic::Time(_)) => value(v.clone()),
        (Type::Time, Dynamic::String(s)) => {
            let (t, z) = layout::parse("2006-01-02T15:04:05Z07:00", s, &jiff::tz::TimeZone::UTC)
                .ok_or_else(|| FactsError::new(path, &alloc::format!("needs an RFC 3339 time, found `{s}`")))?;
            value(Dynamic::Time(t.to_zoned(z)))
        }
        (Type::Duration, Dynamic::Duration(_)) => value(v.clone()),
        (Type::Duration, Dynamic::String(s)) => builtins::parse_go_duration(s)
            .map(|d| Fact::Value(Dynamic::Duration(d)))
            .map_err(|_| FactsError::new(path, &alloc::format!("needs a duration such as `1h30m`, found `{s}`"))),
        (Type::Location, Dynamic::Location(_)) => value(v.clone()),
        (Type::Location, Dynamic::String(s)) => builtins::load_location(s)
            .map(|z| Fact::Value(Dynamic::Location(z)))
            .map_err(|_| FactsError::new(path, &alloc::format!("needs a time zone name, found `{s}`"))),
        #[cfg(feature = "decimal")]
        (Type::Decimal, Dynamic::Decimal(_)) => value(v.clone()),
        #[cfg(feature = "decimal")]
        (Type::Decimal, Dynamic::String(s)) => s
            .parse()
            .map(|d| Fact::Value(Dynamic::Decimal(d)))
            .map_err(|_| FactsError::new(path, &alloc::format!("needs a decimal, found `{s}`"))),
        #[cfg(feature = "decimal")]
        (Type::Decimal, Dynamic::Int(n)) => value(Dynamic::Decimal((*n).into())),
        (Type::Array(t), Dynamic::Array(xs)) => {
            let read_one = |(i, x): (usize, &Dynamic)| read(t, x, &alloc::format!("{path}[{i}]"));
            Ok(Fact::Array(xs.iter().enumerate().map(read_one).collect::<Result<_, _>>()?))
        }
        (Type::Map(t), Dynamic::Map(m)) => {
            let mut out = BTreeMap::new();
            for (k, x) in m.iter() {
                out.insert(k.to_string(), read(t, x, &alloc::format!("{path}.{k}"))?);
            }
            Ok(Fact::Map(out))
        }
        (Type::Struct(desc), _) => read_struct(desc, v, path),
        (Type::Bool, _) => Err(mismatch("a bool")),
        (Type::Int, _) => Err(mismatch("an integer")),
        (Type::Float, _) => Err(mismatch("a number")),
        (Type::String, _) => Err(mismatch("a string")),
        (Type::Bytes, _) => Err(mismatch("bytes or a string")),
        (Type::Time, _) => Err(mismatch("an RFC 3339 time")),
        (Type::Duration, _) => Err(mismatch("a duration string")),
        (Type::Location, _) => Err(mismatch("a time zone name")),
        #[cfg(feature = "decimal")]
        (Type::Decimal, _) => Err(mismatch("a decimal")),
        (Type::Array(_), _) => Err(mismatch("an array")),
        (Type::Map(_), _) => Err(mismatch("an object")),
        (Type::Nil, _) => Err(mismatch("null")),
    }
}

fn kind(v: &Dynamic) -> &'static str {
    match v {
        Dynamic::Nil => "null",
        Dynamic::Bool(_) => "a bool",
        Dynamic::Int(_) => "an integer",
        Dynamic::Float(_) => "a float",
        #[cfg(feature = "decimal")]
        Dynamic::Decimal(_) => "a decimal",
        Dynamic::String(_) => "a string",
        Dynamic::Bytes(_) => "bytes",
        Dynamic::Time(_) => "a time",
        Dynamic::Duration(_) => "a duration",
        Dynamic::Location(_) => "a location",
        Dynamic::Array(_) => "an array",
        Dynamic::Map(_) => "an object",
    }
}
