# dictum verification

What "done" means for a change, and which check gives the evidence. Every check is a `just` recipe; CI runs the same recipes (`.github/workflows`).

`scripts/verify` is the entry point. It runs `just verify`, `just hatches`, `scripts/check-feature-map.sh` (every path and test docs/feature-map.md names exists), and the recipes the changed files need (its `TRIGGERS` table: `drt` for the engine, checker, parser or Lean, `tla` for sharing and budgets, `nostd msrv deny` for library code and manifests, and so on), keeps each log under `target/verify/logs/`, and writes `target/verify/summary.md`: each step's result, the tests that were skipped and why, and what else the changed files need. `scripts/verify full` runs `just ci`; `scripts/verify deep` runs the property and differential tests with 100k cases.

| when | command | time |
| --- | --- | --- |
| before calling a change done | `scripts/verify`, then read `target/verify/summary.md` | 2-5 min |
| before every commit | `just verify` (formatting, clippy, rustdoc, typos, actionlint, all tests) | ~1 min |
| every PR | `just ci` (adds cargo-deny, the conformance cases against Go, the tests on WASM, MSRV, no_std, bench and fuzz builds, Lean, the differential tests against Lean, TLA+) | ~1 min locally, parallel jobs in CI |
| nightly | `.github/workflows/deep.yml`: 10 min of fuzzing per target, property and differential tests with 100k cases | |

Tools come from `nix develop`. Rust comes from rustup (`rust-toolchain.toml`, pinned).

## Layers

| check | what it shows | where |
| --- | --- | --- |
| rustfmt, taplo, typos, actionlint | formatting and obvious slips | `just fmt lint` |
| clippy `-D warnings` | includes `unwrap_used`, `panic`, `indexing_slicing` and `arithmetic_side_effects`, so evaluation code cannot panic by construction | workspace `[lints]` |
| unit tests | primitives (`engine/ops.rs`, `Budget`) and the single lists (`ErrorCode`, `Builtin`, reserved words) | `#[cfg(test)]`, `crates/*/tests` |
| property tests (proptest) | invariants over generated input: `Type::option` normalization, `Span`, Go `%` semantics; `eval == explain` (equivalence); `RuleSet` vs standalone rules (sharing) | `crates/dictum/tests/properties.rs` |
| conformance (e2e) | `tests/conformance/*.toml` through the public API; each case at every reached stage (parse, check, eval), and every builtin called by some case. A case that names a `feature` is ignored in a build without it | `crates/dictum/tests/conformance.rs` |
| conformance against Go | every case without a `deviation` gives Go expr's result, and every `deviation` still differs from Go | `just conformance-go`: `crates/dictum/benches/go/conformance_test.go` |
| optimizer | the rewrites of each rule in `Opt/Simp.lean` and `Opt/Reorder.lean`, and the ranges of `Opt/Range.lean`, on a schema with a `u8` field; seeded bugs (every check skipped, every operand safe, reordering past an operand that can fail) each fail them | `engine/optimize.rs` unit tests, `tests/conformance/optimize.toml` |
| conformance through the CLI | the same cases give the same results with the TOML schema and JSON facts in `tests/conformance/facts/`, and those facts equal the derived sample | `crates/dictum-cli/tests/conformance.rs` |
| tests on WASM | the workspace's tests pass on `wasm32-wasip1` (a 32-bit target) under wasmtime, with all features and with none; those that need a thread or a process (the thread test, trybuild, the differential tests against Lean) are ignored there | `just wasm` |
| API e2e | contexts, `ClockUnset`, rule sets, explain reports, limits, threads | `crates/dictum/tests/api.rs` |
| snapshots (insta) | rendered diagnostics | `crates/dictum/tests/diagnostics.rs` |
| compile-fail (trybuild) | what the derive and the type system reject: `HashMap`, `u64`, reserved names, generics, enums, a wrong facts type, a non-`Output` result, the private engine | `crates/dictum/tests/ui` |
| fuzzing (cargo-fuzz) | parser, compiler and eval/explain agreement on arbitrary input and facts | `fuzz/` |
| Lean | proofs over a core fragment: soundness, and every optimization the engine may apply (see below) | `formal/lean` |
| differential tests against Lean | the Rust code computes what the Lean definitions say: the parser against the reference parser, the checker against `Dictum.infer`, `Rule::eval` against `Dictum.eval` | `just drt`: `crates/dictum-syntax/tests/drt.rs`, `crates/dictum-check/tests/drt.rs`, `crates/dictum/tests/drt_eval.rs` |
| TLA+ (TLC) | exhaustive model checking of `RuleSet::eval` sharing and hot reload, one seeded bug per invariant, and validation of recorded runs | `formal/tla` |
| builds | MSRV 1.85, `thumbv7em-none-eabihf` without std, benchmarks, fuzz targets | `just msrv nostd bench-build fuzz-build` |

