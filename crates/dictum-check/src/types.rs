//! The type lattice and the struct descriptors `#[derive(Dictum)]` emits.

use alloc::sync::Arc;
use core::fmt;

/// A dictum type.
///
/// There is no `Error` variant: the checker tracks poisoned nodes in its own
/// inference type, so a `Type` in the IR is always a real type. There is no `Func`
/// variant: functions are not values (only `name(args)` calls exist).
#[derive(Clone, Debug, PartialEq)]
pub enum Type {
    /// Unknown until run time; every use inserts a runtime check.
    Any,
    /// The type of `nil`.
    Nil,
    /// `bool`
    Bool,
    /// `int` (`i64`, overflow is an error)
    Int,
    /// `float` (`f64`)
    Float,
    /// `decimal` (`rust_decimal::Decimal`)
    #[cfg(feature = "decimal")]
    Decimal,
    /// `string` (UTF-8; indices are byte offsets)
    String,
    /// `bytes`
    Bytes,
    /// `time`: an instant and the time zone it is shown in, as Go's `time.Time`
    Time,
    /// `duration` (`jiff::SignedDuration`)
    Duration,
    /// `location`: a time zone (`jiff::tz::TimeZone`), as Go's `*time.Location`
    Location,
    /// `array<T>`
    Array(Arc<Type>),
    /// `map<string, T>`. Keys are always strings.
    Map(Arc<Type>),
    /// `Option<T>`; build with [`Type::option`].
    Option(OptionType),
    /// A derived struct. Identity is the descriptor's address.
    Struct(&'static StructDesc),
}

/// The payload of `Type::Option`.
///
/// Invariant (enforced by the private field): the inner type is never `Option`, `Nil`
/// or `Any`, so `Option<Option<T>>` cannot be represented and `x?.f` where `f` is
/// already optional stays `Option<T>`.
#[derive(Clone, Debug, PartialEq)]
pub struct OptionType(Arc<Type>);

impl OptionType {
    /// The non-optional inner type.
    pub fn inner(&self) -> &Type {
        &self.0
    }
}

impl Type {
    /// `Option<t>`, normalized: `option(Option<T>) = Option<T>`, `option(Nil) = Nil`,
    /// `option(Any) = Any`.
    pub fn option(t: Type) -> Type {
        match t {
            Type::Option(_) | Type::Nil | Type::Any => t,
            other => Type::Option(OptionType(Arc::new(other))),
        }
    }

    /// `array<t>`
    pub fn array(t: Type) -> Type {
        Type::Array(Arc::new(t))
    }

    /// `map<string, t>`
    pub fn map(t: Type) -> Type {
        Type::Map(Arc::new(t))
    }

    /// The runtime tag of a value of this type, if it is fixed. `None` for `Any` and
    /// `Option` (whose values are `Nil` or the inner tag).
    pub fn tag(&self) -> Option<Tag> {
        Some(match self {
            Type::Any | Type::Option(_) => return None,
            Type::Nil => Tag::Nil,
            Type::Bool => Tag::Bool,
            Type::Int => Tag::Int,
            Type::Float => Tag::Float,
            #[cfg(feature = "decimal")]
            Type::Decimal => Tag::Decimal,
            Type::String => Tag::String,
            Type::Bytes => Tag::Bytes,
            Type::Time => Tag::Time,
            Type::Duration => Tag::Duration,
            Type::Location => Tag::Location,
            Type::Array(_) => Tag::Array,
            Type::Map(_) => Tag::Map,
            Type::Struct(_) => Tag::Struct,
        })
    }

    /// Whether `nil` is a value of this type.
    pub fn admits_nil(&self) -> bool {
        matches!(self, Type::Any | Type::Nil | Type::Option(_))
    }

    /// The type without `Option`: `Option<T>` is `T`, others are themselves.
    pub fn non_optional(&self) -> &Type {
        match self {
            Type::Option(o) => o.inner(),
            other => other,
        }
    }
}

impl fmt::Display for Type {
    /// `int`, `array<string>`, `Option<User>`, ... (used in diagnostics).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Any => f.write_str("any"),
            Type::Array(t) => write!(f, "array<{t}>"),
            Type::Map(t) => write!(f, "map<string, {t}>"),
            Type::Option(o) => write!(f, "Option<{}>", o.inner()),
            Type::Struct(d) => f.write_str(d.name()),
            other => f.write_str(other.tag().map_or("any", Tag::name)),
        }
    }
}

