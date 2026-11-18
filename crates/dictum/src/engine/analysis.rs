//! What each node's value depends on, and which nodes a `RuleSet` shares.
//!
//! Results are side tables indexed by `NodeId`; the IR itself is never annotated.

use alloc::vec::Vec;

use dictum_check::ir;

use crate::engine::SchemaCore;

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
    // TODO: post-order walk. Own dependence by kind:
    //   Const → Const; Root, Intrinsic::Now → Input; Slot, Pointer → Local;
    //   Call → Const if FuncImpl::purity == Pure else Volatile (a pure call is as
    //   dependent as its arguments, so `isHoliday(date("2026-01-01"))` folds);
    //   everything else Const.
    // node = max(own, children...). A Predicate's body is Local (it reads `#`), but the
    // Predicate node itself is max(seq, body without the pointer contribution) — i.e.
    // `all(user.Tags, # != "x")` is Input as a whole: compute body dependence with
    // Pointer counted as Const for the enclosing predicate.
    todo!()
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
