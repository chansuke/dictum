//! What each node's value depends on, and which nodes a `RuleSet` shares.
//!
//! Results are side tables indexed by `NodeId`; the IR itself is never annotated.

use alloc::vec::Vec;

use dictum_check::ir::{self, ExprKind, Intrinsic};

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
/// TODO pseudocode:
/// ```text
/// for each rule r, each node n with dependence[n] == Input and cost(n) >= MIN_SHARE_COST:
///     key = structural_key(n)   // ignores ids and spans; commutative ops (`==`, `!=`,
///                               // `+` and `*` on int/float) hash and
///                               // compare as multisets, so `a == b` matches `b == a`
///                               // without rewriting either rule. `&&`/`||` are not
///                               // commutative here: `false && 1/0` is `false` but
///                               // `1/0 && false` fails, so a value cached from one
///                               // order would hide the other order's error
///     occurrences[key].push((r, n))
/// for each key with occurrences in >= 2 places (across or within rules):
///     slot = next slot; per_rule[r][n] = Some(slot) for each occurrence
/// ```
/// Nested shared nodes each get their own slot, so the inner part is also computed
/// once when outer expressions differ. Each rule keeps its own closure for a shared
/// node; only the slot is shared, so an error raised inside it carries the spans of
/// the rule that is running.
pub(crate) fn share(rules: &[(&ir::Program, &[Dependence])]) -> Sharing {
    todo!()
}
