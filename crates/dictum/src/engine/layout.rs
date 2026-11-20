//! Go's time layouts (`time.Format`, `time.ParseInLocation`), ported from Go's
//! `time/format.go` so that `date(s, layout)` and `t.Format(layout)` accept and
//! print exactly what Go's do. A layout is the reference time
//! `Mon Jan 2 15:04:05 MST 2006` written the way the text looks.

use alloc::string::String;
use core::fmt::Write;

use jiff::Timestamp;
use jiff::tz::{Offset, TimeZone};

use super::builtins;

/// One element of a layout (Go's `std*` constants).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Chunk {
    LongMonth,
    Month,
    NumMonth,
    ZeroMonth,
    LongWeekDay,
    WeekDay,
    Day,
    UnderDay,
    ZeroDay,
    UnderYearDay,
    ZeroYearDay,
    Hour,
    Hour12,
    ZeroHour12,
    Minute,
    ZeroMinute,
    Second,
    ZeroSecond,
    LongYear,
    Year,
    UpperPm,
    LowerPm,
    ZoneName,
    Offset(OffsetForm),
    /// `.000` (`nines: false`, exact digits) or `.999` (trailing zeros trimmed).
    Fraction {
        nines: bool,
        digits: usize,
        comma: bool,
    },
}

/// The ten numeric offset forms: `-0700`, `Z07:00` and the rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OffsetForm {
    /// `Z...`: a zero offset is written `Z`.
    iso: bool,
    colon: bool,
    minutes: bool,
    seconds: bool,
}

const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Go's `nextStdChunk`: the text before the first element, the element, and the rest.
fn next_chunk(layout: &[u8]) -> (&[u8], Option<Chunk>, &[u8]) {
    let starts = |i: usize, s: &str| layout.get(i..).is_some_and(|r| r.starts_with(s.as_bytes()));
    let lower_at = |i: usize| layout.get(i).is_some_and(u8::is_ascii_lowercase);
    let cut = |i: usize, c: Chunk, len: usize| {
        let (before, rest) = layout.split_at(i);
        (before, Some(c), rest.get(len..).unwrap_or(&[]))
    };
    let offset = |iso, colon, minutes, seconds| Chunk::Offset(OffsetForm { iso, colon, minutes, seconds });
    for (i, &c) in layout.iter().enumerate() {
        match c {
            b'J' if starts(i, "Jan") => {
                if starts(i, "January") {
                    return cut(i, Chunk::LongMonth, 7);
                }
                if !lower_at(i.saturating_add(3)) {
                    return cut(i, Chunk::Month, 3);
                }
            }
            b'M' => {
                if starts(i, "Mon") {
                    if starts(i, "Monday") {
                        return cut(i, Chunk::LongWeekDay, 6);
                    }
                    if !lower_at(i.saturating_add(3)) {
                        return cut(i, Chunk::WeekDay, 3);
                    }
                }
                if starts(i, "MST") {
                    return cut(i, Chunk::ZoneName, 3);
                }
            }
            b'0' => {
                let zeroed = [
                    Chunk::ZeroMonth,
                    Chunk::ZeroDay,
                    Chunk::ZeroHour12,
                    Chunk::ZeroMinute,
                    Chunk::ZeroSecond,
                    Chunk::Year,
                ];
                if let Some(&d @ b'1'..=b'6') = layout.get(i.saturating_add(1)) {
                    let c = zeroed.get(usize::from(d.saturating_sub(b'1'))).copied().unwrap_or(Chunk::Year);
                    return cut(i, c, 2);
                }
                if starts(i, "002") {
                    return cut(i, Chunk::ZeroYearDay, 3);
                }
            }
            b'1' => return if starts(i, "15") { cut(i, Chunk::Hour, 2) } else { cut(i, Chunk::NumMonth, 1) },
            b'2' => return if starts(i, "2006") { cut(i, Chunk::LongYear, 4) } else { cut(i, Chunk::Day, 1) },
            b'_' => {
                if starts(i, "_2") {
                    // `_2006` is a literal `_` and the year.
                    if starts(i, "_2006") {
                        let (before, rest) = layout.split_at(i.saturating_add(1));
                        return (before, Some(Chunk::LongYear), rest.get(4..).unwrap_or(&[]));
                    }
                    return cut(i, Chunk::UnderDay, 2);
                }
                if starts(i, "__2") {
                    return cut(i, Chunk::UnderYearDay, 3);
                }
            }
            b'3' => return cut(i, Chunk::Hour12, 1),
            b'4' => return cut(i, Chunk::Minute, 1),
            b'5' => return cut(i, Chunk::Second, 1),
            b'P' if starts(i, "PM") => return cut(i, Chunk::UpperPm, 2),
            b'p' if starts(i, "pm") => return cut(i, Chunk::LowerPm, 2),
            b'-' | b'Z' => {
                let iso = c == b'Z';
                let forms: [(&str, bool, bool, bool); 5] = [
                    ("070000", false, true, true),
                    ("07:00:00", true, true, true),
                    ("0700", false, true, false),
                    ("07:00", true, true, false),
                    ("07", false, false, false),
                ];
                for (text, colon, minutes, seconds) in forms {
                    if layout.get(i.saturating_add(1)..).is_some_and(|r| r.starts_with(text.as_bytes())) {
                        return cut(i, offset(iso, colon, minutes, seconds), text.len().saturating_add(1));
                    }
                }
            }
            b'.' | b',' => {
                if let Some(&d @ (b'0' | b'9')) = layout.get(i.saturating_add(1)) {
                    let run =
                        layout.get(i.saturating_add(1)..).map_or(0, |r| r.iter().take_while(|&&x| x == d).count());
                    let end = i.saturating_add(1).saturating_add(run);
                    // Only a fraction that is all digits counts.
                    if !layout.get(end).is_some_and(u8::is_ascii_digit) {
                        let f = Chunk::Fraction { nines: d == b'9', digits: run, comma: c == b',' };
                        return cut(i, f, run.saturating_add(1));
                    }
                }
            }
            _ => {}
        }
    }
    (layout, None, &[])
}