/// Runtime type tag. `Option` has no tag: an optional value is `Nil` or its inner
/// tag. Used by the overload tables in [`crate::resolve`] both statically (from a
/// `Type`) and dynamically (from a value of type `any`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tag {
    /// `nil`
    Nil,
    /// `bool`
    Bool,
    /// `int`
    Int,
    /// `float`
    Float,
    /// `decimal`
    #[cfg(feature = "decimal")]
    Decimal,
    /// `string`
    String,
    /// `bytes`
    Bytes,
    /// `time`
    Time,
    /// `duration`
    Duration,
    /// `location`
    Location,
    /// `array`
    Array,
    /// `map`
    Map,
    /// a struct
    Struct,
}

impl Tag {
    /// The name `type(x)` returns (Go-compatible where Go has one).
    pub const fn name(self) -> &'static str {
        match self {
            Tag::Nil => "nil",
            Tag::Bool => "bool",
            Tag::Int => "int",
            Tag::Float => "float",
            #[cfg(feature = "decimal")]
            Tag::Decimal => "decimal",
            Tag::String => "string",
            Tag::Bytes => "bytes",
            Tag::Time => "time",
            Tag::Duration => "duration",
            Tag::Location => "location",
            Tag::Array => "array",
            Tag::Map => "map",
            Tag::Struct => "struct",
        }
    }
}

/// Descriptor of a derived struct, emitted by `#[derive(Dictum)]` as a `static`.
///
/// Invariants (guaranteed by the derive, which emits this and the field reader from
/// the same field list):
/// - `fields[i]` is what `Object::field(FieldId(i))` returns;
/// - names are valid dictum identifiers (`dictum_syntax::is_identifier`) and unique;
/// - the descriptor lives in a `static` (or, described at run time, is leaked), so
///   `&'static StructDesc` identity is type identity. This is why `Type::Struct`
///   holds a reference, not an `Arc`.
#[derive(Debug)]
pub struct StructDesc {
    name: &'static str,
    fields: &'static [FieldDesc],
}

impl StructDesc {
    /// Used by the derive in a `static` initializer.
    pub const fn new(name: &'static str, fields: &'static [FieldDesc]) -> Self {
        StructDesc { name, fields }
    }

    /// The Rust type name, for diagnostics and explain output.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Fields in `FieldId` order.
    pub fn fields(&self) -> &'static [FieldDesc] {
        self.fields
    }

    /// Resolve a rule-visible field name. Linear scan: this runs at compile time only
    /// and structs have tens of fields.
    pub fn field(&self, name: &str) -> Option<(FieldId, &'static FieldDesc)> {
        let fields = self.fields;
        let i = fields.iter().position(|f| f.name == name)?;
        Some((FieldId(u16::try_from(i).ok()?), fields.get(i)?))
    }
}

impl PartialEq for StructDesc {
    fn eq(&self, other: &Self) -> bool {
        core::ptr::eq(self, other)
    }
}

/// One field of a struct: derived, or described at run time.
pub struct FieldDesc {
    name: &'static str,
    ty: FieldType,
}

/// How a field's type is produced. A function rather than a `Type` so the
/// descriptor is const-constructible (`Type` holds `Arc`s) and recursive structs
/// (`children: Vec<Node>`) need no lazy initialization; a runtime description
/// cannot be a function pointer, so it is a closure, which may read a type that is
/// filled in after every descriptor of a recursive schema exists.
#[derive(Clone, Copy)]
enum FieldType {
    Derived(fn() -> Type),
    Described(&'static (dyn Fn() -> Type + Send + Sync)),
}

impl core::fmt::Debug for FieldDesc {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FieldDesc").field("name", &self.name).finish_non_exhaustive()
    }
}

impl FieldDesc {
    /// Used by the derive.
    pub const fn new(name: &'static str, ty: fn() -> Type) -> Self {
        FieldDesc { name, ty: FieldType::Derived(ty) }
    }

    /// A field of a struct described at run time (`dictum::runtime`).
    pub const fn described(name: &'static str, ty: &'static (dyn Fn() -> Type + Send + Sync)) -> Self {
        FieldDesc { name, ty: FieldType::Described(ty) }
    }

    /// The rule-visible name (after `rename`).
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The field's dictum type.
    pub fn ty(&self) -> Type {
        match self.ty {
            FieldType::Derived(f) => f(),
            FieldType::Described(f) => f(),
        }
    }
}

/// Index of a field in its `StructDesc`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FieldId(u16);

impl FieldId {
    /// Used by the derive in `match id.index()` and by the checker.
    pub const fn new(index: u16) -> Self {
        FieldId(index)
    }

    /// The position in `StructDesc::fields`.
    pub const fn index(self) -> u16 {
        self.0
    }
}
