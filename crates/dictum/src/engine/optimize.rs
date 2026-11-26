//! Rewrites of the folded IR that `formal/lean/Dictum/Opt` proves change no
//! result, errors included, applied under the same conditions:
//!
//! - `range_of`, `skip_check`: `rangeOf`, `skipCheck` (`Opt/Range.lean`). Integer
//!   fields start from their Rust type's bounds (`FieldDesc::range`).
//! - `safe`: `safe` (`Opt/Simp.lean`), an expression that cannot fail.
//! - `simplify_node`: `simp1` (`Opt/Simp.lean`), applied bottom-up as `simp` is.
//! - `reorder`: `and_swap_in_chain`, `or_swap_in_chain` (`Opt/Reorder.lean`):
//!   within a run of consecutive safe operands, cheaper ones go first.
//!
//! The Lean fragment has `int` and `bool` only, so the rules touch only those
//! (`x + 0.0` is not `x` for `x = -0.0`, and `x == x` is false for NaN). A field
//! read is the fragment's `var`: reading a non-optional field cannot fail, at any
//! depth. A comparison is the fragment's `lt`, `le`, `gt` or `ge` only when it has
//! one operator; a chain (`a < b < c`) is left alone.
//! `explain` runs the unoptimized IR, so the eval/explain property tests compare
//! every rewrite with the original rule.

use alloc::collections::BTreeSet;

use dictum_check::Type;
use dictum_check::ir::{self, BinaryOp, CompareKind, Const, EqKind, ExprKind, IntOp, NodeId, UnaryOp};
use dictum_syntax::ast::{CompareOp, LogicOp};

use super::analysis::same_expr;

/// Simplify and reorder `program` in place; the nodes whose overflow check the
/// closure compiler may leave out.
pub(crate) fn optimize(program: &mut ir::Program) -> BTreeSet<NodeId> {
    for root in program.roots_mut() {
        simplify(root);
    }
    let mut unchecked = BTreeSet::new();
    for root in program.roots_mut() {
        collect_unchecked(root, &mut unchecked);
    }
    unchecked
}

/// `lo..=hi`, wide enough that the bounds of `i64` operations do not overflow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Range {
    lo: i128,
    hi: i128,
}

impl Range {
    fn fits(self) -> bool {
        self.lo >= i128::from(i64::MIN) && self.hi <= i128::from(i64::MAX)
    }

    /// Lean `clampI64`: intersect with `i64`; `None` if empty.
    fn clamp_i64(self) -> Option<Self> {
        let lo = self.lo.max(i128::from(i64::MIN));
        let hi = self.hi.min(i128::from(i64::MAX));
        (lo <= hi).then_some(Self { lo, hi })
    }

    fn abs_max(self) -> Option<i128> {
        Some(self.lo.checked_abs()?.max(self.hi.checked_abs()?))
    }
}

/// Lean `opRange`: the exact range of `a op b`, before any overflow check.
fn op_range(op: IntOp, a: Range, b: Range) -> Option<Range> {
    match op {
        IntOp::Add => Some(Range { lo: a.lo.checked_add(b.lo)?, hi: a.hi.checked_add(b.hi)? }),
        IntOp::Sub => Some(Range { lo: a.lo.checked_sub(b.hi)?, hi: a.hi.checked_sub(b.lo)? }),
        IntOp::Mul => {
            let p = a.lo.checked_mul(b.lo)?;
            let q = a.lo.checked_mul(b.hi)?;
            let r = a.hi.checked_mul(b.lo)?;
            let s = a.hi.checked_mul(b.hi)?;
            Some(Range { lo: p.min(q).min(r.min(s)), hi: p.max(q).max(r.max(s)) })
        }
        // `|a % b| <= |a|` and `|a % b| < |b|`; `b.abs_max == 0` means the divisor is 0.
        IntOp::Mod => {
            let b_abs = b.abs_max()?;
            if b_abs == 0 {
                return None;
            }
            let m = a.abs_max()?.min(b_abs.checked_sub(1)?);
            Some(Range { lo: m.checked_neg()?, hi: m })
        }
    }
}

