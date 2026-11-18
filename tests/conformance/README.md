# Conformance cases

End-to-end cases run through the public API by `crates/dictum/tests/conformance.rs`,
against `dictum_testkit::sample()` at `sample_now()` (2026-01-01T00:00:00Z). The case
format is documented in `crates/dictum-testkit/src/conformance.rs`.

Cases ported from [expr-lang/expr](https://github.com/expr-lang/expr) keep Go's
result; where dictum differs on purpose, the case says why in `deviation`.
