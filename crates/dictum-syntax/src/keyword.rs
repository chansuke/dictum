//! Reserved words and identifier rules, shared by the lexer and `#[derive(Dictum)]`.
//! The derive calls these at compile time, so a struct field that rules could never
//! name (`contains`, `matches`, ...) is a Rust compile error, not a surprise later.

/// Words the lexer never returns as identifiers.
pub(crate) const RESERVED: &[&str] = &[
    "true", "false", "nil", "let", "if", "else", "not", "and", "or", "in", "matches",
    "contains", "startsWith", "endsWith",
];

/// Whether `name` is a reserved word.
pub fn is_reserved(name: &str) -> bool {
    RESERVED.contains(&name)
}

/// Whether `name` matches `[A-Za-z_][A-Za-z0-9_]*` and is not reserved, i.e. whether
/// a rule can refer to it as a field or function name.
pub fn is_identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    let head_ok = matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'));
    head_ok && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_') && !is_reserved(name)
}