/// Lean `rangeOf`: a range for every `int` the expression can succeed with.
fn range_of(e: &ir::Expr) -> Option<Range> {
    match e.kind() {
        ExprKind::Const(Const::Int(n)) => Some(Range { lo: i128::from(*n), hi: i128::from(*n) }),
        ExprKind::Field { base, field, optional: false } => match base.ty() {
            Type::Struct(desc) => desc
                .fields()
                .get(usize::from(field.index()))?
                .range()
                .map(|(lo, hi)| Range { lo: i128::from(lo), hi: i128::from(hi) }),
            _ => None,
        },
        ExprKind::Binary { op: BinaryOp::Int(op), lhs, rhs } => {
            op_range(*op, range_of(lhs)?, range_of(rhs)?)?.clamp_i64()
        }
        ExprKind::Conditional { then, otherwise, .. } => {
            let (t, o) = (range_of(then)?, range_of(otherwise)?);
            Some(Range { lo: t.lo.min(o.lo), hi: t.hi.max(o.hi) })
        }
        _ => None,
    }
}

/// Lean `skipCheck`: `lhs op rhs` cannot overflow.
fn skip_check(op: IntOp, lhs: &ir::Expr, rhs: &ir::Expr) -> bool {
    matches!(op, IntOp::Add | IntOp::Sub | IntOp::Mul)
        && range_of(lhs).zip(range_of(rhs)).and_then(|(a, b)| op_range(op, a, b)).is_some_and(Range::fits)
}

/// Lean `cmpDecide`: the value of `a op b` for every `a` in `ra` and `b` in `rb`,
/// when the ranges decide it.
fn cmp_decide(op: CompareOp, a: Range, b: Range) -> Option<bool> {
    let (holds, fails) = match op {
        CompareOp::Lt => (a.hi < b.lo, b.hi <= a.lo),
        CompareOp::Le => (a.hi <= b.lo, b.hi < a.lo),
        CompareOp::Gt => (b.hi < a.lo, a.hi <= b.lo),
        CompareOp::Ge => (b.hi <= a.lo, a.hi < b.lo),
    };
    if holds {
        Some(true)
    } else if fails {
        Some(false)
    } else {
        None
    }
}

/// Lean `safe`: evaluating `e` cannot fail.
fn safe(e: &ir::Expr) -> bool {
    match e.kind() {
        ExprKind::Const(_) | ExprKind::Root => true,
        ExprKind::Field { base, optional: false, .. } => safe(base),
        ExprKind::Binary { op: BinaryOp::Int(op), lhs, rhs } => {
            safe(lhs)
                && safe(rhs)
                && match op {
                    IntOp::Add | IntOp::Sub | IntOp::Mul => skip_check(*op, lhs, rhs),
                    IntOp::Mod => range_of(rhs).is_some_and(|r| r.lo > 0 || r.hi < 0),
                }
        }
        ExprKind::Binary { op: BinaryOp::Eq(EqKind::Int | EqKind::Bool), lhs, rhs } => safe(lhs) && safe(rhs),
        ExprKind::Compare { kind: CompareKind::Int, first, rest } => match rest.as_slice() {
            [(_, b)] => safe(first) && safe(b),
            _ => false,
        },
        ExprKind::Logic { operands, .. } => operands.iter().all(safe),
        ExprKind::Unary { op: UnaryOp::Not, arg } => safe(arg),
        ExprKind::Conditional { cond, then, otherwise } => safe(cond) && safe(then) && safe(otherwise),
        _ => false,
    }
}

fn is_int(e: &ir::Expr, n: i64) -> bool {
    matches!(e.kind(), ExprKind::Const(Const::Int(m)) if *m == n)
}

fn is_bool(e: &ir::Expr, b: bool) -> bool {
    matches!(e.kind(), ExprKind::Const(Const::Bool(c)) if *c == b)
}

fn same(a: &ir::Expr, b: &ir::Expr) -> bool {
    same_expr(a, b)
}

/// Lean `simp`: the children, then the node.
fn simplify(e: &mut ir::Expr) {
    for c in e.children_mut() {
        simplify(c);
    }
    simplify_node(e);
}

