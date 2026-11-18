---
name: verify
description: Verify a change to dictum before committing. Runs the checks in docs/verification.md that the diff requires and reports the evidence. Use before every commit and before saying a change is done.
---

# Verify a dictum change

1. Run the base gate and read its output:

   ```bash
   nix develop -c just verify
   ```

2. Add the checks the diff triggers:

   | the diff touches | also run |
   | --- | --- |
   | `engine/ops.rs`, `engine/fold.rs`, `engine/analysis.rs`, `engine/interp.rs`, typing rules | `just lean`, and update `formal/lean` if the semantics changed |
   | `engine/frame.rs`, `engine/set.rs`, `rule_set.rs`, cache or budget code | `just tla`, and update `formal/tla/RuleSetEval.tla` if the behavior changed |
   | hot reload, `Arc` ownership of `Rule`/`RuleSet` | `just tla` (`HotReload.tla`) |
   | anything compiled for `no_std`, `Cargo.toml` features or dependencies | `just nostd msrv deny` |
   | `fuzz/`, `benches/`, `dictum-testkit` | `just fuzz-build bench-build` |
   | a parser, checker or engine body that was `todo!()` | `just pending`; if a milestone is complete, raise `MILESTONE` and remove its `ignore`s |
   | `.github/workflows` | `actionlint` (part of `just lint`) |

   When unsure, run `nix develop -c just ci`.

3. If something fails, fix the code, not the check (docs/verification.md, "Rules for changing checks"). After three failed attempts, stop and report.

4. Report what ran and what passed, which checks were skipped and why, and the `just pending` counts if they changed.
