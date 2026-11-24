//! Constant membership sets for `x in [..]`.
//!
//! A sorted boxed slice with binary search, linear scan below 8 elements. No hasher:
//! works in `no_std` without choosing a hash function or seed, is deterministic, and
//! the set is built from rule constants so its size is the rule author's, not the
//! facts provider's.

use alloc::boxed::Box;
use alloc::vec::Vec;

use dictum_check::ir::{Const, EqKind};

use crate::engine::value::Value;

/// Below this many elements a scan is as fast as a search.
const LINEAR_BELOW: usize = 8;

pub(crate) enum ConstSet {
    Int(Box<[i64]>),
    /// Sorted by `total_cmp` after mapping `-0.0` to `0.0`; NaN dropped (never equal).
    Float(Box<[f64]>),
    Str(Box<[Box<str>]>),
}

impl ConstSet {
    /// The set of `elements` compared by `kind`, or `None` when `kind` is not a
    /// scalar one or an element is not of that type (the scan then stays).
    pub(crate) fn new(elements: &[Const], kind: EqKind) -> Option<ConstSet> {
        Some(match kind {
            EqKind::Int => {
                let mut xs: Vec<i64> = elements
                    .iter()
                    .map(|c| if let Const::Int(n) = c { Some(*n) } else { None })
                    .collect::<Option<_>>()?;
                xs.sort_unstable();
                xs.dedup();
                ConstSet::Int(xs.into())
            }
            EqKind::Float => {
                let mut xs: Vec<f64> = elements
                    .iter()
                    .map(|c| if let Const::Float(x) = c { Some(normalize(*x)) } else { None })
                    .collect::<Option<_>>()?;
                xs.retain(|x| !x.is_nan());
                xs.sort_unstable_by(f64::total_cmp);
                ConstSet::Float(xs.into())
            }
            EqKind::String => {
                let mut xs: Vec<Box<str>> = elements
                    .iter()
                    .map(|c| if let Const::Str(s) = c { Some(Box::from(&**s)) } else { None })
                    .collect::<Option<_>>()?;
                xs.sort_unstable();
                xs.dedup();
                ConstSet::Str(xs.into())
            }
            _ => return None,
        })
    }

    /// Whether `v` equals an element. A value of another type is in no set.
    pub(crate) fn contains(&self, v: &Value<'_>) -> bool {
        match self {
            ConstSet::Int(xs) => v.as_int().is_some_and(|n| find(xs, &n, i64::cmp)),
            ConstSet::Float(xs) => v.as_float().is_some_and(|x| !x.is_nan() && find(xs, &normalize(x), f64::total_cmp)),
            ConstSet::Str(xs) => v.as_str().is_some_and(|s| find(xs, s, |a, b| str::cmp(a, b))),
        }
    }
}

/// `-0.0 == 0.0`, so both sort as `0.0`.
fn normalize(x: f64) -> f64 {
    if x == 0.0 { 0.0 } else { x }
}

fn find<T, Q: ?Sized>(xs: &[T], x: &Q, cmp: impl Fn(&T, &Q) -> core::cmp::Ordering) -> bool {
    if xs.len() < LINEAR_BELOW {
        xs.iter().any(|y| cmp(y, x).is_eq())
    } else {
        xs.binary_search_by(|y| cmp(y, x)).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;

    use super::*;

    fn ints(xs: &[i64]) -> ConstSet {
        ConstSet::new(&xs.iter().map(|n| Const::Int(*n)).collect::<Vec<_>>(), EqKind::Int).unwrap()
    }

    #[test]
    fn small_and_large_sets_agree_with_a_scan() {
        for n in [0, 1, 7, 8, 9, 40] {
            let xs: Vec<i64> = (0..n).map(|i| i * 3 - 20).collect();
            let set = ints(&xs);
            for probe in -25..(n * 3) {
                assert_eq!(set.contains(&Value::int(probe)), xs.contains(&probe), "n={n} probe={probe}");
            }
        }
    }

    #[test]
    fn floats_follow_ieee_equality() {
        let elements: Vec<Const> = [-0.0, f64::NAN, 1.5, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0].map(Const::Float).into();
        let set = ConstSet::new(&elements, EqKind::Float).unwrap();
        assert!(set.contains(&Value::float(0.0)));
        assert!(set.contains(&Value::float(-0.0)));
        assert!(!set.contains(&Value::float(f64::NAN)));
        assert!(set.contains(&Value::float(1.5)));
        assert!(!set.contains(&Value::float(1.25)));
    }

    #[test]
    fn strings_and_mixed_elements() {
        let s = |x: &str| Const::Str(Arc::from(x));
        let set = ConstSet::new(&[s("b"), s("a"), s("c"), s("a")], EqKind::String).unwrap();
        assert!(set.contains(&Value::string("a".into())));
        assert!(!set.contains(&Value::string("d".into())));
        assert!(ConstSet::new(&[s("a"), Const::Int(1)], EqKind::String).is_none());
        assert!(ConstSet::new(&[Const::Int(1)], EqKind::Structural).is_none());
    }
}
