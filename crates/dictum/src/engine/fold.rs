//! Constant folding.
//!
//! Folding evaluates constant subtrees with the closure compiler itself (compile the
//! subtree, run it on an empty frame), so there is no second implementation of
//! operator semantics to keep in sync. `60 * 60 * 24`, `decimal("19.99")`,
//! `duration("24h")`, `[1, 2, 3]` and pure functions of constants all become
//! `ExprKind::Const`, which later lets `in` build a sorted set and `matches` compile
//! its regex once.

use dictum_check::ir;

use crate::engine::SchemaCore;
use crate::engine::analysis::Dependence;

/// Fold every maximal `Dependence::Const` subtree in place.
///
/// TODO pseudocode:
/// ```text
/// visit(e):
///   if deps[e.id] == Const and e.kind is not Const:
///       tree = Compiler::new(schema, no patterns, no slots, Fast).value(e)
///       run on Frame::new(&EmptyRoot, UNIX_EPOCH, &Limits::default(), 0)
///       Ok(v)  -> e.replace_with_const(to_const(v)) (on Err(_) leave it)
///       Err(_) -> leave the node: `1 / 0` inside `false && ...` must not reject a
///                 rule whose error is unreachable; it fails at run time if reached.
///       return                // a maximal subtree; don't descend
///   for c in e.children_mut(): visit(c)
/// ```
pub(crate) fn fold(program: &mut ir::Program, schema: &SchemaCore, deps: &[Dependence]) {
    todo!()
}
