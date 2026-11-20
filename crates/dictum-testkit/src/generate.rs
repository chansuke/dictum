//! Random well-typed rule sources over [`Facts`](crate::fixture::Facts).
//!
//! The generator works on source text, not on the AST, so the parser and the checker
//! are part of every property that uses it. Every composite is parenthesized: the
//! generated text means exactly the tree that produced it, whatever the precedence
//! table says. Precedence is tested separately, by the parser's own tests.
//!
//! What it covers: arithmetic with overflow and division by zero, int-to-float
//! promotion, short-circuit logic, comparison chains, `in`, string builtins, field
//! reads (including optional ones through `??`), `let`, and the predicates `all`,
//! `any`, `count`, `findLast`, `findLastIndex`, `groupBy`, `sortBy` and `reduce`. Errors are expected outcomes: a property compares two evaluations, so
//! a case that fails must fail the same way on both sides.

use proptest::prelude::*;

/// The dictum types the generator produces expressions of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ty {
    Bool,
    Int,
    Float,
    Str,
}

impl Ty {
    /// The type name as `type(x)` returns it.
    pub fn name(self) -> &'static str {
        match self {
            Ty::Bool => "bool",
            Ty::Int => "int",
            Ty::Float => "float",
            Ty::Str => "string",
        }
    }
}

const INT_LEAVES: &[&str] = &[
    "user.Age",
    "limit",
    "len(nums)",
    "len(user.Tags)",
    "len(user.Name)",
    "len(order.Items)",
    "get(nums, 0) ?? 0",
    "9223372036854775807",
    "-9223372036854775807",
];
const FLOAT_LEAVES: &[&str] = &["user.Score", "order.Total", "order.Discount ?? 0.0", "0.5", "1e300", "-0.0"];
const BOOL_LEAVES: &[&str] = &["user.Active", "true", "false", "user.Email == nil"];
const STR_LEAVES: &[&str] = &["user.Name", "user.Email ?? \"\"", "\"\"", "\"é\"", "order.Notes[\"gift\"] ?? \"no\""];

fn leaf(ty: Ty) -> BoxedStrategy<String> {
    let literals: BoxedStrategy<String> = match ty {
        Ty::Int => (-5i64..=5).prop_map(|n| if n < 0 { format!("({n})") } else { n.to_string() }).boxed(),
        Ty::Float => (-8i32..=8).prop_map(|n| format!("{:.2}", f64::from(n) / 4.0)).boxed(),
        Ty::Bool => any::<bool>().prop_map(|b| b.to_string()).boxed(),
        Ty::Str => "[a-c]{0,3}".prop_map(|s| format!("\"{s}\"")).boxed(),
    };
    let fields = match ty {
        Ty::Int => INT_LEAVES,
        Ty::Float => FLOAT_LEAVES,
        Ty::Bool => BOOL_LEAVES,
        Ty::Str => STR_LEAVES,
    };
    prop_oneof![literals, proptest::sample::select(fields).prop_map(|s| format!("({s})"))].boxed()
}

/// An expression of type `ty` with at most `depth` levels of operators.
///
/// Each `prop_oneof!` lists the leaf twice so leaves weigh double and most trees stay
/// shallower than `depth`.
pub fn expr(ty: Ty, depth: u32) -> BoxedStrategy<String> {
    let mut level = Level::of(leaf);
    for _ in 0..depth {
        level = Level::of(|t| composite(t, &level));
    }
    level.get(ty)
}

/// One strategy per type at one depth. Each depth is built once from the one below
/// and shared, so building `expr(ty, depth)` (which `any_rule` does for every case)
/// is linear in `depth`.
struct Level {
    bool: BoxedStrategy<String>,
    int: BoxedStrategy<String>,
    float: BoxedStrategy<String>,
    str: BoxedStrategy<String>,
}

impl Level {
    fn of(f: impl Fn(Ty) -> BoxedStrategy<String>) -> Self {
        Level { bool: f(Ty::Bool), int: f(Ty::Int), float: f(Ty::Float), str: f(Ty::Str) }
    }

    fn get(&self, ty: Ty) -> BoxedStrategy<String> {
        match ty {
            Ty::Bool => self.bool.clone(),
            Ty::Int => self.int.clone(),
            Ty::Float => self.float.clone(),
            Ty::Str => self.str.clone(),
        }
    }
}

