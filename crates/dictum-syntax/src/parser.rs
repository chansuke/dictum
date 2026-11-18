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

use alloc::vec::Vec;

use crate::ast::{Expr, Program};
use crate::lexer::Lexer;
use crate::{Diagnostics, Span};

/// Where the parser is, for rules that depend on context.
#[derive(Clone, Copy, Default)]
struct Context {
    /// Inside a predicate argument: `#`, `#index`, `#acc`, `.Field` are allowed.
    in_predicate: bool,
    /// Current tree depth; exceeding `MAX_DEPTH` is E0105.
    depth: u32,
}

pub(crate) struct Parser<'src> {
    src: &'src str,
    lexer: Lexer<'src>,
    diags: Diagnostics,
}

impl<'src> Parser<'src> {
    pub(crate) fn new(src: &'src str) -> Self {
        Parser { src, lexer: Lexer::new(src), diags: Diagnostics::new() }
    }

    /// `program = { "let" ident "=" expr ";" } expr EOF`
    pub(crate) fn program(self) -> Result<Program<'src>, Diagnostics> {
        // TODO: if src.len() > u32::MAX -> single E0100 diagnostic.
        // Parse lets then the body; on any diagnostic return Err(self.diags).
        todo!()
    }

    /// Pratt loop.
    ///
    /// TODO pseudocode:
    /// ```text
    /// lhs = prefix(ctx)                       // unary ops use their own bp for the operand:
    ///                                         //   `not x` parses x at bp 50, `-x` at bp 90
    /// loop:
    ///   op = peek infix; if op.bp < min_bp: break
    ///   match op:
    ///     `??`:   rhs = parse(bp 501); if lhs or rhs is a bare Binary/Compare/Logic
    ///             produced without parens (track a `parenthesized` bit per node), E0107.
    ///             Likewise any other binary op whose operand is a bare `??` -> E0107.
    ///     `&&`/`||`: collect operands at bp+1 into one Logic node while the same op repeats.
    ///     `<`..`>=`: collect a Compare chain while the next op is also a comparison;
    ///             if the next op is `==`/`!=`/`in`/... after any comparison -> E0106.
    ///     `==` etc: rhs at bp+1; if the next op is again non-associative bp 20 -> E0106.
    ///     `|`:    parse `ident(args)`; insert lhs as arg 0; predicate position from
    ///             Builtin::predicate_arg counts the piped value.
    ///     `?`:    ternary handled by `conditional` above the Pratt loop (lowest).
    /// every recursive descent increments ctx.depth; > MAX_DEPTH -> E0105 and return Error.
    /// ```
    fn expr_bp(&mut self, min_bp: u8, ctx: Context) -> Expr<'src> {
        todo!()
    }

    /// `postfix = primary { "." ident | "?." ident | "[" index "]" | "?.[" index "]" }`
    ///
    /// `(` is accepted only directly after an identifier primary (a call). After any
    /// other postfix (`x.f(`, `x[0](`, `(f)(`) it is E0108.
    fn postfix(&mut self, ctx: Context) -> Expr<'src> {
        todo!()
    }

    /// Arguments of `callee(...)`. If `callee` is a builtin with a predicate position
    /// and this is that position, parse with `in_predicate = true`; a leading `{`
    /// there opens predicate braces (a map literal as a predicate body is `{{a: #}}`).
    fn args(&mut self, predicate_at: Option<usize>, first_index: usize, ctx: Context) -> Vec<Expr<'src>> {
        todo!()
    }

    /// Skip to the next sync point (`,` `)` `]` `}` `;` or EOF) after an error and
    /// return an `ExprKind::Error` node spanning what was skipped.
    fn recover(&mut self, start: Span) -> Expr<'src> {
        todo!()
    }
}
