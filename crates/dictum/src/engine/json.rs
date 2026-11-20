//! `toJSON` and `fromJSON`: Go's `encoding/json` on the values Go would hold.
//!
//! Encoding is `json.MarshalIndent(v, "", "  ")`: map keys sorted, `<`, `>`, `&`,
//! U+2028 and U+2029 escaped, floats as ES6 numbers, times as RFC 3339, durations as
//! nanoseconds, bytes as base64, structs as objects in field order. Decoding is
//! `json.Unmarshal` into `any`: numbers become floats, objects maps whose last
//! duplicate key wins, and nothing may follow the value.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt::Write;
use core::ops::ControlFlow;

use dictum_check::FieldId;
use dictum_syntax::{MAX_DEPTH, Span};

use crate::EvalErrorKind;
use crate::engine::frame::{Fault, Frame};
use crate::engine::interp::{Res, alloc, internal, steps};
use crate::engine::value::{Repr, Str, Value};
use crate::engine::{builtins, layout};

fn invalid(what: &'static str, value: &str, span: Span) -> Fault {
    Frame::fault(EvalErrorKind::InvalidArgument { what, value: value.into() }, span)
}

/// `toJSON(v)`, one step per value visited. The caller charges the text it returns.
pub(crate) fn encode(v: &Value<'_>, f: &mut Frame<'_>, span: Span) -> Result<String, Fault> {
    let mut e = Encoder { out: String::new(), f, span };
    e.value(v, 0)?;
    Ok(e.out)
}

struct Encoder<'f, 'a> {
    out: String,
    f: &'f mut Frame<'a>,
    span: Span,
}

impl Encoder<'_, '_> {
    fn value(&mut self, v: &Value<'_>, depth: usize) -> Result<(), Fault> {
        steps(self.f, 1, self.span)?;
        match &v.0 {
            Repr::Nil => self.out.push_str("null"),
            Repr::Bool(b) => self.out.push_str(if *b { "true" } else { "false" }),
            Repr::Int(n) => {
                let _ = write!(self.out, "{n}");
            }
            Repr::Float(x) => self.float(*x)?,
            #[cfg(feature = "decimal")]
            Repr::Decimal(d) => {
                let _ = write!(self.out, "{d}");
            }
            Repr::Str(s) => self.string(s.as_str()),
            Repr::Bytes(b) => {
                self.out.push('"');
                builtins::base64_encode(b.as_slice(), &mut self.out);
                self.out.push('"');
            }
            Repr::Time(t, z) => self.time(*t, z)?,
            // Go's `*time.Location` has no exported fields.
            Repr::Location(_) => self.out.push_str("{}"),
            Repr::Duration(d) => {
                let _ = write!(self.out, "{}", d.as_nanos());
            }
            Repr::Array(xs) => {
                if xs.len() == 0 {
                    self.out.push_str("[]");
                    return Ok(());
                }
                self.out.push('[');
                for (i, x) in xs.iter().enumerate() {
                    if i > 0 {
                        self.out.push(',');
                    }
                    self.newline(depth.saturating_add(1));
                    self.value(&x, depth.saturating_add(1))?;
                }
                self.newline(depth);
                self.out.push(']');
            }
            Repr::Map(m) => {
                let mut entries: Vec<(String, Value<'_>)> = Vec::with_capacity(m.len());
                m.for_each(&mut |k, x| {
                    entries.push((k.into(), x));
                    ControlFlow::Continue(())
                });
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                self.object(entries.iter().map(|(k, x)| (k.as_str(), x.clone())), depth)?;
            }
            Repr::Object(o) => {
                let fields = o.desc().fields();
                let mut entries = Vec::with_capacity(fields.len());
                for (i, field) in fields.iter().enumerate() {
                    let id = u16::try_from(i).map_err(|_| internal("too many fields", self.span))?;
                    entries.push((field.name(), o.field(FieldId::new(id))));
                }
                self.object(entries.into_iter(), depth)?;
            }
        }
        Ok(())
    }

    fn object<'k, 'v>(
        &mut self,
        entries: impl Iterator<Item = (&'k str, Value<'v>)>,
        depth: usize,
    ) -> Result<(), Fault> {
        let mut empty = true;
        for (k, x) in entries {
            self.out.push(if empty { '{' } else { ',' });
            empty = false;
            self.newline(depth.saturating_add(1));
            self.string(k);
            self.out.push_str(": ");
            self.value(&x, depth.saturating_add(1))?;
        }
        if empty {
            self.out.push_str("{}");
        } else {
            self.newline(depth);
            self.out.push('}');
        }
        Ok(())
    }

    fn newline(&mut self, depth: usize) {
        self.out.push('\n');
        for _ in 0..depth {
            self.out.push_str("  ");
        }
    }

    /// Go's `floatEncoder`: `strconv.FormatFloat` with `'f'`, or `'e'` below 1e-6 and
    /// from 1e21, the exponent unpadded (`1e-7`, `1e+21`).
    fn float(&mut self, x: f64) -> Result<(), Fault> {
        if !x.is_finite() {
            let mut text = String::new();
            builtins::go_float(x, &mut text);
            return Err(invalid("JSON value", &text, self.span));
        }
        let abs = libm::fabs(x);
        if abs != 0.0 && !(1e-6..1e21).contains(&abs) {
            let sci = alloc::format!("{x:e}");
            let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
            self.out.push_str(mantissa);
            self.out.push('e');
            if !exp.starts_with('-') {
                self.out.push('+');
            }
            self.out.push_str(exp);
        } else {
            let _ = write!(self.out, "{x}");
        }
        Ok(())
    }

    /// Go's `appendString` with HTML escaping.
    fn string(&mut self, s: &str) {
        self.out.push('"');
        for c in s.chars() {
            match c {
                '"' => self.out.push_str("\\\""),
                '\\' => self.out.push_str("\\\\"),
                '\u{8}' => self.out.push_str("\\b"),
                '\u{c}' => self.out.push_str("\\f"),
                '\n' => self.out.push_str("\\n"),
                '\r' => self.out.push_str("\\r"),
                '\t' => self.out.push_str("\\t"),
                '\0'..='\u{1f}' | '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => {
                    let _ = write!(self.out, "\\u{:04x}", u32::from(c));
                }
                c => self.out.push(c),
            }
        }
        self.out.push('"');
    }