All tests, fuzz targets and benchmarks use one fixture (`dictum_testkit::Facts`, `sample()`, `sample_now()`), so a rule that fails anywhere can be pasted into a conformance case.

## Invariants and their evidence

| invariant | evidence |
| --- | --- |
| a well-typed rule never hits `Internal`, and its `int` values fit in `i64` | Lean `soundness`, `never_internal`; fuzz `compile`, `eval_differential` |
| the checker accepts exactly the well-typed rules | Lean `infer_iff`; `drt` (checker) against `Dictum.infer`, on well-typed and ill-typed rules |
| an accepted rule's IR satisfies the `ir::Program` invariants | `Program::validate` on every conformance case, every generated rule (`generated_rules_type_check`) and every fuzz input (`check` target) |
| a derived field reads as a value of its declared type | proptest `derived_fields_are_values_of_their_declared_types` (`facts.rs`) |
| the parser follows the precedence table | `#guard`s in Lean `Dictum.Syntax`; `drt` against the reference parser; `parser.rs` row tests |
| evaluation is the Lean `eval` on the core fragment | `drt_eval` |
| evaluation never panics | clippy no-panic lints; fuzz `eval_differential`; proptest `compile_never_panics_*` |
| `eval` and `explain` charge the budget at the same points, except that a folded constant costs `eval` nothing | the charge points listed in `engine/interp.rs`, which the closure tree follows by calling the same functions; proptest `eval_agrees_with_explain`; test `constant_subtrees_are_folded_at_compile_time` |
| constant folding changes no outcome, errors included | Lean `fold_sound`; proptest `eval_agrees_with_explain` (explain runs the unfolded IR) |
| CSE may identify `a + b` with `b + a` but not `a && b` with `b && a` | Lean `comm_ok`, `and_not_comm`, `or_not_comm`; test `short_circuit_operands_are_not_shared_across_orders` |
| errors are never cached; a slot is written once | TLA+ `CacheHoldsOnlyValues`, `SlotWrittenOnce` |
| budgets aside, a `RuleSet` gives what its rules give alone | TLA+ `OkMeansEveryRuleMeansAValue`, `ErrorIsFirstMeaningfulError`; proptest `rule_set_agrees_with_standalone_rules` |
| sharing never makes a rule pay more | TLA+ `NeverChargedMore`, `LimitMeansStandaloneLimit` |
| the implemented `RuleSet::eval` follows the model | TLA+ trace validation (`RuleSetTrace.tla`) of hand-written traces; the Rust side does not record traces yet (see below) |
| hot reload: readers see only complete sets, one per evaluation, never freed under them | TLA+ `HotReload`: `OnlyCompleteSetsArePublished`, `OneSetPerEvaluation`, `NoUseAfterFree`, `EvaluationsFinish` |
| a new builtin fails the build until it is typed and implemented | no `_ =>` or catch-all binding in a `match` on `Builtin` or `ErrorCode` (`crates/dictum-check/tests/exhaustive.rs`); the parser and the checker agree on predicate builtins (`predicate_builtins_agree_with_the_parser`) |
| modules use only lower layers | `tests/architecture.rs` in `dictum-syntax`, `dictum-check` and `dictum` (`dictum_testkit::layers`, self-test `crates/dictum-testkit/tests/layers.rs`) |
| each crate depends only on the crates docs/architecture.md allows; dev-dependency cycles are listed | `crates/dictum-testkit/tests/crate_dependencies.rs` |
| every theorem the optimizations table and `engine/optimize.rs` cite exists in `formal/lean` | `crates/dictum/tests/formal_names.rs` |
| a change adds no TODO or workaround comment, unexplained `#[ignore]`, constant-ignored trial, fixed proptest case count or saturated signed value | `just hatches` (`scripts/check-escape-hatches.sh`, added lines only) |
| schema and facts agree | derive generates both from one field list; trybuild |
| a rule runs only with its own facts type and result type | trybuild `wrong_facts_type`, `result_type_not_output` |

