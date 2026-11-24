# dictum architecture

The design behind the workspace skeleton under `crates/`. It implements [the design document](dictum-design.md) and decides the points that document left open.

## Problem

dictum is an expr-lang dialect for rule engines: compile a rule once against a schema, then evaluate it many times against borrowed fact data. Evaluation must be fast, never panic, stop within limits, and be `Send + Sync` after compilation. The design doc already picks the overall approach (pipeline crates, typed IR compiled to a closure tree, a borrowing `Value`, derive-generated field access), and this candidate keeps it. What makes the shape hard is that the doc's parts do not fit together as written. The grounding lists eight such places: no_std dependencies, the zero-copy claim, `Arc` vs `&'static` struct types, calls on arbitrary expressions, purity and clock, `len` vs byte indexing, how several schema variables map to one `Frame::root`, and missing milestone criteria. I found more while sketching. `Schema` (in `dictum-check`) holds user functions whose implementations need `Value` (in `dictum-eval`), which is a dependency cycle. `Limits::max_depth` is a static property. Milestone 2's untyped interpreter cannot read fields, because `Object::field` takes a `FieldId` and only the checker produces those. `Type::Error` would leak into the IR. And splitting `dictum-eval` from `dictum` would force the evaluator's internals to be public. Greenfield: there is no code to stay compatible with. The constraints are the doc and GROUNDING.md.

## Usage (caller's view)

Written before the types. `crates/dictum/examples/quickstart.rs` contains exactly this code, and `cargo check --all-targets` compiles it, which also runs the real `#[derive(Dictum)]` on it.

```rust
use dictum::jiff::Timestamp;
use dictum::{CompileError, Ctx, Dictum, Dynamic, Limits, Rule, RuleSet, Schema, SchemaError};

#[derive(Dictum)]
#[dictum(rename_all = "PascalCase")]          // rules say user.Age, as in Go
pub struct User {
    pub age: i64,
    pub group: String,
    pub email: Option<String>,                // Option<string>: needs ?. / ??
    pub tags: Vec<String>,                    // borrowed, never converted
    pub attrs: Dynamic,                       // type `any` (e.g. JSON attributes)
    #[dictum(skip)]
    pub password_hash: [u8; 32],
}

#[derive(Dictum)]
#[dictum(rename_all = "PascalCase")]
pub struct Order { pub total: f64, pub currency: String, pub items: Vec<Item>, pub notes: BTreeMap<String, String> }

/// The schema root: its fields are the rule's variables (`user`, `order`).
#[derive(Dictum)]
pub struct Facts<'a> { pub user: &'a User, pub order: &'a Order }
type F = Facts<'static>;
```

Call site 1, startup:

```rust
fn build_schema() -> Result<Schema<F>, SchemaError> {
    Schema::<F>::builder()
        .pure_func("isHoliday", |t: Timestamp| is_holiday(t))        // foldable, shareable
        .func("rate", |code: &str| -> Result<f64, RateError> { ... })  // borrowed arg, fallible
        .build()
}
```

Call site 2, one rule per request:

```rust
let rule: Rule<bool, F> = Rule::compile(r#"user.Group in ["admin"] && user.Age >= 18"#, &schema)?;
let facts = Facts { user: &user, order: &order };
let ctx = Ctx::new(&facts).with_now(Timestamp::now()).with_limits(Limits::default().with_max_steps(10_000));
let ok: bool = rule.eval(&ctx)?;
if !ok { println!("{}", rule.explain(&ctx)); }                       // explain never fails

let discount: Rule<f64, F> = Rule::compile("order.Total > 100 ? 0.1 : 0", &schema)?;
let anything: Rule<Dynamic, F> = Rule::compile("map(order.Items, .Sku)", &schema)?;
```

Call site 3, a rule set loaded from storage, rebuilt on change and swapped with `arc-swap`:

