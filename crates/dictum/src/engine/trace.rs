//! Recording for `explain`, and turning a recording into a `Report`.

use alloc::vec::Vec;

use dictum_check::ir::{self, NodeId};
use dictum_syntax::ErrorCode;

use crate::engine::value::Value;
use crate::explain::ExplainNode;

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

impl<'a> Trace<'a> {
    pub(crate) fn new(node_count: u32) -> Self {
        todo!()
    }

    pub(crate) fn record(&mut self, id: NodeId, outcome: Recorded<'a>, element: Option<u64>) {
        todo!()
    }

    /// Build the public tree from the IR shape plus the outcomes, converting borrowed
    /// values to `Dynamic` (the report outlives the facts).
    pub(crate) fn into_report_tree(self, program: &ir::Program) -> ExplainNode {
        // TODO: walk the IR; skip `Convert` nodes (show the child); collapse a chain of
        // struct `Field`s into its leaf; Logic/Compare children are their operands.
        todo!()
    }
}
