//! Builtin implementations that are more than one `ops` call, and Go's text forms
//! of values (`string(x)`).

use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt::Write;

use dictum_check::ir::BitOp;

use crate::EvalErrorKind;

fn invalid(what: &'static str, value: &str) -> EvalErrorKind {
    EvalErrorKind::InvalidArgument { what, value: value.into() }
}

/// `mean`: `0.0` of no numbers, as Go.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() { 0.0 } else { xs.iter().sum::<f64>() / xs.len() as f64 }
}

/// `median`: the middle number, or the mean of the two middle ones; `0.0` of no
/// numbers. Sorted as Go's `sort.Float64s`, NaN first.
pub(crate) fn median(xs: &mut [f64]) -> f64 {
    xs.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or_else(|| b.is_nan().cmp(&a.is_nan())));
    let half = xs.len() / 2;
    let mid = |i: usize| xs.get(i).copied().unwrap_or(0.0);
    if xs.len() % 2 == 1 { mid(half) } else { (mid(half.saturating_sub(1)) + mid(half)) / 2.0 }
}

/// A stable merge sort with a comparison that can fail; the first failure stops it.
/// `slice::sort_by` would need a comparison that cannot fail, and may panic on one
/// that is not a total order.
pub(crate) fn sort_by<T, E>(mut xs: Vec<T>, cmp: &mut impl FnMut(&T, &T) -> Result<Ordering, E>) -> Result<Vec<T>, E> {
    if xs.len() < 2 {
        return Ok(xs);
    }
    let right = xs.split_off(xs.len() / 2);
    let (left, right) = (sort_by(xs, cmp)?, sort_by(right, cmp)?);
    let mut out = Vec::with_capacity(left.len().saturating_add(right.len()));
    let (mut left, mut right) = (left.into_iter().peekable(), right.into_iter().peekable());
    while let (Some(l), Some(r)) = (left.peek(), right.peek()) {
        let next = if cmp(r, l)? == Ordering::Less { right.next() } else { left.next() };
        out.extend(next);
    }
    out.extend(left);
    out.extend(right);
    Ok(out)
}

/// A bit operation on Go's 64-bit `int`. Shifts wrap as Go's do, a shift of 64 or
/// more shifts every bit out, and a negative count is an invalid argument.
pub(crate) fn bit(op: BitOp, x: i64, y: i64) -> Result<i64, EvalErrorKind> {
    let shift = || match u32::try_from(y) {
        Ok(n) => Ok(n),
        Err(_) if y > 0 => Ok(u32::MAX),
        Err(_) => Err(invalid("shift count", &alloc::format!("{y}"))),
    };
    Ok(match op {
        BitOp::And => x & y,
        BitOp::Or => x | y,
        BitOp::Xor => x ^ y,
        BitOp::AndNot => x & !y,
        BitOp::Not => !x,
        BitOp::Shl => x.checked_shl(shift()?).unwrap_or(0),
        BitOp::Shr => x.checked_shr(shift()?).unwrap_or(if x < 0 { -1 } else { 0 }),
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_wrap)]
        BitOp::Ushr => (x as u64).checked_shr(shift()?).unwrap_or(0) as i64,
    })
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding, as Go's `base64.StdEncoding.EncodeToString`.
pub(crate) fn base64_encode(bytes: &[u8], out: &mut String) {
    for chunk in bytes.chunks(3) {
        let byte = |i: usize| u32::from(chunk.get(i).copied().unwrap_or(0));
        let n = byte(0).wrapping_shl(16) | byte(1).wrapping_shl(8) | byte(2);
        for i in 0..4u32 {
            if usize::try_from(i).is_ok_and(|i| i <= chunk.len()) {
                let six = n.wrapping_shr(18_u32.wrapping_sub(i.wrapping_mul(6))) & 0x3f;
                out.push(char::from(BASE64.get(usize::try_from(six).unwrap_or(0)).copied().unwrap_or(b'=')));
            } else {
                out.push('=');
            }
        }
    }
}

