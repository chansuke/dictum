//! Recording for `explain`, and turning a recording into a `Report`.

use alloc::vec::Vec;

use dictum_check::ir::{self, ExprKind, NodeId};
use dictum_syntax::ErrorCode;

use crate::engine::value::Value;
use crate::explain::{ExplainNode, NodeStatus};

/// What one node did in the iteration being shown.
pub(crate) enum Recorded<'a> {
    Value(Value<'a>),
    Failed(ErrorCode),
}

/// Outcomes by `NodeId` (`len == node_count`). A node never reached stays `None`
/// and is reported as `Skipped`. Inside a predicate body, later iterations overwrite
/// earlier ones until the predicate is decided, so the deciding iteration is shown.
pub(crate) struct Trace<'a> {
    pub(crate) outcomes: Vec<Option<Recorded<'a>>>,
    pub(crate) elements: Vec<Option<u64>>,
}

/// A recorded value with more elements than this is not copied into the report;
/// its node shows `NodeStatus::Elided`. Keeps a report of `count(1..2000000, ..)`
/// from copying two million integers.
const MAX_REPORTED_ELEMENTS: u64 = 10_000;

impl<'a> Trace<'a> {
    pub(crate) fn new(node_count: u32) -> Self {
        let n = usize::try_from(node_count).unwrap_or(0);
        Trace { outcomes: (0..n).map(|_| None).collect(), elements: alloc::vec![None; n] }
    }

    pub(crate) fn record(&mut self, id: NodeId, outcome: Recorded<'a>, element: Option<u64>) {
        if let (Some(o), Some(e)) = (self.outcomes.get_mut(id.index()), self.elements.get_mut(id.index())) {
            *o = Some(outcome);
            *e = element;
        }
    }

    /// Build the public tree from the IR shape plus the outcomes, converting borrowed
    /// values to `Dynamic` (the report outlives the facts).
    ///
    /// Conversions are transparent (the child is shown), a chain of struct field
    /// reads is one node, and the root is the body with the `let` values as its first
    /// children.
    pub(crate) fn into_report_tree(self, program: &ir::Program) -> ExplainNode {
        let mut root = self.node(program.body());
        let lets: Vec<ExplainNode> = program.lets().iter().map(|e| self.node(e)).collect();
        root.children.splice(0..0, lets);
        root
    }

    fn node(&self, e: &ir::Expr) -> ExplainNode {
        let mut shown = e;
        while let ExprKind::Convert { arg, .. } = shown.kind() {
            shown = arg;
        }
        let id = e.id().index();
        let status = match self.outcomes.get(id).and_then(Option::as_ref) {
            None => NodeStatus::Skipped,
            Some(Recorded::Failed(code)) => NodeStatus::Failed(*code),
            Some(Recorded::Value(v)) if size(v) > MAX_REPORTED_ELEMENTS => NodeStatus::Elided,
            Some(Recorded::Value(v)) => NodeStatus::Value(v.to_dynamic()),
        };
        let element = self.elements.get(id).copied().flatten();
        let children = match shown.kind() {
            // `user.Address.City` is one node; what the chain starts from is its child
            // unless it is the root or a pointer.
            ExprKind::Field { base, .. } => {
                let mut start: &ir::Expr = base;
                while let ExprKind::Field { base, .. } = start.kind() {
                    start = base;
                }
                match start.kind() {
                    ExprKind::Root | ExprKind::Pointer(_) => Vec::new(),
                    _ => alloc::vec![self.node(start)],
                }
            }
            _ => shown.children().map(|c| self.node(c)).collect(),
        };
        ExplainNode { span: shown.span(), status, element, children }
    }
}

/// Elements in `v`, counting into nested containers.
fn size(v: &Value<'_>) -> u64 {
    match (v.as_seq(), v.as_map()) {
        (Some(s), _) => s
            .iter()
            .take(usize::try_from(MAX_REPORTED_ELEMENTS).unwrap_or(usize::MAX).saturating_add(1))
            .fold(u64::try_from(s.len()).unwrap_or(u64::MAX), |n, x| n.saturating_add(size(&x))),
        (None, Some(m)) => {
            let mut n = u64::try_from(m.len()).unwrap_or(u64::MAX);
            m.for_each(&mut |_, x| {
                n = n.saturating_add(size(&x));
                if n > MAX_REPORTED_ELEMENTS {
                    core::ops::ControlFlow::Break(())
                } else {
                    core::ops::ControlFlow::Continue(())
                }
            });
            n
        }
        _ => 0,
    }
}
