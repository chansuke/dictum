//! The evaluator. Everything here is private to the crate.
//!
//! Pipeline (one place, no stage is called from anywhere else):
//!
//! ```text
//! src ──parse──▶ ast ──check──▶ ir ──patterns──▶ Checked
//!                                                  │
//!                      Rule::compile ── fold, compile(None) ┤
//!                      RuleSetBuilder::build ── fold, share(), compile(Some(slots)) per rule
//!                                                  ▼
//!                                      Prepared { checked, fast tree }
//!
//! `explain` does not compile anything: it runs `interp` over `Checked::ir`, which
//! is not folded, so a report shows the rule as written and the differential test
//! checks the folder too.
//! ```

// Numeric casts decide what a rule computes, so each one in the engine is checked:
// a lossy cast needs `#[expect(clippy::cast_.., reason = "..")]` saying why it is exact
// or why Go loses the same bits. Outside the engine these lints stay off.
#![warn(
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

pub(crate) mod analysis;
pub(crate) mod builtins;
pub(crate) mod compile;
pub(crate) mod fold;
pub(crate) mod frame;
pub(crate) mod interp;
pub(crate) mod json;
pub(crate) mod layout;
pub(crate) mod methods;
pub(crate) mod ops;
pub(crate) mod optimize;
pub(crate) mod set;
pub(crate) mod sprint;
pub(crate) mod trace;
pub(crate) mod value;

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;

use dictum_check::Type;
use dictum_check::ir::{self, NodeId};

use crate::engine::frame::{Fault, Frame};
use crate::engine::trace::Trace;
use crate::engine::value::Value;
use crate::explain::ExplainNode;
use crate::func::FuncImpl;
use crate::{CompileError, EvalError, EvalErrorKind};

/// The schema as the engine sees it: root struct, function signatures and their
/// implementations in one table.
pub(crate) type SchemaCore = dictum_check::Env<FuncImpl>;

/// A rule after parse and check: valid, not yet folded or compiled to closures.
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

impl Prepared {
    pub(crate) fn name(&self) -> Option<&str> {
        self.checked.name()
    }
}

/// Parse, check, compile constant patterns.
///
/// Syntax errors stop before checking (so the checker never sees `Error` nodes);
/// otherwise all type errors are reported together.
pub(crate) fn check(
    src: &str,
    name: Option<&str>,
    schema: &Arc<SchemaCore>,
    expected: &Type,
) -> Result<Checked, CompileError> {
    let ast = dictum_syntax::parse(src).map_err(|d| compile_error(src, name, d))?;
    let ir = dictum_check::check(&ast, schema, expected).map_err(|d| compile_error(src, name, d))?;
    let patterns = patterns(&ir).map_err(|d| compile_error(src, name, d))?;
    Ok(Checked { source: src.into(), name: name.map(Into::into), ir, patterns })
}

/// Compile every constant `matches` pattern once. An invalid one is E0211.
fn patterns(ir: &ir::Program) -> Result<BTreeMap<NodeId, Arc<regex::Regex>>, dictum_syntax::Diagnostics> {
    fn visit(e: &ir::Expr, out: &mut BTreeMap<NodeId, Arc<regex::Regex>>, diags: &mut dictum_syntax::Diagnostics) {
        let constant = match e.kind() {
            ir::ExprKind::Binary { op: ir::BinaryOp::Matches, rhs, .. } => match rhs.kind() {
                ir::ExprKind::Const(ir::Const::Str(pattern)) => Some((pattern, rhs.span())),
                _ => None,
            },
            _ => None,
        };
        if let Some((pattern, span)) = constant {
            match regex::Regex::new(pattern) {
                Ok(re) => {
                    out.insert(e.id(), Arc::new(re));
                }
                Err(err) => {
                    let args = alloc::vec![dictum_syntax::Arg::Text(alloc::format!("{err}").into())];
                    diags.push(dictum_syntax::Diagnostic::new(dictum_syntax::ErrorCode::InvalidPattern, span, args));
                }
            }
        }
        for c in e.children() {
            visit(c, out, diags);
        }
    }
    let mut out = BTreeMap::new();
    let mut diags = dictum_syntax::Diagnostics::new();
    for root in ir.lets().iter().chain(core::iter::once(ir.body())) {
        visit(root, &mut out, &mut diags);
    }
    if diags.is_empty() { Ok(out) } else { Err(diags) }
}

/// The diagnostics of one rule as a `CompileError`.
pub(crate) fn compile_error(src: &str, name: Option<&str>, diags: dictum_syntax::Diagnostics) -> CompileError {
    let (diagnostics, dropped) = diags.finish();
    CompileError { source: src.into(), rule: name.map(Into::into), diagnostics: diagnostics.into(), dropped }
}