/// Lean `simp1`, plus `reorder` on `&&`/`||`. Each replacement keeps the node's
/// type (`replace_with_child` and `replace_with_const` refuse otherwise), so a
/// refused rewrite leaves the node as it was.
fn simplify_node(e: &mut ir::Expr) {
    enum Rewrite {
        Child(usize),
        Int(i64),
        Bool(bool),
        Logic,
        None,
    }
    let rewrite = match e.kind() {
        ExprKind::Binary { op: BinaryOp::Int(IntOp::Add), lhs, rhs } => {
            if is_int(rhs, 0) {
                Rewrite::Child(0)
            } else if is_int(lhs, 0) {
                Rewrite::Child(1)
            } else {
                Rewrite::None
            }
        }
        ExprKind::Binary { op: BinaryOp::Int(IntOp::Sub), rhs, .. } if is_int(rhs, 0) => Rewrite::Child(0),
        ExprKind::Binary { op: BinaryOp::Int(IntOp::Mul), lhs, rhs } => {
            if is_int(rhs, 1) {
                Rewrite::Child(0)
            } else if is_int(lhs, 1) {
                Rewrite::Child(1)
            } else if (is_int(rhs, 0) && safe(lhs)) || (is_int(lhs, 0) && safe(rhs)) {
                Rewrite::Int(0)
            } else {
                Rewrite::None
            }
        }
        ExprKind::Compare { kind: CompareKind::Int, first, rest } => match rest.as_slice() {
            [(op, b)] if safe(first) && safe(b) => match (range_of(first), range_of(b)) {
                (Some(ra), Some(rb)) => cmp_decide(*op, ra, rb).map_or(Rewrite::None, Rewrite::Bool),
                _ => Rewrite::None,
            },
            _ => Rewrite::None,
        },
        ExprKind::Binary { op: BinaryOp::Eq(EqKind::Int | EqKind::Bool), lhs, rhs } if safe(lhs) && same(lhs, rhs) => {
            Rewrite::Bool(true)
        }
        ExprKind::Unary { op: UnaryOp::Not, arg } if matches!(arg.kind(), ExprKind::Unary { op: UnaryOp::Not, .. }) => {
            Rewrite::Child(0)
        }
        ExprKind::Logic { .. } => Rewrite::Logic,
        ExprKind::Conditional { cond, then, otherwise } => {
            if is_bool(cond, true) {
                Rewrite::Child(1)
            } else if is_bool(cond, false) {
                Rewrite::Child(2)
            } else if safe(cond) && same(then, otherwise) {
                Rewrite::Child(1)
            } else {
                Rewrite::None
            }
        }
        _ => Rewrite::None,
    };
    match rewrite {
        Rewrite::Child(i) => {
            let double_not = matches!(e.kind(), ExprKind::Unary { op: UnaryOp::Not, .. });
            if e.replace_with_child(i) && double_not {
                // `not not x`: the inner `not` is now the node; `x` is its child.
                e.replace_with_child(0);
            }
        }
        Rewrite::Int(n) => {
            let _ = e.replace_with_const(Const::Int(n));
        }
        Rewrite::Bool(b) => {
            let _ = e.replace_with_const(Const::Bool(b));
        }
        Rewrite::Logic => logic(e),
        Rewrite::None => {}
    }
}

/// `&&` (and, dually, `||`) over its operands in order, as Lean's left-nested
/// `and` (`and_assoc`): an operand `true` adds nothing; nothing after a `false` is
/// evaluated; and a `false` with only safe operands before it is the result.
fn logic(e: &mut ir::Expr) {
    let ExprKind::Logic { op, .. } = e.kind() else { return };
    // The operand that decides the result (`false` for `&&`), and the neutral one.
    let absorbing = *op == LogicOp::Or;
    let Some(operands) = e.logic_operands_mut() else { return };
    operands.retain(|o| !is_bool(o, !absorbing));
    if let Some(k) = operands.iter().position(|o| is_bool(o, absorbing)) {
        operands.truncate(k.saturating_add(1));
        if operands.iter().take(k).all(safe) {
            let _ = e.replace_with_const(Const::Bool(absorbing));
            return;
        }
    }
    reorder(operands);
    match operands.len() {
        0 => {
            let _ = e.replace_with_const(Const::Bool(!absorbing));
        }
        1 => {
            e.replace_with_child(0);
        }
        _ => {}
    }
}

/// Node count of `e` (matches `analysis::subtree_cost(e)` without allocating).
fn node_cost(e: &ir::Expr) -> u32 {
    let mut cost: u32 = 1;
    for c in e.children() {
        cost = cost.saturating_add(node_cost(c));
    }
    cost
}

