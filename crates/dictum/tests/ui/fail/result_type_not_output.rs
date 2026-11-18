// Only `Output` types can be rule results, and `Output` is sealed.
use dictum::{Dictum, Rule, Schema};

#[derive(Dictum)]
pub struct Facts {
    pub x: i64,
}

fn compile(schema: &Schema<Facts>) {
    let _ = Rule::<u64, Facts>::compile("x", schema);
}

fn main() {}