## What the formal models cover

The Lean fragment (`formal/lean/Dictum/Core.lean`) has checked 64-bit integers, booleans, field reads, strict binary operators, short-circuit `&&`/`||`, `not` and conditionals. It mirrors the spec interpreter (`engine/interp.rs`) and the primitives (`engine/ops.rs`). Strings, floats, collections, predicates, `let` and budgets are left to the tests.

### Optimizations, and the theorem that allows each

The engine may only apply an optimization that has a theorem here, under the same side conditions. Constant folding is `engine/fold.rs`; the interval analysis, the simplifications and the reordering are `engine/optimize.rs`, which names the Lean definition each function follows. They touch `int` and `bool` only, decide `<` only, and treat a field read at any depth as the fragment's `var`. Each theorem says the optimized rule gives the same result as the original on every input, errors included. A new optimization is proved first, then implemented, and the differential tests compare the two.

| optimization | condition | theorem |
| --- | --- | --- |
| constant folding | the subtree reads no facts and evaluates without error | `fold_sound` (`Fold.lean`) |
| skip an overflow check (`wrapping_add` instead of `checked_add`) | the interval analysis bounds the result inside `i64`; field ranges come from the Rust field type (`u8`, `i32`, ...) | `evalFast_eq` (`Opt/Range.lean`) |
| `x + 0`, `x - 0`, `x * 1`, `1 * x` → `x`; `not not x` → `x`; `true && x` → `x`, `false && x` → `false`, and the `\|\|` duals; `if true`/`if false` | the rule type-checks | `simp_sound` (`Opt/Simp.lean`) |
| `x * 0` → `0`; `x && false` → `false`; `x == x` → `true`; `if c { t } else { t }` → `t`; `a < b` decided by the ranges | additionally, `x` (or `c`, `a`, `b`) is `safe`: it cannot fail | `simp_sound`, `safe_ok` |
| evaluate cheap `&&`/`\|\|` operands first | only within a run of consecutive `safe` operands | `and_assoc`, `and_swap_in_chain`, `or_swap_in_chain` (`Opt/Reorder.lean`) |
| `x in [constants]` by binary search | the constants are sorted | `bsearch_iff` (`Opt/Member.lean`) |
| `x in lo..hi` as `lo <= x && x <= hi` | none | `mem_rangeList` |
| share `a + b` with `b + a` across rules (CSE) | `+`, `*`, `==` only, never `&&`/`\|\|`; values only, never errors | `comm_ok`, `and_not_comm`, `comm_error_differs` (`Commute.lean`) |

The examples at the end of each file show what the analyses decide, for instance that `age + 1` with `age: u8` needs no overflow check and that `7 % 0 * 0` is not simplified to `0`.

### Connecting the proofs to the code

The theorems are about Lean definitions. `dictum-oracle` (`formal/lean/Oracle.lean`, built by `just lean`) runs those definitions, and the differential tests send it the same generated inputs as the Rust code:

