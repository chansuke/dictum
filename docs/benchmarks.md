# dictum benchmarks

dictum and Go's [expr](https://github.com/expr-lang/expr) on the same rules and the same facts (`dictum_testkit::sample()`). The rules are `RULES` in `crates/dictum/benches/eval.rs`; `crates/dictum/benches/go/bench_test.go` copies them and checks that both sides give the same result. Run both with `just bench`.

Each dictum iteration is one `Rule::eval`, which builds a new `Frame`. Each Go iteration is one `vm.Run` on a reused `vm.VM`. Compile is the `arithmetic` rule, from source to a runnable rule.

## M4 (closure tree, constant folding)

2026-11-19, AMD Ryzen Threadripper PRO 5945WX, Linux x86_64. Rust 1.99.0 (criterion, median), Go 1.27.3 with expr v1.17.8 (`go test -bench`).

| rule | dictum | Go expr | Go / dictum |
| --- | --- | --- | --- |
| compare | 88.5 ns | 184.9 ns | 2.1 |
| membership | 59.6 ns | 247.7 ns | 4.2 |
| predicate | 796.9 ns | 2141 ns | 2.7 |
| strings | 301.7 ns | 561.3 ns | 1.9 |
| arithmetic | 374.9 ns | 1058 ns | 2.8 |
| compile | 5.59 µs | 45.9 µs | 8.2 |

This table was recorded at M4, before the interval-analysis, algebraic-simplification and `&&`/`||` reordering optimizations (`crates/dictum/src/engine/optimize.rs`) landed, so these numbers do not include them.

## M5 (RuleSet)

Same machine and toolchains, 2026-11-19. `rule_set` evaluates the five rules above as one `RuleSet`. Go's expr has no rule set, so the comparison is the five Go rules run one after another (the sum of the M4 Go numbers).

| benchmark | dictum | Go expr |
| --- | --- | --- |
| rule_set | 1.64 µs | 4.19 µs (sum) |
| the five single rules, summed | 1.62 µs | 4.19 µs |

These rules have no subexpression in common, so the set shares nothing and costs about what the single rules cost together.

## Current (M6 + optimizations and safety checks)

2026-11-26, same machine and toolchains (AMD Ryzen Threadripper PRO 5945WX, Linux x86_64, Rust 1.99.0, Go 1.27.3 with expr v1.17.8). Includes `engine/optimize.rs` (interval analysis, algebraic simplification, safe operand reordering, post-optimization refolding), runtime-schema root checking, incremental allocation pre-charging, stack-array `Call`, and bottom-up CSE interning.

| rule | dictum | Go expr | Go / dictum |
| --- | --- | --- | --- |
| compare | 91.2 ns | 219.0 ns | 2.4 |
| membership | 61.1 ns | 244.9 ns | 4.0 |
| predicate | 883.5 ns | 1784 ns | 2.0 |
| strings | 283.7 ns | 388.2 ns | 1.4 |
| arithmetic | 386.7 ns | 611.9 ns | 1.6 |
| compile | 7.52 µs | 25.8 µs | 3.4 |

| benchmark | dictum | Go expr |
| --- | --- | --- |
| rule_set | 1.71 µs | 3.25 µs (sum) |
| the five single rules, summed | 1.71 µs | 3.25 µs |

Compared with M4/M5: `strings` improved (~6% faster) from removing per-character allocations in `upper` and tightening `join`; `compare` and `membership` added ~1.5–3 ns from the runtime-schema descriptor check and budget accounting; `compile` rose from 5.59 µs to 7.52 µs because it now runs `optimize::optimize` and dependence analysis. None of these five rules uses the narrow integer fields (`level: u8`, `delta: i8`) or shares subexpressions across rules, so overflow-check elision and CSE sharing are not exercised here.
