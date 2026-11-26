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
use crate::engine::frame::{Fault, Frame, Res, alloc, internal, steps};
use crate::engine::value::{MAX_VALUE_DEPTH, Object, Repr, Seq, Str, Value};
use crate::engine::{builtins, layout};

fn invalid(what: &'static str, value: &str, span: Span) -> Fault {
    Frame::fault(EvalErrorKind::InvalidArgument { what, value: value.into() }, span)
}

/// `toJSON(v)`, one step per value visited and one alloc per byte written, charged
/// before appending to the output buffer. Uses an explicit stack so deeply nested
/// values stay within small stacks (e.g. WASM and `no_std`) up to [`MAX_VALUE_DEPTH`].
pub(crate) fn encode<'a>(v: &Value<'a>, f: &mut Frame<'_>, span: Span) -> Result<String, Fault> {
    let mut e = Encoder { out: String::new(), f, span };
    let mut stack: Vec<EncodeFrame<'a>> = Vec::new();
    e.value(v, &mut stack)?;
    loop {
        let depth = stack.len();
        let Some(top) = stack.last_mut() else { break };
        let next: Option<Value<'a>> = match top {
            EncodeFrame::Array { seq, index } => {
                let i = *index;
                if i < seq.len() {
                    if let Some(x) = seq.get(i) {
                        *index = i.saturating_add(1);
                        if i > 0 {
                            e.push_char(',')?;
                        }
                        e.newline(depth)?;
                        Some(x)
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            EncodeFrame::Map { entries, first } => {
                if let Some((k, x)) = entries.pop() {
                    let is_first = *first;
                    *first = false;
                    if !is_first {
                        e.push_char(',')?;
                    }
                    e.newline(depth)?;
                    e.string(&k)?;
                    e.push_str(": ")?;
                    Some(x)
                } else {
                    None
                }
            }
            EncodeFrame::Object { obj, index } => {
                let i = *index;
                if let Some(field) = obj.desc().fields().get(i) {
                    *index = i.saturating_add(1);
                    let id = u16::try_from(i).map_err(|_| internal("too many fields", e.span))?;
                    let x = obj.field(FieldId::new(id));
                    if i > 0 {
                        e.push_char(',')?;
                    }
                    e.newline(depth)?;
                    e.string(field.name())?;
                    e.push_str(": ")?;
                    Some(x)
                } else {
                    None
                }
            }
        };
        if let Some(x) = next {
            e.value(&x, &mut stack)?;
        } else {
            let close = match stack.pop() {
                Some(EncodeFrame::Array { .. }) => ']',
                _ => '}',
            };
            e.newline(stack.len())?;
            e.push_char(close)?;
        }
    }
    Ok(e.out)
}

enum EncodeFrame<'a> {
    Array { seq: Seq<'a>, index: usize },
    Map { entries: Vec<(String, Value<'a>)>, first: bool },
    Object { obj: &'a dyn Object, index: usize },
}

struct Encoder<'f, 'a> {
    out: String,
    f: &'f mut Frame<'a>,
    span: Span,
}

impl Encoder<'_, '_> {
    fn push_str(&mut self, s: &str) -> Result<(), Fault> {
        alloc(self.f, s.len(), self.span)?;
        self.out.push_str(s);
        Ok(())
    }

    fn push_char(&mut self, c: char) -> Result<(), Fault> {
        alloc(self.f, c.len_utf8(), self.span)?;
        self.out.push(c);
        Ok(())
    }

    fn value<'a>(&mut self, v: &Value<'a>, stack: &mut Vec<EncodeFrame<'a>>) -> Result<(), Fault> {
        if stack.len() > usize::from(MAX_VALUE_DEPTH) {
            return Err(Frame::fault(EvalErrorKind::DepthLimit, self.span));
        }
        steps(self.f, 1, self.span)?;
        match &v.0 {
            Repr::Nil => self.push_str("null")?,
            Repr::Bool(b) => self.push_str(if *b { "true" } else { "false" })?,
            Repr::Int(n) => {
                let text = alloc::format!("{n}");
                self.push_str(&text)?;
            }
            Repr::Float(x) => self.float(*x)?,
            #[cfg(feature = "decimal")]
            Repr::Decimal(d) => {
                let text = alloc::format!("{d}");
                self.push_str(&text)?;
            }
            Repr::Str(s) => self.string(s.as_str())?,
            Repr::Bytes(b) => {
                let bytes = b.as_slice();
                let encoded = bytes
                    .len()
                    .div_ceil(3)
                    .checked_mul(4)
                    .and_then(|n| n.checked_add(2))
                    .ok_or_else(|| Frame::fault(EvalErrorKind::AllocLimit, self.span))?;
                alloc(self.f, encoded, self.span)?;
                self.out.push('"');
                builtins::base64_encode(bytes, &mut self.out);
                self.out.push('"');
            }
            Repr::Time(t, z) => self.time(*t, z)?,
            // Go's `*time.Location` has no exported fields.
            Repr::Location(_) => self.push_str("{}")?,
            Repr::Duration(d) => {
                let text = alloc::format!("{}", d.as_nanos());
                self.push_str(&text)?;
            }
            Repr::Array(xs) => {
                if xs.len() == 0 {
                    self.push_str("[]")?;
                    return Ok(());
                }
                self.push_char('[')?;
                stack.push(EncodeFrame::Array { seq: xs.clone(), index: 0 });
            }
            Repr::Map(m) => {
                if m.len() == 0 {
                    self.push_str("{}")?;
                    return Ok(());
                }
                let mut entries: Vec<(String, Value<'a>)> = Vec::with_capacity(m.len());
                m.for_each(&mut |k, x| {
                    entries.push((k.into(), x));
                    ControlFlow::Continue(())
                });
                if entries.is_empty() {
                    self.push_str("{}")?;
                    return Ok(());
                }
                // Reverse order so `pop()` yields keys in ascending byte order.
                entries.sort_by(|a, b| b.0.cmp(&a.0));
                self.push_char('{')?;
                stack.push(EncodeFrame::Map { entries, first: true });
            }
            Repr::Object(o) => {
                if o.desc().fields().is_empty() {
                    self.push_str("{}")?;
                    return Ok(());
                }
                self.push_char('{')?;
                stack.push(EncodeFrame::Object { obj: *o, index: 0 });
            }
        }
        Ok(())
    }

    fn newline(&mut self, depth: usize) -> Result<(), Fault> {
        alloc(self.f, depth.saturating_mul(2).saturating_add(1), self.span)?;
        self.out.push('\n');
        for _ in 0..depth {
            self.out.push_str("  ");
        }
        Ok(())
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
            self.push_str(mantissa)?;
            self.push_char('e')?;
            if !exp.starts_with('-') {
                self.push_char('+')?;
            }
            self.push_str(exp)
        } else {
            let text = alloc::format!("{x}");
            self.push_str(&text)
        }
    }

    /// Go's `appendString` with HTML escaping.
    fn string(&mut self, s: &str) -> Result<(), Fault> {
        self.push_char('"')?;
        let mut start = 0usize;
        for (i, c) in s.char_indices() {
            let escaped = match c {
                '"' => Some("\\\""),
                '\\' => Some("\\\\"),
                '\u{8}' => Some("\\b"),
                '\u{c}' => Some("\\f"),
                '\n' => Some("\\n"),
                '\r' => Some("\\r"),
                '\t' => Some("\\t"),
                '\0'..='\u{1f}' | '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => Some(""),
                _ => None,
            };
            if let Some(esc) = escaped {
                if let Some(plain) = s.get(start..i) {
                    if !plain.is_empty() {
                        self.push_str(plain)?;
                    }
                }
                if esc.is_empty() {
                    alloc(self.f, 6, self.span)?;
                    let _ = write!(self.out, "\\u{:04x}", u32::from(c));
                } else {
                    self.push_str(esc)?;
                }
                start = i.saturating_add(c.len_utf8());
            }
        }
        if let Some(tail) = s.get(start..) {
            if !tail.is_empty() {
                self.push_str(tail)?;
            }
        }
        self.push_char('"')
    }

    /// Go's `Time.MarshalJSON`: RFC 3339 in the time's zone, the nanoseconds trimmed
    /// and `Z` for a zero offset, years 0 to 9999.
    fn time(&mut self, t: jiff::Timestamp, z: &jiff::tz::TimeZone) -> Result<(), Fault> {
        let (dt, offset, _) = layout::zoned(t, z);
        if offset.unsigned_abs() / 3600 >= 24 {
            let mut text = String::new();
            builtins::go_time(t, z, &mut text);
            return Err(invalid("JSON time, offset outside -23:59 to +23:59", &text, self.span));
        }
        if dt.year() < 0 {
            let mut text = String::new();
            builtins::go_time(t, z, &mut text);
            return Err(invalid("JSON time, year outside 0 to 9999", &text, self.span));
        }
        let mut formatted = String::new();
        formatted.push('"');
        layout::format(t, z, "2006-01-02T15:04:05.999999999Z07:00", &mut formatted);
        formatted.push('"');
        self.push_str(&formatted)
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
            let chunk = self.text.get(run..self.pos).unwrap_or("");
            if !chunk.is_empty() {
                alloc(self.f, chunk.len(), self.span)?;
                out.push_str(chunk);
            }
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
                let c = c.unwrap_or(char::REPLACEMENT_CHARACTER);
                alloc(self.f, c.len_utf8(), self.span)?;
                out.push(c);
                return Ok(());
            }
            Some(_) => return Err(self.fail("JSON, unexpected text")),
        };
        self.bump();
        alloc(self.f, c.len_utf8(), self.span)?;
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
            if !out.contains_key(key.as_str()) {
                alloc(self.f, 1, self.span)?;
            }
            out.insert(key.into(), x);
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
