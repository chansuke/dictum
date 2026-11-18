//! The evaluator. Everything here is private to the crate.
//!
//! Pipeline (one place, no stage is called from anywhere else):
//!
//! ```text
//! src ──parse──▶ ast ──check──▶ ir ──fold──▶ ir' ──patterns──▶ Checked
//!                                                     │
//!                       Rule::compile ── compile(None) ┤
//!                       RuleSetBuilder::build ── share() then compile(Some(slots)) per rule
//!                                                     ▼
//!                                         Prepared { checked, fast tree }
//!
//! `explain` does not compile anything: it runs `interp` over `Checked::ir`.
//! ```

pub(crate) mod analysis;
pub(crate) mod builtins;
pub(crate) mod compile;
pub(crate) mod fold;
pub(crate) mod frame;
pub(crate) mod interp;
pub(crate) mod ops;
pub(crate) mod set;
pub(crate) mod trace;
pub(crate) mod value;

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;

use dictum_check::Type;
use dictum_check::ir::{self, NodeId};

use crate::CompileError;
use crate::engine::frame::{Fault, Frame};
use crate::engine::trace::Trace;
use crate::engine::value::Value;
use crate::func::FuncImpl;

/// The schema as the engine sees it: root struct, function signatures and their
/// implementations in one table.
pub(crate) type SchemaCore = dictum_check::Env<FuncImpl>;

/// A rule after parse, check and fold: valid, not yet compiled to closures.
pub(crate) struct Checked {
    source: Arc<str>,
    name: Option<Arc<str>>,
    ir: ir::Program,
    /// Constant `matches` patterns, compiled once here (an invalid one is a compile
    /// error, E0211) and shared by the closure tree and `interp`.
    patterns: BTreeMap<NodeId, Arc<regex::Regex>>,
}

impl Checked {
    pub(crate) fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

/// Parse, check, fold, compile constant patterns.
///
/// Syntax errors stop before checking (so the checker never sees `Error` nodes);
/// otherwise all type errors are reported together.
pub(crate) fn check(
    src: &str,
    name: Option<&str>,
    schema: &Arc<SchemaCore>,
    expected: &Type,
) -> Result<Checked, CompileError> {
    // TODO:
    // let ast = dictum_syntax::parse(src).map_err(|d| compile_error(src, name, d))?;
    // let mut ir = dictum_check::check(&ast, schema, expected).map_err(...)?;
    // let deps = analysis::dependence(&ir, schema);
    // fold::fold(&mut ir, schema, &deps);
    // let patterns = compile constant patterns of BinaryOp::Matches nodes; errors -> E0211
    todo!()
}

/// A rule ready to run.
pub(crate) struct Prepared {
    checked: Checked,
    schema: Arc<SchemaCore>,
    fast: compile::Tree,
}

impl Prepared {
    /// Compile the fast tree. `slots` assigns shared-cache slots (RuleSet only).
    pub(crate) fn new(
        checked: Checked,
        schema: Arc<SchemaCore>,
        slots: Option<&[Option<analysis::CacheSlot>]>,
    ) -> Result<Prepared, CompileError> {
        todo!()
    }

    pub(crate) fn source(&self) -> &str {
        &self.checked.source
    }

    /// Run `interp` with a trace for `explain`.
    pub(crate) fn explain<'a>(&self, frame: &mut Frame<'a>) -> (Result<Value<'a>, Fault>, Trace<'a>) {
        // TODO: trace = Trace::new(self.checked.ir.node_count());
        //       result = interp::eval(&self.checked.ir, &self.schema, &self.checked.patterns,
        //                             frame, Some(&mut trace));
        todo!()
    }
}

/// The rules of a `RuleSet`, compiled with shared slots.
pub(crate) struct PreparedSet {
    rules: Vec<Prepared>,
    by_name: BTreeMap<Arc<str>, usize>,
    /// Size of the per-evaluation cache.
    cache_slots: u32,
}

impl PreparedSet {
    /// `analysis::share` over all rules, then compile each with its slot table.
    pub(crate) fn new(rules: Vec<Checked>, schema: Arc<SchemaCore>) -> PreparedSet {
        todo!()
    }

    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.rules.iter().filter_map(|r| r.checked.name())
    }
}
