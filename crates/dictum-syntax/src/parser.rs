//! Hand-written Pratt parser. Private to this crate.
//!
//! Binding powers (low to high), all taken from Go expr-lang's `operator.go` and
//! `parser.go` (main, 2026-07):
//!
//! | bp  | operators                                                   | assoc        |
//! |-----|-------------------------------------------------------------|--------------|
//! | -   | `? :`, `?:`, `if else` (after all binary operators)         | right        |
//! | 0   | `\|` (pipe)                                                 | left         |
//! | 10  | `or` `\|\|`                                                 | left, n-ary  |
//! | 15  | `and` `&&`                                                  | left, n-ary  |
//! | 20  | `<` `>` `<=` `>=`                                           | chain        |
//! | 20  | `==` `!=` `in` `not in` `matches` `contains` `startsWith` `endsWith` | none |
//! | 25  | `..`                                                        | none         |
//! | 30  | `+` `-`                                                     | left         |
//! | 50  | prefix `not` `!`                                            | prefix       |
//! | 60  | `*` `/` `%`                                                 | left         |
//! | 90  | prefix `-` `+`                                              | prefix       |
//! | 100 | `**` `^`                                                    | right        |
//! | 500 | `??`                                                        | left, unmixable |
//! | -   | `.` `?.` `[]` `[:]` `()`                                    | postfix      |
//!
//! As in Go's `parseExpression`, a prefix operator parses its operand with its own
//! binding power, and the binary loop then continues with the caller's. So
//! `not a == b` is `(not a) == b` and `-2 ** 2` is `-(2 ** 2)`. The pipe is part of
//! the binary loop at bp 0: `1 + 2 | f()` is `f(1 + 2)`, and `a | f() + 1` is
//! `f(a) + 1`. The Lean reference parser (`formal/lean/Dictum/Syntax.lean`) states
//! the same rules independently; `tests/drt.rs` compares the two.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::ast::{
    BinaryOp, Callee, CompareOp, Expr, ExprKind, Ident, Let, LogicOp, MapKey, MemberKey, Pointer, Program, UnaryOp,
};
use crate::lexer::{Lexer, Token};
use crate::{Arg, Builtin, Diagnostic, Diagnostics, ErrorCode, Help, MAX_DEPTH, Span};

/// Where the parser is, for rules that depend on context.
#[derive(Clone, Copy, Default)]
struct Context {
    /// Inside a predicate argument: `#`, `#index`, `#acc`, `.Field` are allowed.
    in_predicate: bool,
    /// Current recursion depth; exceeding `MAX_DEPTH` is E0105.
    depth: u32,
}

/// How a node was written, for the `??` mixing rule.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bare {
    /// Not a binary operator, or one written in parentheses.
    No,
    /// A `??` without parentheses.
    Coalesce,
    /// Another binary operator without parentheses, by its source text.
    Binary(&'static str),
}

/// A parsed expression with what the parser needs to know about it.
struct Parsed<'src> {
    expr: Expr<'src>,
    bare: Bare,
    /// Tree height; a flattened `&&`/`||` chain or comparison chain counts once.
    height: u32,
}

/// Which binary operator a token is.
#[derive(Clone, Copy)]
enum Infix {
    Pipe,
    Coalesce,
    Logic(LogicOp, u16),
    Relational(CompareOp),
    Equality(BinaryOp),
    Range,
    /// Left-associative (`+ - * / %`) or right-associative (`**`).
    Arith(BinaryOp, u16),
}