/// An expression of type `ty` whose operands come from `below`.
fn composite(ty: Ty, below: &Level) -> BoxedStrategy<String> {
    let sub = |t: Ty| below.get(t);
    let cond = (sub(Ty::Bool), sub(ty), sub(ty)).prop_map(|(c, a, b)| format!("({c} ? {a} : {b})"));
    match ty {
        Ty::Int => prop_oneof![
            leaf(Ty::Int),
            leaf(Ty::Int),
            (sub(Ty::Int), proptest::sample::select(&["+", "-", "*", "%"][..]), sub(Ty::Int))
                .prop_map(|(a, op, b)| format!("({a} {op} {b})")),
            sub(Ty::Int).prop_map(|a| format!("(-{a})")),
            sub(Ty::Int).prop_map(|a| format!("abs({a})")),
            (sub(Ty::Int), sub(Ty::Int)).prop_map(|(a, b)| format!("max({a}, {b})")),
            sub(Ty::Str).prop_map(|s| format!("len({s})")),
            sub(Ty::Bool).prop_map(|p| format!("count(nums, {p} && # > 2)")),
            sub(Ty::Bool).prop_map(|p| format!("findLastIndex(nums, {p} && # > 2)")),
            (sub(Ty::Int), sub(Ty::Int)).prop_map(|(k, d)| format!("(findLast(nums, # > {k}) ?? {d})")),
            sub(Ty::Int).prop_map(|k| format!("len(groupBy(nums, string(# % {k})))")),
            (sub(Ty::Int), sort_order(sub(Ty::Str)))
                .prop_map(|(k, o)| format!("(get(sortBy(nums, # % {k}, {o}), 0) ?? 0)")),
            (sub(Ty::Int), sub(Ty::Int)).prop_map(|(k, init)| format!("reduce(nums, #acc + # * {k}, {init})")),
            sub(Ty::Int).prop_map(|k| format!("reduce(nums, #acc - #index * {k})")),
            cond,
        ]
        .boxed(),
        Ty::Float => prop_oneof![
            leaf(Ty::Float),
            leaf(Ty::Float),
            (sub(Ty::Float), proptest::sample::select(&["+", "-", "*", "/"][..]), sub(Ty::Float))
                .prop_map(|(a, op, b)| format!("({a} {op} {b})")),
            // int promoted to float
            (sub(Ty::Float), sub(Ty::Int)).prop_map(|(a, b)| format!("({a} + {b})")),
            (sub(Ty::Int), sub(Ty::Int)).prop_map(|(a, b)| format!("({a} / {b})")),
            sub(Ty::Float).prop_map(|a| format!("floor({a})")),
            sub(Ty::Float).prop_map(|init| format!("reduce(order.Items, #acc + .Price * .Qty, {init})")),
            cond,
        ]
        .boxed(),
        Ty::Bool => prop_oneof![
            leaf(Ty::Bool),
            leaf(Ty::Bool),
            (sub(Ty::Bool), proptest::sample::select(&["&&", "||"][..]), sub(Ty::Bool))
                .prop_map(|(a, op, b)| format!("({a} {op} {b})")),
            sub(Ty::Bool).prop_map(|a| format!("(not {a})")),
            (sub(Ty::Int), proptest::sample::select(&["<", "<=", ">", ">=", "==", "!="][..]), sub(Ty::Int))
                .prop_map(|(a, op, b)| format!("({a} {op} {b})")),
            (sub(Ty::Int), sub(Ty::Int), sub(Ty::Int)).prop_map(|(a, b, c)| format!("({a} < {b} <= {c})")),
            (sub(Ty::Float), sub(Ty::Int)).prop_map(|(a, b)| format!("({a} >= {b})")),
            sub(Ty::Int).prop_map(|a| format!("({a} in nums)")),
            sub(Ty::Int).prop_map(|a| format!("({a} in 1..5)")),
            sub(Ty::Str).prop_map(|s| format!("({s} in user.Tags)")),
            (sub(Ty::Str), proptest::sample::select(&["contains", "startsWith", "endsWith", "=="][..]), sub(Ty::Str))
                .prop_map(|(a, op, b)| format!("({a} {op} {b})")),
            sub(Ty::Int).prop_map(|k| format!("all(nums, # < {k})")),
            sub(Ty::Float).prop_map(|k| format!("any(order.Items, .Price > {k})")),
            cond,
        ]
        .boxed(),
        Ty::Str => prop_oneof![
            leaf(Ty::Str),
            leaf(Ty::Str),
            (sub(Ty::Str), sub(Ty::Str)).prop_map(|(a, b)| format!("({a} + {b})")),
            sub(Ty::Str).prop_map(|a| format!("upper({a})")),
            sub(Ty::Str).prop_map(|a| format!("trim({a})")),
            sub(Ty::Int).prop_map(|a| format!("string({a})")),
            (sub(Ty::Str), sub(Ty::Int)).prop_map(|(s, i)| format!("{s}[{i}:]")),
            (sub(Ty::Str), sort_order(sub(Ty::Str)))
                .prop_map(|(s, o)| format!("join(sortBy(user.Tags, {s} + #, {o}))")),
            sub(Ty::Str).prop_map(|init| format!("reduce(user.Tags, #acc + #, {init})")),
            (sub(Ty::Str), sub(Ty::Str)).prop_map(|(s, d)| format!("(findLast(user.Tags, # != {s}) ?? {d})")),
            cond,
        ]
        .boxed(),
    }
}

