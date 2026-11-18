//! Constant membership sets for `x in [..]`.
//!
//! A sorted boxed slice with binary search, linear scan below 8 elements. No hasher:
//! works in `no_std` without choosing a hash function or seed, is deterministic, and
//! the set is built from rule constants so its size is the rule author's, not the
//! facts provider's.

use alloc::boxed::Box;

use dictum_check::ir::Const;

use crate::engine::value::Value;

pub(crate) enum ConstSet {
    Int(Box<[i64]>),
    /// Sorted by `total_cmp` after mapping `-0.0` to `0.0`; NaN dropped (never equal).
    Float(Box<[f64]>),
    Str(Box<[Box<str>]>),
    /// Mixed or compound element types: linear scan with structural equality.
    Other(Box<[Const]>),
}

impl ConstSet {
    pub(crate) fn new(elements: &[Const]) -> ConstSet {
        todo!()
    }

    pub(crate) fn contains(&self, v: &Value<'_>) -> bool {
        todo!()
    }
}