- `drt`: generated sources in the operator language, and token soup. Both parsers must accept the same sources and build the same tree. The reference parser (`Dictum/Syntax.lean`) follows Go expr-lang's `parseExpression`, not the Pratt parser, so both share only the table. It is checked by examples and by this agreement, not proved.
- `drt` (checker): generated rules of the core fragment, well typed or not. The checker must accept the same rules as `Dictum.infer`, with the same type. `infer_iff` proves `infer` accepts exactly what `HasType` types, so the soundness theorem covers every rule the checker accepts in this fragment.
- `drt_eval`: generated well-typed rules of the core fragment, evaluated by `Rule::eval` on the fixture and by `Dictum.eval` on the same field values.

`Rule::eval` runs the closure tree, so `drt_eval` compares it with `Dictum.eval`, and `eval_agrees_with_explain` ties it to the spec interpreter, which `explain` runs.

Each test also has a trial that checks that the generator is useful: most sources parse, the rules are well typed, and they produce values, overflows and divisions by zero.

### TLA+

The TLA+ models treat a rule as the sequence of node evaluations its closure tree performs, with fixed outcomes and costs, and check every configuration up to the sizes in the `.cfg` files.

Each model has a `Bug` constant. Each `<Spec>_<bug>_mutant.cfg` turns one seeded bug on and names, on its `\* expect:` line, the invariant that must catch it. `check.sh` fails if TLC reports no violation or a different invariant. Every invariant has a mutant: `budget_per_set`, `cache_errors` and `ignore_cache` for `RuleSetEval`; `publish_early`, `forget_readers` and `reload_per_rule` for `HotReload`.

`RuleSetTrace.tla` checks one recorded run against `RuleSetEval`: the configuration, the events (hit, compute, fail, next rule) and the returned result. An event the model does not allow is a deadlock. A wrong result violates `SameResult`. `Trace_accept_*` must pass and `Trace_reject_*` must fail. The Rust side does not record traces yet. The model's rule is a flat sequence of nodes, but a shared node can contain another shared node, and when the outer one is a cache hit the inner one is not visited. Recording real runs needs a model in which a hit skips the nodes inside it.

Update the models in the same change when you change what they describe:

- operator semantics, folding policy, an optimization or the CSE key: `formal/lean`, and the oracle if the fragment grows
- the precedence table: `formal/lean/Dictum/Syntax.lean` and `dictum_testkit::syntax`
- the cache, budgets, `RuleSet::eval` order or error policy: `formal/tla/RuleSetEval.tla` and `RuleSetTrace.tla`
- how sets are built, published or dropped: `formal/tla/HotReload.tla`

## Milestones

Every milestone (M1-M6) is done: `dictum_testkit::MILESTONE` is 6, and no test waits on it. Only `just fuzz` still reads it, to skip a fuzz target whose `# M<n>` comment in `fuzz/Cargo.toml` names a later one. A test is ignored only when it cannot run where it is (the Lean oracle is not built, or WASI has no threads or processes); `target/verify/summary.md` says which.

## Rules for changing checks

- Do not weaken an assertion, special-case a fixture value, or mark a test ignored to make a check pass. If the check is wrong, fix it and write why next to it. For example, the `RuleSet` property compares against unbudgeted standalone results because TLC found that a rule can reach an error in a set that it never reaches alone.
- Flaky tests: no retries, sleeps or skips. Find the cause. If you cannot, delete the test and write down why.
- If a check still fails after three attempts to fix the code, stop and ask.
- A seeded mutant (`*_mutant.cfg`) must keep failing, by the invariant it names. If a model change makes a mutant pass, the model has stopped checking that property.
- A differential test disagreement is a bug in the Rust code or in the Lean definition. Find out which before changing either, and say which in the commit.
- Changes to checks (`.github/`, `justfile`, `formal/`, `tests/conformance/`, test files, `dictum-testkit`) need code-owner review (`.github/CODEOWNERS`).
- Every conformance case whose expected result differs from Go's says why in `deviation`.
