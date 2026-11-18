# dictum verification

What "done" means for a change, and which check gives the evidence. Every check is a `just` recipe; CI runs the same recipes (`.github/workflows`).

| when | command | time |
| --- | --- | --- |
| before every commit | `just verify` (formatting, clippy, rustdoc, typos, actionlint, all tests) | ~1 min |
| every PR | `just ci` (adds cargo-deny, MSRV, no_std, bench and fuzz builds, Lean, TLA+) | ~1 min locally, parallel jobs in CI |
| nightly | `.github/workflows/deep.yml`: 10 min of fuzzing per target, property tests with 100k cases | |

Tools come from `nix develop`. Rust comes from rustup (`rust-toolchain.toml`, pinned).

## Layers

| check | what it shows | where |
| --- | --- | --- |
| rustfmt, taplo, typos, actionlint | formatting and obvious slips | `just fmt lint` |
| clippy `-D warnings` | includes `unwrap_used`, `panic`, `indexing_slicing` and `arithmetic_side_effects`, so evaluation code cannot panic by construction | workspace `[lints]` |
| unit tests | primitives (`engine/ops.rs`, `Budget`) and the single lists (`ErrorCode`, `Builtin`, reserved words) | `#[cfg(test)]`, `crates/*/tests` |
| property tests (proptest) | invariants over generated input: `Type::option` normalization, `Span`, Go `%` semantics; `eval == explain` (equivalence); `RuleSet` vs standalone rules (sharing) | `crates/dictum/tests/properties.rs` |
| conformance (e2e) | `tests/conformance/*.toml` through the public API; each case at every reached stage (parse, check, eval) | `crates/dictum/tests/conformance.rs` |
| API e2e | contexts, `ClockUnset`, rule sets, explain reports, limits, threads | `crates/dictum/tests/api.rs` |
| snapshots (insta) | rendered diagnostics | `crates/dictum/tests/diagnostics.rs` |
| compile-fail (trybuild) | what the derive and the type system reject: `HashMap`, `u64`, reserved names, generics, enums, a wrong facts type, a non-`Output` result, the private engine | `crates/dictum/tests/ui` |
| fuzzing (cargo-fuzz) | parser, compiler and eval/explain agreement on arbitrary input and facts | `fuzz/` |
| Lean | proofs over a core fragment (see below) | `formal/lean` |
| TLA+ (TLC) | exhaustive model checking of `RuleSet::eval` sharing and hot reload, each with a seeded mutant that must be caught | `formal/tla` |
| builds | MSRV 1.85, `thumbv7em-none-eabihf` without std, benchmarks, fuzz targets | `just msrv nostd bench-build fuzz-build` |

All tests, fuzz targets and benchmarks use one fixture (`dictum_testkit::Facts`, `sample()`, `sample_now()`), so a rule that fails anywhere can be pasted into a conformance case.

## Invariants and their evidence

| invariant | evidence |
| --- | --- |
| a well-typed rule never hits `Internal` | Lean `soundness`, `never_internal`; fuzz `compile`, `eval_differential` |
| evaluation never panics | clippy no-panic lints; fuzz; proptest `compile_never_panics_*` |
| constant folding changes no outcome, errors included | Lean `fold_sound`; proptest `eval_agrees_with_explain` (explain runs the unfolded IR) |
| CSE may identify `a + b` with `b + a` but not `a && b` with `b && a` | Lean `comm_ok`, `and_not_comm`, `or_not_comm`; test `short_circuit_operands_are_not_shared_across_orders` |
| errors are never cached; a slot is written once | TLA+ `CacheHoldsOnlyValues`, `SlotWrittenOnce` |
| budgets aside, a `RuleSet` gives what its rules give alone | TLA+ `OkMeansEveryRuleMeansAValue`, `ErrorIsFirstMeaningfulError`; proptest `rule_set_agrees_with_standalone_rules` |
| sharing never makes a rule pay more | TLA+ `NeverChargedMore`, `LimitMeansStandaloneLimit` |
| hot reload: readers see only complete sets, one per evaluation, never freed under them | TLA+ `HotReload`: `OnlyCompleteSetsArePublished`, `OneSetPerEvaluation`, `NoUseAfterFree`, `EvaluationsFinish` |
| schema and facts agree | derive generates both from one field list; trybuild |
| a rule runs only with its own facts type and result type | trybuild `wrong_facts_type`, `result_type_not_output` |

## What the formal models cover

The Lean fragment (`formal/lean/Dictum/Core.lean`) has checked 64-bit integers, booleans, field reads, strict binary operators, short-circuit `&&`/`||`, `not` and conditionals. It mirrors the spec interpreter (`engine/interp.rs`) and the primitives (`engine/ops.rs`). Strings, floats, collections, predicates, `let` and budgets are left to the tests.

The TLA+ models treat a rule as the sequence of node evaluations its closure tree performs, with fixed outcomes and costs, and check every configuration up to the sizes in the `.cfg` files.

Update the models in the same change when you change what they describe:

- operator semantics, folding policy or the CSE key: `formal/lean`
- the cache, budgets, `RuleSet::eval` order or error policy: `formal/tla/RuleSetEval.tla`
- how sets are built, published or dropped: `formal/tla/HotReload.tla`

## Milestones and pending checks

Most checks describe behavior that does not exist yet (bodies are `todo!()`). They are written now, as the specification, and wait for their milestone:

- `#[ignore = "pending Mn: ..."]` on a test, and
- `dictum_testkit::MILESTONE` (the last completed milestone) for conformance cases and fuzz targets.

`just pending` lists what is waiting. The change that completes a milestone raises `MILESTONE`, removes the matching `ignore` attributes, and passes them. Ignored means "not reached", never "fails".

## Rules for changing checks

- Do not weaken an assertion, special-case a fixture value, or mark a test ignored to make a check pass. If the check is wrong, fix it and write why next to it. For example, the `RuleSet` property compares against unbudgeted standalone results because TLC found that a rule can reach an error in a set that it never reaches alone.
- Flaky tests: no retries, sleeps or skips. Find the cause. If you cannot, delete the test and write down why.
- If a check still fails after three attempts to fix the code, stop and ask.
- A seeded mutant (`*_mutant.cfg`) must keep failing. If a model change makes a mutant pass, the model has stopped checking that property.
- Changes to checks (`.github/`, `justfile`, `formal/`, `tests/conformance/`, test files, `dictum-testkit`) need code-owner review (`.github/CODEOWNERS`).
- Every conformance case whose expected result differs from Go's says why in `deviation`.
