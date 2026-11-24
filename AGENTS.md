# Working on dictum

Read [docs/architecture.md](docs/architecture.md) for the crates, what each
may depend on, and the module layers. Read
[docs/feature-map.md](docs/feature-map.md) for the names of features and
where each one lives; use those names in commits and pull requests.
[docs/verification.md](docs/verification.md) says which check is the
evidence for each invariant.

The checks hold the rules: `[workspace.lints]` and `clippy.toml` (no
panics, no `#[allow]`, an `expect` with a reason), `tests/architecture.rs`
in each crate (module layers), `crates/dictum-testkit/tests/crate_dependencies.rs`,
`crates/dictum-check/tests/exhaustive.rs` (no catch-all arm on `Builtin`),
`crates/dictum/tests/formal_names.rs`, and `scripts/check-escape-hatches.sh`
on added lines. When one fails, its message names the fix. Change the
design to pass it; do not silence the check.

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
