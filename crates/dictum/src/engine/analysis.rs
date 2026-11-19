//! What each node's value depends on, and which nodes a `RuleSet` shares.
//!
//! Results are side tables indexed by `NodeId`; the IR itself is never annotated.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use dictum_check::ir::{self, BinaryOp, ExprKind, FloatOp, IntOp, Intrinsic};

use crate::engine::SchemaCore;
use crate::func::Purity;

/// What a node's value depends on. Ordered: the dependence of a node is the max of
/// its own and its children's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Dependence {
    /// Nothing: constants and pure functions of constants. The folder evaluates
    /// these at compile time.
    Const,
    /// The evaluation's input only: facts (`Root`) and `now()`, possibly through
    /// pure functions. Identical across the rules of one `RuleSet::eval`, so
    /// shareable.
    Input,
    /// A `let` slot or a predicate pointer: meaningful only inside one rule or one
    /// iteration. Never shared.
    Local,
    /// A volatile schema function (`SchemaBuilder::func`). Never folded or shared.
    Volatile,
}

/// Dependence per node id (`len == program.node_count()`).
pub(crate) fn dependence(program: &ir::Program, schema: &SchemaCore) -> Vec<Dependence> {
    let len = usize::try_from(program.node_count()).unwrap_or(0);
    let mut out = alloc::vec![Dependence::Const; len];
    for root in program.lets().iter().chain(core::iter::once(program.body())) {
        visit(root, schema, &mut out);
    }
    out
}

/// Record `e`'s dependence and return it as its parent sees it: without the
/// pointers of the enclosing predicate, and whether it reads them. A node that reads
/// them is `Local`, but the predicate that binds them is not: `all([1, 2], # > 0)`
/// is `Const`.
fn visit(e: &ir::Expr, schema: &SchemaCore, out: &mut [Dependence]) -> (Dependence, bool) {
    let mut dep = match e.kind() {
        ExprKind::Root | ExprKind::Intrinsic { op: Intrinsic::Now, .. } => Dependence::Input,
        ExprKind::Slot(_) => Dependence::Local,
        // A pure call is as dependent as its arguments, so `isHoliday(date("2026-01-01"))` folds.
        ExprKind::Call { func, .. } => match schema.payload(*func) {
            Some(imp) if imp.purity == Purity::Pure => Dependence::Const,
            _ => Dependence::Volatile,
        },
        _ => Dependence::Const,
    };
    let mut reads_pointer = matches!(e.kind(), ExprKind::Pointer(_));
    if let ExprKind::Predicate { seq, body, .. } = e.kind() {
        let (d, p) = visit(seq, schema, out);
        dep = dep.max(d);
        reads_pointer = p;
        if let Some(b) = body {
            dep = dep.max(visit(b, schema, out).0);
        }
    } else {
        for c in e.children() {
            let (d, p) = visit(c, schema, out);
            dep = dep.max(d);
            reads_pointer |= p;
        }
    }
    if let Some(slot) = out.get_mut(e.id().index()) {
        *slot = if reads_pointer { dep.max(Dependence::Local) } else { dep };
    }
    (dep, reads_pointer)
}

/// Index into `Frame::cache`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CacheSlot(pub(crate) u32);

/// Slot assignments for a whole set.
pub(crate) struct Sharing {
    /// `per_rule[r][node.index()]`: the slot of that node in rule `r`, if shared.
    pub(crate) per_rule: Vec<Vec<Option<CacheSlot>>>,
    pub(crate) slot_count: u32,
}

/// Nodes cheaper than this are recomputed rather than cached (a field read costs
/// less than the cache check).
const MIN_SHARE_COST: u32 = 4;