/// A rule ready to run: the closure tree for `eval`, the checked IR for `explain`.
pub(crate) struct Prepared {
    checked: Checked,
    schema: Arc<SchemaCore>,
    /// `Err` is an internal compiler error. `Rule::compile` reports it; a rule of a
    /// set reports it as `Internal` when it runs, since no source is to blame.
    tree: Result<compile::Tree, compile::Ice>,
}

/// A copy of `checked`'s IR for the closure tree: folded, then simplified
/// (`optimize`), with the dependence of each node it keeps.
struct Optimized {
    ir: ir::Program,
    deps: Vec<analysis::Dependence>,
    /// The integer operations that cannot overflow, so compile without the check.
    unchecked: alloc::collections::BTreeSet<ir::NodeId>,
}

fn folded(checked: &Checked, schema: &Arc<SchemaCore>) -> Optimized {
    let mut ir = checked.ir.clone();
    let before = analysis::dependence(&ir, schema);
    fold::fold(&mut ir, schema, &before);
    let mut unchecked = optimize::optimize(&mut ir);
    let mut deps = analysis::dependence(&ir, schema);
    if has_new_const(&ir, &before, &deps) {
        fold::fold(&mut ir, schema, &deps);
        unchecked = optimize::optimize(&mut ir);
        deps = analysis::dependence(&ir, schema);
    }
    Optimized { ir, deps, unchecked }
}

/// Whether `optimize` made any non-`Const` node newly constant (excluding nodes
/// that were already `Const` before optimization, so intentionally unfolded
/// constants are not evaluated a second time).
fn has_new_const(program: &ir::Program, before: &[analysis::Dependence], after: &[analysis::Dependence]) -> bool {
    if !before.iter().zip(after).any(|(b, a)| *a == analysis::Dependence::Const && *b != analysis::Dependence::Const) {
        return false;
    }
    fn visit(e: &ir::Expr, before: &[analysis::Dependence], after: &[analysis::Dependence]) -> bool {
        let id = e.id().index();
        if !matches!(e.kind(), ir::ExprKind::Const(_))
            && after.get(id) == Some(&analysis::Dependence::Const)
            && before.get(id) != Some(&analysis::Dependence::Const)
        {
            return true;
        }
        e.children().any(|c| visit(c, before, after))
    }
    program.lets().iter().chain(core::iter::once(program.body())).any(|r| visit(r, before, after))
}

impl Prepared {
    /// Fold a copy of the IR and compile it, for a single rule.
    pub(crate) fn new(checked: Checked, schema: Arc<SchemaCore>) -> Result<Prepared, CompileError> {
        let opt = folded(&checked, &schema);
        let ir = &opt.ir;
        let prepared = Prepared::compile(checked, schema, &opt, None);
        if let Err(ice) = &prepared.tree {
            let args = alloc::vec![dictum_syntax::Arg::Text(ice.0.into())];
            let mut diags = dictum_syntax::Diagnostics::new();
            diags.push(dictum_syntax::Diagnostic::new(dictum_syntax::ErrorCode::Internal, ir.body().span(), args));
            return Err(compile_error(&prepared.checked.source, prepared.checked.name(), diags));
        }
        Ok(prepared)
    }

    /// Compile the folded `ir` of `checked`. `slots` assigns shared-cache slots
    /// (RuleSet only).
    fn compile(
        checked: Checked,
        schema: Arc<SchemaCore>,
        opt: &Optimized,
        slots: Option<&[Option<analysis::CacheSlot>]>,
    ) -> Prepared {
        let child_spans = compile::child_spans(&checked.ir);
        let tree =
            compile::Compiler::new(&schema, &checked.patterns, slots, &opt.unchecked, Some(&child_spans)).tree(&opt.ir);
        Prepared { checked, schema, tree }
    }

    pub(crate) fn source(&self) -> &str {
        &self.checked.source
    }

    /// Whether `frame`'s facts were read by this rule's schema. Fields are read by
    /// index, so facts of another root would be read by the wrong ids; derived facts
    /// match by type, runtime facts (one Rust type for every root) only by this check.
    fn check_root(&self, frame: &Frame<'_>) -> Result<(), Fault> {
        if core::ptr::eq(frame.root.desc(), self.schema.root()) {
            Ok(())
        } else {
            Err(Fault { kind: EvalErrorKind::SchemaMismatch, span: self.checked.ir.body().span() })
        }
    }