/// Lean `and_swap_in_chain`/`or_swap_in_chain`: each maximal run of safe operands
/// sorted by size, smallest first (stable), so a cheap operand that decides the
/// result spares the others. An operand that can fail keeps its place, so every
/// error the original order reports, this order reports too.
fn reorder(operands: &mut [ir::Expr]) {
    let mut start = 0;
    while start < operands.len() {
        let len = operands.get(start..).map_or(0, |rest| rest.iter().take_while(|o| safe(o)).count());
        if let Some(run) = operands.get_mut(start..start.saturating_add(len)) {
            run.sort_by_key(node_cost);
        }
        start = start.saturating_add(len.max(1));
    }
}

fn collect_unchecked(e: &ir::Expr, out: &mut BTreeSet<NodeId>) {
    if let ExprKind::Binary { op: BinaryOp::Int(op), lhs, rhs } = e.kind() {
        if skip_check(*op, lhs, rhs) {
            out.insert(e.id());
        }
    }
    for c in e.children() {
        collect_unchecked(c, out);
    }
}

#[cfg(test)]
mod tests {
    use super::super::analysis::subtree_cost;
    use super::*;
    use dictum_check::{Env, FieldDesc, StructDesc};

    fn int() -> Type {
        Type::Int
    }
    fn float() -> Type {
        Type::Float
    }
    fn boolean() -> Type {
        Type::Bool
    }

    /// `age: u8`, `n: i64`, `x: f64`, `b: bool`: the Lean examples' environment.
    static FIELDS: [FieldDesc; 4] = [
        FieldDesc::new("age", int).with_range(Some((0, 255))),
        FieldDesc::new("n", int).with_range(Some((i64::MIN, i64::MAX))),
        FieldDesc::new("x", float),
        FieldDesc::new("b", boolean),
    ];
    static ROOT: StructDesc = StructDesc::new("Root", &FIELDS);

    /// The optimized program, and the unchecked operations' count.
    fn optimized(src: &str) -> (ir::Program, usize) {
        let env = Env::<()>::new(&ROOT, alloc::vec::Vec::new()).unwrap();
        let ast = dictum_syntax::parse(src).unwrap();
        let mut ir = dictum_check::check(&ast, &env, &Type::Any).unwrap();
        let unchecked = optimize(&mut ir);
        ir.validate(&env).unwrap();
        (ir, unchecked.len())
    }

    fn same_as(src: &str, simpler: &str) -> bool {
        same_expr(optimized(src).0.body(), optimized(simpler).0.body())
    }

    #[test]
    fn ranges_decide_which_checks_go() {
        // `Opt/Range.lean`'s examples.
        assert_eq!(optimized("age + 1").1, 1);
        assert_eq!(optimized("age * 1000").1, 1);
        // The three products inside the `let` fit; `a4 * a4` reads a variable, which has
        // no range, so it keeps its check.
        assert_eq!(optimized("let a4 = age * age * (age * age); a4 * a4").1, 3);
        assert_eq!(optimized("age * age * (age * age) * (age * age * (age * age))").1, 6);
        assert_eq!(optimized("n + 1").1, 0, "an i64 field can overflow");
        assert_eq!(optimized("n - n").1, 0);
    }

    #[test]
    fn rewrites_follow_simp() {
        // `Opt/Simp.lean`'s examples, and each rule.
        assert!(same_as("age < 256", "true"));
        assert!(same_as("age < 0", "false"));
        assert!(same_as("age >= 0", "true") && same_as("age > 255", "false"));
        assert!(same_as("age <= 255", "true") && same_as("256 <= age", "false"));
        assert!(same_as("256 > age", "true") && same_as("age >= 256", "false"));
        assert!(!same_as("age <= 254", "true") && !same_as("age <= 254", "false"));
        assert!(same_as("age * 0 + n", "n"));
        assert!(same_as("n + 0", "n") && same_as("0 + n", "n") && same_as("n - 0", "n"));
        assert!(same_as("n * 1", "n") && same_as("1 * n", "n"));
        assert!(same_as("not not b", "b"));
        assert!(same_as("true ? n : age", "n") && same_as("false ? n : age", "age"));
        assert!(same_as("age < 3 ? n : n", "n"));
        assert!(same_as("n == n", "true") && same_as("b == b", "true"));
        assert!(same_as("b && true", "b") && same_as("false || b", "b"));
        assert!(same_as("b && false", "false") && same_as("b || true", "true"));
    }

