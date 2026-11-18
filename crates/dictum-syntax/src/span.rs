/// Byte range `start..end` in the rule source.
///
/// Invariant: `start <= end <= src.len()` and both ends fall on UTF-8 boundaries.
/// Every diagnostic, runtime error and explain node points at the source with one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span {
    /// Inclusive start, in bytes.
    pub start: u32,
    /// Exclusive end, in bytes.
    pub end: u32,
}

impl Span {
    /// The span covering both `self` and `other`.
    pub const fn to(self, other: Span) -> Span {
        let start = if self.start < other.start { self.start } else { other.start };
        let end = if self.end > other.end { self.end } else { other.end };
        Span { start, end }
    }

    /// The text this span covers. `None` if the span does not belong to `src`.
    pub fn text(self, src: &str) -> Option<&str> {
        src.get(self.start as usize..self.end as usize)
    }
}
