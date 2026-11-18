// A rule compiled for one facts type cannot be evaluated with another.
use dictum::{Ctx, Dictum, Rule};

#[derive(Dictum)]
pub struct A {
    pub x: i64,
}

#[derive(Dictum)]
pub struct B {
    pub x: i64,
}

fn eval(rule: &Rule<bool, A>, b: &B) {
    let _ = rule.eval(&Ctx::new(b));
}

fn main() {}