```rust
let mut builder = RuleSet::builder(&schema);
for (name, src) in rows {
    if let Err(e) = builder.insert(name, src) { errors.push(e) }      // per-rule diagnostics
}
let set: RuleSet<bool, F> = builder.build();                         // sharing decided here
let outcomes = set.eval(&ctx)?;                                      // first failing rule: err.rule()
for name in outcomes.matched() { ... }
if let Some(report) = set.explain("adult", &ctx) { ... }
```

Compile-time rejections, checked with a probe crate. Each is a Rust compile error: a `HashMap` field (custom `on_unimplemented` message), a `u64` field, a field named `contains` (a reserved word, reported by the derive), a struct with type parameters, `rule.eval(&Ctx::new(&other_facts))` with the wrong facts type (`E0271`), and `use dictum::engine::...` (`E0603`, private module).

## Shape

### Crates and what each may see

Four crates. The doc's `dictum-eval` is merged into `dictum`.

| crate | owns | public = contract with | depends on |
| --- | --- | --- | --- |
| `dictum-syntax` | lexer, Pratt parser, AST, `Span`, `Diagnostic`/`ErrorCode`, the `Builtin` name list, identifier rules | `dictum-check` (AST), everyone (diagnostics) | nothing |
| `dictum-check` | `Type`, `StructDesc`, `Env<P>`, typed IR, checker, overload tables (`resolve`) | `dictum` | syntax, jiff (folded time constants) |
| `dictum` | the public API, plus `engine/` (private): `Value`, folding, closure compiler, `Frame`/`Budget`, CSE, spec interpreter (`interp`, which implements explain), builtin impls | users | check, derive, jiff, regex, libm |
| `dictum-derive` | `#[derive(Dictum)]` | users via `dictum::Dictum` | syn, quote, syntax |

Enforcement: `unreachable_pub = "deny"` (workspace lint), so every `pub` item is either reachable from its crate root on purpose or the build fails. Internals are `pub(crate)`. The engine is a private module, so importing it from outside fails the build (`E0603`). Inner crates say "no API stability" in their manifests, and `dictum` pins them with `=0.1.0`. The one reachable internal is `dictum::__private`, `#[doc(hidden)]`. Derive output has to name something, and that module holds only what it names. `Value` is opaque even there: it is a struct with a private `Repr` enum.

Inside a crate, modules are layered: a module uses only modules of lower layers. `tests/architecture.rs` in `dictum-syntax`, `dictum-check` and `dictum` lists the layers (for `dictum`, the top-level modules and the submodules of `engine`) and checks every `crate::`, `super::` and `self::` path outside test code (`dictum_testkit::layers`). Two cycles in `dictum` predate the check: `value` with `facts` and with `dynamic`. They are listed as `KNOWN` and removed one at a time; the test fails on a new upward use and on a `KNOWN` edge that is no longer used.

### Data structures, input to output

