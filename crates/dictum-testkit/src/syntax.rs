//! The AST as an s-expression, so a test failure shows the shape that was parsed.
//!
//! The format is shared with the Lean reference parser (`formal/lean/Dictum/Syntax.lean`,
//! `Ast.sexpr`): the differential test compares the two strings, so a change here
//! needs the same change there.

use dictum_syntax::Span;
use dictum_syntax::ast::{
    BinaryOp, Callee, CompareOp, Expr, ExprKind, LogicOp, MapKey, MemberKey, Pointer, Program, UnaryOp,
};

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

/// One expression.
pub fn sexpr(e: &Expr<'_>) -> String {
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
        ExprKind::Method { obj, name, args, optional } => {
            let q = if *optional { "?" } else { "" };
            let args: String = args.iter().map(|a| format!(" {}", sexpr(a))).collect();
            format!("({q}.call {} {}{})", sexpr(obj), name.name, args)
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

/// A whole program: `(let x v)` for each binding, then the body.
pub fn program(p: &Program<'_>) -> String {
    let mut parts: Vec<String> = p.lets.iter().map(|l| format!("(let {} {})", l.name.name, sexpr(&l.value))).collect();
    parts.push(sexpr(&p.body));
    parts.join(" ")
}

/// Every span in the program (nodes, names, keys), for the invariant that they all
/// lie inside the source on character boundaries.
pub fn spans(p: &Program<'_>) -> Vec<Span> {
    let mut out = Vec::new();
    for l in &p.lets {
        out.push(l.name.span);
        expr_spans(&l.value, &mut out);
    }
    expr_spans(&p.body, &mut out);
    out
}

fn expr_spans(e: &Expr<'_>, out: &mut Vec<Span>) {
    out.push(e.span);
    match &e.kind {
        ExprKind::Array(xs) | ExprKind::Logic { operands: xs, .. } => xs.iter().for_each(|x| expr_spans(x, out)),
        ExprKind::Call { callee_span, args, .. } => {
            out.push(*callee_span);
            args.iter().for_each(|x| expr_spans(x, out));
        }
        ExprKind::Map(pairs) => {
            for (k, v) in pairs {
                match k {
                    MapKey::Name(_, span) => out.push(*span),
                    MapKey::Computed(k) => expr_spans(k, out),
                }
                expr_spans(v, out);
            }
        }
        ExprKind::Unary { expr, .. } | ExprKind::Predicate(expr) => expr_spans(expr, out),
        ExprKind::Binary { lhs, rhs, .. } => {
            expr_spans(lhs, out);
            expr_spans(rhs, out);
        }
        ExprKind::Compare { first, rest } => {
            expr_spans(first, out);
            rest.iter().for_each(|(_, x)| expr_spans(x, out));
        }
        ExprKind::Member { obj, key, .. } => {
            expr_spans(obj, out);
            match key {
                MemberKey::Field(f) => out.push(f.span),
                MemberKey::Index(i) => expr_spans(i, out),
            }
        }
        ExprKind::Method { obj, name, args, .. } => {
            expr_spans(obj, out);
            out.push(name.span);
            args.iter().for_each(|x| expr_spans(x, out));
        }
        ExprKind::Slice { obj, from, to, .. } => {
            expr_spans(obj, out);
            from.iter().chain(to.iter()).for_each(|x| expr_spans(x, out));
        }
        ExprKind::Conditional { cond, then, otherwise } => {
            expr_spans(cond, out);
            expr_spans(then, out);
            expr_spans(otherwise, out);
        }
        ExprKind::Nil
        | ExprKind::Bool(_)
        | ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Bytes(_)
        | ExprKind::Ident(_)
        | ExprKind::Env
        | ExprKind::Pointer(_)
        | ExprKind::Error => {}
    }
}
