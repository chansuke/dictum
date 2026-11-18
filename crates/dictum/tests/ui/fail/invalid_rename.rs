#[derive(dictum::Dictum)]
pub struct Facts {
    #[dictum(rename = "not-an-ident")]
    pub x: bool,
}

fn main() {}
