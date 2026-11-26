# dictum

A statically typed [expr-lang](https://expr-lang.org/) dialect for Rust rule engines: compile rules once against a schema, then evaluate them many times against borrowed fact data.

## Highlights

- **Static typing**: Rules are type-checked at compile time against a `#[derive(Dictum)]` Rust schema (`Rule<T, F>`), with explicit numeric promotions, optional-chaining discipline (`?.` / `??`), and zero runtime type tests on typed paths.
- **Zero-copy borrowed facts**: Strings, nested structs, slices/vectors, and `BTreeMap`s are borrowed directly from your Rust values per evaluation without conversion or cloning.
- **Deterministic, `no_std`, and WASM-ready**: `#![no_std]` + `alloc` core, explicit evaluation clock via `Ctx::with_now` (no hidden wall clock), `libm` float operations for bit-identical results across targets, `unsafe_code = "forbid"`, no-panic clippy lints, bounded step and allocation budgets (`Limits`), and `wasm32-wasip1` support.
- **Explainable evaluation**: `rule.explain(&ctx)` runs the reference interpreter over the typed IR to produce a per-node trace (values, skipped short-circuit branches, and failures) without compiling a second engine.
- **Rule sets and sharing**: `RuleSet` compiles rules together, shares pure subexpressions across rules per evaluation (CSE), and supports lock-free hot reload via `Arc<RuleSet>`.
- **Formally verified core**: Core semantics, constant folding, interval-based overflow-check elision, algebraic simplifications, reordering, and CSE commutativity are proved in **Lean 4** (`formal/lean`) with differential tests against the Rust engine; `RuleSet` caching, budgets, and hot reload are model-checked in **TLA+** (`formal/tla`).

## Quickstart

See [crates/dictum/examples/quickstart.rs](crates/dictum/examples/quickstart.rs) for the complete runnable example.

```rust
use std::collections::BTreeMap;
use dictum::jiff::Timestamp;
use dictum::{Ctx, Dictum, Dynamic, Limits, Rule, RuleSet, Schema};

#[derive(Dictum)]
#[dictum(rename_all = "PascalCase")]
struct User {
    age: i64,
    group: String,
    email: Option<String>,
    tags: Vec<String>,
    attrs: Dynamic,
    #[dictum(skip)]
    password_hash: [u8; 32],
}

#[derive(Dictum)]
#[dictum(rename_all = "PascalCase")]
struct Order {
    total: f64,
    currency: String,
    notes: BTreeMap<String, String>,
}

/// The schema root: its fields are the rule's top-level variables (`user`, `order`).
#[derive(Dictum)]
struct Facts<'a> {
    user: &'a User,
    order: &'a Order,
}
type F = Facts<'static>;

// Build the schema once at startup.
let schema = Schema::<F>::builder()
    .pure_func("isHoliday", |_t: Timestamp| false)
    .build()?;

// Compile a single rule and evaluate against borrowed facts.
let rule: Rule<bool, F> =
    Rule::compile(r#"user.Group in ["admin"] && user.Age >= 18"#, &schema)?;

let user = User {
    age: 20,
    group: "admin".into(),
    email: None,
    tags: vec!["beta".into()],
    attrs: Dynamic::Nil,
    password_hash: [0; 32],
};
let order = Order { total: 120.0, currency: "USD".into(), notes: BTreeMap::new() };
let facts = Facts { user: &user, order: &order };
let ctx = Ctx::new(&facts)
    .with_now(Timestamp::now())
    .with_limits(Limits::default().with_max_steps(10_000));

let ok: bool = rule.eval(&ctx)?;
if !ok {
    println!("{}", rule.explain(&ctx));
}

// Compile and evaluate a rule set with cross-rule sharing.
let mut builder = RuleSet::<bool, F>::builder(&schema);
builder.insert("adult", "user.Age >= 18")?;
builder.insert("vip", "user.Age >= 18 && order.Total > 100")?;
let set = builder.build();
let outcomes = set.eval(&ctx)?;
for name in outcomes.matched() {
    println!("matched {name}");
}
```

## Cargo features

| Feature | Default | Description |
| --- | --- | --- |
| `std` | yes | Enables `std` in `regex` and `jiff` (thread-local regex pool); semantics are identical without it. |
| `tzdb` | yes | Bundles the IANA time zone database (`jiff/tzdb-bundle-always`); without it only `"UTC"` resolves. |
| `decimal` | no | Enables fixed-point `decimal` (`rust_decimal::Decimal`) literals, arithmetic, and schema functions. |
| `runtime-schema` | no | Enables `dictum::runtime` for schemas described at run time (used by `dictum-cli`). |

## Documentation

- [Architecture](docs/architecture.md) — workspace crates, module layers, data structures, and invariants
- [Language design](docs/dictum-design.md) — syntax, type system, builtins, and error codes
- [Feature map](docs/feature-map.md) — feature names, code locations, and checks
- [Verification](docs/verification.md) — tests, fuzzing, Lean 4 proofs, TLA+ models, and CI checks
- [Benchmarks](docs/benchmarks.md) — criterion comparisons against Go's `expr`
