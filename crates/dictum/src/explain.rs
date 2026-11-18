//! Why a rule produced its result.

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
        todo!()
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
}
