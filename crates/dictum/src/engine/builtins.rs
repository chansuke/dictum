//! Builtin implementations that are more than one `ops` call.

use crate::EvalErrorKind;

/// `duration(s)` with Go's `time.ParseDuration` grammar: an optional sign, then one
/// or more `<decimal><unit>` with units `ns us µs ms s m h` (`"1h30m"`, `"-1.5h"`).
/// Implemented here rather than through jiff's parser so the accepted language is
/// exactly Go's.
pub(crate) fn parse_go_duration(s: &str) -> Result<jiff::SignedDuration, EvalErrorKind> {
    todo!()
}

/// `date(s)`: RFC 3339 with offset, or `YYYY-MM-DD` / `YYYY-MM-DDTHH:MM:SS` read as
/// UTC (no time zone database is needed, so this works in `no_std`). Go layouts and
/// time zones (`date(s, layout, tz)`) are post-MVP.
pub(crate) fn parse_date(s: &str) -> Result<jiff::Timestamp, EvalErrorKind> {
    todo!()
}
