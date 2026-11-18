#!/usr/bin/env bash
# Model-check every TLA+ model here with TLC.
#
#   <Spec>.cfg          must pass: "No error has been found".
#   <Spec>_<x>_mutant.cfg / <Spec>_mutant.cfg
#                       seeded bug: must be caught ("is violated"). A mutant that
#                       passes means the model no longer checks what it claims to.
#
# TLC is `tlc` (nix devShell) unless $TLC says otherwise, e.g.
#   TLC="java -XX:+UseParallelGC -cp tla2tools.jar tlc2.TLC" ./check.sh
set -euo pipefail
cd "$(dirname "$0")"
read -r -a tlc <<< "${TLC:-tlc}"
status=0
for cfg in *.cfg; do
    spec="${cfg%.cfg}"
    spec="${spec%%_*}"
    out="$("${tlc[@]}" -workers auto -cleanup -config "$cfg" "$spec.tla" 2>&1 || true)"
    if [[ "$cfg" == *mutant.cfg ]]; then
        if grep -q "is violated" <<< "$out"; then
            echo "ok    $cfg (seeded bug caught)"
        else
            echo "FAIL  $cfg: the seeded bug was not caught"; echo "$out" | tail -20; status=1
        fi
    else
        if grep -q "No error has been found" <<< "$out"; then
            echo "ok    $cfg ($(grep -oE '[0-9,]+ distinct states found' <<< "$out" | tail -1))"
        else
            echo "FAIL  $cfg"; echo "$out" | tail -40; status=1
        fi
    fi
done
exit "$status"