/// Decide which subexpressions the rules of a set share.
///
/// A node is shared when it depends on the input only, is at least
/// `MIN_SHARE_COST` nodes, and its structural key occurs twice or more (across or
/// within rules). The key ignores ids and spans. `==`, `!=`, and `+` and `*` on
/// `int` and `float` compare their operands as a multiset, so `a == b` matches
/// `b == a` without rewriting either rule (sound for values: Lean `comm_ok`).
/// `&&` and `||` keep their order: `false && 1/0` is `false` but `1/0 && false`
/// fails, so a value cached from one order would hide the other order's error
/// (Lean `and_not_comm`).
///
/// Nested shared nodes each get their own slot, so the inner part is also computed
/// once when outer expressions differ. Each rule keeps its own closure for a shared
/// node; only the slot is shared, so an error raised inside it carries the spans of
/// the rule that is running.
pub(crate) fn share(rules: &[(&ir::Program, &[Dependence])]) -> Sharing {
    // Every candidate occurrence, keyed by structure.
    let mut occurrences: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
    for (r, (program, deps)) in rules.iter().enumerate() {
        for root in program.lets().iter().chain(core::iter::once(program.body())) {
            key(root, &mut |id, cost, k| {
                if cost >= MIN_SHARE_COST && deps.get(id) == Some(&Dependence::Input) {
                    occurrences.entry(k.into()).or_default().push((r, id));
                }
            });
        }
    }
    let mut per_rule: Vec<Vec<Option<CacheSlot>>> =
        rules.iter().map(|(p, _)| alloc::vec![None; usize::try_from(p.node_count()).unwrap_or(0)]).collect();
    let mut slot_count: u32 = 0;
    for places in occurrences.values().filter(|places| places.len() >= 2) {
        let slot = CacheSlot(slot_count);
        slot_count = slot_count.saturating_add(1);
        for &(r, id) in places {
            if let Some(entry) = per_rule.get_mut(r).and_then(|t| t.get_mut(id)) {
                *entry = Some(slot);
            }
        }
    }
    Sharing { per_rule, slot_count }
}

/// The structural key of `e` and its size in nodes; `visit(id, cost, key)` is
/// called for every node, children first.
fn key(e: &ir::Expr, visit: &mut dyn FnMut(usize, u32, &str)) -> (String, u32) {
    use core::fmt::Write;
    let mut children: Vec<String> = Vec::new();
    let mut cost: u32 = 1;
    for c in e.children() {
        let (k, n) = key(c, visit);
        children.push(k);
        cost = cost.saturating_add(n);
    }
    let mut out = String::new();
    let _ = match e.kind() {
        ExprKind::Const(c) => write!(out, "{c:?}"),
        ExprKind::Root => write!(out, "root"),
        ExprKind::Slot(s) => write!(out, "slot{}", s.index()),
        ExprKind::Pointer(p) => write!(out, "{p:?}"),
        ExprKind::Field { field, optional, .. } => write!(out, "field{field:?}{optional}"),
        ExprKind::Index { op, optional, .. } => write!(out, "index{op:?}{optional}"),
        ExprKind::Slice { from, to, op, optional, .. } => {
            write!(out, "slice{op:?}{optional}{}{}", from.is_some(), to.is_some())
        }
        ExprKind::Unary { op, .. } => write!(out, "{op:?}"),
        ExprKind::Binary { op, .. } => {
            if commutes(*op) {
                children.sort();
            }
            write!(out, "{op:?}")
        }
        ExprKind::Logic { op, .. } => write!(out, "{op:?}"),
        ExprKind::Compare { kind, rest, .. } => {
            write!(out, "{kind:?}{:?}", rest.iter().map(|(op, _)| *op).collect::<Vec<_>>())
        }
        ExprKind::In { op, negated, .. } => write!(out, "in{op:?}{negated}"),
        ExprKind::Conditional { .. } => write!(out, "cond"),
        ExprKind::Coalesce { .. } => write!(out, "coalesce"),
        ExprKind::Array(_) => write!(out, "array"),
        ExprKind::Map(_) => write!(out, "map"),
        ExprKind::Range { .. } => write!(out, "range"),
        ExprKind::Predicate { op, body, .. } => write!(out, "{op:?}{}", body.is_some()),
        ExprKind::Intrinsic { op, .. } => write!(out, "{op:?}"),
        ExprKind::Call { func, .. } => write!(out, "{func:?}"),
        // `Check` converts to the node's type, so the type is part of the key.
        ExprKind::Convert { op, .. } => write!(out, "{op:?}{:?}", e.ty()),
    };
    out.push('(');
    for (i, c) in children.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(c);
    }
    out.push(')');
    visit(e.id().index(), cost, &out);
    (out, cost)
}

/// The operators whose operands sharing may swap (Lean `BinOp.shareCommutes`, and
/// `!=` as the negation of `==`).
fn commutes(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Int(IntOp::Add | IntOp::Mul)
            | BinaryOp::Float(FloatOp::Add | FloatOp::Mul)
            | BinaryOp::Eq(_)
            | BinaryOp::Ne(_)
    )
}
