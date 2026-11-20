//! Constant folding.
//!
//! Folding evaluates constant subtrees with the closure compiler itself (compile the
//! subtree, run it on an empty frame), so there is no second implementation of
//! operator semantics to keep in sync. `60 * 60 * 24`, `duration("24h")`,
//! `[1, 2, 3]` and pure functions of constants all become `ExprKind::Const`, which
//! later lets `in` build a sorted set and `matches` compile its regex once.
//!
//! The policy is `fold` in formal/lean/Dictum/Fold.lean, proved not to change any
//! outcome: children first, then the node if it is constant and evaluates. A node
//! whose evaluation fails stays, so `false && 7 % 0 > 0` folds to `false` and a
//! reachable `7 % 0` still fails at run time with its own span.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;

use dictum_check::StructDesc;
use dictum_check::ir::{self, ExprKind};

use crate::ctx::Limits;
use crate::engine::SchemaCore;
use crate::engine::analysis::Dependence;
use crate::engine::compile::Compiler;
use crate::engine::frame::Frame;
use crate::engine::value::{Value, missing_field};
use crate::facts::Object;

/// A budget for evaluating one constant at compile time. A constant that needs more
/// (`len(1..1000000)`) is left to run time, where the caller's limits apply.
const FOLD_LIMITS: Limits = Limits { max_steps: 10_000, max_alloc: 1 << 16 };

/// Fold every constant subtree of `program` in place.
pub(crate) fn fold(program: &mut ir::Program, schema: &Arc<SchemaCore>, deps: &[Dependence]) {
    let patterns = BTreeMap::new();
    let unchecked = alloc::collections::BTreeSet::new();
    let compiler = Compiler::new(schema, &patterns, None, &unchecked);
    for root in program.roots_mut() {
        visit(root, &compiler, deps);
    }
}

fn visit(e: &mut ir::Expr, compiler: &Compiler<'_>, deps: &[Dependence]) {
    for c in e.children_mut() {
        visit(c, compiler, deps);
    }
    if deps.get(e.id().index()) != Some(&Dependence::Const) || matches!(e.kind(), ExprKind::Const(_)) {
        return;
    }
    let Ok(node) = compiler.value(e) else { return };
    let mut frame = Frame::new(&EmptyRoot, None, &FOLD_LIMITS, 0);
    // A value a constant cannot hold (a range, kept lazy) leaves the node as it is.
    if let Some(c) = node(&mut frame).ok().and_then(|v| v.to_const()) {
        // `Err` means the value does not inhabit the node's type, a folder bug; the
        // node then stays unfolded.
        let _ = e.replace_with_const(c);
    }
}

/// The root of a fold-time frame. A constant subtree never reads it.
struct EmptyRoot;

static EMPTY: StructDesc = StructDesc::new("", &[]);

impl Object for EmptyRoot {
    fn desc(&self) -> &'static StructDesc {
        &EMPTY
    }

    fn field(&self, _: dictum_check::FieldId) -> Value<'_> {
        missing_field()
    }
}