    /// Go's `Time.MarshalJSON`: RFC 3339 in the time's zone, the nanoseconds trimmed
    /// and `Z` for a zero offset, years 0 to 9999.
    fn time(&mut self, t: jiff::Timestamp, z: &jiff::tz::TimeZone) -> Result<(), Fault> {
        let (dt, _, _) = builtins::zoned(t, z);
        if dt.year() < 0 {
            let mut text = String::new();
            builtins::go_time(t, z, &mut text);
            return Err(invalid("JSON time, year outside 0 to 9999", &text, self.span));
        }
        self.out.push('"');
        layout::format(t, z, "2006-01-02T15:04:05.999999999Z07:00", &mut self.out);
        self.out.push('"');
        Ok(())
    }
}

/// `fromJSON(text)`: one step per value, one alloc per element of a built array or
/// map and per byte of a built string. Nesting stops at [`MAX_DEPTH`] (Go allows
/// 10000), so every later recursive walk over the value stays as bounded as one over
/// a rule's own literals.
pub(crate) fn decode<'a>(text: &str, f: &mut Frame<'a>, span: Span) -> Res<'a> {
    let mut d = Decoder { text, pos: 0, f, span };
    d.space();
    let v = d.value(0)?;
    d.space();
    if d.pos < text.len() {
        return Err(d.fail("JSON, text after the value"));
    }
    Ok(v)
}

struct Decoder<'s, 'f, 'a> {
    text: &'s str,
    /// The byte offset of the next unread byte; always at a character boundary.
    pos: usize,
    f: &'f mut Frame<'a>,
    span: Span,
}