/// Go's `appendInt`: `x` zero-padded to `width` digits, the sign in front.
fn push_int(out: &mut String, x: i64, width: usize) {
    if x < 0 {
        out.push('-');
    }
    let _ = write!(out, "{:0width$}", x.unsigned_abs());
}

/// `t.Format(layout)` for `t` shown in zone `z`.
pub(crate) fn format(t: Timestamp, z: &TimeZone, layout: &str, out: &mut String) {
    let (dt, offset, name) = builtins::zoned(t, z);
    let (year, month, day) = (i64::from(dt.year()), usize::from(dt.month().unsigned_abs()), i64::from(dt.day()));
    let (hour, minute, second) = (i64::from(dt.hour()), i64::from(dt.minute()), i64::from(dt.second()));
    let weekday = usize::from(dt.weekday().to_sunday_zero_offset().unsigned_abs());
    let yday = i64::from(dt.day_of_year());
    let month_name = MONTHS.get(month.saturating_sub(1)).copied().unwrap_or("");
    let day_name = DAYS.get(weekday).copied().unwrap_or("");
    let twelve = if hour % 12 == 0 { 12 } else { hour % 12 };
    let mut rest = layout.as_bytes();
    loop {
        let (prefix, chunk, suffix) = next_chunk(rest);
        out.push_str(core::str::from_utf8(prefix).unwrap_or(""));
        let Some(chunk) = chunk else { break };
        rest = suffix;
        match chunk {
            Chunk::Year => push_int(out, year.abs() % 100, 2),
            Chunk::LongYear => push_int(out, year, 4),
            Chunk::Month => out.push_str(month_name.get(..3).unwrap_or("")),
            Chunk::LongMonth => out.push_str(month_name),
            Chunk::NumMonth => push_int(out, i64::try_from(month).unwrap_or(0), 0),
            Chunk::ZeroMonth => push_int(out, i64::try_from(month).unwrap_or(0), 2),
            Chunk::WeekDay => out.push_str(day_name.get(..3).unwrap_or("")),
            Chunk::LongWeekDay => out.push_str(day_name),
            Chunk::Day => push_int(out, day, 0),
            Chunk::UnderDay => {
                if day < 10 {
                    out.push(' ');
                }
                push_int(out, day, 0);
            }
            Chunk::ZeroDay => push_int(out, day, 2),
            Chunk::UnderYearDay => {
                for limit in [100, 10] {
                    if yday < limit {
                        out.push(' ');
                    }
                }
                push_int(out, yday, 0);
            }
            Chunk::ZeroYearDay => push_int(out, yday, 3),
            Chunk::Hour => push_int(out, hour, 2),
            Chunk::Hour12 => push_int(out, twelve, 0),
            Chunk::ZeroHour12 => push_int(out, twelve, 2),
            Chunk::Minute => push_int(out, minute, 0),
            Chunk::ZeroMinute => push_int(out, minute, 2),
            Chunk::Second => push_int(out, second, 0),
            Chunk::ZeroSecond => push_int(out, second, 2),
            Chunk::UpperPm => out.push_str(if hour >= 12 { "PM" } else { "AM" }),
            Chunk::LowerPm => out.push_str(if hour >= 12 { "pm" } else { "am" }),
            Chunk::Offset(form) => {
                if offset == 0 && form.iso {
                    out.push('Z');
                    continue;
                }
                let abs = i64::from(offset).abs();
                out.push(if offset < 0 { '-' } else { '+' });
                push_int(out, abs / 3600, 2);
                if form.minutes {
                    if form.colon {
                        out.push(':');
                    }
                    push_int(out, abs / 60 % 60, 2);
                }
                if form.seconds {
                    if form.colon {
                        out.push(':');
                    }
                    push_int(out, abs % 60, 2);
                }
            }
            Chunk::ZoneName => match &name {
                Some(name) => out.push_str(name),
                None => builtins::go_offset(offset, false, out),
            },
            Chunk::Fraction { nines, digits, comma } => {
                let nanos = dt.subsec_nanosecond();
                if nines && (digits == 0 || nanos == 0) {
                    continue;
                }
                let mut text = alloc::format!("{nanos:09}");
                text.truncate(digits.min(9));
                if nines {
                    let trimmed = text.trim_end_matches('0').len();
                    text.truncate(trimmed);
                    if text.is_empty() {
                        continue;
                    }
                }
                out.push(if comma { ',' } else { '.' });
                out.push_str(&text);
            }
        }
    }
}

