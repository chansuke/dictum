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
    // `None` here only for `i64::MIN % -1`, whose result is 0.
    Ok(a.checked_rem(b).unwrap_or(0))
}

/// `float ** float` via `libm` (bit-identical on every target).
pub(crate) fn pow_float(a: f64, b: f64) -> f64 {
    libm::pow(a, b)
}

/// Resolve an index (negative counts from the end) against `len`.
pub(crate) fn index(i: i64, len: usize) -> Result<usize, EvalErrorKind> {
    let out = || EvalErrorKind::IndexOutOfBounds { index: i, len: u64::try_from(len).unwrap_or(u64::MAX) };
    let n = i64::try_from(len).map_err(|_| out())?;
    let j = if i < 0 { n.checked_add(i).ok_or_else(out)? } else { i };
    if (0..n).contains(&j) { usize::try_from(j).map_err(|_| out()) } else { Err(out()) }
}

/// `x[from:to]` over `len` elements: a negative bound counts from the end, bounds
/// outside `0..=len` are clamped, and `from > to` is empty, as Go expr does.
pub(crate) fn slice(from: Option<i64>, to: Option<i64>, len: usize) -> core::ops::Range<usize> {
    let n = i64::try_from(len).unwrap_or(i64::MAX);
    let bound = |b: i64| {
        let b = if b < 0 { b.saturating_add(n) } else { b };
        usize::try_from(b.clamp(0, n)).unwrap_or(len)
    };
    let to = to.map_or(len, bound);
    let from = from.map_or(0, bound).min(to);
    from..to
}

/// `s[from:to]` by byte offsets, as [`slice`]; both ends must be char boundaries.
pub(crate) fn slice_str(s: &str, from: Option<i64>, to: Option<i64>) -> Result<core::ops::Range<usize>, EvalErrorKind> {
    let r = slice(from, to, s.len());
    for b in [r.start, r.end] {
        if !s.is_char_boundary(b) {
            return Err(EvalErrorKind::CharBoundary { index: i64::try_from(b).unwrap_or(i64::MAX) });
        }
    }
    Ok(r)
}

/// `s[i]`: the character starting at byte `i` (negative counts from the end).
pub(crate) fn index_str(s: &str, i: i64) -> Result<&str, EvalErrorKind> {
    let at = index(i, s.len())?;
    let boundary = || EvalErrorKind::CharBoundary { index: i64::try_from(at).unwrap_or(i64::MAX) };
    let rest = s.get(at..).ok_or_else(boundary)?;
    let width = rest.chars().next().map_or(0, char::len_utf8);
    rest.get(..width).ok_or_else(boundary)
}

/// `len(string)`: characters, as Go's `utf8.RuneCountInString`.
pub(crate) fn len_str(s: &str) -> i64 {
    // Infallible: a string cannot hold more than isize::MAX chars.
    i64::try_from(s.chars().count()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::arithmetic_side_effects)]

    use super::*;
    use proptest::prelude::*;

    #[test]
    fn add_int_reports_overflow() {
        assert_eq!(add_int(1, 2).unwrap(), 3);
        assert!(matches!(add_int(i64::MAX, 1), Err(EvalErrorKind::Overflow)));
        assert!(matches!(add_int(i64::MIN, -1), Err(EvalErrorKind::Overflow)));
    }

    #[test]
    fn mod_int_follows_go() {
        assert_eq!(mod_int(7, 3).unwrap(), 1);
        assert_eq!(mod_int(-7, 3).unwrap(), -1);
        assert_eq!(mod_int(7, -3).unwrap(), 1);
        assert_eq!(mod_int(i64::MIN, -1).unwrap(), 0);
        assert!(matches!(mod_int(1, 0), Err(EvalErrorKind::DivisionByZero)));
    }

    #[test]
    fn len_str_counts_characters() {
        assert_eq!(len_str(""), 0);
        assert_eq!(len_str("héllo"), 5);
        assert_eq!(len_str("🦀"), 1);
    }

    #[test]
    fn pow_float_is_libm() {
        assert_eq!(pow_float(2.0, 10.0), 1024.0);
        assert!(pow_float(-8.0, 1.0 / 3.0).is_nan());
    }

    #[test]
    fn index_counts_negative_indices_from_the_end() {
        assert_eq!(index(0, 3).unwrap(), 0);
        assert_eq!(index(-1, 3).unwrap(), 2);
        assert!(matches!(index(3, 3), Err(EvalErrorKind::IndexOutOfBounds { index: 3, len: 3 })));
        assert!(matches!(index(-4, 3), Err(EvalErrorKind::IndexOutOfBounds { .. })));
        assert!(matches!(index(i64::MIN, 0), Err(EvalErrorKind::IndexOutOfBounds { .. })));
    }

    #[test]
    fn slice_str_rejects_bounds_inside_a_character() {
        assert_eq!(slice_str("héllo", Some(1), Some(3)).unwrap(), 1..3);
        assert!(matches!(slice_str("héllo", Some(2), None), Err(EvalErrorKind::CharBoundary { index: 2 })));
    }

    #[test]
    fn slice_clamps_and_counts_negative_bounds_from_the_end() {
        assert_eq!(slice(Some(1), Some(3), 5), 1..3);
        assert_eq!(slice(Some(-2), None, 5), 3..5);
        assert_eq!(slice(None, Some(99), 5), 0..5);
        assert_eq!(slice(Some(4), Some(2), 5), 2..2);
        assert_eq!(slice(Some(i64::MIN), Some(i64::MAX), 5), 0..5);
    }

    #[test]
    fn index_str_takes_the_character_at_a_byte_offset() {
        assert_eq!(index_str("abc", 1).unwrap(), "b");
        assert_eq!(index_str("abc", -1).unwrap(), "c");
        assert_eq!(index_str("héllo", 1).unwrap(), "é");
        assert!(matches!(index_str("héllo", 2), Err(EvalErrorKind::CharBoundary { index: 2 })));
        assert!(matches!(index_str("abc", 5), Err(EvalErrorKind::IndexOutOfBounds { index: 5, len: 3 })));
    }

    proptest! {
        /// `mod_int` agrees with Go's `%`, which is Rust's `%` (truncated division)
        /// wherever Rust's does not panic or overflow.
        #[test]
        fn mod_int_matches_truncated_remainder(a: i64, b in any::<i64>().prop_filter("nonzero", |b| *b != 0)) {
            let want = if a == i64::MIN && b == -1 { 0 } else { a % b };
            prop_assert_eq!(mod_int(a, b).unwrap(), want);
        }

        #[test]
        fn add_int_is_commutative_including_overflow(a: i64, b: i64) {
            prop_assert_eq!(add_int(a, b).ok(), add_int(b, a).ok());
        }

        #[test]
        fn slice_str_never_panics(s in "\\PC{0,12}", from: Option<i8>, to: Option<i8>) {
            if let Ok(r) = slice_str(&s, from.map(i64::from), to.map(i64::from)) {
                prop_assert!(s.get(r).is_some());
            }
        }
    }
}