/// Go's `base64.StdEncoding.DecodeString`: padding is required, `\r` and `\n` are
/// skipped anywhere, and the unused bits of the last character may be set. The error
/// is the byte offset where the input stops being valid.
pub(crate) fn base64_decode(text: &[u8]) -> Result<Vec<u8>, usize> {
    let sextet = |c: u8| match c {
        b'A'..=b'Z' => Some(c.wrapping_sub(b'A')),
        b'a'..=b'z' => Some(c.wrapping_sub(b'a').wrapping_add(26)),
        b'0'..=b'9' => Some(c.wrapping_sub(b'0').wrapping_add(52)),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let mut out = Vec::with_capacity((text.len() / 4).saturating_mul(3));
    let mut quantum: Vec<u8> = Vec::with_capacity(4);
    let (mut pads, mut done) = (0usize, false);
    let flush = |quantum: &[u8], out: &mut Vec<u8>| {
        let n = quantum.iter().enumerate().fold(0u32, |n, (i, &six)| {
            n | u32::from(six).wrapping_shl(18_u32.wrapping_sub(u32::try_from(i).unwrap_or(0).wrapping_mul(6)))
        });
        let bytes = [n.wrapping_shr(16), n.wrapping_shr(8), n].map(|b| (b & 0xff) as u8);
        out.extend(bytes.iter().take(quantum.len().saturating_sub(1)));
    };
    for (i, &c) in text.iter().enumerate() {
        if c == b'\n' || c == b'\r' {
            continue;
        }
        if done {
            return Err(i);
        }
        match sextet(c) {
            Some(six) if pads == 0 => {
                quantum.push(six);
                if quantum.len() == 4 {
                    flush(&quantum, &mut out);
                    quantum.clear();
                }
            }
            None if c == b'=' && quantum.len() >= 2 => {
                pads = pads.saturating_add(1);
                done = quantum.len().saturating_add(pads) == 4;
            }
            _ => return Err(i),
        }
    }
    if !quantum.is_empty() && !done {
        return Err(text.len());
    }
    flush(&quantum, &mut out);
    Ok(out)
}

/// At most 16 characters of `text` from byte `at`, to show where an argument stopped
/// being valid.
pub(crate) fn excerpt(text: &str, at: usize) -> &str {
    let rest = text.get(at..).unwrap_or("");
    rest.char_indices().nth(16).map_or(rest, |(end, _)| rest.get(..end).unwrap_or(rest))
}

/// Go's `strings.SplitN` (or `SplitAfterN` when `after`, which keeps each separator
/// on the part before it). `n < 0` splits at every separator, `n == 0` gives no
/// parts, and otherwise the last of at most `n` parts is the unsplit rest. An empty
/// separator splits between characters.
pub(crate) fn go_split<'s>(s: &'s str, sep: &str, after: bool, n: i64) -> Vec<&'s str> {
    let limit = match usize::try_from(n) {
        Ok(0) => return Vec::new(),
        Ok(n) => n,
        Err(_) => usize::MAX,
    };
    let mut parts = Vec::new();
    let mut rest = s;
    while parts.len().saturating_add(1) < limit {
        let (cut, skip) = if sep.is_empty() {
            match rest.chars().next() {
                Some(c) => (c.len_utf8(), c.len_utf8()),
                None => return parts,
            }
        } else {
            match rest.find(sep) {
                Some(m) => (if after { m.saturating_add(sep.len()) } else { m }, m.saturating_add(sep.len())),
                None => break,
            }
        };
        parts.push(rest.get(..cut).unwrap_or(""));
        rest = rest.get(skip..).unwrap_or("");
    }
    if !(sep.is_empty() && rest.is_empty()) {
        parts.push(rest);
    }
    parts
}