/// Go's `ParseInLocation(layout, value, loc)`. `None` when the value does not match.
pub(crate) fn parse(layout: &str, value: &str, loc: &TimeZone) -> Option<(Timestamp, TimeZone)> {
    let mut p = Parsed { year: 0, month: -1, day: -1, yday: -1, hour: 0, min: 0, sec: 0, nsec: 0 };
    let (mut am, mut pm) = (false, false);
    let mut utc = false;
    let mut zone_offset: Option<i64> = None;
    let mut zone_name: &[u8] = &[];
    let mut layout = layout.as_bytes();
    let mut v = value.as_bytes();
    loop {
        let (prefix, chunk, suffix) = next_chunk(layout);
        v = skip(v, prefix)?;
        let Some(chunk) = chunk else {
            if !v.is_empty() {
                return None;
            }
            break;
        };
        layout = suffix;
        match chunk {
            Chunk::Year => {
                // Go's `atoi` takes a sign, so `+1` is the year 2001.
                let (n, rest) = (v.get(..2)?, v.get(2..)?);
                let y = atoi_signed(n)?;
                p.year = y.saturating_add(if y >= 69 { 1900 } else { 2000 });
                v = rest;
            }
            Chunk::LongYear => {
                if !v.first().is_some_and(u8::is_ascii_digit) {
                    return None;
                }
                p.year = atoi(v.get(..4)?)?;
                v = v.get(4..)?;
            }
            Chunk::Month | Chunk::LongMonth => {
                let short = chunk == Chunk::Month;
                let (i, rest) = lookup(v, MONTHS.iter().map(|m| if short { m.get(..3).unwrap_or("") } else { m }))?;
                p.month = i64::try_from(i).ok()?.saturating_add(1);
                v = rest;
            }
            Chunk::NumMonth | Chunk::ZeroMonth => {
                let (m, rest) = getnum(v, chunk == Chunk::ZeroMonth)?;
                if !(1..=12).contains(&m) {
                    return None;
                }
                (p.month, v) = (m, rest);
            }
            Chunk::WeekDay | Chunk::LongWeekDay => {
                let short = chunk == Chunk::WeekDay;
                v = lookup(v, DAYS.iter().map(|d| if short { d.get(..3).unwrap_or("") } else { d }))?.1;
            }
            Chunk::Day | Chunk::UnderDay | Chunk::ZeroDay => {
                if chunk == Chunk::UnderDay && v.first() == Some(&b' ') {
                    v = v.get(1..)?;
                }
                (p.day, v) = getnum(v, chunk == Chunk::ZeroDay)?;
            }
            Chunk::UnderYearDay | Chunk::ZeroYearDay => {
                for _ in 0..2 {
                    if chunk == Chunk::UnderYearDay && v.first() == Some(&b' ') {
                        v = v.get(1..)?;
                    }
                }
                (p.yday, v) = getnum3(v, chunk == Chunk::ZeroYearDay)?;
            }
            Chunk::Hour => {
                (p.hour, v) = getnum(v, false)?;
                if p.hour >= 24 {
                    return None;
                }
            }
            Chunk::Hour12 | Chunk::ZeroHour12 => {
                (p.hour, v) = getnum(v, chunk == Chunk::ZeroHour12)?;
                if p.hour > 12 {
                    return None;
                }
            }
            Chunk::Minute | Chunk::ZeroMinute => {
                (p.min, v) = getnum(v, chunk == Chunk::ZeroMinute)?;
                if p.min >= 60 {
                    return None;
                }
            }
            Chunk::Second | Chunk::ZeroSecond => {
                (p.sec, v) = getnum(v, chunk == Chunk::ZeroSecond)?;
                if p.sec >= 60 {
                    return None;
                }
                // A fraction in the value that the layout does not name.
                if v.len() >= 2 && matches!(v.first(), Some(b'.' | b',')) && v.get(1).is_some_and(u8::is_ascii_digit) {
                    let next = next_chunk(layout).1;
                    if !matches!(next, Some(Chunk::Fraction { .. })) {
                        let n = v
                            .get(1..)
                            .map_or(0, |r| r.iter().take_while(|c| c.is_ascii_digit()).count())
                            .saturating_add(1);
                        p.nsec = nanoseconds(v, n)?;
                        v = v.get(n..)?;
                    }
                }
            }
            Chunk::UpperPm | Chunk::LowerPm => {
                let (word, rest) = (v.get(..2)?, v.get(2..)?);
                let (a, b) = if chunk == Chunk::UpperPm { (b"AM", b"PM") } else { (b"am", b"pm") };
                if word == a {
                    am = true;
                } else if word == b {
                    pm = true;
                } else {
                    return None;
                }
                v = rest;
            }
            Chunk::Offset(form) => {
                if form.iso && v.first() == Some(&b'Z') {
                    v = v.get(1..)?;
                    utc = true;
                    continue;
                }
                let (sign, h, m, s, len) = match (form.colon, form.minutes, form.seconds) {
                    (true, true, false) => {
                        (v.first()?, v.get(1..3)?, v.get(4..6)?, &b"00"[..], (v.get(3) == Some(&b':')).then_some(6)?)
                    }
                    (_, false, _) => (v.first()?, v.get(1..3)?, &b"00"[..], &b"00"[..], 3),
                    (true, _, true) => {
                        let ok = v.get(3) == Some(&b':') && v.get(6) == Some(&b':');
                        (v.first()?, v.get(1..3)?, v.get(4..6)?, v.get(7..9)?, ok.then_some(9)?)
                    }
                    (false, _, true) => (v.first()?, v.get(1..3)?, v.get(3..5)?, v.get(5..7)?, 7),
                    (false, true, false) => (v.first()?, v.get(1..3)?, v.get(3..5)?, &b"00"[..], 5),
                };
                let (hr, mm, ss) = (getnum(h, true)?.0, getnum(m, true)?.0, getnum(s, true)?.0);
                if hr > 24 || mm > 60 || ss > 60 {
                    return None;
                }
                let off = hr.saturating_mul(3600).saturating_add(mm.saturating_mul(60)).saturating_add(ss);
                zone_offset = Some(match sign {
                    b'+' => off,
                    b'-' => off.saturating_neg(),
                    _ => return None,
                });
                v = v.get(len..)?;
            }
            Chunk::ZoneName => {
                if v.starts_with(b"UTC") {
                    utc = true;
                    v = v.get(3..)?;
                    continue;
                }
                let n = zone_name_len(v)?;
                (zone_name, v) = (v.get(..n)?, v.get(n..)?);
            }
            Chunk::Fraction { nines: false, digits, .. } => {
                let n = digits.saturating_add(1);
                if v.len() < n {
                    return None;
                }
                p.nsec = nanoseconds(v, n)?;
                v = v.get(n..)?;
            }
            Chunk::Fraction { nines: true, .. } => {
                if v.len() < 2 || !matches!(v.first(), Some(b'.' | b',')) || !v.get(1).is_some_and(u8::is_ascii_digit) {
                    continue;
                }
                let n = v.get(1..).map_or(0, |r| r.iter().take_while(|c| c.is_ascii_digit()).count()).saturating_add(1);
                p.nsec = nanoseconds(v, n)?;
                v = v.get(n..)?;
            }
        }
    }
    if pm && p.hour < 12 {
        p.hour = p.hour.saturating_add(12);
    } else if am && p.hour == 12 {
        p.hour = 0;
    }
    p.resolve_day()?;
    if utc {
        return Some((date(&p, &TimeZone::UTC)?, TimeZone::UTC));
    }
    if let Some(offset) = zone_offset {
        let t = Timestamp::new(p.unix_utc()?.checked_sub(offset)?, p.nanos()?).ok()?;
        let info = loc.to_offset_info(t);
        let named = zone_name.is_empty() || info.abbreviation().as_bytes() == zone_name;
        if i64::from(info.offset().seconds()) == offset && named {
            return Some((t, loc.clone()));
        }
        return Some((t, TimeZone::fixed(Offset::from_seconds(i32::try_from(offset).ok()?).ok()?)));
    }
    if !zone_name.is_empty() {
        let t = Timestamp::new(p.unix_utc()?, p.nanos()?).ok()?;
        if let Some(offset) = offset_named(loc, zone_name, t) {
            return Some((Timestamp::new(t.as_second().checked_sub(offset)?, p.nanos()?).ok()?, loc.clone()));
        }
        // Go fabricates a zone with this name, so the instant is the civil time as UTC
        // and only `GMT+3` and the like carry an offset; dictum cannot name it.
        let mut offset = 0;
        if let Some(hours) = zone_name.strip_prefix(b"GMT").filter(|h| !h.is_empty()) {
            offset = atoi_signed(hours)?.saturating_mul(3600);
        }
        return Some((t, TimeZone::fixed(Offset::from_seconds(i32::try_from(offset).ok()?).ok()?)));
    }
    Some((date(&p, loc)?, loc.clone()))
}

