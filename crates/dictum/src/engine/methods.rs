//! The methods of times, durations and locations, with the semantics of Go's
//! `time.Time`, `time.Duration` and `*time.Location`.

use alloc::string::String;

use dictum_check::ir::Method;
use dictum_syntax::Span;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};

use super::frame::{Fault, Frame, Res, Text, at, internal};
use super::layout;
use super::sprint::go_string;
use super::value::{Repr, Value};
use crate::EvalErrorKind;

/// Seconds from 1970 to January 1, year 1, 00:00 UTC: Go's zero time.
const ZERO_TIME: i64 = -62_135_596_800;
const NANOS: i128 = 1_000_000_000;
const MINUTE: i128 = 60_000_000_000;
const HOUR: i128 = 3_600_000_000_000;
/// [`ZERO_TIME`] in nanoseconds.
const ZERO_NANOS: i128 = -62_135_596_800_000_000_000;

/// `recv.method(args)`, the argument types already checked.
pub(super) fn call<'a>(method: Method, recv: &Value<'a>, args: &[Value<'a>], f: &mut Frame<'a>, span: Span) -> Res<'a> {
    let arg = |i: usize| args.get(i).ok_or_else(|| internal("missing method argument", span));
    let overflow = || Frame::fault(EvalErrorKind::Overflow, span);
    let int = |n: i128| i64::try_from(n).map(Value::int).map_err(|_| overflow());
    let time = |t: Timestamp, z: &TimeZone| Value(Repr::Time(t, z.clone()));
    let duration = |n: i128| {
        let n = i64::try_from(n).map_err(|_| overflow())?;
        Ok::<_, Fault>(Value(Repr::Duration(SignedDuration::from_nanos(n))))
    };
    if method == Method::String {
        let mut out = Text::new();
        go_string(recv, &mut out, f, span)?;
        return Ok(out.into_value());
    }
    if let Some(d) = recv.as_duration() {
        let d = d.as_nanos();
        let m = || arg(0).and_then(|v| v.as_duration().ok_or_else(|| internal("expected a duration", span)));
        return match method {
            // Go truncates toward zero, then adds the remainder's fraction.
            #[expect(
                clippy::cast_precision_loss,
                reason = "Go converts the whole units and the remainder to float64 the same way"
            )]
            Method::Hours | Method::Minutes | Method::Seconds => {
                let unit = match method {
                    Method::Hours => HOUR,
                    Method::Minutes => MINUTE,
                    _ => NANOS,
                };
                let (q, r) = (d.checked_div(unit).unwrap_or(0), d.checked_rem(unit).unwrap_or(0));
                Ok(Value::float(q as f64 + r as f64 / unit as f64))
            }
            Method::Milliseconds => int(d.checked_div(1_000_000).unwrap_or(0)),
            Method::Microseconds => int(d.checked_div(1_000).unwrap_or(0)),
            Method::Nanoseconds => int(d),
            Method::TruncateDuration => {
                let m = m()?.as_nanos();
                duration(if m <= 0 { d } else { d.saturating_sub(d.checked_rem(m).unwrap_or(0)) })
            }
            Method::RoundDuration => duration(round_duration(d, m()?.as_nanos())),
            Method::Abs => duration(if d == i128::from(i64::MIN) { i128::from(i64::MAX) } else { d.saturating_abs() }),
            _ => Err(internal("not a duration method", span)),
        };
    }
    if recv.as_location().is_some() {
        return Err(internal("not a location method", span));
    }
    let (t, z) = recv.as_zoned().ok_or_else(|| internal("expected a time", span))?;
    let other = || arg(0).and_then(|v| v.as_time().ok_or_else(|| internal("expected a time", span)));
    let (dt, _, _) = layout::zoned(t, z);
    let nanos = t.as_nanosecond();
    Ok(match method {
        Method::Year => Value::int(i64::from(dt.year())),
        Method::Month => Value::int(i64::from(dt.month())),
        Method::Day => Value::int(i64::from(dt.day())),
        Method::Hour => Value::int(i64::from(dt.hour())),
        Method::Minute => Value::int(i64::from(dt.minute())),
        Method::Second => Value::int(i64::from(dt.second())),
        Method::Nanosecond => Value::int(i64::from(dt.subsec_nanosecond())),
        Method::Weekday => Value::int(i64::from(dt.weekday().to_sunday_zero_offset())),
        Method::YearDay => Value::int(i64::from(dt.day_of_year())),
        Method::Unix => int(nanos.div_euclid(NANOS))?,
        Method::UnixMilli => int(nanos.div_euclid(1_000_000))?,
        Method::UnixMicro => int(nanos.div_euclid(1_000))?,
        Method::UnixNano => int(nanos)?,
        Method::Add => {
            let d = arg(0)?.as_duration().ok_or_else(|| internal("expected a duration", span))?;
            time(t.checked_add(d).map_err(|_| overflow())?, z)
        }
        Method::Sub => Value(Repr::Duration(t.duration_since(other()?))),
        Method::AddDate => {
            let n = |i: usize| arg(i).and_then(|v| v.as_int().ok_or_else(|| internal("expected an int", span)));
            let (years, months, days) = (n(0)?, n(1)?, n(2)?);
            time(add_date(t, z, years, months, days).ok_or_else(overflow)?, z)
        }
        Method::Before => Value::bool(t < other()?),
        Method::After => Value::bool(t > other()?),
        Method::Equal => Value::bool(t == other()?),
        Method::Compare => Value::int(match t.cmp(&other()?) {
            core::cmp::Ordering::Less => -1,
            core::cmp::Ordering::Equal => 0,
            core::cmp::Ordering::Greater => 1,
        }),
        Method::Format => {
            let layout_text = arg(0)?.as_str().ok_or_else(|| internal("expected a string", span))?;
            // Formatted aside: its length is a small multiple of the layout's, which
            // was charged when it was built.
            let mut out = String::new();
            layout::format(t, z, layout_text, &mut out);
            return Text::copy(f, &out, span);
        }
        Method::In => time(t, arg(0)?.as_location().ok_or_else(|| internal("expected a location", span))?),
        Method::Utc => time(t, &TimeZone::UTC),
        Method::Location => Value(Repr::Location(z.clone())),
        Method::Truncate | Method::Round => {
            let d = arg(0)?.as_duration().ok_or_else(|| internal("expected a duration", span))?.as_nanos();
            if d <= 0 {
                return Ok(time(t, z));
            }
            // The remainder of the time since Go's zero time, as Go's `div`.
            let r = nanos.saturating_sub(ZERO_NANOS).rem_euclid(d);
            let shift = if method == Method::Truncate || r.saturating_add(r) < d {
                r.saturating_neg()
            } else {
                d.saturating_sub(r)
            };
            let shifted = nanos.checked_add(shift).ok_or_else(overflow)?;
            // `Timestamp::new` checks the range; `from_nanosecond` (jiff 0.2.38) only
            // debug-asserts it.
            let secs = i64::try_from(shifted.div_euclid(NANOS)).map_err(|_| overflow())?;
            let nanos = i32::try_from(shifted.rem_euclid(NANOS)).map_err(|_| overflow())?;
            time(Timestamp::new(secs, nanos).map_err(|_| overflow())?, z)
        }
        Method::IsZero => Value::bool(t.as_second() == ZERO_TIME && t.subsec_nanosecond() == 0),
        Method::Hours
        | Method::Minutes
        | Method::Seconds
        | Method::Milliseconds
        | Method::Microseconds
        | Method::Nanoseconds
        | Method::TruncateDuration
        | Method::RoundDuration
        | Method::Abs
        | Method::String => return Err(internal("not a time method", span)),
    })
}