- AST (`dictum_syntax::ast`). Identifiers are `&'src str`, with no interner, because the AST lives for one compile call and the IR replaces every name with an index. The parser desugars pipes, `.Field`, and `?:` (to `||`). It flattens `&&`/`||` into n-ary `Logic` nodes, makes comparison chains one `Compare` node, and places `let` only at program level (`ast::Program { lets, body }`), as the EBNF says. Callees are `Callee::Builtin(Builtin) | Callee::Func(&str)`.
- `Type` (`dictum_check`). `Struct(&'static StructDesc)` uses address identity. `Option(OptionType)` has a private field, so `Option<Option<T>>` and `Option<Nil>` cannot exist. `Map` has string keys only. There is no `Func` variant (functions are not values) and no `Error` variant (the checker's `Inferred::Poisoned` is private, so a poisoned type cannot reach the IR). `Tag` is the runtime shadow of a type and is used for `any` dispatch.
- `StructDesc` / `FieldDesc`: a `static` emitted by the derive. `FieldDesc::ty` is a `fn() -> Type`, so the static can be built in a const context and recursive structs need no lazy initialization.
- `Env<P>`: root descriptor plus functions as `(name, FuncSig, P)`. The checker never looks inside `P`, and `dictum` instantiates it with `FuncImpl { purity, call }`. This breaks the doc's schema/value cycle, and signature and implementation stay in one entry.
- Typed IR (`dictum_check::ir`). Every overload is resolved to a monomorphic op (`BinaryOp::Int(IntOp::Add)`, `Eq(EqKind::String)`, `Intrinsic::LenString`, ...), and every implicit conversion is a `Convert` node. Names become `FieldId`, `SlotId`, or `FuncId`, and each node has a dense `NodeId`. `Expr` fields are private: `dictum` can read nodes but can only replace one with a constant of the same type (`replace_with_const`), so folding cannot make the IR ill-typed. The crate boundary is what enforces this, which is one reason `dictum-check` stays a separate crate.
- `Value<'a>` (private repr). Scalars are held by value. `Str`/`Bytes` are borrowed or `Arc`-shared. `Seq` is `Borrowed(&dyn SeqView) | Owned(Arc<[Value]>) | Range{from,to}`, `MapRef` is `Borrowed(&dyn MapView) | Owned(Arc<[(Arc<str>, Value)]>)`, and `Object(&dyn Object)` covers nested structs.
- Compiled tree: `Node<K> = Box<dyn for<'a> Fn(&mut Frame<'a>) -> Result<K::Out<'a>, Fault> + Send + Sync>`, where `K` follows the dictum type (`IntK → i64`, `BoolK → bool`, `StrK → Str<'a>`, `ValueK → Value<'a>`). `Int + Int` composes two `Node<IntK>` with no runtime tag test.
- `Frame<'a>` (per evaluation): root, optional `now`, let slots, predicate scopes, `Budget`, and the shared cache `Vec<Option<Value>>`. The explain trace is passed to the interpreter separately, so the fast path carries no trace field. Compiled rules hold no mutable state.
- Side tables are keyed by `NodeId`: dependence, CSE slots, constant regexes, explain outcomes. The IR is never annotated.

Flow. `Rule::compile`: `parse → check(expected = T::expected()) → dependence → fold → constant regexes → closure tree`. `Rule::eval`: `Frame::new → run → T::from_value`. `RuleSetBuilder::insert` parses and checks only. `build` runs `share()` across all rules, then compiles each rule's closure tree with its slot table. `explain` compiles nothing: it runs the spec interpreter (`engine/interp.rs`) over the rule's typed IR and records a trace. Tracing any call takes at most three files: the public type (`rule.rs`), `engine/mod.rs`, and the engine stage.

### Decisions on the grounding issues

| # | decision |
| --- | --- |
| 1 | `regex` 1.x supports no_std with alloc only. Verified two ways: in the cached 1.13.1 source, `lib.rs` is `#![no_std]` and links `std` only with the `std` feature, and `cargo tree --no-default-features` shows no `std` feature on any runtime dependency. Without std its cache pool falls back to a spin lock, which is acceptable on single-core no_std targets. Constant `in` sets are a sorted boxed slice with binary search (`engine/set.rs`), with no hasher, no seed choice and no `indexmap`/`foldhash` dependency. Map literals are small insertion-ordered vec-maps. There is no lazily built explain tree, so no once-cell is needed (explain runs the interpreter). Also found: `floor`/`ceil`/`round`/`pow` are not in `core`, so the build always uses `libm`, which also makes results bit-identical across targets. |
| 2 | What is really borrowed: string bytes, nested structs, sequences through `&dyn SeqView` (elements read on access, so a `Vec<String>` is never converted), and maps through `&dyn MapView`. Scalars are copied. Values computed during evaluation are owned through `Arc`. `HashMap` fields are rejected (nondeterministic `keys()`). Supported containers are `BTreeMap<String, T>`, `Vec`, `Box<[T]>`, and `&[T]`. |
| 3 | `&'static StructDesc` everywhere (`Type::Struct`, `Object::desc`, `Dictum::__desc`), emitted as a `static`. No `Arc`. Consequence: the derive rejects type parameters, because a static cannot be generic. |
| 4 | Only `ident(args)` is a call. `(` after any other postfix is E0108. `Type::Func` is removed. Method calls are out of scope for v0.1. |
| 5 | Purity is the method name: `func` (volatile: never folded, never shared) vs `pure_func`. There is no clock object and no default time: the caller passes a timestamp with `Ctx::with_now`, and tests pass a constant. A rule that calls `now()` on a `Ctx` without one fails with `ClockUnset` (E0311), so a forgotten time is loud, not a silent wall clock, and rules that never call `now()` need no time (no_std callers without a clock). Because `now` is fixed per evaluation, `now()` has dependence `Input`, so CSE shares it across the rules of one `RuleSet::eval`. It is never constant-folded. |
| 6 | Kept as the doc decided: `len` counts characters (Go-compatible), and indices, slices and `indexOf` are byte offsets. The mismatch cannot silently produce wrong text: a misuse raises `CharBoundary` (E0304) carrying `Help::ByteIndices`, which says why. |
| 7 | The schema is built from one root `#[derive(Dictum)]` struct, and its fields are the variables. `$env` is the root. `var`/`var_type` are gone, so there is no second variable list to keep in sync with the facts type. Several sources compose by reference fields (`Facts<'a> { user: &'a User }`), and dynamic data uses `Dynamic` fields (`any`). `Frame::root` is that struct. |
| 8 | Criteria for every milestone are below. M2 and M3 swap places, because the reference interpreter must read fields by `FieldId`, which only the checker produces. |

### Other inconsistencies fixed

- Schema/value dependency cycle: solved by `Env<P>` (above).
- `Limits::max_depth` becomes a parser constant, `MAX_DEPTH = 128`. This also bounds native stack use in every recursive pass and in evaluation. Flattened `&&`/`||` keep long conjunctions under it.
- `Object: Send + Sync` is dropped. Facts are borrowed by one evaluation on one thread. `Rule: Send + Sync` holds for any `F`, asserted at compile time with a `!Sync` `F`.
- `RuleSet::insert` on a live set would recompute shared slots on every insert. Replaced by a `RuleSetBuilder` and an immutable `RuleSet`, which also matches the doc's hot-reload model. `insert_many` is dropped (a loop over `insert` does the same).
- CSE: each rule keeps its own closure for a shared node, and only the cache slot is shared. A value from rule A is valid for rule B, but an error is not, because its spans would point into A's source. So errors are never cached, and a rule that hits one recomputes with its own spans.
- `RuleSet::eval` runs rules in insertion order and stops at the first error, as the doc's `matching` did. The error names the failing rule (`EvalError::rule`). Each rule has its own budget, and shared work is charged to the rule that computes it first.
- Errors are typed per operation (`CompileError`, `EvalError`). The umbrella `Error` enum is gone.
- Constant folding runs the closure compiler on the constant subtree, so operator semantics are not implemented twice. Fold failures leave the node unfolded: `false && 1/0 > 0` must still compile.
- Undecided points, decided here: `m[k]` gives `Option<V>`; array out-of-bounds is E0303 (negative indices count from the end); `?.` on a non-optional value acts like `.`; `let` bindings are evaluated eagerly in order; `**` gives `float` (per my reading of Go's `runtime.Exponent`, not checked against the Go source); `date(s)` without an offset is read as UTC; `duration(s)` uses Go's grammar, implemented in-crate.

### Invariants, validation, and depth

Where each invariant lives, strongest first:

- In types: no poisoned or `Func` types in the IR. No nested options. IR nodes can only be built by the checker. A rule evaluates only with its own facts type (`Rule<T, F>` with `G: Dictum<Static = F>`). Result types (`T: Output`, sealed) are checked at compile time. Field types are checked at Rust compile time (`Field` bound).
- By construction: `StructDesc` and `Object::field` come from one field list in one derive expansion.
- At boundaries: name validity (derive and `Env::new`). A single `ErrorCode` list with templates and a single `Builtin` list, matched exhaustively (no `_ =>`) in the checker and the engine, so a new builtin fails the build until it is both typed and implemented. A single overload table (`resolve`) serves both static typing and `any` dispatch.
- At runtime: budget charged before work is done, a type check on `any` → `T` (deep for containers), `Ice`/`Internal` instead of panics, and clippy `unwrap_used`, `panic`, `indexing_slicing` and `arithmetic_side_effects` as CI errors.

Deliberately not done: method calls, generic or enum facts, dynamic (runtime-described) roots, `HashMap` facts, serialization of compiled rules, warnings, localized message catalogs (codes and args are exposed so others can build them), and a bytecode VM.

Interface depth: users write `#[derive(Dictum)]` and call `Schema::builder` → `func`/`pure_func` → `build`, `Rule::compile` → `eval`/`explain`, and `RuleSet::builder` → `insert` → `build` → `eval`/`explain`, plus `Ctx::new`/`with_now`. That is about 15 methods. Behind them: parsing with recovery, static typing with promotions and optional discipline, resolution of overloads, constant folding, regex and set precompilation, type-specialized closure generation, cross-rule sharing, bounded and non-panicking evaluation, and explain recording. The remaining public names are result and error shapes: `Report`, `ExplainNode`, `NodeStatus`, `Outcomes`, `CompileError`, `EvalError`, `EvalErrorKind`, `Diagnostic`, `ErrorCode`, `Arg`, `Help`, `Span`, `Dynamic`, `DynMap`, `Limits`, `SchemaError`, `Output`, `IntoFunc`. No pipeline stage, IR, `Value` variant, or wire type is public.

### Milestones, with completion criteria

1. Syntax. Every conformance expression parses. `cargo fuzz` (parser) runs 10 CPU-minutes without a panic. Precedence tests cover every row of the binding-power table. Insta snapshots cover diagnostic rendering.
2. Types (was M3). Every conformance case type-checks to its expected type, or fails with its expected E02xx code and span. A debug validator asserts the `ir::Program` invariants on every case. The checker fuzz target runs 10 CPU-minutes without a panic. Minimal derive: trybuild pass tests for every supported field type, and fail tests for enum, generic, reserved name, duplicate name, `HashMap` and `u64`. A property test: for derived structs, `field(i)`'s tag matches `fields()[i].ty()`.
3. Reference evaluation (was M2). The spec interpreter (`engine/interp.rs`) over the typed IR plus the MVP builtins passes the conformance suite. Every intentional deviation from Go is recorded as `deviation = "..."`.
4. Fast evaluation. Folding, closure compiler, budget. The proptest differential suite (`rule.eval(ctx) == rule.explain(ctx).into_result()`, and a `RuleSet` result equal to the standalone `Rule` results: every value on success, the first failing rule's error otherwise) passes 100k cases. Criterion benchmarks against Go are recorded.
5. Rule engine. `Rule`, `RuleSet` with sharing, explain, `Ctx`. A differential test shows each `RuleSet` result equals the standalone `Rule` results. The quickstart example runs.
6. Extensions, each done separately. Each remaining builtin has its `Builtin` entry, typing, implementation, and ported conformance cases. CLI/REPL: `dictum eval --schema s.toml --facts f.json '<expr>'` prints the expected result for the whole conformance corpus. WASM: the conformance runner passes on `wasm32-wasip1` under wasmtime in CI.

## Synthesis decision

Two candidates were sketched in parallel and scored by a separate judge against a six-criterion rubric (usage, interface depth, open issues, invariants in types, hot path, feasibility). This document is candidate 1 with grafts from candidate 2. The judge scored candidate 1 at 27 and candidate 2 at 23, and the orchestrator agreed on the base.

Base: candidate 1 (four crates, typed closure tree, `Env<P>`). It won on invariants in types: no poisoned or function type can reach the IR, IR nodes are built only by the checker, and fact roots can borrow their parts (`Facts<'a> { user: &'a User }`). Candidate 2 required `Object: 'static`, which rules that out.

Grafted from candidate 2:
- Explain is the spec interpreter. This removes the second (recording) closure tree, the `OnceBox`, the `once_cell` dependency and its atomics requirement on thumbv6m, and turns the former test-only reference interpreter into the oracle the differential test reaches through the public API.
- `Output` is sealed, so its hidden methods (which name the internal `Type` and `Value`) cannot be implemented outside the crate.
- Scalar `Field` impls come from one table (`scalar_fields!`), so the `on_unimplemented` note and the impls list the same types.

Defects in candidate 1 fixed during synthesis:
- A pure function call had dependence `Input`, so pure calls of constants never folded. Its own dependence is now `Const`.
- CSE matched `&&`/`||` with swapped operands. Because they short-circuit, `false && 1/0` succeeds while `1/0 && false` fails, so a value cached from one order could hide the other's error. Only `==`, `!=`, `+` and `*` are matched as commutative.
- The function-parameter trait `__private::Arg` clashed with the diagnostic `dictum::Arg`; it is now `FromArg`. The `IntoFunc` doc listed parameter types that have no impl; it now lists only the implemented ones.

Rejected from candidate 2:
- One crate. It hides the IR completely, but loses the crate boundary that keeps IR fields private from the evaluator.
- The hash-consed program DAG with level-scoped memo slots. It shares more (within a rule, across `let`s and chains), but in that candidate impure `let` values were recomputed and impure nodes could be hash-consed across rules. Revisit after M5 if the benchmarks show the extra sharing matters.

Changed after review:
- The clock follows candidate 2 after all: `Ctx::new(&facts)` takes no time, `with_now` sets one, and `now()` without one is `ClockUnset` (E0311). The synthesis had required `now` in `Ctx::new`; the reviewer preferred the smaller constructor and rules that need no time.
- `RuleSet::eval` stops at the first error and returns `Result<Outcomes, EvalError>`, as the doc's `matching` did. The synthesis had returned a result per rule so that one bad rule did not block the others; the reviewer chose the doc's behavior.

## Tradeoffs accepted

- We accept a `F` type parameter on `Schema`, `Rule`, `RuleSet` and `Ctx`, plus the `Facts<'static>` spelling for borrowing roots, in exchange for a wrong facts type being a compile error rather than a runtime check.
- We accept that a forgotten `with_now` is found at evaluation time (`ClockUnset`, E0311), not at Rust compile time, in exchange for `Ctx::new(&facts)` staying small and rules that never call `now()` needing no time. Results stay deterministic because there is still no default clock.
- We accept no generic or enum facts, no `HashMap`, and no `u64` in v0.1, in exchange for `static` descriptors (no lazy init, no `Arc`) and deterministic, overflow-free reads.
- We accept a 128-level AST depth limit, in exchange for bounded stacks in every pass and in evaluation. Rules with 150 chained `+` are rejected.
- We accept linear-time map-literal lookup and binary-search constant sets, in exchange for no hasher in `no_std` and no hash-seeding questions.
- We accept that `explain` runs a tree-walking interpreter, slower than the closure tree and held to the same no-panic and budget bar, in exchange for no second compiler, no once-cell, exact per-node spans, and a differential oracle reachable through the public API.
- We accept that a rule in a set may stay within its budget only because another rule paid for shared work, in exchange for computing shared work once.
- We accept that one failing rule in a set hides the values of every rule, including those that ran before it, in exchange for one `Result` to handle, as in the doc. A caller that needs the others evaluates them as separate `Rule`s.
- We accept `ops.rs` primitives being shared by the spec interpreter, which narrows what the differential test checks to control flow, specialization, caching and budgets, in exchange for having one definition of `checked_add`.
- We accept the `__private` module and its trait names (`Field`, `Object`, `Arg`, ...) as nameable, unavoidable for derive output, and contain them in one doc-hidden module without stability promises.

## Alternatives considered

- Keep `dictum-eval` as its own crate (the doc's five crates). It lost on interface depth. The typed API in `dictum` would need an erased engine API from `dictum-eval` (`Program`, `ProgramSet`, `Checked`, `Input`, `Fault`, `Trace`, the `Value` repr, `FuncImpl`), all public cross-crate items and so importable by anyone. If the typed API moved into `dictum-eval` instead, `dictum` would become a pure re-export layer. Either way it is one more boundary that hides nothing.
- Fold `dictum-check` into `dictum` as well (three crates). This would hide the IR completely, but it loses the one enforcement the crate boundary gives: private `Expr` fields stop the evaluator from building ill-typed IR. It also loses a checker that tooling (LSP, linters) can use without regex or the evaluator. I kept the boundary where it encodes an invariant and removed it where it encoded none.
- Runtime-bound facts (the doc's `Rule<bool>` and `Ctx::new(&facts)`, with a descriptor-identity check at eval). Signatures are smaller and match the doc, but a wrong facts type becomes an `EvalError` on first use in production, and multiple variables still need a binding API (`var`/`var_type` plus a name-keyed binder), which is a second list to keep in sync with the data. It lost on "types over runtime checks".
- Explain via a second, recording closure tree built lazily (the first sketch of this design). Explain then runs the production code path, but it needs a second compiler mode, a once-cell that works without `std`, and a `Frame` field for the trace. Since both evaluators share the `ops` primitives, the interpreter is the smaller design.

## Open questions and risks

- Dynamic roots (decided for M6). The CLI takes a TOML schema that names every type, so rules over JSON facts type-check as derived ones do. `dictum::runtime` (feature `runtime-schema`) builds the descriptors and leaks them, which suits one schema per process; a second, `Arc`-held descriptor kind is left until hot reload of runtime schemas is needed.
- Is `MAX_DEPTH = 128` right for the smallest no_std stacks you target? Should it be a compile option?
- Not verified against the Go source: `**` returning `float`; `?:` being exactly `a ? a : b` with a `bool` `a` (implemented as `a || b`); and how expr-lang treats `{` in predicate position (I chose "always predicate braces", so a map-literal body is written `{{a: #}}`). These should be pinned by conformance cases before M1 ends.
- Newtype fields (`UserId(i64)`). Add `#[dictum(transparent)]`, or let users implement `Field` by delegating (possible because `Value` is opaque)?
- Risk: the bevy-style double bound in `IntoFunc` compiles on 1.85.1 and nightly for the shapes in the example (`|t: Timestamp|`, `|code: &str| -> Result<..>`). Closures without type annotations will not infer, so the docs must say "annotate parameters".

## Next implementation step

M1: implement `Lexer::next` and `Parser::expr_bp` against the binding-power table in `parser.rs`, with the precedence and recovery tests first.

## Verification

Run at the repository root, offline, with no network installs:

- `cargo check --workspace --offline`: ok, 0 warnings.
- `cargo check --workspace --offline --all-targets`: ok, 0 warnings (includes `examples/quickstart.rs`, so the derive expands on real structs).
- `cargo check --workspace --offline --no-default-features`: ok.
- `cargo check --workspace --offline --features dictum/decimal`, with and without `--no-default-features`: ok.
- `cargo +1.85.1 check --workspace --offline --all-targets` (MSRV, fresh target dir): ok.
- `cargo tree -p dictum --no-default-features -e features`: no dependency has its `std` feature enabled.
- A separate probe crate confirmed the six compile-time rejections listed under Usage. That probe ran against candidate 1 before the grafts and was not re-run; the grafts did not touch those code paths except `Field`, whose `HashMap` rejection is unchanged.
- Since then, `just ci` (docs/verification.md) also builds for `thumbv7em-none-eabihf`, runs clippy with `-D warnings`, the tests, the Lean proofs and the TLA+ models.