struct Parsed {
    year: i64,
    month: i64,
    day: i64,
    yday: i64,
    hour: i64,
    min: i64,
    sec: i64,
    nsec: i64,
}

impl Parsed {
    /// Month and day from the day of the year, or the defaults; then the range check.
    fn resolve_day(&mut self) -> Option<()> {
        if self.yday >= 0 {
            let (mut yday, mut m, mut d) = (self.yday, 0, 0);
            if is_leap(self.year) {
                if yday == 31 + 29 {
                    (m, d) = (2, 29);
                } else if yday > 31 + 29 {
                    yday = yday.saturating_sub(1);
                }
            }
            if !(1..=365).contains(&yday) {
                return None;
            }
            if m == 0 {
                m = yday.saturating_sub(1).div_euclid(31).saturating_add(1);
                if days_before(m.saturating_add(1)) < yday {
                    m = m.saturating_add(1);
                }
                d = yday.saturating_sub(days_before(m));
            }
            if (self.month >= 0 && self.month != m) || (self.day >= 0 && self.day != d) {
                return None;
            }
            (self.month, self.day) = (m, d);
        } else {
            if self.month < 0 {
                self.month = 1;
            }
            if self.day < 0 {
                self.day = 1;
            }
        }
        (self.day >= 1 && self.day <= days_in(self.month, self.year)).then_some(())
    }