/// `duration(s)` with Go's `time.ParseDuration` grammar: an optional sign, then one
/// or more `<decimal><unit>` with units `ns us µs ms s m h` (`"1h30m"`, `"-1.5h"`).
/// Implemented here rather than through jiff's parser so the accepted language is
/// exactly Go's, including its range (an `i64` of nanoseconds).
pub(crate) fn parse_go_duration(s: &str) -> Result<jiff::SignedDuration, EvalErrorKind> {
    const LIMIT: u64 = 1 << 63;
    let bad = || invalid("duration", s);
    let (neg, mut rest) = match s.as_bytes().first() {
        Some(b'-') => (true, s.get(1..).unwrap_or("")),
        Some(b'+') => (false, s.get(1..).unwrap_or("")),
        _ => (false, s),
    };
    if rest == "0" {
        return Ok(jiff::SignedDuration::ZERO);
    }
    if rest.is_empty() {
        return Err(bad());
    }
    let mut total: u64 = 0;
    while !rest.is_empty() {
        let digits = |t: &str| t.bytes().take_while(u8::is_ascii_digit).count();
        // Integer part: an overflow is an error.
        let n = digits(rest);
        let mut whole: u64 = 0;
        for c in rest.bytes().take(n) {
            whole =
                whole.checked_mul(10).and_then(|w| w.checked_add(u64::from(c.saturating_sub(b'0')))).ok_or_else(bad)?;
            if whole > LIMIT {
                return Err(bad());
            }
        }
        rest = rest.get(n..).unwrap_or("");
        // Fraction: digits past what fits are dropped, as Go does.
        let (mut frac, mut scale, mut post) = (0u64, 1f64, false);
        if let Some(after) = rest.strip_prefix('.') {
            let m = digits(after);
            post = m > 0;
            let mut overflow = false;
            for c in after.bytes().take(m) {
                let next = frac.checked_mul(10).and_then(|f| f.checked_add(u64::from(c.saturating_sub(b'0'))));
                match next {
                    Some(y) if !overflow && y <= LIMIT => {
                        frac = y;
                        scale *= 10.0;
                    }
                    _ => overflow = true,
                }
            }
            rest = after.get(m..).unwrap_or("");
        }
        if n == 0 && !post {
            return Err(bad());
        }
        let unit_len = rest.find(|c: char| c == '.' || c.is_ascii_digit()).unwrap_or(rest.len());
        let unit: u64 = match rest.get(..unit_len).unwrap_or("") {
            "ns" => 1,
            "us" | "\u{b5}s" | "\u{3bc}s" => 1_000,
            "ms" => 1_000_000,
            "s" => 1_000_000_000,
            "m" => 60_000_000_000,
            "h" => 3_600_000_000_000,
            _ => return Err(bad()),
        };
        rest = rest.get(unit_len..).unwrap_or("");
        let mut v = whole.checked_mul(unit).filter(|v| *v <= LIMIT).ok_or_else(bad)?;
        if frac > 0 {
            // As Go: the fraction goes through float64, truncated.
            #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let part = (frac as f64 * (unit as f64 / scale)) as u64;
            v = v.checked_add(part).filter(|v| *v <= LIMIT).ok_or_else(bad)?;
        }
        total = total.checked_add(v).filter(|t| *t <= LIMIT).ok_or_else(bad)?;
    }
    let nanos = if neg {
        0i64.checked_sub_unsigned(total).ok_or_else(bad)?
    } else {
        i64::try_from(total).map_err(|_| bad())?
    };
    Ok(jiff::SignedDuration::from_nanos(nanos))
}

/// `date(s)`: RFC 3339 with offset, or `YYYY-MM-DD` / `YYYY-MM-DDTHH:MM:SS` read as
/// UTC (no time zone database is needed, so this works in `no_std`). Go layouts and
/// time zones (`date(s, layout, tz)`) are post-MVP.
pub(crate) fn parse_date(s: &str) -> Result<jiff::Timestamp, EvalErrorKind> {
    if let Ok(t) = s.parse::<jiff::Timestamp>() {
        return Ok(t);
    }
    let civil = s.parse::<jiff::civil::DateTime>().or_else(|_| s.parse::<jiff::civil::Date>().map(Into::into));
    civil.ok().and_then(|dt| jiff::tz::Offset::UTC.to_timestamp(dt).ok()).ok_or_else(|| invalid("date", s))
}

