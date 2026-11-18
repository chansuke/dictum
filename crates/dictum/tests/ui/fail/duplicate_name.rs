#[derive(dictum::Dictum)]
pub struct Facts {
    #[dictum(rename = "x")]
    pub a: bool,
    pub x: bool,
}

fn main() {}
