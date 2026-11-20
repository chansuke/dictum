//! The one fact root every test, fuzz target and benchmark compiles rules against.
//!
//! Rules see PascalCase names, as in Go: `user.Age`, `order.Items`, `nums`, `limit`.
//! Keep [`crate::generate`] in step with these fields: it only names fields listed in
//! its leaf tables, and a field added here is unused until a leaf names it.

use std::collections::BTreeMap;

use dictum::jiff::Timestamp;
use dictum::{Dictum, Dynamic};

/// A person.
#[derive(Dictum, Clone, Debug)]
#[dictum(rename_all = "PascalCase")]
pub struct User {
    pub name: String,
    pub age: i64,
    pub score: f64,
    pub active: bool,
    pub email: Option<String>,
    pub tags: Vec<String>,
    pub created: Timestamp,
    /// `any` in rules.
    pub attrs: Dynamic,
    /// An optional nested struct (and a recursive type): needs `?.`.
    pub manager: Option<Box<User>>,
}

/// One order line.
#[derive(Dictum, Clone, Debug)]
#[dictum(rename_all = "PascalCase")]
pub struct Item {
    pub sku: String,
    pub price: f64,
    pub qty: i64,
}

/// An order.
#[derive(Dictum, Clone, Debug)]
#[dictum(rename_all = "PascalCase")]
pub struct Order {
    pub total: f64,
    pub items: Vec<Item>,
    pub notes: BTreeMap<String, String>,
    pub discount: Option<f64>,
}

/// The schema root. Owned (no lifetime), so `Facts::Static == Facts`.
#[derive(Dictum, Clone, Debug)]
pub struct Facts {
    pub user: User,
    pub order: Order,
    pub nums: Vec<i64>,
    pub limit: i64,
    /// Narrow integers, so the interval analysis has ranges to work with.
    pub level: u8,
    pub delta: i8,
}

/// The facts the conformance cases are written against. Changing a value here
/// changes the expected results in `tests/conformance`.
pub fn sample() -> Facts {
    let mut notes = BTreeMap::new();
    notes.insert("gift".to_owned(), "yes".to_owned());
    notes.insert("note".to_owned(), "leave at door".to_owned());
    Facts {
        user: User {
            name: "Ada".to_owned(),
            age: 36,
            score: 4.5,
            active: true,
            email: None,
            tags: vec!["admin".to_owned(), "beta".to_owned()],
            created: Timestamp::from_second(1_700_000_000).unwrap_or(Timestamp::UNIX_EPOCH),
            attrs: Dynamic::Int(7),
            manager: None,
        },
        order: Order {
            total: 120.5,
            items: vec![
                Item { sku: "A-1".to_owned(), price: 100.0, qty: 1 },
                Item { sku: "B-2".to_owned(), price: 10.25, qty: 2 },
            ],
            notes,
            discount: Some(0.1),
        },
        nums: vec![3, 1, 4, 1, 5, 9, 2, 6],
        limit: 10,
        level: 200,
        delta: -100,
    }
}

/// The time conformance cases evaluate at (`now()`): 2026-01-01T00:00:00Z.
pub fn sample_now() -> Timestamp {
    Timestamp::from_second(1_767_225_600).unwrap_or(Timestamp::UNIX_EPOCH)
}

#[cfg(feature = "arbitrary")]
mod arbitrary_impl {
    use super::*;
    use arbitrary::{Arbitrary, Result, Unstructured};

    fn short_string(u: &mut Unstructured<'_>) -> Result<String> {
        let len = u.int_in_range(0..=8)?;
        (0..len).map(|_| Ok(char::from(u.int_in_range(b' '..=b'~')?))).collect()
    }

    impl<'a> Arbitrary<'a> for Facts {
        fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
            let items = (0..u.int_in_range(0..=4)?)
                .map(|_| Ok(Item { sku: short_string(u)?, price: u.arbitrary()?, qty: u.arbitrary()? }))
                .collect::<Result<Vec<_>>>()?;
            let notes = (0..u.int_in_range(0..=3)?)
                .map(|_| Ok((short_string(u)?, short_string(u)?)))
                .collect::<Result<BTreeMap<_, _>>>()?;
            let tags = (0..u.int_in_range(0..=4)?).map(|_| short_string(u)).collect::<Result<Vec<_>>>()?;
            let created = Timestamp::from_second(u.int_in_range(0..=4_102_444_800)?).unwrap_or(Timestamp::UNIX_EPOCH);
            Ok(Facts {
                user: User {
                    name: short_string(u)?,
                    age: u.arbitrary()?,
                    score: u.arbitrary()?,
                    active: u.arbitrary()?,
                    email: if u.arbitrary()? { Some(short_string(u)?) } else { None },
                    tags,
                    created,
                    attrs: Dynamic::Int(u.arbitrary()?),
                    manager: None,
                },
                order: Order {
                    total: u.arbitrary()?,
                    items,
                    notes,
                    discount: if u.arbitrary()? { Some(u.arbitrary()?) } else { None },
                },
                nums: u.arbitrary()?,
                limit: u.arbitrary()?,
                level: u.arbitrary()?,
                delta: u.arbitrary()?,
            })
        }
    }
}
