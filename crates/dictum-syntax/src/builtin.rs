//! The one list of builtin function names.
//!
//! The parser needs it (to know which argument is a predicate), the checker needs it
//! (typing rules, `match` on `Builtin` is exhaustive), and schema validation needs it
//! (a user function may not shadow a builtin). Adding a builtin is one line here;
//! the exhaustive matches in `dictum-check` and `dictum` then fail the build until
//! the new builtin is typed and implemented.

macro_rules! builtins {
    ($( $variant:ident = $name:literal $(, predicate = $pred:literal)? ; )*) => {
        /// A builtin function.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Builtin {
            $(
                #[doc = concat!("`", $name, "`")]
                $variant,
            )*
        }

        impl Builtin {
            /// Every builtin, in declaration order.
            pub const ALL: &'static [Builtin] = &[$(Builtin::$variant),*];

            /// Resolve a name used in call position.
            pub fn from_name(name: &str) -> Option<Builtin> {
                match name {
                    $($name => Some(Builtin::$variant),)*
                    _ => None,
                }
            }

            /// The name as written in rules.
            pub const fn name(self) -> &'static str {
                match self {
                    $(Builtin::$variant => $name,)*
                }
            }

            /// The argument index (counting a piped value as argument 0) that is
            /// parsed as a predicate, if any. A predicate argument may be omitted
            /// where the builtin allows it (`count(xs)`).
            pub const fn predicate_arg(self) -> Option<usize> {
                match self {
                    $(Builtin::$variant => builtins!(@pred $($pred)?),)*
                }
            }
        }
    };
    (@pred) => { None };
    (@pred $p:literal) => { Some($p) };
}

builtins! {
    // strings
    Trim = "trim";
    Upper = "upper";
    Lower = "lower";
    Split = "split";
    Replace = "replace";
    HasPrefix = "hasPrefix";
    HasSuffix = "hasSuffix";
    IndexOf = "indexOf";
    // numbers
    Max = "max";
    Min = "min";
    Abs = "abs";
    Ceil = "ceil";
    Floor = "floor";
    Round = "round";
    // array predicates
    All = "all", predicate = 1;
    Any = "any", predicate = 1;
    None = "none", predicate = 1;
    One = "one", predicate = 1;
    Filter = "filter", predicate = 1;
    Map = "map", predicate = 1;
    Count = "count", predicate = 1;
    Find = "find", predicate = 1;
    FindIndex = "findIndex", predicate = 1;
    // arrays
    First = "first";
    Last = "last";
    Sum = "sum";
    Len = "len";
    Join = "join";
    // maps
    Keys = "keys";
    Values = "values";
    // conversions
    Int = "int";
    Float = "float";
    String = "string";
    Type = "type";
    Decimal = "decimal";
    // time
    Now = "now";
    Duration = "duration";
    Date = "date";
    // misc
    Get = "get";
    // Post-MVP builtins (trimPrefix, reduce, sortBy, toJSON, timezone, ...) are added
    // here when implemented; until then they are unknown functions (E0203).
}
