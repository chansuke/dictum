# Feature map

Each row is a feature as a user names it, where its code is, and what checks it. Use these names in issues, commits and pull requests. `scripts/check-feature-map.sh`, which `scripts/verify` runs, fails when a path or a test named here no longer exists.

| Feature | How a user reaches it | Code | Checked by |
|---|---|---|---|
| Rules | `Rule::compile`, `eval` | `crates/dictum/src/rule.rs`, `crates/dictum/src/engine/compile.rs`, `crates/dictum/src/engine/fold.rs` | `tests/conformance/`, `eval_agrees_with_explain`, `crates/dictum/tests/drt_eval.rs` |
| Syntax | the rule text | `crates/dictum-syntax/src/lexer.rs`, `crates/dictum-syntax/src/parser.rs`, `formal/lean/Dictum/Syntax.lean` | `crates/dictum-syntax/tests/parser.rs`, `compile_never_panics_on_token_soup`, `crates/dictum-syntax/tests/drt.rs` |
| Types and diagnostics | compile errors with codes and spans | `crates/dictum-check/src/checker/mod.rs`, `crates/dictum-check/src/resolve.rs`, `crates/dictum-syntax/src/diag.rs` | `crates/dictum/tests/diagnostics.rs`, `compile_errors_are_collected_and_capped`, `crates/dictum-check/tests/drt.rs` |
| Builtins | `len(xs)`, `all(xs, # > 0)`, `date(s)`, and the rest | `crates/dictum-syntax/src/builtin.rs`, `crates/dictum/src/engine/builtins.rs`, `crates/dictum/src/engine/layout.rs`, `crates/dictum/src/engine/sprint.rs` | `builtins_are_unique_callable_names`, `crates/dictum/benches/go/` (`just conformance-go`) |
| Facts | `#[derive(Dictum)]` on a struct | `crates/dictum-derive/src/lib.rs`, `crates/dictum/src/facts.rs`, `crates/dictum/src/engine/value.rs` | `derive_and_api_compile_time_checks`, `crates/dictum/tests/ui/` |
| Schema functions | `Schema::builder().func(..)`, `pure_func(..)` | `crates/dictum/src/schema.rs`, `crates/dictum/src/func.rs` | `schema_functions_pure_and_volatile`, `only_pure_functions_are_called_at_compile_time` |
| Explain | `rule.explain(ctx)` | `crates/dictum/src/explain.rs`, `crates/dictum/src/engine/interp.rs`, `crates/dictum/src/engine/trace.rs` | `explain_marks_skipped_and_failed_nodes`, `explain_elides_large_values_without_changing_the_result`, `eval_agrees_with_explain` |
| Rule sets | `RuleSet::builder`, `insert`, `build`, `eval` | `crates/dictum/src/rule_set.rs`, `crates/dictum/src/engine/set.rs`, `formal/tla/` | `rule_set_agrees_with_standalone_rules`, `a_rule_set_computes_a_shared_subexpression_once_per_eval`, `just tla` |
| Limits | `Limits` on `Ctx` | `crates/dictum/src/ctx.rs`, `crates/dictum/src/engine/frame.rs` | `step_limit_is_enforced`, `json_conversions_are_charged_to_the_budget` |
| Optimizations | automatic at compile time | `crates/dictum/src/engine/optimize.rs`, `crates/dictum/src/engine/analysis.rs`, `formal/lean/Dictum/Opt/` | `rewrites_follow_simp`, `a_rewrite_that_could_hide_an_error_is_not_made`, `every_cited_lean_name_is_declared`, `tests/conformance/optimize.toml` |
| Runtime schemas | feature `runtime-schema`, `dictum::runtime` | `crates/dictum/src/runtime.rs` | `a_recursive_described_struct_reads_and_type_checks`, `facts_that_do_not_match_say_where` |
| CLI | `dictum eval`, `dictum repl`, `dictum schema infer` | `crates/dictum-cli/src/main.rs`, `crates/dictum-cli/src/lib.rs` | `eval_prints_the_value_as_a_rule_literal`, `repl_evaluates_each_line_until_quit`, `crates/dictum-cli/tests/conformance.rs` |
| no_std | `default-features = false` | `crates/dictum/Cargo.toml` | `just nostd` |
| WASM | `wasm32-wasip1` | `justfile` (`wasm`) | `just wasm` |