    fn unix_utc(&self) -> Option<i64> {
        civil_seconds(self.year, self.month, self.day, self.hour, self.min, self.sec)
    }

    fn nanos(&self) -> Option<i32> {
        i32::try_from(self.nsec).ok()
    }
}

/// Go's `time.Date(...)` for in-range fields: the civil time read in `loc`, with
/// Go's choice of offset around a transition.
fn date(p: &Parsed, loc: &TimeZone) -> Option<Timestamp> {
    let unix = p.unix_utc()?;
    Timestamp::new(local_to_unix(unix, loc)?, p.nanos()?).ok()
}

/// Go's offset lookup in `time.Date`: the offset at the civil time read as UTC, and
/// when that is not zero, the offset in effect at the corrected instant.
pub(crate) fn local_to_unix(civil: i64, loc: &TimeZone) -> Option<i64> {
    let at = |s: i64| Timestamp::from_second(s).ok().map(|t| i64::from(loc.to_offset(t).seconds()));
    let first = at(civil)?;
    if first == 0 {
        return Some(civil);
    }
    let offset = at(civil.checked_sub(first)?)?;
    civil.checked_sub(offset)
}

/// Go's `Location.lookupName`: the offset of a zone abbreviation of `loc`, preferring
/// the one in effect at `t`.
fn offset_named(loc: &TimeZone, name: &[u8], t: Timestamp) -> Option<i64> {
    let mut seen: alloc::vec::Vec<i64> = alloc::vec::Vec::new();
    let mut fallback = None;
    let candidates = loc.preceding(t).take(1).chain(loc.following(t).take(1)).chain(loc.preceding(t).skip(1));
    let current = loc.to_offset_info(t);
    let mut check = |abbr: &str, offset: i64| {
        if abbr.as_bytes() != name || seen.contains(&offset) {
            return None;
        }
        seen.push(offset);
        fallback.get_or_insert(offset);
        let at = Timestamp::from_second(t.as_second().checked_sub(offset)?).ok()?;
        (loc.to_offset_info(at).abbreviation().as_bytes() == name).then(|| i64::from(loc.to_offset(at).seconds()))
    };
    if let Some(found) = check(current.abbreviation(), i64::from(current.offset().seconds())) {
        return Some(found);
    }
    for tr in candidates {
        if let Some(found) = check(tr.abbreviation(), i64::from(tr.offset().seconds())) {
            return Some(found);
        }
    }
    fallback
}