    /// Evaluate for `eval`.
    pub(crate) fn run<'a>(&self, frame: &mut Frame<'a>) -> Result<Value<'a>, Fault> {
        self.check_root(frame)?;
        let v = match &self.tree {
            Ok(tree) => tree.run(frame)?,
            Err(ice) => {
                return Err(Fault { kind: EvalErrorKind::Internal(ice.0), span: self.checked.ir.body().span() });
            }
        };
        self.charge_result(v, frame)
    }

    /// Run `interp` with a trace for `explain`.
    pub(crate) fn explain<'a>(&self, frame: &mut Frame<'a>) -> (Result<Value<'a>, Fault>, ExplainNode) {
        let mut trace = Trace::new(self.checked.ir.node_count());
        if let Err(fault) = self.check_root(frame) {
            trace.record(self.checked.ir.body().id(), trace::Recorded::Failed(fault.kind.code()), None);
            return (Err(fault), trace.into_report_tree(&self.checked.ir));
        }
        let r = interp::eval(&self.checked.ir, &self.schema, &self.checked.patterns, frame, Some(&mut trace));
        let r = r.and_then(|v| self.charge_result(v, frame));
        (r, trace.into_report_tree(&self.checked.ir))
    }

    /// A range in the result is materialized, and a shared value duplicated, when
    /// it is copied out, so the copy is charged to the allocation budget here, first.
    fn charge_result<'a>(&self, v: Value<'a>, frame: &mut Frame<'a>) -> Result<Value<'a>, Fault> {
        let span = self.checked.ir.body().span();
        let copied = v.copied_elements(frame.budget.alloc_left());
        frame.budget.alloc(copied).map_err(|kind| Fault { kind, span })?;
        Ok(v)
    }

    /// The public error for a fault in this rule.
    pub(crate) fn error(&self, fault: Fault) -> EvalError {
        EvalError {
            kind: fault.kind,
            span: fault.span,
            source: self.checked.source.clone(),
            rule: self.checked.name.clone(),
        }
    }

    /// The public error for a result that did not convert to the output type.
    /// An error converting the result to the rule's output type, at the body.
    pub(crate) fn output_error(&self, kind: EvalErrorKind) -> EvalError {
        let span = self.checked.ir.body().span();
        self.error(Fault { kind, span })
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
    /// Fold every rule, `analysis::share` over all of them, then compile each with
    /// its slot table.
    pub(crate) fn new(rules: Vec<Checked>, schema: Arc<SchemaCore>) -> PreparedSet {
        let folded: Vec<_> = rules.iter().map(|c| folded(c, &schema)).collect();
        let inputs: Vec<_> = folded.iter().map(|o| (&o.ir, o.deps.as_slice())).collect();
        let sharing = analysis::share(&inputs);
        let mut by_name = BTreeMap::new();
        let mut prepared = Vec::with_capacity(rules.len());
        for (i, (checked, opt)) in rules.into_iter().zip(&folded).enumerate() {
            if let Some(name) = &checked.name {
                by_name.insert(name.clone(), i);
            }
            let slots = sharing.per_rule.get(i).map(Vec::as_slice);
            prepared.push(Prepared::compile(checked, schema.clone(), opt, slots));
        }
        PreparedSet { rules: prepared, by_name, cache_slots: sharing.slot_count }
    }

    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.rules.iter().filter_map(|r| r.checked.name())
    }

    /// The rules in insertion order.
    pub(crate) fn rules(&self) -> &[Prepared] {
        &self.rules
    }

    pub(crate) fn get(&self, name: &str) -> Option<&Prepared> {
        self.by_name.get(name).and_then(|&i| self.rules.get(i))
    }

    pub(crate) fn cache_slots(&self) -> u32 {
        self.cache_slots
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dictum_check::{FieldDesc, StructDesc};

    fn int() -> Type {
        Type::Int
    }

    static FIELDS: [FieldDesc; 2] = [
        FieldDesc::new("age", int).with_range(Some((0, 255))),
        FieldDesc::new("n", int).with_range(Some((i64::MIN, i64::MAX))),
    ];
    static ROOT: StructDesc = StructDesc::new("Root", &FIELDS);

    fn fold_ir(src: &str, expected: &Type) -> Optimized {
        let schema = Arc::new(SchemaCore::new(&ROOT, Vec::new()).unwrap());
        let checked = check(src, None, &schema, expected).unwrap();
        let opt = folded(&checked, &schema);
        opt.ir.validate(&schema).unwrap();
        opt
    }

    #[test]
    fn post_optimize_refolds_newly_constant_expressions() {
        let str_opt = fold_ir(r#"(age < 256 ? "a" : "b") + "c""#, &Type::String);
        assert!(
            matches!(str_opt.ir.body().kind(), ir::ExprKind::Const(ir::Const::Str(s)) if s.as_ref() == "ac"),
            "{:?}",
            str_opt.ir.body().kind()
        );

        let len_opt = fold_ir(r#"len(age < 256 ? "abc" : "defg")"#, &Type::Int);
        assert!(
            matches!(len_opt.ir.body().kind(), ir::ExprKind::Const(ir::Const::Int(3))),
            "{:?}",
            len_opt.ir.body().kind()
        );

        // Re-folding can also expose another algebraic identity (`(1 - 1) + n` -> `0 + n` -> `n`).
        let id_opt = fold_ir("((age < 256 ? 1 : 2) - 1) + n", &Type::Int);
        assert!(matches!(id_opt.ir.body().kind(), ir::ExprKind::Field { .. }), "{:?}", id_opt.ir.body().kind());
        assert!(id_opt.unchecked.is_empty());
    }
}
