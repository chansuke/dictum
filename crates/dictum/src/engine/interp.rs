//! The spec interpreter: a direct tree walk over the typed IR, written for
//! obviousness, not speed.
//!
//! It has two jobs. It implements `explain` (with a `Trace`), so an explanation
//! describes the per-rule IR with exact spans and the fast path carries no recording
//! code. And it is the oracle of the differential test, reached through the public
//! API: `rule.eval(ctx)` must equal `rule.explain(ctx).into_result()`.
//!
//! Because `explain` runs it in production, it meets the same bar as the closure
//! tree: no panics (IR invariant failures become `Internal`) and the same budget
//! charges at the same points.
//!
//! TODO differential test (proptest, dev-dependency):
//! ```text
//! strategy: random well-typed source over a fixed test root struct
//!           (generated from a typed grammar, then compiled, so the parser and
//!            checker are exercised too)
//! property: for random facts and `now`,
//!           rule.eval(ctx) == rule.explain(ctx).into_result()          // compiler, folder
//!           RuleSet of N variants: each outcome == standalone Rule outcome
//!                                                                    // sharing
//! equality: values equal, or both errors with the same ErrorCode and span
//!           (cases that hit a budget limit are skipped: the RuleSet charges shared
//!            work to the first rule that computes it)
//! ```

use alloc::collections::BTreeMap;
use alloc::sync::Arc;

use dictum_check::ir::{self, NodeId};

use crate::engine::SchemaCore;
use crate::engine::frame::{Fault, Frame};
use crate::engine::trace::Trace;
use crate::engine::value::Value;

/// Evaluate `program` in `frame`. When `trace` is given, record every node's outcome
/// into it by `NodeId`.
///
/// Uses `ops` for every primitive, so the closure tree and this walk differ only in
/// control flow, specialization and caching.
pub(crate) fn eval<'a>(
    program: &ir::Program,
    schema: &SchemaCore,
    patterns: &BTreeMap<NodeId, Arc<regex::Regex>>,
    frame: &mut Frame<'a>,
    trace: Option<&mut Trace<'a>>,
) -> Result<Value<'a>, Fault> {
    // TODO: evaluate `lets` in order into `frame.slots`, then the body.
    // Per node: compute with `ops`, then `trace.record(id, outcome, element)`.
    // Predicates charge one step per element before running the body, as the
    // closure tree does.
    todo!()
}