/// Seconds since the epoch of a civil time read as UTC.
fn civil_seconds(year: i64, month: i64, day: i64, hour: i64, min: i64, sec: i64) -> Option<i64> {
    let days = days_from_civil(year, month, day)?;
    let clock = hour.checked_mul(3600)?.checked_add(min.checked_mul(60)?)?.checked_add(sec)?;
    days.checked_mul(86_400)?.checked_add(clock)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`); `day` may be outside the month.
pub(crate) fn days_from_civil(year: i64, month: i64, day: i64) -> Option<i64> {
    let y = if month <= 2 { year.checked_sub(1)? } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = month.checked_add(9)?.rem_euclid(12);
    let doy = mp.checked_mul(153)?.checked_add(2)?.div_euclid(5);
    let doe =
        yoe.checked_mul(365)?.checked_add(yoe.div_euclid(4))?.checked_sub(yoe.div_euclid(100))?.checked_add(doy)?;
    era.checked_mul(146_097)?.checked_add(doe.checked_sub(719_468)?)?.checked_add(day.checked_sub(1)?)
}

const fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Days before the first of `month` in a non-leap year (Go's `daysBefore`).
fn days_before(month: i64) -> i64 {
    const BEFORE: [i64; 13] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334, 365];
    usize::try_from(month.saturating_sub(1)).ok().and_then(|i| BEFORE.get(i)).copied().unwrap_or(0)
}

fn days_in(month: i64, year: i64) -> i64 {
    if month == 2 && is_leap(year) {
        29
    } else {
        days_before(month.saturating_add(1)).saturating_sub(days_before(month))
    }
}

/// Go's `skip`: the literal `prefix`, where a run of spaces matches a run of spaces.
fn skip<'v>(mut v: &'v [u8], mut prefix: &[u8]) -> Option<&'v [u8]> {
    while let Some(&c) = prefix.first() {
        if c == b' ' {
            if v.first().is_some_and(|&x| x != b' ') {
                return None;
            }
            prefix = trim_spaces(prefix);
            v = trim_spaces(v);
            continue;
        }
        if v.first() != Some(&c) {
            return None;
        }
        (prefix, v) = (prefix.get(1..)?, v.get(1..)?);
    }
    Some(v)
}

fn trim_spaces(s: &[u8]) -> &[u8] {
    let n = s.iter().take_while(|&&c| c == b' ').count();
    s.get(n..).unwrap_or(&[])
}

/// Go's `lookup`: the first name that starts `v`, ignoring ASCII case.
fn lookup<'v, 'n>(v: &'v [u8], names: impl Iterator<Item = &'n str>) -> Option<(usize, &'v [u8])> {
    for (i, name) in names.enumerate() {
        if let Some(head) = v.get(..name.len()) {
            if head.eq_ignore_ascii_case(name.as_bytes()) {
                return Some((i, v.get(name.len()..)?));
            }
        }
    }
    None
}

/// Go's `getnum`: one or two digits (`fixed`: exactly two).
fn getnum(v: &[u8], fixed: bool) -> Option<(i64, &[u8])> {
    let digit = |i: usize| v.get(i).filter(|c| c.is_ascii_digit()).map(|c| i64::from(c.saturating_sub(b'0')));
    let first = digit(0)?;
    match digit(1) {
        Some(second) => Some((first.saturating_mul(10).saturating_add(second), v.get(2..)?)),
        None if fixed => None,
        None => Some((first, v.get(1..)?)),
    }
}

/// Go's `getnum3`: one to three digits (`fixed`: exactly three).
fn getnum3(v: &[u8], fixed: bool) -> Option<(i64, &[u8])> {
    let n = v.iter().take(3).take_while(|c| c.is_ascii_digit()).count();
    if n == 0 || (fixed && n != 3) {
        return None;
    }
    Some((atoi(v.get(..n)?)?, v.get(n..)?))
}

/// Unsigned decimal digits only.
fn atoi(s: &[u8]) -> Option<i64> {
    if s.is_empty() || !s.iter().all(u8::is_ascii_digit) {
        return None;
    }
    core::str::from_utf8(s).ok()?.parse().ok()
}

fn atoi_signed(s: &[u8]) -> Option<i64> {
    match s.split_first() {
        Some((b'-', rest)) => atoi(rest).map(i64::saturating_neg),
        Some((b'+', rest)) => atoi(rest),
        _ => atoi(s),
    }
}

/// Go's `parseNanoseconds`: `v[0]` is the separator, `v[1..n]` the digits, at most
/// nine of which count.
fn nanoseconds(v: &[u8], n: usize) -> Option<i64> {
    if !matches!(v.first(), Some(b'.' | b',')) {
        return None;
    }
    let n = n.min(10);
    let digits = v.get(1..n)?;
    let mut ns = atoi(digits)?;
    for _ in n..10 {
        ns = ns.saturating_mul(10);
    }
    Some(ns)
}

/// Go's `parseTimeZone`: the length of a zone abbreviation at the start of `v`.
fn zone_name_len(v: &[u8]) -> Option<usize> {
    if v.len() < 3 {
        return None;
    }
    if v.starts_with(b"ChST") || v.starts_with(b"MeST") {
        return Some(4);
    }
    if let Some(rest) = v.strip_prefix(b"GMT") {
        return Some(if rest.is_empty() { 0 } else { signed_offset_len(rest) }.saturating_add(3));
    }
    if matches!(v.first(), Some(b'+' | b'-')) {
        return Some(signed_offset_len(v)).filter(|&n| n > 0);
    }
    let upper = v.iter().take(6).take_while(|c| c.is_ascii_uppercase()).count();
    match upper {
        5 if v.get(4) == Some(&b'T') => Some(5),
        4 if v.get(3) == Some(&b'T') || v.starts_with(b"WITA") => Some(4),
        3 => Some(3),
        _ => None,
    }
}

/// Go's `parseSignedOffset`: a sign and an hour from -23 to +23, or 0.
fn signed_offset_len(v: &[u8]) -> usize {
    let Some((&sign, rest)) = v.split_first() else { return 0 };
    if sign != b'+' && sign != b'-' {
        return 0;
    }
    let n = rest.iter().take_while(|c| c.is_ascii_digit()).count();
    match rest.get(..n).and_then(atoi) {
        Some(x) if n > 0 && x <= 23 => n.saturating_add(1),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn tz(name: &str) -> TimeZone {
        builtins::load_location(name).unwrap()
    }

    fn fmt(secs: i64, nanos: i32, zone: &str, layout: &str) -> String {
        let mut s = String::new();
        format(Timestamp::new(secs, nanos).unwrap(), &tz(zone), layout, &mut s);
        s
    }

    // Expected strings are Go 1.27's `time.Unix(secs, nanos).In(zone).Format(layout)`.
    #[test]
    fn formats_as_go() {
        let t = 1_692_000_000; // 2023-08-14 08:00:00 UTC
        assert_eq!(fmt(t, 0, "UTC", "2006-01-02 15:04:05.999999999 -0700 MST"), "2023-08-14 08:00:00 +0000 UTC");
        assert_eq!(fmt(t, 0, "Europe/Zurich", "Mon Jan _2 15:04:05 MST 2006"), "Mon Aug 14 10:00:00 CEST 2023");
        assert_eq!(fmt(t, 120_000_000, "UTC", "15:04:05.000 .999 ,00"), "08:00:00.120 .12 ,12");
        assert_eq!(fmt(t, 0, "America/New_York", "3:04PM 03 pm Z07:00 -07 Z0700"), "4:00AM 04 am -04:00 -04 -0400");
        assert_eq!(fmt(t, 0, "UTC", "Z07:00 Z07 -07:00:00"), "Z Z +00:00:00");
        assert_eq!(fmt(t, 0, "UTC", "January Monday 002 __2 06 _2 1 2"), "August Monday 226 226 23 14 8 14");
        assert_eq!(fmt(0, 0, "UTC", "__2 _2006 Jan2"), "  1 _1970 Jan1");
        assert_eq!(fmt(0, 0, "UTC", "Janx Monx"), "Janx Monx");
        assert_eq!(fmt(0, 0, "UTC", "Janx Mondayy"), "Janx Thursdayy");
    }

    fn parsed(layout: &str, value: &str, zone: &str) -> Option<String> {
        let (t, z) = parse(layout, value, &tz(zone))?;
        let mut s = String::new();
        format(t, &z, "2006-01-02 15:04:05.999999999 -0700 MST", &mut s);
        Some(s)
    }

    #[test]
    fn parses_as_go() {
        let p = |l, v| parsed(l, v, "UTC");
        assert_eq!(p("2006-01-02", "2023-08-14").as_deref(), Some("2023-08-14 00:00:00 +0000 UTC"));
        assert_eq!(p("15:04:05", "08:30:00").as_deref(), Some("0000-01-01 08:30:00 +0000 UTC"));
        assert_eq!(
            p("2006-01-02T15:04:05Z07:00", "2023-08-14T09:00:00+09:00").as_deref(),
            Some("2023-08-14 09:00:00 +0900 +0900")
        );
        assert_eq!(
            p("2006-01-02T15:04:05Z07:00", "2023-08-14T09:00:00Z").as_deref(),
            Some("2023-08-14 09:00:00 +0000 UTC")
        );
        assert_eq!(
            p("2006-01-02 15:04:05", "2023-08-14 09:00:00.5").as_deref(),
            Some("2023-08-14 09:00:00.5 +0000 UTC")
        );
        assert_eq!(p("Jan _2 2006", "aug  4 2023").as_deref(), Some("2023-08-04 00:00:00 +0000 UTC"));
        assert_eq!(p("2006 002", "2024 060").as_deref(), Some("2024-02-29 00:00:00 +0000 UTC"));
        assert_eq!(p("3:04PM", "12:30AM").as_deref(), Some("0000-01-01 00:30:00 +0000 UTC"));
        assert_eq!(p("02/01/06", "31/12/68").as_deref(), Some("2068-12-31 00:00:00 +0000 UTC"));
        for (l, v) in [("2006-01-02", "2023-02-30"), ("2006-01-02", "2023-8-14"), ("15:04", "24:00"), ("2006", "2023x")]
        {
            assert_eq!(p(l, v), None, "{l} {v}");
        }
        let zurich = parsed("2006-01-02 15:04", "2023-08-14 10:00", "Europe/Zurich");
        assert_eq!(zurich.as_deref(), Some("2023-08-14 10:00:00 +0200 CEST"));
        let named = parsed("2006-01-02 15:04 MST", "2023-01-14 10:00 CET", "Europe/Zurich");
        assert_eq!(named.as_deref(), Some("2023-01-14 10:00:00 +0100 CET"));
    }
}
