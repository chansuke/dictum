use std::collections::HashMap;

#[derive(dictum::Dictum)]
pub struct Facts {
    pub notes: HashMap<String, String>,
}

fn main() {}
