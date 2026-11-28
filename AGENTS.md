# Working on dictum

Read [docs/architecture.md](docs/architecture.md) for the crates, what each
may depend on, and the module layers. Read
[docs/feature-map.md](docs/feature-map.md) for the names of features and
where each one lives; use those names in commits and pull requests.
[docs/verification.md](docs/verification.md) says which check is the
evidence for each invariant.

The checks hold the rules. When one fails, its message names the fix.
Change the design to pass it; do not silence the check.

| rule | enforced by |
| --- | --- |
| no panics, no `#[allow]`, an `expect` with a reason | `[workspace.lints]`, `clippy.toml` |
| a module uses only lower layers | `tests/architecture.rs` in each crate |
| a crate depends only on the crates docs/architecture.md allows | `crates/dictum-testkit/tests/crate_dependencies.rs` |
| a match on `Builtin` or `ErrorCode` lists every variant | `crates/dictum-check/tests/exhaustive.rs` |
| every Lean name the optimizations cite exists | `crates/dictum/tests/formal_names.rs` |
| no TODO, workaround, unexplained `#[ignore]` or fixed case count on added lines | `scripts/check-escape-hatches.sh` |
| a computed string is charged to `max_alloc` before it grows: build it with `engine::frame::Text` | `crates/dictum/tests/charged_strings.rs`; `Value::string` and `built` no longer exist |
| a recursive walk over a value takes a `Depth` | `crates/dictum/tests/bounded_walks.rs`, `a_host_dynamic_nested_past_the_walk_limit_is_a_depth_error` |
| the checks that run locally are the ones CI runs | CI runs `scripts/verify` |
| a change to a rule's meaning or typing changes `formal/lean`; a change to sharing, caching or budgets changes `formal/tla` | nothing: judgment, below. `scripts/verify` runs `drt` and `tla` for the files that need them |

## Before you call a change done

- Run `scripts/verify`, then read `target/verify/summary.md`, including
  what the changed files also need and which tests were ignored.
- If you change what a rule means or how it is typed, change `formal/lean`
  in the same change. A new optimization is proved there first.
- If you change sharing, caching or budgets, change `formal/tla`.
- Before a release or a change to manifests or CI, run `scripts/verify full`.

## When a bug is fixed or a review finds something

First ask whether a lint, a type, a test, a layer or dependency check, or a
`docs/verification.md` item can stop it from coming back, and add that.
Write a rule here only when none of them can express it.