    #[test]
    fn a_rewrite_that_could_hide_an_error_is_not_made() {
        // `7 % 0 * 0` fails; `0` would not.
        assert!(!same_as("7 % 0 * 0", "0"));
        assert!(!same_as("n * 2 * 0", "0"), "n * 2 can overflow");
        assert!(!same_as("7 % n > 0 && false", "false"));
        assert!(!same_as("n * 2 == n * 2", "true"));
        // Outside the `int`/`bool` fragment: `-0.0 + 0.0` is `0.0`, NaN is not NaN.
        assert!(!same_as("x + 0.0", "x"));
        assert!(!same_as("x == x", "true"));
        // A chain is not one of the fragment's comparisons.
        assert!(!same_as("0 <= age <= 255", "true"));
        assert!(!same_as("7 % n >= 0", "true"), "the operand can fail");
    }

    #[test]
    fn nothing_after_false_is_kept() {
        assert!(same_as("7 % n > 0 && false && b", "7 % n > 0 && false"));
        assert!(same_as("7 % n > 0 || true || b", "7 % n > 0 || true"));
    }

    #[test]
    fn safe_operands_reorder_and_others_keep_their_place() {
        // The cheaper safe operand goes first; the one that can fail stays first.
        assert!(same_as("age + 1 < 9 && b", "b && age + 1 < 9"));
        assert!(same_as("7 % n > 0 && age + 1 < 9 && b", "7 % n > 0 && b && age + 1 < 9"));
        assert!(!same_as("b && 7 % n > 0", "7 % n > 0 && b"));
    }

    fn checked_ir(src: &str, expected: &Type) -> ir::Program {
        let env = Env::<()>::new(&ROOT, alloc::vec::Vec::new()).unwrap();
        let ast = dictum_syntax::parse(src).unwrap();
        dictum_check::check(&ast, &env, expected).unwrap()
    }

    #[test]
    fn mod_zero_and_i64_clamping_match_lean() {
        // Guaranteed division by zero has no successful result, matching Lean `opRange`.
        assert_eq!(range_of(checked_ir("7 % 0", &Type::Int).body()), None);
        assert_eq!(range_of(checked_ir("age % 0", &Type::Int).body()), None);
        assert_eq!(range_of(checked_ir("n % 0", &Type::Int).body()), None);

        // `n + 1` clamps to `[i64::MIN + 1, i64::MAX]`, while `i64::MAX + 1` has empty range.
        assert_eq!(
            range_of(checked_ir("n + 1", &Type::Int).body()),
            Some(Range { lo: i128::from(i64::MIN) + 1, hi: i128::from(i64::MAX) })
        );
        assert_eq!(range_of(checked_ir("9223372036854775807 + 1", &Type::Int).body()), None);
        // `(n + 1) - 1`: `n + 1` keeps its check, but if it succeeds its result is at least
        // `i64::MIN + 1`, so `- 1` cannot underflow.
        assert_eq!(optimized("(n + 1) - 1").1, 1);

        // Deep `i64` products stay clamped to `i64` rather than saturating `i128`.
        let deep = checked_ir("((n * n) * (n * n)) * ((n * n) * (n * n))", &Type::Int);
        let r = range_of(deep.body()).unwrap();
        assert_eq!(r, Range { lo: i128::from(i64::MIN), hi: i128::from(i64::MAX) });
        assert!(r.fits());
        // Multiplying two `i64`-clamped ranges never overflows `i128`.
        assert!(op_range(IntOp::Mul, r, r).is_some());
    }

    #[test]
    fn node_cost_matches_exact_key_and_preserves_reorder() {
        for src in [
            "42",
            "b",
            "age + 1 < 9",
            "age * 2 + n - 1",
            "b ? age + 1 : n * 3",
            "age < 10 && age > 5 && b",
            "7 % n > 0 && age + 1 < 9 && b",
        ] {
            let ir = checked_ir(src, &Type::Any);
            assert_eq!(node_cost(ir.body()), subtree_cost(ir.body()), "{src}");
        }
        // Equal-cost safe operands keep their original order (stable sort).
        assert!(same_as("age < 10 && age > 5", "age < 10 && age > 5"));
        assert!(!same_as("age < 10 && age > 5", "age > 5 && age < 10"));
    }
}
