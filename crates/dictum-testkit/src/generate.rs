//! Random well-typed rule sources over [`Facts`](crate::fixture::Facts).
//!
//! The generator works on source text, not on the AST, so the parser and the checker
//! are part of every property that uses it. Every composite is parenthesized: the
//! generated text means exactly the tree that produced it, whatever the precedence
//! table says. Precedence is tested separately, by the parser's own tests.
//!
//! What it covers: arithmetic with overflow and division by zero, int-to-float
//! promotion, short-circuit logic, comparison chains, `in`, string builtins, field
//! reads (including optional ones through `??`), `let`, and the `all`/`any`/`count`
//! predicates. Errors are expected outcomes: a property compares two evaluations, so
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
    if depth == 0 {
        return leaf(ty);
    }
    let sub = move |t: Ty| expr(t, depth - 1);
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
            cond,
        ]
        .boxed(),
    }
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