impl<'a> Decoder<'_, '_, 'a> {
    fn fail(&self, what: &'static str) -> Fault {
        invalid(what, builtins::excerpt(self.text, self.pos), self.span)
    }

    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.pos).copied()
    }

    fn bump(&mut self) {
        self.pos = self.pos.saturating_add(1);
    }

    fn space(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.peek() {
            self.bump();
        }
    }

    fn expect(&mut self, b: u8) -> Result<(), Fault> {
        match self.peek() {
            Some(c) if c == b => {
                self.bump();
                Ok(())
            }
            None => Err(self.fail("JSON, unexpected end")),
            Some(_) => Err(self.fail("JSON, unexpected text")),
        }
    }

    fn value(&mut self, depth: u32) -> Res<'a> {
        steps(self.f, 1, self.span)?;
        match self.peek() {
            None => Err(self.fail("JSON, unexpected end")),
            Some(b'[' | b'{') if depth >= MAX_DEPTH => Err(self.fail("JSON, nested too deep")),
            Some(b'[') => self.array(depth.saturating_add(1)),
            Some(b'{') => self.object(depth.saturating_add(1)),
            Some(b'"') => {
                let s = self.string()?;
                Ok(Value(Repr::Str(Str::Shared(s.into()))))
            }
            Some(b't') => self.literal("true", Value::bool(true)),
            Some(b'f') => self.literal("false", Value::bool(false)),
            Some(b'n') => self.literal("null", Value::NIL),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.fail("JSON, unexpected text")),
        }
    }

    fn literal(&mut self, word: &str, v: Value<'static>) -> Res<'a> {
        if !self.text.get(self.pos..).is_some_and(|rest| rest.starts_with(word)) {
            return Err(self.fail("JSON, unexpected text"));
        }
        self.pos = self.pos.saturating_add(word.len());
        Ok(v)
    }

    /// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`, as a float. Out of range is an
    /// error, as Go's `strconv.ParseFloat` reports it.
    fn number(&mut self) -> Res<'a> {
        let start = self.pos;
        let digits = |d: &mut Self| -> Result<(), Fault> {
            if !matches!(d.peek(), Some(b'0'..=b'9')) {
                return Err(match d.peek() {
                    None => d.fail("JSON, unexpected end"),
                    Some(_) => d.fail("JSON, unexpected text"),
                });
            }
            while let Some(b'0'..=b'9') = d.peek() {
                d.bump();
            }
            Ok(())
        };
        if self.peek() == Some(b'-') {
            self.bump();
        }
        if self.peek() == Some(b'0') {
            self.bump();
        } else {
            digits(self)?;
        }
        if self.peek() == Some(b'.') {
            self.bump();
            digits(self)?;
        }
        if let Some(b'e' | b'E') = self.peek() {
            self.bump();
            if let Some(b'+' | b'-') = self.peek() {
                self.bump();
            }
            digits(self)?;
        }
        let text = self.text.get(start..self.pos).unwrap_or("");
        match text.parse::<f64>() {
            Ok(x) if x.is_finite() => Ok(Value::float(x)),
            _ => {
                self.pos = start;
                Err(self.fail("JSON, number out of range"))
            }
        }
    }

    /// A string at `"`, unescaped. A `\u` escape of an unpaired surrogate is U+FFFD,
    /// as in Go.
    fn string(&mut self) -> Result<String, Fault> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let run = self.pos;
            while let Some(c) = self.peek() {
                if c == b'"' || c == b'\\' || c < 0x20 {
                    break;
                }
                self.bump();
            }
            out.push_str(self.text.get(run..self.pos).unwrap_or(""));
            match self.peek() {
                None => return Err(self.fail("JSON, unexpected end")),
                Some(b'"') => {
                    self.bump();
                    break;
                }
                Some(b'\\') => self.escape(&mut out)?,
                Some(_) => return Err(self.fail("JSON, unexpected text")),
            }
        }
        alloc(self.f, out.len(), self.span)?;
        Ok(out)
    }

    fn escape(&mut self, out: &mut String) -> Result<(), Fault> {
        let at = self.pos;
        self.bump();
        let c = match self.peek() {
            None => return Err(self.fail("JSON, unexpected end")),
            Some(b'"') => '"',
            Some(b'\\') => '\\',
            Some(b'/') => '/',
            Some(b'b') => '\u{8}',
            Some(b'f') => '\u{c}',
            Some(b'n') => '\n',
            Some(b'r') => '\r',
            Some(b't') => '\t',
            Some(b'u') => {
                self.pos = at;
                let unit = self.unit().ok_or_else(|| self.fail("JSON, unexpected text"))?;
                self.pos = self.pos.saturating_add(6);
                let c = if (0xD800..0xDC00).contains(&unit) {
                    match self.unit() {
                        Some(low @ 0xDC00..0xE000) => {
                            self.pos = self.pos.saturating_add(6);
                            let high = unit.wrapping_sub(0xD800).wrapping_shl(10);
                            char::from_u32(high.wrapping_add(low.wrapping_sub(0xDC00)).wrapping_add(0x1_0000))
                        }
                        _ => None,
                    }
                } else {
                    char::from_u32(unit)
                };
                out.push(c.unwrap_or(char::REPLACEMENT_CHARACTER));
                return Ok(());
            }
            Some(_) => return Err(self.fail("JSON, unexpected text")),
        };
        self.bump();
        out.push(c);
        Ok(())
    }

    /// The code unit of a `\uXXXX` escape at the current position, without moving.
    fn unit(&self) -> Option<u32> {
        let hex = self.text.get(self.pos..)?.strip_prefix("\\u")?.get(..4)?;
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(hex, 16).ok()
    }

    fn array(&mut self, depth: u32) -> Res<'a> {
        self.expect(b'[')?;
        self.space();
        let mut out = Vec::new();
        if self.peek() == Some(b']') {
            self.bump();
            return Ok(Value::array(out));
        }
        loop {
            self.space();
            let x = self.value(depth)?;
            alloc(self.f, 1, self.span)?;
            out.push(x);
            self.space();
            match self.peek() {
                Some(b',') => self.bump(),
                Some(b']') => {
                    self.bump();
                    return Ok(Value::array(out));
                }
                None => return Err(self.fail("JSON, unexpected end")),
                Some(_) => return Err(self.fail("JSON, unexpected text")),
            }
        }
    }

    fn object(&mut self, depth: u32) -> Res<'a> {
        self.expect(b'{')?;
        self.space();
        let mut out: BTreeMap<Arc<str>, Value<'a>> = BTreeMap::new();
        if self.peek() == Some(b'}') {
            self.bump();
            return Ok(Value::map(Vec::new()));
        }
        loop {
            self.space();
            let key = self.string()?;
            self.space();
            self.expect(b':')?;
            self.space();
            let x = self.value(depth)?;
            if out.insert(key.into(), x).is_none() {
                alloc(self.f, 1, self.span)?;
            }
            self.space();
            match self.peek() {
                Some(b',') => self.bump(),
                Some(b'}') => {
                    self.bump();
                    let entries: Vec<(Arc<str>, Value<'a>)> = out.into_iter().collect();
                    return Ok(Value::map(entries));
                }
                None => return Err(self.fail("JSON, unexpected end")),
                Some(_) => return Err(self.fail("JSON, unexpected text")),
            }
        }
    }
}