/// A float as Go's `%v` prints it (`strconv.FormatFloat(x, 'g', -1, 64)`): the
/// shortest digits that read back exactly, in exponent form when the decimal
/// exponent is below -4 or at least 6 (`1e+06`, `123456`, `1.5e-07`).
pub(crate) fn go_float(x: f64, out: &mut String) {
    if x.is_nan() {
        out.push_str("NaN");
        return;
    }
    if x.is_infinite() {
        out.push_str(if x > 0.0 { "+Inf" } else { "-Inf" });
        return;
    }
    // Rust's `{:e}` gives the shortest round-trip digits: `-1.2345e6`.
    let sci = alloc::format!("{x:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let (sign, mantissa) = mantissa.strip_prefix('-').map_or(("", mantissa), |m| ("-", m));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    out.push_str(sign);
    if !(-4..6).contains(&exp) {
        let (first, rest) = digits.split_at(1.min(digits.len()));
        out.push_str(first);
        if !rest.is_empty() {
            out.push('.');
            out.push_str(rest);
        }
        let _ = write!(out, "e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.unsigned_abs());
    } else if exp < 0 {
        out.push_str("0.");
        for _ in 1..exp.unsigned_abs() {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        let int_len = usize::try_from(exp).unwrap_or(0).saturating_add(1);
        if digits.len() <= int_len {
            out.push_str(&digits);
            for _ in digits.len()..int_len {
                out.push('0');
            }
        } else {
            let (int, frac) = digits.split_at(int_len);
            out.push_str(int);
            out.push('.');
            out.push_str(frac);
        }
    }
}

/// A duration as Go's `time.Duration.String` prints it: `1h30m0s`, `1.5s`,
/// `300ms`, `0s`.
pub(crate) fn go_duration(d: jiff::SignedDuration, out: &mut String) {
    let nanos = d.as_nanos();
    if nanos < 0 {
        out.push('-');
    }
    let u = nanos.unsigned_abs();
    // `v` with `digits` decimals as Go's fmtFrac: trailing zeros and a bare `.` dropped.
    let frac = |out: &mut String, v: u128, digits: u32| {
        let p = 10u128.pow(digits);
        let (int, f) = (v.checked_div(p).unwrap_or(0), v.checked_rem(p).unwrap_or(0));
        let _ = write!(out, "{int}");
        if f > 0 {
            let mut f = alloc::format!("{f:0width$}", width = digits as usize);
            while f.ends_with('0') {
                f.pop();
            }
            out.push('.');
            out.push_str(&f);
        }
    };
    if u == 0 {
        out.push_str("0s");
    } else if u < 1_000 {
        let _ = write!(out, "{u}ns");
    } else if u < 1_000_000 {
        frac(out, u, 3);
        out.push_str("\u{b5}s");
    } else if u < 1_000_000_000 {
        frac(out, u, 6);
        out.push_str("ms");
    } else {
        let secs = u.checked_div(1_000_000_000).unwrap_or(0);
        let (h, m) =
            (secs.checked_div(3600).unwrap_or(0), secs.checked_div(60).unwrap_or(0).checked_rem(60).unwrap_or(0));
        if h > 0 {
            let _ = write!(out, "{h}h");
        }
        if h > 0 || m > 0 {
            let _ = write!(out, "{m}m");
        }
        frac(out, u.checked_rem(60_000_000_000).unwrap_or(0), 9);
        out.push('s');
    }
}

/// A time as Go's `time.Time.String` prints a UTC time:
/// `2026-01-01 00:00:00 +0000 UTC`, with a fraction only when nonzero.
pub(crate) fn go_time(t: jiff::Timestamp, out: &mut String) {
    let dt = jiff::tz::Offset::UTC.to_datetime(t);
    let _ = write!(
        out,
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        dt.year(),
        dt.month(),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second()
    );
    let nanos = dt.subsec_nanosecond();
    if nanos > 0 {
        let mut f = alloc::format!("{nanos:09}");
        while f.ends_with('0') {
            f.pop();
        }
        out.push('.');
        out.push_str(&f);
    }
    out.push_str(" +0000 UTC");
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use jiff::SignedDuration;

    fn dur(s: &str) -> Option<i128> {
        parse_go_duration(s).ok().map(|d| d.as_nanos())
    }

    #[test]
    fn go_duration_grammar() {
        assert_eq!(dur("0"), Some(0));
        assert_eq!(dur("-0"), Some(0));
        assert_eq!(dur("1h30m"), Some(5_400_000_000_000));
        assert_eq!(dur("-1.5h"), Some(-5_400_000_000_000));
        assert_eq!(dur("300ms"), Some(300_000_000));
        assert_eq!(dur("1.5us"), Some(1_500));
        assert_eq!(dur("1\u{b5}s"), Some(1_000));
        assert_eq!(dur(".5s"), Some(500_000_000));
        assert_eq!(dur("1.s"), Some(1_000_000_000));
        assert_eq!(dur("2h45m30.5s"), Some(9_930_500_000_000));
        assert_eq!(dur("9223372036854775807ns"), Some(i128::from(i64::MAX)));
        assert_eq!(dur("-9223372036854775808ns"), Some(i128::from(i64::MIN)));
        for bad in ["", "1", "1x", "x1s", ".s", "-", "1h-1m", "9223372036854775808ns", "3000000h"] {
            assert_eq!(dur(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn dates_with_and_without_offsets() {
        let t = |s| parse_date(s).ok().map(jiff::Timestamp::as_second);
        assert_eq!(t("2026-01-01"), Some(1_767_225_600));
        assert_eq!(t("2026-01-01T00:00:00"), Some(1_767_225_600));
        assert_eq!(t("2026-01-01T09:00:00+09:00"), Some(1_767_225_600));
        assert_eq!(t("2026-01-01T00:00:00Z"), Some(1_767_225_600));
        assert_eq!(t("01/02/2026"), None);
    }

    #[test]
    fn floats_print_as_go_v() {
        let f = |x: f64| {
            let mut s = String::new();
            go_float(x, &mut s);
            s
        };
        assert_eq!(f(3.0), "3");
        assert_eq!(f(-0.0), "-0");
        assert_eq!(f(1.5), "1.5");
        assert_eq!(f(123_456.0), "123456");
        assert_eq!(f(1e6), "1e+06");
        assert_eq!(f(1_234_567.0), "1.234567e+06");
        assert_eq!(f(0.0001), "0.0001");
        assert_eq!(f(0.00001), "1e-05");
        assert_eq!(f(1.5e-7), "1.5e-07");
        assert_eq!(f(1e100), "1e+100");
        assert_eq!(f(f64::NAN), "NaN");
        assert_eq!(f(f64::NEG_INFINITY), "-Inf");
    }

    #[test]
    fn durations_print_as_go() {
        let d = |n: i64| {
            let mut s = String::new();
            go_duration(SignedDuration::from_nanos(n), &mut s);
            s
        };
        assert_eq!(d(0), "0s");
        assert_eq!(d(5), "5ns");
        assert_eq!(d(1_500), "1.5\u{b5}s");
        assert_eq!(d(300_000_000), "300ms");
        assert_eq!(d(1_500_000_000), "1.5s");
        assert_eq!(d(3_600_000_000_000), "1h0m0s");
        assert_eq!(d(-5_400_000_000_000), "-1h30m0s");
        assert_eq!(d(61_000_000_000), "1m1s");
    }

    #[test]
    fn times_print_as_go_utc() {
        let mut s = String::new();
        go_time(jiff::Timestamp::from_second(1_767_225_600).unwrap(), &mut s);
        assert_eq!(s, "2026-01-01 00:00:00 +0000 UTC");
    }

    #[test]
    fn base64_round_trips_any_bytes() {
        let bytes: Vec<u8> = (0..=255).rev().collect();
        for len in 0..=bytes.len() {
            let prefix = bytes.get(..len).unwrap();
            let mut text = String::new();
            base64_encode(prefix, &mut text);
            assert_eq!(text.len(), len.div_ceil(3).saturating_mul(4));
            assert_eq!(base64_decode(text.as_bytes()), Ok(prefix.to_vec()), "{text}");
        }
    }
}
