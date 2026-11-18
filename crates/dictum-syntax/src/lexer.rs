//! Tokens and the lexer. Private to this crate.

use alloc::borrow::Cow;

use crate::{Diagnostics, Span};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Token<'src> {
    Ident(&'src str),
    /// `$env`
    Env,
    Int(i64),
    Float(f64),
    Str(Cow<'src, str>),
    Bytes(Cow<'src, [u8]>),
    /// `#`, `#index`, `#acc`
    Pointer(crate::ast::Pointer),
    /// Reserved words and punctuation, by their source text (`"&&"`, `"?."`, `"not"`).
    Punct(&'static str),
    Eof,
}

pub(crate) struct Lexer<'src> {
    src: &'src str,
    pos: u32,
}

impl<'src> Lexer<'src> {
    pub(crate) fn new(src: &'src str) -> Self {
        Lexer { src, pos: 0 }
    }

    /// Next token. Lexical errors are pushed to `diags` and the lexer resyncs at the
    /// next character, so it always makes progress (no infinite loop on bad input).
    pub(crate) fn next(&mut self, diags: &mut Diagnostics) -> (Token<'src>, Span) {
        // TODO:
        // - skip whitespace, `// ...\n`, `/* ... */` (unterminated block: E0102-like)
        // - ident: [A-Za-z_][A-Za-z0-9_]*; if `keyword::RESERVED` contains it -> Punct
        //   `not` followed by whitespace + `in` -> Punct("not in") (one token)
        // - numbers: 0x/0o/0b, `_` separators, `.5`, `1e3`; overflow -> E0104
        //   `1..2` must lex as Int(1) Punct("..") Int(2), not Float(1.) — look ahead
        //   for a second '.' before treating '.' as a fraction.
        // - strings: '..' and ".." with \n \t \\ \' \" \uXXXX; `...` raw
        //   b"..": \xNN \NNN
        // - longest-match punctuation: `?.[` `?.` `??` `?:` `**` `..` `==` `!=` `<=` `>=`
        //   `&&` `||` then single chars.
        todo!()
    }
}
