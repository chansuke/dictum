//! Primitive semantics, as plain functions. Shared by the closure compiler and the
//! spec interpreter (`interp`, which also implements `explain`), so the differential test exercises control flow,
//! specialization, short-circuiting, caching and budget accounting — the parts that
//! differ — rather than two copies of `checked_add`.

use crate::EvalErrorKind;

/// `int + int`
pub(crate) fn add_int(a: i64, b: i64) -> Result<i64, EvalErrorKind> {
    a.checked_add(b).ok_or(EvalErrorKind::Overflow)
}

/// `int % int`: Go semantics (sign of the dividend); `i64::MIN % -1` is `0`.
pub(crate) fn mod_int(a: i64, b: i64) -> Result<i64, EvalErrorKind> {
    if b == 0 {
        return Err(EvalErrorKind::DivisionByZero);
    }
    Ok(a.wrapping_rem(b))
}

/// `float ** float` via `libm` (bit-identical on every target).
pub(crate) fn pow_float(a: f64, b: f64) -> f64 {
    libm::pow(a, b)
}

/// Resolve an index (negative counts from the end) against `len`.
pub(crate) fn index(i: i64, len: usize) -> Result<usize, EvalErrorKind> {
    // TODO: let n = i64::try_from(len); i < 0 => n + i; check 0 <= j < n;
    //       else IndexOutOfBounds { index: i, len }
    todo!()
}

/// `s[from:to]` by byte offsets; both ends must be char boundaries.
pub(crate) fn slice_str(s: &str, from: Option<i64>, to: Option<i64>) -> Result<core::ops::Range<usize>, EvalErrorKind> {
    // TODO: clamp like Go expr (out-of-range bounds clamp, from > to is empty), then
    //       !s.is_char_boundary(x) -> CharBoundary { index: x }
    todo!()
}

/// `len(string)`: characters, as Go's `utf8.RuneCountInString`.
pub(crate) fn len_str(s: &str) -> i64 {
    // Infallible: a string cannot hold more than isize::MAX chars.
    i64::try_from(s.chars().count()).unwrap_or(i64::MAX)
}
