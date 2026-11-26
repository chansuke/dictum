//! What each node's value depends on, and which nodes a `RuleSet` shares.
//!
//! Results are side tables indexed by `NodeId`; the IR itself is never annotated.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use dictum_check::ir::{
    self, BinaryOp, CompareKind, Const, Conversion, ExprKind, FloatOp, InOp, IndexOp, IntOp, Intrinsic, PredicateOp,
    SliceOp, SlotId, UnaryOp,
};
use dictum_check::{FieldId, FuncId, Type};
use dictum_syntax::ast::{CompareOp, LogicOp, Pointer};

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
    if let ExprKind::Predicate { seq, arg, body, .. } = e.kind() {
        for c in core::iter::once(&**seq).chain(arg.as_deref()) {
            let (d, p) = visit(c, schema, out);
            dep = dep.max(d);
            reads_pointer |= p;
        }
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
    // Every candidate occurrence, keyed by interned structural id.
    let mut interner = Interner::default();
    let mut occurrences: BTreeMap<u32, Vec<(usize, usize)>> = BTreeMap::new();
    for (r, (program, deps)) in rules.iter().enumerate() {
        for root in program.lets().iter().chain(core::iter::once(program.body())) {
            interner.intern(root, true, &mut |id, cost, key_id| {
                if cost >= MIN_SHARE_COST && deps.get(id) == Some(&Dependence::Input) {
                    occurrences.entry(key_id).or_default().push((r, id));
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

/// Whether `a` and `b` have the exact same structure, operand order included
/// (spans and node ids aside). Allocates nothing.
pub(crate) fn same_expr(a: &ir::Expr, b: &ir::Expr) -> bool {
    match (a.kind(), b.kind()) {
        (ExprKind::Const(x), ExprKind::Const(y)) => same_const(x, y),
        (ExprKind::Root, ExprKind::Root) => true,
        (ExprKind::Slot(x), ExprKind::Slot(y)) => x == y,
        (ExprKind::Pointer(x), ExprKind::Pointer(y)) => x == y,
        (
            ExprKind::Field { base: b1, field: f1, optional: o1 },
            ExprKind::Field { base: b2, field: f2, optional: o2 },
        ) => f1 == f2 && o1 == o2 && same_expr(b1, b2),
        (
            ExprKind::Index { base: b1, index: i1, op: op1, optional: o1 },
            ExprKind::Index { base: b2, index: i2, op: op2, optional: o2 },
        ) => op1 == op2 && o1 == o2 && same_expr(b1, b2) && same_expr(i1, i2),
        (
            ExprKind::Slice { base: b1, from: f1, to: t1, op: op1, optional: o1 },
            ExprKind::Slice { base: b2, from: f2, to: t2, op: op2, optional: o2 },
        ) => {
            op1 == op2
                && o1 == o2
                && same_expr(b1, b2)
                && same_opt(f1.as_deref(), f2.as_deref())
                && same_opt(t1.as_deref(), t2.as_deref())
        }
        (ExprKind::Unary { op: op1, arg: a1 }, ExprKind::Unary { op: op2, arg: a2 }) => op1 == op2 && same_expr(a1, a2),
        (ExprKind::Binary { op: op1, lhs: l1, rhs: r1 }, ExprKind::Binary { op: op2, lhs: l2, rhs: r2 }) => {
            op1 == op2 && same_expr(l1, l2) && same_expr(r1, r2)
        }
        (ExprKind::Logic { op: op1, operands: xs1 }, ExprKind::Logic { op: op2, operands: xs2 }) => {
            op1 == op2 && same_slice(xs1, xs2)
        }
        (ExprKind::Compare { kind: k1, first: f1, rest: r1 }, ExprKind::Compare { kind: k2, first: f2, rest: r2 }) => {
            k1 == k2
                && r1.len() == r2.len()
                && same_expr(f1, f2)
                && r1.iter().zip(r2).all(|((op1, e1), (op2, e2))| op1 == op2 && same_expr(e1, e2))
        }
        (
            ExprKind::In { op: op1, needle: n1, haystack: h1, negated: neg1 },
            ExprKind::In { op: op2, needle: n2, haystack: h2, negated: neg2 },
        ) => op1 == op2 && neg1 == neg2 && same_expr(n1, n2) && same_expr(h1, h2),
        (
            ExprKind::Conditional { cond: c1, then: t1, otherwise: o1 },
            ExprKind::Conditional { cond: c2, then: t2, otherwise: o2 },
        ) => same_expr(c1, c2) && same_expr(t1, t2) && same_expr(o1, o2),
        (ExprKind::Coalesce { lhs: l1, rhs: r1 }, ExprKind::Coalesce { lhs: l2, rhs: r2 }) => {
            same_expr(l1, l2) && same_expr(r1, r2)
        }
        (ExprKind::Array(xs1), ExprKind::Array(xs2)) => same_slice(xs1, xs2),
        (ExprKind::Map(kvs1), ExprKind::Map(kvs2)) => {
            kvs1.len() == kvs2.len()
                && kvs1.iter().zip(kvs2).all(|((k1, v1), (k2, v2))| same_expr(k1, k2) && same_expr(v1, v2))
        }
        (ExprKind::Range { from: f1, to: t1 }, ExprKind::Range { from: f2, to: t2 }) => {
            same_expr(f1, f2) && same_expr(t1, t2)
        }
        (
            ExprKind::Predicate { op: op1, seq: s1, arg: a1, body: b1 },
            ExprKind::Predicate { op: op2, seq: s2, arg: a2, body: b2 },
        ) => {
            op1 == op2
                && same_expr(s1, s2)
                && same_opt(a1.as_deref(), a2.as_deref())
                && same_opt(b1.as_deref(), b2.as_deref())
        }
        (ExprKind::Intrinsic { op: op1, args: a1 }, ExprKind::Intrinsic { op: op2, args: a2 }) => {
            op1 == op2 && same_slice(a1, a2)
        }
        (ExprKind::Call { func: f1, args: a1 }, ExprKind::Call { func: f2, args: a2 }) => {
            f1 == f2 && same_slice(a1, a2)
        }
        (ExprKind::Convert { op: op1, arg: a1 }, ExprKind::Convert { op: op2, arg: a2 }) => {
            op1 == op2 && a.ty() == b.ty() && same_expr(a1, a2)
        }
        _ => false,
    }
}

fn same_opt(a: Option<&ir::Expr>, b: Option<&ir::Expr>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => same_expr(x, y),
        (None, None) => true,
        _ => false,
    }
}

fn same_slice(a: &[ir::Expr], b: &[ir::Expr]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_expr(x, y))
}

fn same_const(a: &Const, b: &Const) -> bool {
    match (a, b) {
        (Const::Nil, Const::Nil) => true,
        (Const::Bool(x), Const::Bool(y)) => x == y,
        (Const::Int(x), Const::Int(y)) => x == y,
        (Const::Float(x), Const::Float(y)) => x.to_bits() == y.to_bits(),
        #[cfg(feature = "decimal")]
        (Const::Decimal(x), Const::Decimal(y)) => x.serialize() == y.serialize(),
        (Const::Str(x), Const::Str(y)) => x == y,
        (Const::Bytes(x), Const::Bytes(y)) => x == y,
        (Const::Time(t1, z1), Const::Time(t2, z2)) => t1 == t2 && z1 == z2,
        (Const::Duration(x), Const::Duration(y)) => x == y,
        (Const::Location(x), Const::Location(y)) => x == y,
        (Const::Array(xs), Const::Array(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys.iter()).all(|(x, y)| same_const(x, y))
        }
        (Const::Map(xs), Const::Map(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys.iter()).all(|((k1, v1), (k2, v2))| k1 == k2 && same_const(v1, v2))
        }
        _ => false,
    }
}

/// Structural interner mapping each unique subtree (and constant) to a dense `u32` id.
/// Each node's key stores only its own tag and child ids, so total work is linear in
/// the IR size (modulo `BTreeMap` lookup) rather than quadratic in tree depth.
#[derive(Default)]
struct Interner {
    nodes: BTreeMap<NodeKey, u32>,
    consts: BTreeMap<ConstKey, u32>,
    next_node: u32,
    next_const: u32,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct NodeKey {
    tag: NodeTag,
    children: Vec<u32>,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum NodeTag {
    Const(u32),
    Root,
    Slot(SlotId),
    Pointer(Pointer),
    Field(FieldId, bool),
    Index(IndexOp, bool),
    Slice(SliceOp, bool, bool, bool),
    Unary(UnaryOp),
    Binary(BinaryOp),
    Logic(LogicOp),
    Compare(CompareKind, Vec<CompareOp>),
    In(InOp, bool),
    Conditional,
    Coalesce,
    Array,
    Map,
    Range,
    Predicate(PredicateOp, bool, bool),
    Intrinsic(Intrinsic),
    Call(FuncId),
    // `Check` converts to the node's type, so the type is part of the key.
    Convert(Conversion, TypeKey),
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ConstKey {
    Nil,
    Bool(bool),
    Int(i64),
    Float(u64),
    #[cfg(feature = "decimal")]
    Decimal([u8; 16]),
    Str(Arc<str>),
    Bytes(Arc<[u8]>),
    Time(jiff::Timestamp, ZoneKey),
    Duration(jiff::SignedDuration),
    Location(ZoneKey),
    Array(Vec<u32>),
    Map(Vec<(Arc<str>, u32)>),
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ZoneKey {
    iana: Option<String>,
    fixed_secs: Option<i32>,
    debug: String,
}

fn zone_key(z: &jiff::tz::TimeZone) -> ZoneKey {
    ZoneKey {
        iana: z.iana_name().map(String::from),
        fixed_secs: z.to_fixed_offset().ok().map(jiff::tz::Offset::seconds),
        debug: format!("{z:?}"),
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum TypeKey {
    Any,
    Nil,
    Bool,
    Int,
    Float,
    #[cfg(feature = "decimal")]
    Decimal,
    String,
    Bytes,
    Time,
    Duration,
    Location,
    Array(Box<TypeKey>),
    Map(Box<TypeKey>),
    Option(Box<TypeKey>),
    Struct(usize),
}

fn type_key(ty: &Type) -> TypeKey {
    match ty {
        Type::Any => TypeKey::Any,
        Type::Nil => TypeKey::Nil,
        Type::Bool => TypeKey::Bool,
        Type::Int => TypeKey::Int,
        Type::Float => TypeKey::Float,
        #[cfg(feature = "decimal")]
        Type::Decimal => TypeKey::Decimal,
        Type::String => TypeKey::String,
        Type::Bytes => TypeKey::Bytes,
        Type::Time => TypeKey::Time,
        Type::Duration => TypeKey::Duration,
        Type::Location => TypeKey::Location,
        Type::Array(inner) => TypeKey::Array(Box::new(type_key(inner))),
        Type::Map(inner) => TypeKey::Map(Box::new(type_key(inner))),
        Type::Option(opt) => TypeKey::Option(Box::new(type_key(opt.inner()))),
        Type::Struct(desc) => TypeKey::Struct(core::ptr::from_ref(*desc).addr()),
    }
}

impl Interner {
    fn intern_const(&mut self, c: &Const) -> u32 {
        let key = match c {
            Const::Nil => ConstKey::Nil,
            Const::Bool(b) => ConstKey::Bool(*b),
            Const::Int(n) => ConstKey::Int(*n),
            Const::Float(x) => ConstKey::Float(x.to_bits()),
            #[cfg(feature = "decimal")]
            Const::Decimal(d) => ConstKey::Decimal(d.serialize()),
            Const::Str(s) => ConstKey::Str(s.clone()),
            Const::Bytes(b) => ConstKey::Bytes(b.clone()),
            Const::Time(t, z) => ConstKey::Time(*t, zone_key(z)),
            Const::Duration(d) => ConstKey::Duration(*d),
            Const::Location(z) => ConstKey::Location(zone_key(z)),
            Const::Array(xs) => ConstKey::Array(xs.iter().map(|x| self.intern_const(x)).collect()),
            Const::Map(kvs) => ConstKey::Map(kvs.iter().map(|(k, v)| (k.clone(), self.intern_const(v))).collect()),
        };
        if let Some(&id) = self.consts.get(&key) {
            return id;
        }
        let id = self.next_const;
        self.next_const = id.saturating_add(1);
        self.consts.insert(key, id);
        id
    }

    /// Intern `e` bottom-up, returning its structural id and its size in nodes.
    /// `visit(node_id, cost, key_id)` is called for every node, children first.
    /// With `commute`, the operands of a commutative operator are sorted, so
    /// `a + b` and `b + a` get the same id.
    fn intern(&mut self, e: &ir::Expr, commute: bool, visit: &mut dyn FnMut(usize, u32, u32)) -> (u32, u32) {
        let mut children: Vec<u32> = Vec::new();
        let mut cost: u32 = 1;
        for c in e.children() {
            let (child_id, n) = self.intern(c, commute, visit);
            children.push(child_id);
            cost = cost.saturating_add(n);
        }
        let tag = match e.kind() {
            ExprKind::Const(c) => NodeTag::Const(self.intern_const(c)),
            ExprKind::Root => NodeTag::Root,
            ExprKind::Slot(s) => NodeTag::Slot(*s),
            ExprKind::Pointer(p) => NodeTag::Pointer(*p),
            ExprKind::Field { field, optional, .. } => NodeTag::Field(*field, *optional),
            ExprKind::Index { op, optional, .. } => NodeTag::Index(*op, *optional),
            ExprKind::Slice { from, to, op, optional, .. } => {
                NodeTag::Slice(*op, *optional, from.is_some(), to.is_some())
            }
            ExprKind::Unary { op, .. } => NodeTag::Unary(*op),
            ExprKind::Binary { op, .. } => {
                if commute && commutes(*op) {
                    children.sort_unstable();
                }
                NodeTag::Binary(*op)
            }
            ExprKind::Logic { op, .. } => NodeTag::Logic(*op),
            ExprKind::Compare { kind, rest, .. } => NodeTag::Compare(*kind, rest.iter().map(|(op, _)| *op).collect()),
            ExprKind::In { op, negated, .. } => NodeTag::In(*op, *negated),
            ExprKind::Conditional { .. } => NodeTag::Conditional,
            ExprKind::Coalesce { .. } => NodeTag::Coalesce,
            ExprKind::Array(_) => NodeTag::Array,
            ExprKind::Map(_) => NodeTag::Map,
            ExprKind::Range { .. } => NodeTag::Range,
            ExprKind::Predicate { op, arg, body, .. } => NodeTag::Predicate(*op, arg.is_some(), body.is_some()),
            ExprKind::Intrinsic { op, .. } => NodeTag::Intrinsic(*op),
            ExprKind::Call { func, .. } => NodeTag::Call(*func),
            ExprKind::Convert { op, .. } => NodeTag::Convert(*op, type_key(e.ty())),
        };
        let key = NodeKey { tag, children };
        let key_id = if let Some(&id) = self.nodes.get(&key) {
            id
        } else {
            let id = self.next_node;
            self.next_node = id.saturating_add(1);
            self.nodes.insert(key, id);
            id
        };
        visit(e.id().index(), cost, key_id);
        (key_id, cost)
    }
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

#[cfg(test)]
pub(crate) fn subtree_cost(e: &ir::Expr) -> u32 {
    Interner::default().intern(e, false, &mut |_, _, _| {}).1
}

#[cfg(test)]
mod tests {
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
    fn opt_str() -> Type {
        Type::option(Type::String)
    }
    fn any_ty() -> Type {
        Type::Any
    }
    fn arr_int() -> Type {
        Type::array(Type::Int)
    }

    static FIELDS: [FieldDesc; 6] = [
        FieldDesc::new("a", int),
        FieldDesc::new("b", int),
        FieldDesc::new("x", float),
        FieldDesc::new("flag", boolean),
        FieldDesc::new("opt", opt_str),
        FieldDesc::new("dyn_val", any_ty),
    ];
    static EXTRA_FIELDS: [FieldDesc; 1] = [FieldDesc::new("xs", arr_int)];
    static ROOT: StructDesc = StructDesc::new("Root", &FIELDS);
    static OTHER: StructDesc = StructDesc::new("Other", &EXTRA_FIELDS);

    fn check_with(root: &'static StructDesc, src: &str, expected: &Type) -> ir::Program {
        let env = Env::<()>::new(root, Vec::new()).unwrap();
        let ast = dictum_syntax::parse(src).unwrap();
        dictum_check::check(&ast, &env, expected).unwrap()
    }

    fn check(src: &str) -> ir::Program {
        check_with(&ROOT, src, &Type::Any)
    }

    #[test]
    fn interner_distinguishes_float_bits_and_time_zones() {
        let mut interner = Interner::default();
        let pos_zero = interner.intern_const(&Const::Float(0.0));
        let neg_zero = interner.intern_const(&Const::Float(-0.0));
        let nan_a = interner.intern_const(&Const::Float(f64::NAN));
        let nan_b = interner.intern_const(&Const::Float(f64::from_bits(f64::NAN.to_bits() ^ 1)));
        assert_ne!(pos_zero, neg_zero);
        assert_ne!(pos_zero, nan_a);
        assert_ne!(nan_a, nan_b);
        assert_eq!(pos_zero, interner.intern_const(&Const::Float(0.0)));
        assert!(!same_const(&Const::Float(0.0), &Const::Float(-0.0)));
        assert!(same_const(&Const::Float(-0.0), &Const::Float(-0.0)));

        let ts = jiff::Timestamp::from_second(1_700_000_000).unwrap();
        let utc = jiff::tz::TimeZone::UTC;
        let plus_nine = jiff::tz::TimeZone::fixed(jiff::tz::Offset::from_seconds(9 * 3600).unwrap());
        let t_utc = interner.intern_const(&Const::Time(ts, utc.clone()));
        let t_p9 = interner.intern_const(&Const::Time(ts, plus_nine.clone()));
        assert_ne!(t_utc, t_p9);
        assert_eq!(t_utc, interner.intern_const(&Const::Time(ts, utc.clone())));
        assert!(!same_const(&Const::Time(ts, utc.clone()), &Const::Time(ts, plus_nine.clone())));

        let nested_a = interner.intern_const(&Const::Array(Arc::from([Const::Float(0.0), Const::Int(1)])));
        let nested_b = interner.intern_const(&Const::Array(Arc::from([Const::Float(-0.0), Const::Int(1)])));
        assert_ne!(nested_a, nested_b);
    }

    #[cfg(feature = "decimal")]
    #[test]
    fn interner_distinguishes_decimal_scale() {
        let mut interner = Interner::default();
        let d1 = rust_decimal::Decimal::new(10, 1);
        let d2 = rust_decimal::Decimal::new(100, 2);
        assert_eq!(d1, d2);
        assert_ne!(interner.intern_const(&Const::Decimal(d1)), interner.intern_const(&Const::Decimal(d2)));
        assert!(!same_const(&Const::Decimal(d1), &Const::Decimal(d2)));
    }

    #[test]
    fn interner_preserves_metadata_and_commutativity_rules() {
        let mut interner = Interner::default();
        let mut id_of = |e: &ir::Expr, commute: bool| interner.intern(e, commute, &mut |_, _, _| {}).0;

        // Commutative `+` matches across order when `commute = true`, not when `false`.
        let lhs_rhs = check("a + b");
        let rhs_lhs = check("b + a");
        assert_eq!(id_of(lhs_rhs.body(), true), id_of(rhs_lhs.body(), true));
        assert_ne!(id_of(lhs_rhs.body(), false), id_of(rhs_lhs.body(), false));
        assert!(!same_expr(lhs_rhs.body(), rhs_lhs.body()));

        // Short-circuit `&&` is never commutative.
        let and1 = check("a > 0 && b > 0");
        let and2 = check("b > 0 && a > 0");
        assert_ne!(id_of(and1.body(), true), id_of(and2.body(), true));

        // `Convert::Check` target type is part of the key.
        let chk_int = check_with(&ROOT, "dyn_val", &Type::Int);
        let chk_bool = check_with(&ROOT, "dyn_val", &Type::Bool);
        assert_ne!(id_of(chk_int.body(), true), id_of(chk_bool.body(), true));
        assert!(!same_expr(chk_int.body(), chk_bool.body()));

        // Slice bounds presence (`from` vs `to`) is distinguished.
        let slice_from = check_with(&OTHER, "xs[1:]", &Type::Any);
        let slice_to = check_with(&OTHER, "xs[:1]", &Type::Any);
        assert_ne!(id_of(slice_from.body(), true), id_of(slice_to.body(), true));
        assert!(!same_expr(slice_from.body(), slice_to.body()));

        // `in` vs `not in` is distinguished.
        let in_pos = check("a in [1, 2]");
        let in_neg = check("a not in [1, 2]");
        assert_ne!(id_of(in_pos.body(), true), id_of(in_neg.body(), true));
        assert!(!same_expr(in_pos.body(), in_neg.body()));

        // Predicate with `arg` vs without is distinguished.
        let red_no_init = check_with(&OTHER, "reduce(xs, #acc + #)", &Type::Int);
        let red_init = check_with(&OTHER, "reduce(xs, #acc + #, 0)", &Type::Int);
        assert_ne!(id_of(red_no_init.body(), true), id_of(red_init.body(), true));
        assert!(!same_expr(red_no_init.body(), red_init.body()));
    }
}
