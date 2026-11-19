//! Why a rule produced its result.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;

use dictum_syntax::{ErrorCode, Span};

use crate::{Dynamic, EvalError};

/// The result of [`Rule::explain`](crate::Rule::explain): the outcome plus a tree of
/// the nodes that were evaluated.
///
/// Explaining never fails: a failed evaluation is the case that most needs an
/// explanation, so the error is inside the report (`result()`), and the tree shows
/// how far evaluation got (`NodeStatus::Failed` on the failing node).
pub struct Report<T> {
    pub(crate) result: Result<T, EvalError>,
    pub(crate) root: ExplainNode,
    pub(crate) source: Arc<str>,
}

impl<T> Report<T> {
    /// What `eval` would have returned for the same `Ctx`.
    pub fn result(&self) -> &Result<T, EvalError> {
        &self.result
    }

    /// Take the result.
    pub fn into_result(self) -> Result<T, EvalError> {
        self.result
    }

    /// The root node (the rule body; `let` bindings are its first children).
    pub fn root(&self) -> &ExplainNode {
        &self.root
    }
}

impl<T> fmt::Display for Report<T> {
    /// ```text
    /// user.Group in ["admin"] && user.Age >= 18   → false
    /// ├─ user.Group in ["admin"]                 → true   (user.Group = "admin")
    /// └─ user.Age >= 18                          → false  (user.Age = 16)
    /// ```
    /// Leaf field reads are shown inline in parentheses rather than as children.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut lines = Vec::new();
        lines_of(&self.root, &self.source, String::new(), String::new(), &mut lines);
        let width = lines.iter().map(|(left, _)| left.chars().count()).max().unwrap_or(0);
        for (i, (left, right)) in lines.iter().enumerate() {
            if i > 0 {
                f.write_str("\n")?;
            }
            let pad = width.saturating_sub(left.chars().count());
            write!(f, "{left}{:pad$}   {right}", "")?;
        }
        Ok(())
    }
}

/// The source text of a node, on one line.
fn text_of<'s>(node: &ExplainNode, source: &'s str) -> alloc::borrow::Cow<'s, str> {
    let text = source.get(node.span.start as usize..node.span.end as usize).unwrap_or("");
    if text.contains('\n') { text.replace('\n', " ").into() } else { text.into() }
}

/// One `(left, right)` line per shown node: the tree drawing plus source on the
/// left, the outcome on the right. Evaluated leaves other than literals are shown
/// inline on their parent's line, and a skipped node's children are not shown.
fn lines_of(node: &ExplainNode, source: &str, lead: String, rest: String, out: &mut Vec<(String, String)>) {
    let mut right = status_text(&node.status);
    if matches!(node.status, NodeStatus::Value(_)) {
        right.insert_str(0, "→ ");
    }
    if let Some(i) = node.element {
        right.push_str(&alloc::format!("  [element {i}]"));
    }
    let inline = |c: &ExplainNode| c.children.is_empty() && matches!(c.status, NodeStatus::Value(_));
    let mut shown = Vec::new();
    let mut notes = Vec::new();
    // Below a skipped node everything is skipped too; the node alone says so.
    let children = if matches!(node.status, NodeStatus::Skipped) { &[][..] } else { &node.children[..] };
    for c in children {
        if inline(c) {
            let text = text_of(c, source);
            let value = status_text(&c.status);
            if *text != *value {
                notes.push(alloc::format!("{text} = {value}"));
            }
        } else {
            shown.push(c);
        }
    }
    if !notes.is_empty() {
        right.push_str(&alloc::format!("  ({})", notes.join(", ")));
    }
    out.push((alloc::format!("{lead}{}", text_of(node, source)), right));
    for (i, c) in shown.iter().enumerate() {
        let last = i.saturating_add(1) == shown.len();
        let (branch, next) = if last { ("└─ ", "   ") } else { ("├─ ", "│  ") };
        lines_of(c, source, alloc::format!("{rest}{branch}"), alloc::format!("{rest}{next}"), out);
    }
}

fn status_text(status: &NodeStatus) -> String {
    match status {
        NodeStatus::Value(v) => {
            let mut s = String::new();
            write_value(v, &mut s);
            s
        }
        NodeStatus::Skipped => "skipped".into(),
        NodeStatus::Failed(code) => alloc::format!("failed {}", code.as_str()),
        NodeStatus::Elided => "(too large to show)".into(),
    }
}

/// A value as a rule literal would write it.
fn write_value(v: &Dynamic, out: &mut String) {
    use core::fmt::Write;
    match v {
        Dynamic::Nil => out.push_str("nil"),
        Dynamic::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Dynamic::Int(n) => {
            let _ = write!(out, "{n}");
        }
        Dynamic::Float(x) => crate::engine::builtins::go_float(*x, out),
        #[cfg(feature = "decimal")]
        Dynamic::Decimal(d) => {
            let _ = write!(out, "{d}");
        }
        Dynamic::String(s) => {
            let _ = write!(out, "{s:?}");
        }
        Dynamic::Bytes(b) => {
            let _ = write!(out, "{b:?}");
        }
        Dynamic::Time(t) => {
            let _ = write!(out, "{t}");
        }
        Dynamic::Duration(d) => crate::engine::builtins::go_duration(*d, out),
        Dynamic::Array(xs) => {
            out.push('[');
            for (i, x) in xs.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_value(x, out);
            }
            out.push(']');
        }
        Dynamic::Map(m) => {
            out.push('{');
            for (i, (k, x)) in m.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                let _ = write!(out, "{k}: ");
                write_value(x, out);
            }
            out.push('}');
        }
    }
}

/// One node of the explanation.
///
/// Granularity: one node per IR node, except that implicit conversions are
/// transparent and a chain of struct field reads (`user.Address.City`) is one node.
/// Inside a predicate, the body's nodes show the iteration that decided the result
/// (the first `false` for `all`, the first `true` for `any`, the last one otherwise),
/// and `element()` says which.
#[derive(Clone, Debug)]
pub struct ExplainNode {
    pub(crate) span: Span,
    pub(crate) status: NodeStatus,
    pub(crate) element: Option<u64>,
    pub(crate) children: Vec<ExplainNode>,
}

impl ExplainNode {
    /// Source range of this node.
    pub fn span(&self) -> Span {
        self.span
    }

    /// What happened to it.
    pub fn status(&self) -> &NodeStatus {
        &self.status
    }

    /// For nodes inside a predicate body: the index of the element shown.
    pub fn element(&self) -> Option<u64> {
        self.element
    }

    /// Children in evaluation order (short-circuited ones are `Skipped`).
    pub fn children(&self) -> &[ExplainNode] {
        &self.children
    }
}

/// Outcome of one node.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum NodeStatus {
    /// Evaluated to this value.
    Value(Dynamic),
    /// Not evaluated (short-circuit, untaken branch, or after an earlier failure).
    Skipped,
    /// Evaluation failed here; the error is `Report::result()`.
    Failed(ErrorCode),
    /// Evaluated to a value too large to copy into the report (more than 10 000
    /// elements, such as a long range). The result is unaffected.
    Elided,
}