fn infix(tok: &Token<'_>) -> Option<Infix> {
    let Token::Punct(p) = tok else { return None };
    Some(match *p {
        "|" => Infix::Pipe,
        "??" => Infix::Coalesce,
        "||" | "or" => Infix::Logic(LogicOp::Or, 10),
        "&&" | "and" => Infix::Logic(LogicOp::And, 15),
        "<" => Infix::Relational(CompareOp::Lt),
        "<=" => Infix::Relational(CompareOp::Le),
        ">" => Infix::Relational(CompareOp::Gt),
        ">=" => Infix::Relational(CompareOp::Ge),
        "==" => Infix::Equality(BinaryOp::Eq),
        "!=" => Infix::Equality(BinaryOp::Ne),
        "in" => Infix::Equality(BinaryOp::In),
        "not in" => Infix::Equality(BinaryOp::NotIn),
        "matches" => Infix::Equality(BinaryOp::Matches),
        "contains" => Infix::Equality(BinaryOp::Contains),
        "startsWith" => Infix::Equality(BinaryOp::StartsWith),
        "endsWith" => Infix::Equality(BinaryOp::EndsWith),
        ".." => Infix::Range,
        "+" => Infix::Arith(BinaryOp::Add, 30),
        "-" => Infix::Arith(BinaryOp::Sub, 30),
        "*" => Infix::Arith(BinaryOp::Mul, 60),
        "/" => Infix::Arith(BinaryOp::Div, 60),
        "%" => Infix::Arith(BinaryOp::Mod, 60),
        "**" | "^" => Infix::Arith(BinaryOp::Pow, 100),
        _ => return None,
    })
}

/// Comparison-level operators (bp 20), which `a op b op c` may not mix except as a
/// `<`/`>` chain.
fn is_comparison(tok: &Token<'_>) -> bool {
    matches!(infix(tok), Some(Infix::Relational(_) | Infix::Equality(_)))
}

pub(crate) struct Parser<'src> {
    src: &'src str,
    lexer: Lexer<'src>,
    diags: Diagnostics,
    tok: Token<'src>,
    span: Span,
    /// End of the last consumed token, for node spans.
    prev_end: u32,
    /// E0105 is reported once per rule; everything deeper is the same error.
    too_deep: bool,
}

impl<'src> Parser<'src> {
    pub(crate) fn new(src: &'src str) -> Self {
        Parser {
            src,
            lexer: Lexer::new(src),
            diags: Diagnostics::new(),
            tok: Token::Eof,
            span: Span { start: 0, end: 0 },
            prev_end: 0,
            too_deep: false,
        }
    }