/// A `sortBy` order: mostly a valid one, sometimes any string (usually invalid).
fn sort_order(s: BoxedStrategy<String>) -> BoxedStrategy<String> {
    prop_oneof![Just("\"asc\"".to_owned()), Just("\"desc\"".to_owned()), s].boxed()
}

/// A whole rule: zero to two `let` bindings, then a body of type `ty`.
///
/// The body does not name the bindings. They still matter: `let` is evaluated
/// eagerly and in order, so a binding that fails is the rule's error.
pub fn rule(ty: Ty, depth: u32) -> BoxedStrategy<String> {
    let binding = proptest::sample::select(&[Ty::Int, Ty::Bool, Ty::Str][..])
        .prop_flat_map(move |t| expr(t, depth.saturating_sub(1)));
    (proptest::collection::vec(binding, 0..=2), expr(ty, depth))
        .prop_map(|(lets, body)| {
            let mut out = String::new();
            for (i, src) in lets.iter().enumerate() {
                out.push_str(&format!("let v{i} = {src}; "));
            }
            out.push_str(&body);
            out
        })
        .boxed()
}

/// Any rule type with its source.
pub fn any_rule(depth: u32) -> BoxedStrategy<(Ty, String)> {
    proptest::sample::select(&[Ty::Bool, Ty::Int, Ty::Float, Ty::Str][..])
        .prop_flat_map(move |t| rule(t, depth).prop_map(move |src| (t, src)))
        .boxed()
}

/// Text that is mostly dictum tokens, for "never panics" properties. Most of it does
/// not parse.
pub fn token_soup() -> BoxedStrategy<String> {
    const TOKENS: &[&str] = &[
        "user", ".", "Age", "?.", "[", "]", "(", ")", "{", "}", ",", ":", "?", "??", "|", "#", "#index", "#acc", "+",
        "-", "*", "/", "%", "**", "==", "!=", "<", "<=", "..", "&&", "||", "not", "in", "let", "=", ";", "if", "else",
        "1", "0x", "1e", "\"", "'", "`", "\\", "é", "nums", "all", "len", "matches", " ",
    ];
    proptest::collection::vec(proptest::sample::select(TOKENS), 0..40).prop_map(|t| t.concat()).boxed()
}

/// Sources in the operator language the Lean reference parser covers
/// (`formal/lean/Dictum/Syntax.lean`), for the parser differential test.
///
/// Unlike [`expr`], these are not parenthesized to fix their meaning: each operand
/// is wrapped in parentheses only by chance, so the precedence table decides the
/// tree, and many sources are syntax errors (`a == b == c`, `a ?? b + c`). Both
/// parsers must agree on which sources parse and on the tree. Tokens are separated
/// by spaces, so the lexers' handling of adjacent tokens is not compared here.
pub fn operator_source(depth: u32) -> BoxedStrategy<String> {
    let binding = operator_expr(depth.saturating_sub(1)).prop_map(|v| format!("let v = {v}; "));
    (proptest::option::weighted(0.2, binding), operator_expr(depth))
        .prop_map(|(lets, body)| lets.unwrap_or_default() + &body)
        .boxed()
}

const OPERATOR_ATOMS: &[&str] = &["a", "b", "c", "x", "0", "1", "42", "true", "false", "nil", "$env"];
const BINARY_OPS: &[&str] = &[
    "+",
    "-",
    "*",
    "/",
    "%",
    "**",
    "^",
    "??",
    "==",
    "!=",
    "<",
    ">",
    "<=",
    ">=",
    "in",
    "not in",
    "matches",
    "contains",
    "startsWith",
    "endsWith",
    "..",
    "&&",
    "||",
    "and",
    "or",
];
const POSTFIXES: &[&str] =
    &[" .b", " ?.c", " [0]", " ?.[x]", " [1:]", " [:2]", " [:]", " [a:b]", " .m ( )", " .m ( a )", " ?.m ( a , b )"];

