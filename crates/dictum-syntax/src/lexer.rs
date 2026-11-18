//! Tokens and the lexer. Private to this crate.

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::ast::Pointer;
use crate::keyword::RESERVED;
use crate::{Arg, Diagnostic, Diagnostics, ErrorCode, Span};

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

/// Longest match first.
const PUNCTS: &[&str] = &[
    "?.[", "?.", "??", "?:", "**", "..", "==", "!=", "<=", ">=", "&&", "||", "+", "-", "*", "/", "%", "^", "<", ">",
    "!", "?", ":", ".", ",", "(", ")", "[", "]", "{", "}", "|", ";", "=",
];

pub(crate) struct Lexer<'src> {
    src: &'src str,
    pos: u32,
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

impl<'src> Lexer<'src> {
    pub(crate) fn new(src: &'src str) -> Self {
        Lexer { src, pos: 0 }
    }

    fn bytes(&self) -> &'src [u8] {
        self.src.as_bytes()
    }

    fn at(&self, i: u32) -> Option<u8> {
        self.bytes().get(i as usize).copied()
    }

    fn peek(&self) -> Option<u8> {
        self.at(self.pos)
    }

    fn rest(&self) -> &'src str {
        self.src.get(self.pos as usize..).unwrap_or("")
    }

    fn span_from(&self, start: u32) -> Span {
        Span { start, end: self.pos }
    }

    fn text(&self, span: Span) -> &'src str {
        span.text(self.src).unwrap_or("")
    }

    /// Advance by `n` bytes. The caller has checked they are in the source, and the
    /// source is shorter than `u32::MAX` (the parser rejects longer ones first).
    fn advance(&mut self, n: usize) {
        self.pos = self.pos.saturating_add(u32::try_from(n).unwrap_or(u32::MAX));
    }

    /// Advance over one character, whatever its UTF-8 length.
    fn advance_char(&mut self) {
        let n = self.rest().chars().next().map_or(1, char::len_utf8);
        self.advance(n);
    }

    fn advance_while(&mut self, f: impl Fn(u8) -> bool) {
        while self.peek().is_some_and(&f) {
            self.advance(1);
        }
    }

    fn error(diags: &mut Diagnostics, code: ErrorCode, span: Span, args: Vec<Arg>) {
        diags.push(Diagnostic::new(code, span, args));
    }

    /// Next token. Lexical errors are pushed to `diags` and the lexer resyncs at the
    /// next character, so it always makes progress (no infinite loop on bad input).
    pub(crate) fn next(&mut self, diags: &mut Diagnostics) -> (Token<'src>, Span) {
        loop {
            self.skip_trivia(diags);
            let start = self.pos;
            let Some(b) = self.peek() else {
                return (Token::Eof, self.span_from(start));
            };
            if let Some(tok) = self.token(b, diags) {
                return (tok, self.span_from(start));
            }
            // An unknown character: reported, skipped, and lexing goes on.
            self.advance_char();
            let span = self.span_from(start);
            let found = alloc::format!("`{}`", self.text(span));
            Self::error(
                diags,
                ErrorCode::UnexpectedToken,
                span,
                vec![Arg::Name("a token".into()), Arg::Name(found.into())],
            );
        }
    }

    fn skip_trivia(&mut self, diags: &mut Diagnostics) {
        loop {
            let rest = self.rest();
            if rest.starts_with("//") {
                let line = rest.find('\n').unwrap_or(rest.len());
                self.advance(line);
            } else if rest.starts_with("/*") {
                let start = self.pos;
                match rest.get(2..).and_then(|r| r.find("*/")) {
                    Some(end) => self.advance(end.saturating_add(4)),
                    None => {
                        self.advance(rest.len());
                        Self::error(
                            diags,
                            ErrorCode::UnexpectedToken,
                            self.span_from(start),
                            vec![Arg::Name("`*/`".into()), Arg::Name("end of input".into())],
                        );
                    }
                }
            } else if self.peek().is_some_and(|b| b.is_ascii_whitespace()) {
                self.advance_while(|b| b.is_ascii_whitespace());
            } else {
                return;
            }
        }
    }

    /// The token starting with byte `b`, or `None` for a character that starts none.
    fn token(&mut self, b: u8, diags: &mut Diagnostics) -> Option<Token<'src>> {
        let start = self.pos;
        if b == b'b' && self.at(start.saturating_add(1)) == Some(b'"') {
            self.advance(1);
            return Some(self.bytes_literal(diags));
        }
        if is_ident_start(b) {
            return Some(self.word());
        }
        if b.is_ascii_digit() || (b == b'.' && self.at(start.saturating_add(1)).is_some_and(|c| c.is_ascii_digit())) {
            return Some(self.number(diags));
        }
        match b {
            b'"' | b'\'' => return Some(self.string(b, diags)),
            b'`' => return Some(self.raw_string(diags)),
            b'$' if self.rest().get(1..4) == Some("env")
                && !self.at(start.saturating_add(4)).is_some_and(is_ident_char) =>
            {
                self.advance(4);
                return Some(Token::Env);
            }
            b'#' => {
                self.advance(1);
                let word = |w: &str| {
                    self.rest().starts_with(w)
                        && !self
                            .at(self.pos.saturating_add(u32::try_from(w.len()).unwrap_or(0)))
                            .is_some_and(is_ident_char)
                };
                let pointer = if word("index") {
                    self.advance(5);
                    Pointer::Index
                } else if word("acc") {
                    self.advance(3);
                    Pointer::Acc
                } else {
                    Pointer::Current
                };
                return Some(Token::Pointer(pointer));
            }
            _ => {}
        }
        let rest = self.rest();
        let p = PUNCTS.iter().find(|p| rest.starts_with(**p))?;
        self.advance(p.len());
        Some(Token::Punct(p))
    }

    /// An identifier or a reserved word; `not` followed by whitespace and `in` is the
    /// single token `not in`.
    fn word(&mut self) -> Token<'src> {
        let start = self.pos;
        self.advance_while(is_ident_char);
        let word = self.text(self.span_from(start));
        let Some(reserved) = RESERVED.iter().find(|r| **r == word) else {
            return Token::Ident(word);
        };
        if *reserved == "not" {
            let after = self.rest();
            let trimmed = after.trim_start_matches(|c: char| c.is_ascii_whitespace());
            let is_in = trimmed.len() < after.len()
                && trimmed.starts_with("in")
                && !trimmed.as_bytes().get(2).copied().is_some_and(is_ident_char);
            if is_in {
                self.advance(after.len().saturating_sub(trimmed.len()).saturating_add(2));
                return Token::Punct("not in");
            }
        }
        Token::Punct(reserved)
    }

    fn number(&mut self, diags: &mut Diagnostics) -> Token<'src> {
        let start = self.pos;
        let radix = match self.rest().get(..2) {
            Some("0x" | "0X") => 16,
            Some("0o" | "0O") => 8,
            Some("0b" | "0B") => 2,
            _ => 10,
        };
        if radix != 10 {
            self.advance(2);
            self.advance_while(|b| b.is_ascii_alphanumeric() || b == b'_');
            let span = self.span_from(start);
            let digits: String = self.text(span).get(2..).unwrap_or("").chars().filter(|c| *c != '_').collect();
            let valid = !digits.is_empty() && digits.chars().all(|c| c.is_digit(radix));
            if !valid {
                Self::error(diags, ErrorCode::InvalidNumber, span, Vec::new());
                return Token::Int(0);
            }
            return match i64::from_str_radix(&digits, radix) {
                Ok(n) => Token::Int(n),
                Err(_) => {
                    Self::error(diags, ErrorCode::IntegerTooLarge, span, Vec::new());
                    Token::Int(0)
                }
            };
        }

        let digits = |b: u8| b.is_ascii_digit() || b == b'_';
        self.advance_while(digits);
        let mut float = false;
        // A fraction needs a digit after the dot: `1..2` is a range and `1.b` a member.
        if self.peek() == Some(b'.') && self.at(self.pos.saturating_add(1)).is_some_and(|b| b.is_ascii_digit()) {
            float = true;
            self.advance(1);
            self.advance_while(digits);
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            float = true;
            self.advance(1);
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.advance(1);
            }
            self.advance_while(digits);
        }
        // A letter glued to the number (`1x`, `1e`) makes it invalid, not two tokens.
        self.advance_while(is_ident_char);
        let span = self.span_from(start);
        let text = self.text(span);
        let clean: String = text.chars().filter(|c| *c != '_').collect();
        let well_formed = !text.ends_with('_') && !text.contains("_.") && !text.contains("._");
        if float {
            match clean.parse::<f64>() {
                Ok(f) if well_formed => Token::Float(f),
                _ => {
                    Self::error(diags, ErrorCode::InvalidNumber, span, Vec::new());
                    Token::Float(0.0)
                }
            }
        } else if !well_formed || !clean.bytes().all(|b| b.is_ascii_digit()) {
            Self::error(diags, ErrorCode::InvalidNumber, span, Vec::new());
            Token::Int(0)
        } else {
            match clean.parse::<i64>() {
                Ok(n) => Token::Int(n),
                Err(_) => {
                    Self::error(diags, ErrorCode::IntegerTooLarge, span, Vec::new());
                    Token::Int(0)
                }
            }
        }
    }

    /// `"..."` or `'...'`. Borrowed when there are no escapes.
    fn string(&mut self, quote: u8, diags: &mut Diagnostics) -> Token<'src> {
        let start = self.pos;
        self.advance(1);
        let body_start = self.pos;
        let mut owned: Option<String> = None;
        loop {
            let Some(c) = self.rest().chars().next() else {
                Self::error(diags, ErrorCode::UnterminatedString, self.span_from(start), Vec::new());
                break;
            };
            if c == char::from(quote) {
                let body = self.text(Span { start: body_start, end: self.pos });
                self.advance(1);
                return Token::Str(owned.map_or(Cow::Borrowed(body), Cow::Owned));
            }
            if c == '\\' {
                let so_far = self.text(Span { start: body_start, end: self.pos });
                let buf = owned.get_or_insert_with(|| String::from(so_far));
                let esc_start = self.pos;
                self.advance(1);
                match self.escape() {
                    Some(ch) => buf.push(ch),
                    None => {
                        let span = self.span_from(esc_start);
                        Self::error(diags, ErrorCode::InvalidEscape, span, vec![Arg::Name(self.text(span).into())]);
                    }
                }
                continue;
            }
            if let Some(buf) = owned.as_mut() {
                buf.push(c);
            }
            self.advance(c.len_utf8());
        }
        let body = self.text(Span { start: body_start, end: self.pos });
        Token::Str(owned.map_or(Cow::Borrowed(body), Cow::Owned))
    }

    /// After a backslash: `n t r 0 \ ' " uXXXX`. Consumes what it reads; `None` if
    /// the escape is invalid.
    fn escape(&mut self) -> Option<char> {
        let c = self.rest().chars().next()?;
        self.advance(c.len_utf8());
        Some(match c {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            '0' => '\0',
            '\\' => '\\',
            '\'' => '\'',
            '"' => '"',
            'u' => {
                let hex = self.rest().get(..4)?;
                if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return None;
                }
                self.advance(4);
                char::from_u32(u32::from_str_radix(hex, 16).ok()?)?
            }
            _ => return None,
        })
    }

    /// `` `...` ``: no escapes.
    fn raw_string(&mut self, diags: &mut Diagnostics) -> Token<'src> {
        let start = self.pos;
        self.advance(1);
        let body_start = self.pos;
        match self.rest().find('`') {
            Some(end) => {
                self.advance(end);
                let body = self.text(Span { start: body_start, end: self.pos });
                self.advance(1);
                Token::Str(Cow::Borrowed(body))
            }
            None => {
                let rest = self.rest();
                self.advance(rest.len());
                Self::error(diags, ErrorCode::UnterminatedString, self.span_from(start), Vec::new());
                Token::Str(Cow::Borrowed(rest))
            }
        }
    }

    /// `b"..."` (the `b` is consumed): `\xNN`, `\NNN` (octal) and the string escapes.
    fn bytes_literal(&mut self, diags: &mut Diagnostics) -> Token<'src> {
        let start = self.pos.saturating_sub(1);
        self.advance(1);
        let body_start = self.pos;
        let mut owned: Option<Vec<u8>> = None;
        loop {
            let Some(c) = self.rest().chars().next() else {
                Self::error(diags, ErrorCode::UnterminatedString, self.span_from(start), Vec::new());
                break;
            };
            if c == '"' {
                let body = self.text(Span { start: body_start, end: self.pos }).as_bytes();
                self.advance(1);
                return Token::Bytes(owned.map_or(Cow::Borrowed(body), Cow::Owned));
            }
            if c == '\\' {
                let so_far = self.text(Span { start: body_start, end: self.pos }).as_bytes();
                let buf = owned.get_or_insert_with(|| so_far.to_vec());
                let esc_start = self.pos;
                self.advance(1);
                match self.byte_escape() {
                    Some(b) => buf.push(b),
                    None => {
                        let span = self.span_from(esc_start);
                        Self::error(diags, ErrorCode::InvalidEscape, span, vec![Arg::Name(self.text(span).into())]);
                    }
                }
                continue;
            }
            if let Some(buf) = owned.as_mut() {
                let mut utf8 = [0; 4];
                buf.extend_from_slice(c.encode_utf8(&mut utf8).as_bytes());
            }
            self.advance(c.len_utf8());
        }
        let body = self.text(Span { start: body_start, end: self.pos }).as_bytes();
        Token::Bytes(owned.map_or(Cow::Borrowed(body), Cow::Owned))
    }

    fn byte_escape(&mut self) -> Option<u8> {
        let rest = self.rest();
        if let Some(hex) = rest.strip_prefix('x').and_then(|r| r.get(..2)) {
            let b = u8::from_str_radix(hex, 16).ok()?;
            self.advance(3);
            return Some(b);
        }
        if let Some(oct) = rest.get(..3).filter(|o| o.bytes().all(|b| (b'0'..=b'7').contains(&b))) {
            let b = u8::try_from(u32::from_str_radix(oct, 8).ok()?).ok()?;
            self.advance(3);
            return Some(b);
        }
        let c = self.escape()?;
        u8::try_from(c).ok().filter(u8::is_ascii)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;

    fn lex(src: &str) -> (Vec<Token<'_>>, Vec<ErrorCode>) {
        let mut diags = Diagnostics::new();
        let mut lexer = Lexer::new(src);
        let mut toks = Vec::new();
        loop {
            let (t, _) = lexer.next(&mut diags);
            if t == Token::Eof {
                break;
            }
            toks.push(t);
        }
        (toks, diags.finish().0.iter().map(Diagnostic::code).collect())
    }

    #[test]
    fn ranges_are_not_fractions() {
        assert_eq!(lex("1..2").0, vec![Token::Int(1), Token::Punct(".."), Token::Int(2)]);
        assert_eq!(lex("1.5").0, vec![Token::Float(1.5)]);
        assert_eq!(lex(".5").0, vec![Token::Float(0.5)]);
        assert_eq!(lex("a.b").0, vec![Token::Ident("a"), Token::Punct("."), Token::Ident("b")]);
    }

    #[test]
    fn not_in_is_one_token_only_before_the_word_in() {
        assert_eq!(lex("a not in b").0, vec![Token::Ident("a"), Token::Punct("not in"), Token::Ident("b")]);
        assert_eq!(lex("not inx").0, vec![Token::Punct("not"), Token::Ident("inx")]);
        assert_eq!(lex("notin").0, vec![Token::Ident("notin")]);
    }

    #[test]
    fn longest_punctuation_wins() {
        assert_eq!(
            lex("a?.[b]?.c??d?:e").0,
            vec![
                Token::Ident("a"),
                Token::Punct("?.["),
                Token::Ident("b"),
                Token::Punct("]"),
                Token::Punct("?."),
                Token::Ident("c"),
                Token::Punct("??"),
                Token::Ident("d"),
                Token::Punct("?:"),
                Token::Ident("e"),
            ]
        );
    }

    #[test]
    fn strings_borrow_unless_escaped() {
        assert!(matches!(&lex("'abc'").0[0], Token::Str(Cow::Borrowed("abc"))));
        assert!(matches!(&lex(r#""a\nb""#).0[0], Token::Str(Cow::Owned(s)) if s == "a\nb"));
        assert!(matches!(&lex(r#""\u00e9""#).0[0], Token::Str(Cow::Owned(s)) if s == "é"));
        assert!(matches!(&lex(r"`a\n`").0[0], Token::Str(Cow::Borrowed(r"a\n"))));
        assert!(matches!(&lex(r#"b"\x00\101""#).0[0], Token::Bytes(Cow::Owned(b)) if b == &[0, 65]));
    }

    #[test]
    fn lexical_errors_are_reported_and_skipped() {
        assert_eq!(lex(r#""abc"#).1, vec![ErrorCode::UnterminatedString]);
        assert_eq!(lex(r#""\q""#).1, vec![ErrorCode::InvalidEscape]);
        assert_eq!(lex("9223372036854775808").1, vec![ErrorCode::IntegerTooLarge]);
        assert_eq!(lex("0x").1, vec![ErrorCode::InvalidNumber]);
        assert_eq!(lex("1e").1, vec![ErrorCode::InvalidNumber]);
        assert_eq!(lex("1_").1, vec![ErrorCode::InvalidNumber]);
        assert_eq!(lex("1 @ 2"), (vec![Token::Int(1), Token::Int(2)], vec![ErrorCode::UnexpectedToken]));
        assert_eq!(lex("/* open").1, vec![ErrorCode::UnexpectedToken]);
    }

    #[test]
    fn pointers_and_env() {
        assert_eq!(
            lex("# #index #acc $env").0,
            vec![
                Token::Pointer(Pointer::Current),
                Token::Pointer(Pointer::Index),
                Token::Pointer(Pointer::Acc),
                Token::Env
            ]
        );
    }

    #[test]
    fn comments_are_skipped() {
        assert_eq!(lex("1 /* x */ + // y\n 2").0, vec![Token::Int(1), Token::Punct("+"), Token::Int(2)]);
    }
}