    /// `program = { "let" ident "=" expr ";" } expr EOF`
    pub(crate) fn program(mut self) -> Result<Program<'src>, Diagnostics> {
        if u32::try_from(self.src.len()).is_err() {
            self.diags.push(Diagnostic::new(ErrorCode::SourceTooLarge, Span { start: 0, end: 0 }, Vec::new()));
            return Err(self.diags);
        }
        self.bump();
        let ctx = Context::default();
        let mut lets = Vec::new();
        while self.at("let") {
            self.bump();
            let name = match self.tok {
                Token::Ident(name) => {
                    let span = self.span;
                    self.bump();
                    Ident { name, span }
                }
                _ => {
                    self.unexpected("an identifier");
                    Ident { name: "", span: self.span }
                }
            };
            self.expect("=");
            let value = self.expr(ctx).expr;
            self.expect(";");
            lets.push(Let { name, value });
        }
        let body = self.expr(ctx).expr;
        if self.tok != Token::Eof {
            self.unexpected("end of input");
        }
        if self.diags.is_empty() { Ok(Program { lets, body }) } else { Err(self.diags) }
    }

    // ---- tokens -------------------------------------------------------------

    fn bump(&mut self) {
        self.prev_end = self.span.end;
        let (tok, span) = self.lexer.next(&mut self.diags);
        self.tok = tok;
        self.span = span;
    }

    fn at(&self, p: &str) -> bool {
        matches!(self.tok, Token::Punct(q) if q == p)
    }

    fn eat(&mut self, p: &str) -> bool {
        let found = self.at(p);
        if found {
            self.bump();
        }
        found
    }

    fn found(&self) -> String {
        match self.tok {
            Token::Eof => "end of input".into(),
            _ => format!("`{}`", self.span.text(self.src).unwrap_or("")),
        }
    }

    fn unexpected(&mut self, expected: &str) {
        let args = vec![Arg::Name(expected.into()), Arg::Name(self.found().into())];
        self.diags.push(Diagnostic::new(ErrorCode::UnexpectedToken, self.span, args));
    }

    fn expect(&mut self, p: &str) {
        if !self.eat(p) {
            self.unexpected(&format!("`{p}`"));
        }
    }

    /// Expect a closing bracket. If something else is there, report it, skip to the
    /// next sync point and take the bracket if that is where the skipping stopped.
    fn expect_closing(&mut self, p: &str) {
        if self.eat(p) {
            return;
        }
        self.unexpected(&format!("`{p}`"));
        self.skip_to_sync();
        self.eat(p);
    }

    fn span_from(&self, start: Span) -> Span {
        Span { start: start.start, end: self.prev_end.max(start.start) }
    }

    // ---- nodes --------------------------------------------------------------

    fn leaf(kind: ExprKind<'src>, span: Span) -> Parsed<'src> {
        Parsed { expr: Expr { kind, span }, bare: Bare::No, height: 1 }
    }

    /// A node over children of height `child_height`. Past `MAX_DEPTH` it is an
    /// error: every later pass recurses over the tree.
    fn node(&mut self, kind: ExprKind<'src>, span: Span, child_height: u32, bare: Bare) -> Parsed<'src> {
        let height = child_height.saturating_add(1);
        if height > MAX_DEPTH {
            self.report_too_deep(span);
            return Self::leaf(ExprKind::Error, span);
        }
        Parsed { expr: Expr { kind, span }, bare, height }
    }

    fn report_too_deep(&mut self, span: Span) {
        if !self.too_deep {
            self.too_deep = true;
            let args = vec![Arg::Int(i64::from(MAX_DEPTH))];
            self.diags.push(Diagnostic::new(ErrorCode::TooDeep, span, args));
        }
    }

    /// One level deeper in the recursion, or `None` (and E0105) past `MAX_DEPTH`.
    fn deeper(&mut self, ctx: Context) -> Option<Context> {
        let depth = ctx.depth.saturating_add(1);
        if depth > MAX_DEPTH {
            self.report_too_deep(self.span);
            None
        } else {
            Some(Context { depth, ..ctx })
        }
    }

    // ---- expressions --------------------------------------------------------

    /// `expr = conditional`: `if c { a } else { b }`, `c ? a : b`, `a ?: b`, or a
    /// binary expression.
    fn expr(&mut self, ctx: Context) -> Parsed<'src> {
        let start = self.span;
        let Some(ctx) = self.deeper(ctx) else { return self.recover(start) };
        if self.eat("if") {
            let cond = self.expr(ctx);
            self.expect("{");
            let then = self.expr(ctx);
            self.expect_closing("}");
            self.expect("else");
            let otherwise = if self.eat("{") {
                let e = self.expr(ctx);
                self.expect_closing("}");
                e
            } else {
                self.expr(ctx)
            };
            return self.conditional(start, cond, then, otherwise);
        }
        let lhs = self.binary(0, ctx);
        if self.eat("?") {
            if self.eat(":") {
                let rhs = self.expr(ctx);
                return self.elvis(start, lhs, rhs);
            }
            let then = self.expr(ctx);
            self.expect(":");
            let otherwise = self.expr(ctx);
            return self.conditional(start, lhs, then, otherwise);
        }
        if self.eat("?:") {
            let rhs = self.expr(ctx);
            return self.elvis(start, lhs, rhs);
        }
        Parsed { bare: Bare::No, ..lhs }
    }

    fn conditional(
        &mut self,
        start: Span,
        cond: Parsed<'src>,
        then: Parsed<'src>,
        otherwise: Parsed<'src>,
    ) -> Parsed<'src> {
        let h = cond.height.max(then.height).max(otherwise.height);
        let kind = ExprKind::Conditional {
            cond: Box::new(cond.expr),
            then: Box::new(then.expr),
            otherwise: Box::new(otherwise.expr),
        };
        self.node(kind, self.span_from(start), h, Bare::No)
    }

    /// `a ?: b` is `a ? a : b` with `a: bool`, which is `a || b`. A new node: it is
    /// not merged into an `||` chain on either side.
    fn elvis(&mut self, start: Span, lhs: Parsed<'src>, rhs: Parsed<'src>) -> Parsed<'src> {
        let h = lhs.height.max(rhs.height);
        let kind = ExprKind::Logic { op: LogicOp::Or, operands: vec![lhs.expr, rhs.expr] };
        self.node(kind, self.span_from(start), h, Bare::No)
    }

    /// The Pratt loop: operators with binding power at least `min_bp`.
    fn binary(&mut self, min_bp: u16, ctx: Context) -> Parsed<'src> {
        let start = self.span;
        let mut lhs = self.unary(ctx);
        while let Some(op) = infix(&self.tok) {
            let Token::Punct(text) = self.tok else { break };
            lhs = match op {
                Infix::Pipe => {
                    if min_bp > 0 {
                        break;
                    }
                    self.bump();
                    self.pipe(start, lhs, ctx)
                }
                Infix::Coalesce => {
                    if min_bp > 500 {
                        break;
                    }
                    self.bump();
                    let rhs = self.binary(501, ctx);
                    for side in [lhs.bare, rhs.bare] {
                        if let Bare::Binary(other) = side {
                            self.mixed_coalesce(start, other);
                        }
                    }
                    self.binary_node(start, BinaryOp::Coalesce, lhs, rhs, Bare::Coalesce)
                }
                Infix::Logic(op, bp) => {
                    if min_bp > bp {
                        break;
                    }
                    self.no_coalesce(start, &lhs, text);
                    self.logic(start, op, bp, lhs, ctx)
                }
                Infix::Relational(_) => {
                    if min_bp > 20 {
                        break;
                    }
                    self.no_coalesce(start, &lhs, text);
                    self.chain(start, lhs, ctx)
                }
                Infix::Equality(op) => {
                    if min_bp > 20 {
                        break;
                    }
                    self.no_coalesce(start, &lhs, text);
                    self.bump();
                    let rhs = self.binary(21, ctx);
                    self.no_coalesce(start, &rhs, text);
                    let node = self.binary_node(start, op, lhs, rhs, Bare::Binary(text));
                    if is_comparison(&self.tok) {
                        self.chained_equality();
                    }
                    node
                }
                Infix::Range => {
                    if min_bp > 25 {
                        break;
                    }
                    self.no_coalesce(start, &lhs, text);
                    self.bump();
                    let rhs = self.binary(26, ctx);
                    self.no_coalesce(start, &rhs, text);
                    let node = self.binary_node(start, BinaryOp::Range, lhs, rhs, Bare::Binary(text));
                    if self.at("..") {
                        self.unexpected("end of the range");
                    }
                    node
                }
                Infix::Arith(op, bp) => {
                    if min_bp > bp {
                        break;
                    }
                    self.no_coalesce(start, &lhs, text);
                    self.bump();
                    // `**` is right-associative: its right operand takes another `**`.
                    let rhs = self.binary(if op == BinaryOp::Pow { bp } else { bp.saturating_add(1) }, ctx);
                    self.no_coalesce(start, &rhs, text);
                    self.binary_node(start, op, lhs, rhs, Bare::Binary(text))
                }
            };
        }
        lhs
    }

    fn binary_node(
        &mut self,
        start: Span,
        op: BinaryOp,
        lhs: Parsed<'src>,
        rhs: Parsed<'src>,
        bare: Bare,
    ) -> Parsed<'src> {
        let h = lhs.height.max(rhs.height);
        let kind = ExprKind::Binary { op, lhs: Box::new(lhs.expr), rhs: Box::new(rhs.expr) };
        self.node(kind, self.span_from(start), h, bare)
    }

    fn no_coalesce(&mut self, start: Span, side: &Parsed<'src>, op: &'static str) {
        if side.bare == Bare::Coalesce {
            self.mixed_coalesce(start, op);
        }
    }

    fn mixed_coalesce(&mut self, start: Span, other: &'static str) {
        let span = Span { start: start.start, end: self.span.end.max(self.prev_end) };
        let d = Diagnostic::new(ErrorCode::MixedCoalesce, span, vec![Arg::Name(other.into())]);
        self.diags.push(d.with_help(Help::AddParentheses));
    }

    fn chained_equality(&mut self) {
        let text = self.span.text(self.src).unwrap_or("");
        let d = Diagnostic::new(ErrorCode::ChainedEquality, self.span, vec![Arg::Name(text.into())]);
        self.diags.push(d.with_help(Help::SplitComparison));
    }

    /// `a && b && c` as one node: operands at `bp + 1` while the same operator repeats.
    fn logic(&mut self, start: Span, op: LogicOp, bp: u16, first: Parsed<'src>, ctx: Context) -> Parsed<'src> {
        let mut h = first.height;
        let mut operands = vec![first.expr];
        while matches!(infix(&self.tok), Some(Infix::Logic(o, _)) if o == op) {
            let Token::Punct(text) = self.tok else { break };
            self.bump();
            let rhs = self.binary(bp.saturating_add(1), ctx);
            self.no_coalesce(start, &rhs, text);
            h = h.max(rhs.height);
            operands.push(rhs.expr);
        }
        let text = if op == LogicOp::And { "&&" } else { "||" };
        self.node(ExprKind::Logic { op, operands }, self.span_from(start), h, Bare::Binary(text))
    }

    /// `a < b <= c` as one node. An equality operator after a comparison is E0106.
    fn chain(&mut self, start: Span, first: Parsed<'src>, ctx: Context) -> Parsed<'src> {
        let mut h = first.height;
        let mut rest = Vec::new();
        while let Some(Infix::Relational(op)) = infix(&self.tok) {
            let Token::Punct(text) = self.tok else { break };
            self.bump();
            let rhs = self.binary(21, ctx);
            self.no_coalesce(start, &rhs, text);
            h = h.max(rhs.height);
            rest.push((op, rhs.expr));
        }
        if is_comparison(&self.tok) {
            self.chained_equality();
        }
        let kind = ExprKind::Compare { first: Box::new(first.expr), rest };
        self.node(kind, self.span_from(start), h, Bare::Binary("<"))
    }

    /// After `|`: `name(args)`, with the piped value as argument 0.
    fn pipe(&mut self, start: Span, piped: Parsed<'src>, ctx: Context) -> Parsed<'src> {
        let Token::Ident(name) = self.tok else {
            self.unexpected("a function call");
            return self.recover(start);
        };
        let callee_span = self.span;
        self.bump();
        if !self.at("(") {
            self.unexpected("`(`");
            return self.recover(start);
        }
        self.call(start, name, callee_span, Some(piped), ctx)
    }

    /// `not x`, `!x`, `-x`, `+x`, or a postfix expression. The operand takes the
    /// prefix operator's own binding power.
    fn unary(&mut self, ctx: Context) -> Parsed<'src> {
        let start = self.span;
        let op = match self.tok {
            Token::Punct("not" | "!") => (UnaryOp::Not, 50),
            Token::Punct("-") => (UnaryOp::Neg, 90),
            Token::Punct("+") => (UnaryOp::Plus, 90),
            _ => return self.postfix(ctx),
        };
        let Some(ctx) = self.deeper(ctx) else { return self.recover(start) };
        self.bump();
        let operand = self.binary(op.1, ctx);
        let kind = ExprKind::Unary { op: op.0, expr: Box::new(operand.expr) };
        self.node(kind, self.span_from(start), operand.height, Bare::No)
    }

    /// `postfix = primary { "." ident | "?." ident | "[" index "]" | "?.[" index "]" }`
    ///
    /// `(` is accepted only directly after an identifier primary (a call). After any
    /// other postfix (`x.f(`, `x[0](`, `(f)(`) it is E0108.
    fn postfix(&mut self, ctx: Context) -> Parsed<'src> {
        let start = self.span;
        let mut e = self.primary(ctx);
        loop {
            let optional = self.at("?.") || self.at("?.[");
            if self.at(".") || self.at("?.") {
                self.bump();
                let Token::Ident(name) = self.tok else {
                    self.unexpected("a field name");
                    return e;
                };
                let key = MemberKey::Field(Ident { name, span: self.span });
                self.bump();
                let kind = ExprKind::Member { obj: Box::new(e.expr), key, optional };
                e = self.node(kind, self.span_from(start), e.height, Bare::No);
            } else if self.at("[") || self.at("?.[") {
                self.bump();
                e = self.index(start, e, optional, ctx);
            } else if self.at("(") {
                let d = Diagnostic::new(ErrorCode::NotCallable, self.span, Vec::new());
                self.diags.push(d);
                // Skip the arguments, so the error is reported once.
                self.bump();
                let _ = self.args(")", None, 0, ctx);
            } else {
                return e;
            }
        }
    }

    /// After `[`: `x[i]`, `x[a:b]`, `x[a:]`, `x[:b]`, `x[:]`.
    fn index(&mut self, start: Span, obj: Parsed<'src>, optional: bool, ctx: Context) -> Parsed<'src> {
        let from = if self.at(":") { None } else { Some(self.expr(ctx)) };
        if self.eat(":") {
            let to = if self.at("]") { None } else { Some(self.expr(ctx)) };
            self.expect_closing("]");
            let h = [Some(&obj), from.as_ref(), to.as_ref()].into_iter().flatten().map(|p| p.height).max().unwrap_or(0);
            let kind = ExprKind::Slice {
                obj: Box::new(obj.expr),
                from: from.map(|p| Box::new(p.expr)),
                to: to.map(|p| Box::new(p.expr)),
                optional,
            };
            return self.node(kind, self.span_from(start), h, Bare::No);
        }
        self.expect_closing("]");
        let Some(index) = from else { return obj };
        let h = obj.height.max(index.height);
        // `x["name"]` is `x.name`: one node kind for both, resolved by the checker.
        let key = match index.expr.kind {
            ExprKind::Str(Cow::Borrowed(name)) => {
                let inner =
                    Span { start: index.expr.span.start.saturating_add(1), end: index.expr.span.end.saturating_sub(1) };
                MemberKey::Field(Ident { name, span: inner })
            }
            _ => MemberKey::Index(Box::new(index.expr)),
        };
        let kind = ExprKind::Member { obj: Box::new(obj.expr), key, optional };
        self.node(kind, self.span_from(start), h, Bare::No)
    }

    fn primary(&mut self, ctx: Context) -> Parsed<'src> {
        let start = self.span;
        let kind = match &self.tok {
            Token::Ident(name) => {
                let name = *name;
                self.bump();
                if self.at("(") {
                    return self.call(start, name, start, None, ctx);
                }
                return Self::leaf(ExprKind::Ident(name), start);
            }
            Token::Int(n) => ExprKind::Int(*n),
            Token::Float(f) => ExprKind::Float(*f),
            Token::Str(s) => ExprKind::Str(s.clone()),
            Token::Bytes(b) => ExprKind::Bytes(b.clone()),
            Token::Env => ExprKind::Env,
            Token::Punct("true") => ExprKind::Bool(true),
            Token::Punct("false") => ExprKind::Bool(false),
            Token::Punct("nil") => ExprKind::Nil,
            Token::Pointer(p) => {
                let p = *p;
                if !ctx.in_predicate {
                    let name = match p {
                        Pointer::Current => "#",
                        Pointer::Index => "#index",
                        Pointer::Acc => "#acc",
                    };
                    self.outside_predicate(name);
                }
                ExprKind::Pointer(p)
            }
            Token::Punct(".") => return self.field_shorthand(ctx),
            Token::Punct("(") => {
                self.bump();
                let inner = self.expr(ctx);
                self.expect_closing(")");
                return Parsed { bare: Bare::No, ..inner };
            }
            Token::Punct("[") => {
                self.bump();
                let items = self.args("]", None, 0, ctx);
                let h = items.iter().map(|p| p.height).max().unwrap_or(0);
                let kind = ExprKind::Array(items.into_iter().map(|p| p.expr).collect());
                return self.node(kind, self.span_from(start), h, Bare::No);
            }
            Token::Punct("{") => {
                self.bump();
                return self.map(start, ctx);
            }
            _ => {
                self.unexpected("an expression");
                return self.recover(start);
            }
        };
        self.bump();
        Self::leaf(kind, start)
    }

    fn outside_predicate(&mut self, name: &str) {
        let d = Diagnostic::new(ErrorCode::PointerOutsidePredicate, self.span, vec![Arg::Name(name.into())]);
        self.diags.push(d);
    }

    /// `.Field` inside a predicate is `#.Field`.
    fn field_shorthand(&mut self, ctx: Context) -> Parsed<'src> {
        let start = self.span;
        if !ctx.in_predicate {
            self.outside_predicate(".Field");
        }
        self.bump();
        let Token::Ident(name) = self.tok else {
            self.unexpected("a field name");
            return self.recover(start);
        };
        let key = MemberKey::Field(Ident { name, span: self.span });
        self.bump();
        let current = Expr { kind: ExprKind::Pointer(Pointer::Current), span: start };
        let kind = ExprKind::Member { obj: Box::new(current), key, optional: false };
        self.node(kind, self.span_from(start), 1, Bare::No)
    }

    /// `name(args)`, the current token being `(`. A piped value is argument 0.
    fn call(
        &mut self,
        start: Span,
        name: &'src str,
        callee_span: Span,
        piped: Option<Parsed<'src>>,
        ctx: Context,
    ) -> Parsed<'src> {
        let builtin = Builtin::from_name(name);
        let callee = builtin.map_or(Callee::Func(name), Callee::Builtin);
        self.bump();
        let first = usize::from(piped.is_some());
        let mut args: Vec<Parsed<'src>> = piped.into_iter().collect();
        args.extend(self.args(")", builtin.and_then(Builtin::predicate_arg), first, ctx));
        let h = args.iter().map(|p| p.height).max().unwrap_or(0);
        let kind = ExprKind::Call { callee, callee_span, args: args.into_iter().map(|p| p.expr).collect() };
        self.node(kind, self.span_from(start), h, Bare::No)
    }

    /// Arguments of `callee(...)` (or the items of `[...]`) up to `close`.
    /// If `callee` is a builtin with a predicate position and this is that position,
    /// parse with `in_predicate = true`; a leading `{` there opens predicate braces (a
    /// map literal as a predicate body is `{{a: #}}`).
    fn args(
        &mut self,
        close: &str,
        predicate_at: Option<usize>,
        first_index: usize,
        ctx: Context,
    ) -> Vec<Parsed<'src>> {
        let mut out = Vec::new();
        loop {
            if self.eat(close) {
                return out;
            }
            let index = first_index.saturating_add(out.len());
            let arg = if predicate_at == Some(index) {
                let start = self.span;
                let inner = Context { in_predicate: true, ..ctx };
                let body = if self.eat("{") {
                    let b = self.expr(inner);
                    self.expect_closing("}");
                    b
                } else {
                    self.expr(inner)
                };
                let h = body.height;
                self.node(ExprKind::Predicate(Box::new(body.expr)), self.span_from(start), h, Bare::No)
            } else {
                self.expr(ctx)
            };
            out.push(arg);
            if self.eat(",") {
                continue;
            }
            if self.eat(close) {
                return out;
            }
            self.unexpected(&format!("`,` or `{close}`"));
            self.skip_to_sync();
            if !self.eat(",") {
                self.eat(close);
                return out;
            }
        }
    }

    /// After `{`: `key: value` pairs. A key is a name, a string, or `(expr)`.
    fn map(&mut self, start: Span, ctx: Context) -> Parsed<'src> {
        let mut pairs = Vec::new();
        let mut h = 0;
        loop {
            if self.eat("}") {
                break;
            }
            let key = match &self.tok {
                Token::Ident(name) => {
                    let key = MapKey::Name(Cow::Borrowed(*name), self.span);
                    self.bump();
                    key
                }
                Token::Str(s) => {
                    let key = MapKey::Name(s.clone(), self.span);
                    self.bump();
                    key
                }
                Token::Punct("(") => {
                    self.bump();
                    let k = self.expr(ctx);
                    self.expect_closing(")");
                    h = h.max(k.height);
                    MapKey::Computed(k.expr)
                }
                _ => {
                    self.unexpected("a map key");
                    self.skip_to_sync();
                    if self.eat(",") {
                        continue;
                    }
                    self.eat("}");
                    break;
                }
            };
            self.expect(":");
            let value = self.expr(ctx);
            h = h.max(value.height);
            pairs.push((key, value.expr));
            if self.eat(",") {
                continue;
            }
            self.expect_closing("}");
            break;
        }
        self.node(ExprKind::Map(pairs), self.span_from(start), h, Bare::No)
    }

    /// Skip to the next sync point (`,` `)` `]` `}` `;` or EOF) after an error and
    /// return an `ExprKind::Error` node spanning what was skipped.
    fn recover(&mut self, start: Span) -> Parsed<'src> {
        self.skip_to_sync();
        Self::leaf(ExprKind::Error, self.span_from(start))
    }

    /// Skip tokens up to a sync point at the current bracket level. Brackets opened
    /// while skipping are skipped with their contents.
    fn skip_to_sync(&mut self) {
        let mut open: u32 = 0;
        loop {
            match self.tok {
                Token::Eof => return,
                Token::Punct("(" | "[" | "{" | "?.[") => open = open.saturating_add(1),
                Token::Punct(")" | "]" | "}") if open == 0 => return,
                Token::Punct(")" | "]" | "}") => open = open.saturating_sub(1),
                Token::Punct("," | ";") if open == 0 => return,
                _ => {}
            }
            self.bump();
        }
    }
}
