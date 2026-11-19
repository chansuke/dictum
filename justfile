# Every check, by name. CI runs these same recipes (.github/workflows), so a
# green `just verify` locally predicts a green PR. Tools come from `nix develop`.
#
#   just verify   before every commit: formatting, lints, tests  (~1 min)
#   just ci       everything CI runs, including formal models and builds

set shell := ["bash", "-euo", "pipefail", "-c"]

published := "-p dictum -p dictum-syntax -p dictum-check -p dictum-derive"

# List recipes.
default:
    @just --list

# Before every commit.
verify: fmt lint test

# Everything CI runs.
ci: fmt lint deny test msrv nostd bench-build fuzz-build lean tla drt

# rustfmt and taplo, check only.
fmt:
    cargo fmt --all --check
    taplo fmt --check

# Apply formatting.
fmt-fix:
    cargo fmt --all
    taplo fmt

# clippy for each feature set, rustdoc, typos, workflow files.
lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo clippy --workspace --all-targets --no-default-features -- -D warnings
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
    typos
    actionlint

# Licenses, advisories, sources.
deny:
    cargo deny check

# Tests waiting for a milestone are reported as ignored; `just pending` lists them.
# Unit, property, conformance (e2e), API e2e, trybuild and doc tests.
test:
    cargo nextest run --workspace --all-features --no-fail-fast
    cargo nextest run --workspace --no-default-features --no-fail-fast
    cargo test --workspace --all-features --doc
    cargo run -p dictum --example quickstart

# What is still ignored, by milestone.
pending:
    @echo "MILESTONE = $(just milestone)"
    @cargo nextest list --workspace --all-features --run-ignored only 2>/dev/null | wc -l | xargs echo "ignored tests:"
    @grep -rhoE 'ignore = "pending M[0-9]+[^"]*"' crates | sort | uniq -c || echo "no pending milestones"

# The last completed milestone (dictum_testkit::MILESTONE).
milestone:
    @grep -oE 'MILESTONE: u8 = [0-9]+' crates/dictum-testkit/src/lib.rs | grep -oE '[0-9]+$'

# The published crates build on the MSRV.
msrv:
    cargo +1.85.1 check {{published}} --all-features --target-dir target/msrv
    cargo +1.85.1 check -p dictum --no-default-features --target-dir target/msrv

# dictum builds without std for a bare-metal target.
nostd:
    cargo build -p dictum --no-default-features --target thumbv7em-none-eabihf
    cargo build -p dictum --no-default-features --features decimal --target thumbv7em-none-eabihf

# Benchmarks compile.
bench-build:
    cargo bench -p dictum --no-run

# dictum and Go's expr on the same rules and facts (docs/benchmarks.md).
bench:
    cargo bench -p dictum --bench eval
    cd crates/dictum/benches/go && go test -run TestRules -bench . -benchmem

# The conformance cases against Go's expr (tests/conformance/README.md).
conformance-go:
    cd crates/dictum/benches/go && go test -run TestConformance

# The host triple. cargo-fuzz otherwise builds for the triple it was itself built
# for, which is musl for the prebuilt binary CI installs, and ASan cannot link that.
host := `rustc -vV | sed -n 's/^host: //p'`

# Fuzz targets compile (nightly + cargo-fuzz).
fuzz-build:
    cargo +nightly fuzz build --target {{host}}

# Fuzz one target for `seconds`, if its milestone is reached.
fuzz target seconds="60":
    #!/usr/bin/env bash
    set -euo pipefail
    need=$(grep -B3 '^name = "{{target}}"' fuzz/Cargo.toml | grep -oE '^# M[0-9]+' | grep -oE '[0-9]+')
    have=$(just milestone)
    if (( have < need )); then echo "{{target}}: pending M$need (MILESTONE = $have)"; exit 0; fi
    cargo +nightly fuzz run --target {{host}} {{target}} -- -max_total_time={{seconds}} -rss_limit_mb=2048

# Lean proofs and the dictum-oracle executable (a `sorry` or any warning fails the build).
lean:
    cd formal/lean && lake build

oracle := justfile_directory() / "formal/lean/.lake/build/bin/dictum-oracle"

# Differential tests against the Lean definitions: the parser against the reference
# parser, the checker against `Dictum.infer`, evaluation against `Dictum.eval`. Tests
# whose milestone is not reached are
# ignored; with DICTUM_ORACLE set, a missing oracle fails instead of skipping.
drt: lean
    DICTUM_ORACLE={{oracle}} cargo test -p dictum-syntax --test drt
    DICTUM_ORACLE={{oracle}} cargo test -p dictum-check --test drt
    DICTUM_ORACLE={{oracle}} cargo test -p dictum --test drt_eval

# TLA+ models, the seeded mutants that must be caught, and trace validation.
tla:
    formal/tla/check.sh
