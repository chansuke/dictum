// Every supported field type derives, including borrowing and recursive structs.
use std::collections::BTreeMap;

use dictum::jiff::{SignedDuration, Timestamp};
use dictum::{Dictum, Dynamic};

#[derive(Dictum)]
#[dictum(rename_all = "PascalCase")]
pub struct Scalars {
    pub b: bool,
    pub i8_: i8,
    pub i16_: i16,
    pub i32_: i32,
    pub i64_: i64,
    pub u8_: u8,
    pub u16_: u16,
    pub u32_: u32,
    pub f32_: f32,
    pub f64_: f64,
    pub s: String,
    pub bs: Box<str>,
    pub t: Timestamp,
    pub d: SignedDuration,
    pub any: Dynamic,
}

#[derive(Dictum)]
pub struct Node {
    pub label: String,
    pub children: Vec<Node>,
    pub parent: Option<Box<Node>>,
    pub attrs: BTreeMap<String, i64>,
    pub fixed: Box<[f64]>,
}

#[derive(Dictum)]
pub struct Borrowing<'a> {
    pub name: &'a str,
    pub node: &'a Node,
    pub tags: &'a [String],
    #[dictum(rename = "Renamed")]
    pub original: i64,
    #[dictum(skip)]
    pub secret: [u8; 4],
}

fn main() {}
