//! Recording for `explain`, and turning a recording into a `Report`.

use alloc::vec::Vec;

use dictum_check::ir::{self, ExprKind, NodeId};
use dictum_syntax::ErrorCode;

use crate::Dynamic;
use crate::engine::value::{Depth, Value};
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
            Some(Recorded::Value(v)) => Dynamic::copy_of(v, Depth::TOP).map_or(NodeStatus::Elided, NodeStatus::Value),
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
            // A literal of constants (`["admin"]`) is a leaf, as a constant is.
            ExprKind::Array(_) | ExprKind::Map(_) if literal(shown) => Vec::new(),
            _ => shown.children().map(|c| self.node(c)).collect(),
        };
        ExplainNode { span: shown.span(), status, element, children }
    }
}

/// Whether `e` is a constant or a literal built only of constants.
fn literal(e: &ir::Expr) -> bool {
    match e.kind() {
        ExprKind::Const(_) => true,
        ExprKind::Convert { arg, .. } => literal(arg),
        ExprKind::Array(_) | ExprKind::Map(_) => e.children().all(literal),
        _ => false,
    }
}

/// Elements in `v`, counting into nested containers, once per reference. Counting
/// stops past [`MAX_REPORTED_ELEMENTS`], so a value shared many times costs at most
/// that many visits.
fn size(v: &Value<'_>) -> u64 {
    let mut n = 0u64;
    count(v, &mut n, Depth::TOP);
    n
}

/// Past `MAX_WALK_DEPTH` the value counts as too large to report.
fn count(v: &Value<'_>, n: &mut u64, depth: Depth) {
    let Some(inner) = depth.deeper() else {
        *n = MAX_REPORTED_ELEMENTS.saturating_add(1);
        return;
    };
    let add = |n: &mut u64, k: usize| *n = n.saturating_add(u64::try_from(k).unwrap_or(u64::MAX));
    if let Some(s) = v.as_seq() {
        add(n, s.len());
        for x in s.iter() {
            if *n > MAX_REPORTED_ELEMENTS {
                return;
            }
            count(&x, n, inner);
        }
    } else if let Some(m) = v.as_map() {
        add(n, m.len());
        m.for_each(&mut |_, x| {
            if *n > MAX_REPORTED_ELEMENTS {
                return core::ops::ControlFlow::Break(());
            }
            count(&x, n, inner);
            core::ops::ControlFlow::Continue(())
        });
    }
}