fn paren(wrap: bool, s: String) -> String {
    if wrap { format!("( {s} )") } else { s }
}

fn operator_expr(depth: u32) -> BoxedStrategy<String> {
    let atom = proptest::sample::select(OPERATOR_ATOMS).prop_map(str::to_owned).boxed();
    if depth == 0 {
        return atom;
    }
    // Built once per level and cloned: the strategy tree stays linear in `depth`.
    let inner = operator_expr(depth - 1);
    let wrapped_once = (inner.clone(), any::<bool>()).prop_map(|(s, w)| paren(w, s)).boxed();
    let args_once = proptest::collection::vec(inner.clone(), 0..3).prop_map(|xs| xs.join(" , ")).boxed();
    let sub = move || inner.clone();
    let wrapped = move || wrapped_once.clone();
    let args = move || args_once.clone();
    prop_oneof![
        atom.clone(),
        atom,
        (wrapped(), proptest::sample::select(BINARY_OPS), wrapped()).prop_map(|(a, op, b)| format!("{a} {op} {b}")),
        (wrapped(), proptest::sample::select(BINARY_OPS), wrapped(), proptest::sample::select(BINARY_OPS), wrapped())
            .prop_map(|(a, p, b, q, c)| format!("{a} {p} {b} {q} {c}")),
        (proptest::sample::select(&["not", "!", "-", "+"][..]), wrapped()).prop_map(|(op, a)| format!("{op} {a}")),
        (wrapped(), wrapped(), wrapped()).prop_map(|(c, t, e)| format!("{c} ? {t} : {e}")),
        (wrapped(), proptest::sample::select(&["?:", "? :"][..]), wrapped())
            .prop_map(|(a, op, b)| format!("{a} {op} {b}")),
        (sub(), sub(), sub(), any::<bool>()).prop_map(|(c, t, e, braces)| if braces {
            format!("if {c} {{ {t} }} else {{ {e} }}")
        } else {
            format!("if {c} {{ {t} }} else {e}")
        }),
        (wrapped(), proptest::sample::select(POSTFIXES)).prop_map(|(a, p)| format!("{a}{p}")),
        (proptest::sample::select(&["f", "g"][..]), args()).prop_map(|(f, xs)| format!("{f} ( {xs} )")),
        (wrapped(), args()).prop_map(|(a, xs)| format!("{a} | f ( {xs} )")),
        args().prop_map(|xs| format!("[ {xs} ]")),
        (sub(), sub()).prop_map(|(k, v)| format!("{{ k : {k} , ( x ) : {v} }}")),
    ]
    .boxed()
}

/// Token sequences from the operator language, mostly not well formed, for the
/// parser differential test: both parsers must reject the same ones.
pub fn operator_soup() -> BoxedStrategy<String> {
    const TOKENS: &[&str] = &[
        "a", "b", "f", "1", "0", "true", "nil", "$env", "(", ")", "[", "]", "{", "}", ",", ":", "?", "?:", "??", "?.",
        "?.[", ".", "|", "+", "-", "*", "/", "%", "**", "^", "==", "!=", "<", "<=", ">", ">=", "..", "&&", "||", "and",
        "or", "not", "!", "in", "matches", "if", "else", "let", "=", ";",
    ];
    proptest::collection::vec(proptest::sample::select(TOKENS), 0..30).prop_map(|t| t.join(" ")).boxed()
}

