//! Parser tests: one test per row of the precedence table in docs/dictum-design.md,
//! the desugarings, and error recovery. Trees are compared as s-expressions so a
//! failure shows the shape that was parsed.
// Test code: the workspace's no-panic lints are for evaluation code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use dictum_syntax::ast::{
    BinaryOp, Callee, CompareOp, Expr, ExprKind, LogicOp, MapKey, MemberKey, Pointer, Program, UnaryOp,
};
use dictum_syntax::{ErrorCode, parse};

fn binary(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "%",
        BinaryOp::Pow => "**",
        BinaryOp::Coalesce => "??",
        BinaryOp::Eq => "==",
        BinaryOp::Ne => "!=",
        BinaryOp::In => "in",
        BinaryOp::NotIn => "notin",
        BinaryOp::Matches => "matches",
        BinaryOp::Contains => "contains",
        BinaryOp::StartsWith => "startsWith",
        BinaryOp::EndsWith => "endsWith",
        BinaryOp::Range => "..",
    }
}

fn sexpr(e: &Expr<'_>) -> String {
    let all = |xs: &[Expr<'_>]| xs.iter().map(sexpr).collect::<Vec<_>>().join(" ");
    match &e.kind {
        ExprKind::Nil => "nil".into(),
        ExprKind::Bool(b) => b.to_string(),
        ExprKind::Int(i) => i.to_string(),
        ExprKind::Float(f) => format!("{f:?}"),
        ExprKind::Str(s) => format!("{s:?}"),
        ExprKind::Bytes(b) => format!("b{b:?}"),
        ExprKind::Array(xs) => format!("[{}]", all(xs)),
        ExprKind::Map(pairs) => {
            let pairs: Vec<String> = pairs
                .iter()
                .map(|(k, v)| match k {
                    MapKey::Name(n, _) => format!("{n}: {}", sexpr(v)),
                    MapKey::Computed(k) => format!("({}): {}", sexpr(k), sexpr(v)),
                })
                .collect();
            format!("{{{}}}", pairs.join(", "))
        }
        ExprKind::Ident(name) => (*name).into(),
        ExprKind::Env => "$env".into(),
        ExprKind::Pointer(Pointer::Current) => "#".into(),
        ExprKind::Pointer(Pointer::Index) => "#index".into(),
        ExprKind::Pointer(Pointer::Acc) => "#acc".into(),
        ExprKind::Unary { op, expr } => {
            let op = match op {
                UnaryOp::Not => "not",
                UnaryOp::Neg => "neg",
                UnaryOp::Plus => "pos",
            };
            format!("({op} {})", sexpr(expr))
        }
        ExprKind::Binary { op, lhs, rhs } => format!("({} {} {})", binary(*op), sexpr(lhs), sexpr(rhs)),
        ExprKind::Logic { op, operands } => {
            format!("({} {})", if *op == LogicOp::And { "&&" } else { "||" }, all(operands))
        }
        ExprKind::Compare { first, rest } => {
            let mut out = format!("(cmp {}", sexpr(first));
            for (op, e) in rest {
                let op = match op {
                    CompareOp::Lt => "<",
                    CompareOp::Le => "<=",
                    CompareOp::Gt => ">",
                    CompareOp::Ge => ">=",
                };
                out.push_str(&format!(" {op} {}", sexpr(e)));
            }
            out + ")"
        }
        ExprKind::Member { obj, key, optional } => {
            let q = if *optional { "?" } else { "" };
            match key {
                MemberKey::Field(f) => format!("({q}. {} {})", sexpr(obj), f.name),
                MemberKey::Index(i) => format!("({q}[] {} {})", sexpr(obj), sexpr(i)),
            }
        }
        ExprKind::Slice { obj, from, to, optional } => {
            let b = |x: &Option<Box<Expr<'_>>>| x.as_deref().map(sexpr).unwrap_or_else(|| "_".into());
            format!("({}[:] {} {} {})", if *optional { "?" } else { "" }, sexpr(obj), b(from), b(to))
        }
        ExprKind::Call { callee, args, .. } => {
            let name = match callee {
                Callee::Builtin(b) => b.name(),
                Callee::Func(f) => f,
            };
            if args.is_empty() { format!("(call {name})") } else { format!("(call {name} {})", all(args)) }
        }
        ExprKind::Predicate(body) => format!("{{{}}}", sexpr(body)),
        ExprKind::Conditional { cond, then, otherwise } => {
            format!("(? {} {} {})", sexpr(cond), sexpr(then), sexpr(otherwise))
        }
        ExprKind::Error => "<error>".into(),
    }
}

fn program(p: &Program<'_>) -> String {
    let mut parts: Vec<String> = p.lets.iter().map(|l| format!("(let {} {})", l.name.name, sexpr(&l.value))).collect();
    parts.push(sexpr(&p.body));
    parts.join(" ")
}

#[track_caller]
fn parses_as(src: &str, expected: &str) {
    match parse(src) {
        Ok(p) => assert_eq!(program(&p), expected, "source: {src}"),
        Err(d) => panic!("{src}: {:?}", d.finish().0),
    }
}

#[track_caller]
fn fails_with(src: &str, code: ErrorCode) -> usize {
    match parse(src) {
        Ok(p) => panic!("{src}: parsed as {}", program(&p)),
        Err(d) => {
            let (diags, dropped) = d.finish();
            assert_eq!(diags[0].code(), code, "{src}: {diags:?}");
            diags.len() + dropped
        }
    }
}

#[test]
#[ignore = "pending M1: parser"]
fn precedence_table_rows() {
    // 1: conditional, right-associative
    parses_as("a ? b : c ? d : e", "(? a b (? c d e))");
    parses_as("if a { 1 } else if b { 2 } else { 3 }", "(? a 1 (? b 2 3))");
    // 2: pipe is weaker than every binary operator
    parses_as("a || b | f()", "(call f (|| a b))");
    parses_as("a | f() ? x : y", "(? (call f a) x y)");
    // 3, 4: or / and, flattened
    parses_as("a || b && c", "(|| a (&& b c))");
    parses_as("a and b && c", "(&& a b c)");
    parses_as("a or b || c", "(|| a b c)");
    // 5: comparisons chain; equality does not
    parses_as("1 < a <= 3", "(cmp 1 < a <= 3)");
    parses_as("a + b < c * d", "(cmp (+ a b) < (* c d))");
    parses_as("a not in b", "(notin a b)");
    // 6: range
    parses_as("x in 1..3", "(in x (.. 1 3))");
    // 7, 9: additive and multiplicative, left-associative
    parses_as("1 + 2 * 3 - 4", "(- (+ 1 (* 2 3)) 4)");
    parses_as("7 % 3 * 2", "(* (% 7 3) 2)");
    // 8: not is weaker than * and stronger than ==
    parses_as("not a == b", "(== (not a) b)");
    parses_as("not a * b", "(not (* a b))");
    // 10, 11: unary minus is weaker than **, which is right-associative
    parses_as("-2 ** 2", "(neg (** 2 2))");
    parses_as("2 ** 3 ** 2", "(** 2 (** 3 2))");
    parses_as("2 ^ 3", "(** 2 3)");
    // 12: ??
    parses_as("a ?? b ?? c", "(?? (?? a b) c)");
    // 13: postfix binds tightest
    parses_as("-a.b", "(neg (. a b))");
    parses_as("a.b?.c[0]", "([] (?. (. a b) c) 0)");
    parses_as("x[1:]", "([:] x 1 _)");
    parses_as("x?.[:2]", "(?[:] x _ 2)");
}

#[test]
#[ignore = "pending M1: parser"]
fn desugarings() {
    parses_as("a | f(b)", "(call f a b)");
    parses_as("xs | filter(# > 1) | len()", "(call len (call filter xs {(cmp # > 1)}))");
    parses_as("all(xs, .Age > 1)", "(call all xs {(cmp (. # Age) > 1)})");
    parses_as("all(xs, {# > 1})", "(call all xs {(cmp # > 1)})");
    parses_as("a ?: b", "(|| a b)");
    parses_as("let x = 1; let y = x; x + y", "(let x 1) (let y x) (+ x y)");
    parses_as(r#"user["Name"]"#, r#"(. user Name)"#);
    parses_as("{a: 1, 'b': 2, (k): 3}", r#"{a: 1, b: 2, (k): 3}"#);
}

#[test]
#[ignore = "pending M1: parser"]
fn literals() {
    parses_as("0x2A + 0o7 + 0b11 + 1_000", "(+ (+ (+ 42 7) 3) 1000)");
    parses_as(".5 + 1e3", "(+ 0.5 1000.0)");
    parses_as(r#""a\n" + 'b' + `c\n`"#, r#"(+ (+ "a\n" "b") "c\\n")"#);
    parses_as(r#"b"\x00""#, "b[0]");
    parses_as("$env.user", "(. $env user)");
}

#[test]
#[ignore = "pending M1: parser"]
fn syntax_errors() {
    fails_with("1 ==", ErrorCode::UnexpectedToken);
    fails_with("1 == 1 == true", ErrorCode::ChainedEquality);
    fails_with("a ?? 0 + 1", ErrorCode::MixedCoalesce);
    fails_with("f(x)(y)", ErrorCode::NotCallable);
    fails_with("# > 1", ErrorCode::PointerOutsidePredicate);
    fails_with("9223372036854775808", ErrorCode::IntegerTooLarge);
}

#[test]
#[ignore = "pending M1: parser"]
fn recovery_reports_errors_in_several_places() {
    assert!(fails_with("[1, , 2] + (3 + )", ErrorCode::UnexpectedToken) >= 2);
}

#[test]
#[ignore = "pending M1: parser"]
fn depth_is_bounded_without_overflowing_the_stack() {
    let deep = format!("{}1{}", "(".repeat(10_000), ")".repeat(10_000));
    fails_with(&deep, ErrorCode::TooDeep);
    let unary = format!("{}1", "-".repeat(10_000));
    fails_with(&unary, ErrorCode::TooDeep);
    // A long conjunction is one flattened node, not 1000 levels.
    let wide = vec!["a"; 1000].join(" && ");
    assert!(parse(&wide).is_ok());
}