/// Go's `Duration.Round`: halves away from zero, saturating at Go's duration range.
fn round_duration(d: i128, m: i128) -> i128 {
    let (min, max) = (i128::from(i64::MIN), i128::from(i64::MAX));
    if m <= 0 {
        return d;
    }
    let r = d.checked_rem(m).unwrap_or(0).abs();
    let rounded = if r.saturating_add(r) < m {
        if d < 0 { d.saturating_add(r) } else { d.saturating_sub(r) }
    } else if d < 0 {
        d.saturating_sub(m).saturating_add(r)
    } else {
        d.saturating_add(m).saturating_sub(r)
    };
    rounded.clamp(min, max)
}

/// Go's `AddDate`: the civil fields in `z`, plus the deltas, normalized as Go's
/// `time.Date` (January 31 plus one month is March 3 or 2).
fn add_date(t: Timestamp, z: &TimeZone, years: i64, months: i64, days: i64) -> Option<Timestamp> {
    let (dt, _, _) = layout::zoned(t, z);
    let month = i64::from(dt.month()).checked_sub(1)?.checked_add(months)?;
    let year = i64::from(dt.year()).checked_add(years)?.checked_add(month.div_euclid(12))?;
    let first = layout::days_from_civil(year, month.rem_euclid(12).checked_add(1)?, 1)?;
    let day = first.checked_add(i64::from(dt.day()))?.checked_sub(1)?.checked_add(days)?;
    let clock = i64::from(dt.hour())
        .checked_mul(3600)?
        .checked_add(i64::from(dt.minute()).checked_mul(60)?)?
        .checked_add(i64::from(dt.second()))?;
    let civil = day.checked_mul(86_400)?.checked_add(clock)?;
    let unix = layout::local_to_unix(civil, z)?;
    Timestamp::new(unix, dt.subsec_nanosecond()).ok()
}

/// `recv.name(args)` when the receiver or an argument is `any`.
pub(super) fn call_dyn<'a>(name: &str, recv: &Value<'a>, args: &[Value<'a>], f: &mut Frame<'a>, span: Span) -> Res<'a> {
    let tags: alloc::vec::Vec<_> = args.iter().map(Value::tag).collect();
    let Some((method, _)) = dictum_check::resolve::method(recv.tag(), name, &tags) else {
        let found = recv.tag().name();
        return Err(at(span)(EvalErrorKind::TypeMismatch { expected: "a value with this method", found }));
    };
    call(method, recv, args, f, span)
}
