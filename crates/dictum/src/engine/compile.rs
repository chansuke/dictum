//! Typed IR to a tree of type-specialized closures.
//!
//! A node's Rust return type follows its dictum type: an `int` node returns `i64`,
//! a `bool` node returns `bool`. `Int + Int` is then a closure over two
//! `Node<IntK>`s with no runtime type test and no `Value` in between. Only compound,
//! optional and `any` nodes return `Value`.
//!
//! The compiler trusts the IR invariants. If one does not hold it returns `Ice`
//! (internal compiler error) instead of panicking; `Rule::compile` reports it as
//! E0900. There is no other failure mode.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;

use dictum_check::ir::{self, NodeId};

use crate::engine::SchemaCore;
use crate::engine::analysis::CacheSlot;
use crate::engine::frame::{Fault, Frame};
use crate::engine::value::{Str, Value};

/// The Rust type a node of some dictum type evaluates to.
pub(crate) trait Kind: 'static {
    type Out<'a>;
}

pub(crate) enum BoolK {}
pub(crate) enum IntK {}
pub(crate) enum FloatK {}
pub(crate) enum StrK {}
pub(crate) enum TimeK {}
pub(crate) enum DurationK {}
#[cfg(feature = "decimal")]
pub(crate) enum DecimalK {}
pub(crate) enum ValueK {}

impl Kind for BoolK {
    type Out<'a> = bool;
}
impl Kind for IntK {
    type Out<'a> = i64;
}
impl Kind for FloatK {
    type Out<'a> = f64;
}
impl Kind for StrK {
    type Out<'a> = Str<'a>;
}
impl Kind for TimeK {
    type Out<'a> = jiff::Timestamp;
}
impl Kind for DurationK {
    type Out<'a> = jiff::SignedDuration;
}
#[cfg(feature = "decimal")]
impl Kind for DecimalK {
    type Out<'a> = rust_decimal::Decimal;
}
impl Kind for ValueK {
    type Out<'a> = Value<'a>;
}

/// A compiled node producing `K::Out`.
pub(crate) type Node<K> = Box<dyn for<'a> Fn(&mut Frame<'a>) -> Result<<K as Kind>::Out<'a>, Fault> + Send + Sync>;

/// A compiled node, tagged by what it returns.
pub(crate) enum Compiled {
    Bool(Node<BoolK>),
    Int(Node<IntK>),
    Float(Node<FloatK>),
    Str(Node<StrK>),
    Time(Node<TimeK>),
    Duration(Node<DurationK>),
    #[cfg(feature = "decimal")]
    Decimal(Node<DecimalK>),
    Value(Node<ValueK>),
}

/// A compiled program.
pub(crate) struct Tree {
    pub(crate) lets: Vec<Node<ValueK>>,
    pub(crate) body: Node<ValueK>,
    pub(crate) max_predicate_depth: u16,
}

/// Internal compiler error: an IR invariant did not hold.
pub(crate) struct Ice(pub(crate) &'static str);

pub(crate) struct Compiler<'c> {
    schema: &'c Arc<SchemaCore>,
    patterns: &'c BTreeMap<NodeId, Arc<regex::Regex>>,
    /// Shared-cache slot per node id (RuleSet fast trees only).
    slots: Option<&'c [Option<CacheSlot>]>,
}

impl<'c> Compiler<'c> {
    pub(crate) fn new(
        schema: &'c Arc<SchemaCore>,
        patterns: &'c BTreeMap<NodeId, Arc<regex::Regex>>,
        slots: Option<&'c [Option<CacheSlot>]>,
    ) -> Self {
        Compiler { schema, patterns, slots }
    }

    pub(crate) fn tree(&self, program: &ir::Program) -> Result<Tree, Ice> {
        todo!()
    }

    /// Compile one node, then wrap it in its cache slot if it has one.
    ///
    /// TODO dispatch highlights:
    /// - `Field` chains on structs: one closure walking the `FieldId` path from the
    ///   root (`root.field(a).field(b)`), extracting the leaf by the leaf's type.
    /// - `Binary { op: Int(Add) }`: `let (a, b) = (self.int(lhs)?, self.int(rhs)?);
    ///   Compiled::Int(Box::new(move |f| ops::add_int(a(f)?, b(f)?).map_err(at(span))))`
    /// - `Compare` chain: evaluate `first`, then for each `(op, e)` evaluate `e` once,
    ///   compare with the previous operand, stop at the first `false`.
    /// - `In { haystack: Const array }`: `set::ConstSet::new(..)` once, `contains` per eval.
    /// - `Binary { op: Matches }` with a constant pattern: `self.patterns[&id]`.
    /// - `Predicate`: iterate `Seq` (borrowed view, owned, or range) charging one step
    ///   per element, push a `Scope`, run the body, pop; stop when decided.
    /// - `Call`: evaluate args into a small `Vec<Value>`, call `FuncImpl::call`, map
    ///   `CallError::Function` to `EvalErrorKind::Function { name, .. }`.
    /// - cache slot `s` (if any): `if let Some(v) = &f.cache[s] { return v.clone() }`,
    ///   else compute and store on success only.
    fn expr(&self, e: &ir::Expr) -> Result<Compiled, Ice> {
        todo!()
    }

    /// `e` as an `int` node. A `Value` node (a field read, a slot, `any` already
    /// checked) is wrapped with an extraction that reports `Internal` on mismatch.
    fn int(&self, e: &ir::Expr) -> Result<Node<IntK>, Ice> {
        todo!()
    }

    fn bool(&self, e: &ir::Expr) -> Result<Node<BoolK>, Ice> {
        todo!()
    }

    fn value(&self, e: &ir::Expr) -> Result<Node<ValueK>, Ice> {
        todo!()
    }
}