/// A well-typed expression of the Lean core fragment (`formal/lean/Dictum/Core.lean`),
/// as dictum source over [`Facts`](crate::fixture::Facts) and as the oracle's
/// s-expression, for the evaluation differential test.
///
/// Field `$0` is `user.Age`, `$1` is `limit`, `$2` is `user.Active`; see
/// [`core_env`]. Every composite is parenthesized in the source.
pub fn core_expr(ty: Ty, depth: u32) -> BoxedStrategy<(String, String)> {
    let leaf: BoxedStrategy<(String, String)> = match ty {
        Ty::Int => prop_oneof![
            (0i64..=10).prop_map(|n| (n.to_string(), n.to_string())),
            // Zero again, so `%` divides by zero often enough to compare that error too.
            Just(("0".to_owned(), "0".to_owned())),
            proptest::sample::select(&[i64::MAX, 1 << 62, 3_037_000_500][..])
                .prop_map(|n| (n.to_string(), n.to_string())),
            proptest::sample::select(&[("user.Age", "$0"), ("limit", "$1")][..])
                .prop_map(|(s, e)| (s.to_owned(), e.to_owned())),
        ]
        .boxed(),
        _ => prop_oneof![
            any::<bool>().prop_map(|b| (b.to_string(), b.to_string())),
            Just(("user.Active".to_owned(), "$2".to_owned())),
        ]
        .boxed(),
    };
    if depth == 0 {
        return leaf;
    }
    let (ints, bools) = (core_expr(Ty::Int, depth - 1), core_expr(Ty::Bool, depth - 1));
    let sub = move |t: Ty| if t == Ty::Int { ints.clone() } else { bools.clone() };
    let cond = (sub(Ty::Bool), sub(ty), sub(ty))
        .prop_map(|((c, cc), (t, tt), (e, ee))| (format!("({c} ? {t} : {e})"), format!("(? {cc} {tt} {ee})")));
    match ty {
        Ty::Int => prop_oneof![
            leaf.clone(),
            leaf,
            (sub(Ty::Int), proptest::sample::select(&["+", "-", "*", "%"][..]), sub(Ty::Int))
                .prop_map(|((a, aa), op, (b, bb))| (format!("({a} {op} {b})"), format!("({op} {aa} {bb})"))),
            cond,
        ]
        .boxed(),
        _ => prop_oneof![
            leaf.clone(),
            leaf,
            (sub(Ty::Int), proptest::sample::select(&["==", "<"][..]), sub(Ty::Int))
                .prop_map(|((a, aa), op, (b, bb))| (format!("({a} {op} {b})"), format!("({op} {aa} {bb})"))),
            (sub(Ty::Bool), sub(Ty::Bool))
                .prop_map(|((a, aa), (b, bb))| (format!("({a} == {b})"), format!("(== {aa} {bb})"))),
            (sub(Ty::Bool), proptest::sample::select(&["&&", "||"][..]), sub(Ty::Bool))
                .prop_map(|((a, aa), op, (b, bb))| (format!("({a} {op} {b})"), format!("({op} {aa} {bb})"))),
            sub(Ty::Bool).prop_map(|(a, aa)| (format!("(not {a})"), format!("(not {aa})"))),
            cond,
        ]
        .boxed(),
    }
}

/// An expression of the core fragment that may be ill typed (`true + 1`,
/// `user.Age && limit`, `1 ? 2 : 3`), as dictum source and as the oracle's
/// s-expression, for the type checker differential test. The fields are those of
/// [`core_expr`]; see [`core_types`].
pub fn core_untyped(depth: u32) -> BoxedStrategy<(String, String)> {
    let leaf = prop_oneof![
        (0i64..=3).prop_map(|n| (n.to_string(), n.to_string())),
        any::<bool>().prop_map(|b| (b.to_string(), b.to_string())),
        proptest::sample::select(&[("user.Age", "$0"), ("limit", "$1"), ("user.Active", "$2")][..])
            .prop_map(|(s, e)| (s.to_owned(), e.to_owned())),
    ]
    .boxed();
    if depth == 0 {
        return leaf;
    }
    let sub = core_untyped(depth - 1);
    prop_oneof![
        leaf.clone(),
        leaf,
        (sub.clone(), proptest::sample::select(&["+", "-", "*", "%", "==", "<", "&&", "||"][..]), sub.clone())
            .prop_map(|((a, aa), op, (b, bb))| (format!("({a} {op} {b})"), format!("({op} {aa} {bb})"))),
        sub.clone().prop_map(|(a, aa)| (format!("(not {a})"), format!("(not {aa})"))),
        (sub.clone(), sub.clone(), sub)
            .prop_map(|((c, cc), (t, tt), (e, ee))| (format!("({c} ? {t} : {e})"), format!("(? {cc} {tt} {ee})"))),
    ]
    .boxed()
}

/// The oracle's field types for [`core_expr`] and [`core_untyped`].
pub const fn core_types() -> [crate::oracle::CoreTy; 3] {
    use crate::oracle::CoreTy;
    [CoreTy::Int, CoreTy::Int, CoreTy::Bool]
}

/// The oracle's fact root for [`core_expr`]: the fields it names, from `facts`.
pub fn core_env(facts: &crate::Facts) -> [crate::oracle::CoreVal; 3] {
    use crate::oracle::CoreVal;
    [CoreVal::Int(facts.user.age), CoreVal::Int(facts.limit), CoreVal::Bool(facts.user.active)]
}
