#!/usr/bin/env bash
# Model-check every TLA+ model here with TLC.
#
#   <Spec>.cfg          must pass: "No error has been found".
#   <Spec>_<bug>_mutant.cfg
#                       seeded bug: TLC must report a violation of the invariant
#                       named on the cfg's `\* expect:` line. A mutant that passes,
#                       or is caught by another invariant, means the model no
#                       longer checks what it claims to.
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
    if [[ "$cfg" == *_mutant.cfg ]]; then
        expect="$(sed -n 's/^\\\* expect: *//p' "$cfg")"
        if [[ -z "$expect" ]]; then
            echo "FAIL  $cfg: no \`\\* expect: <Invariant>\` line"; status=1
        elif grep -q "Invariant $expect is violated" <<< "$out"; then
            echo "ok    $cfg (caught by $expect)"
        else
            echo "FAIL  $cfg: not caught by $expect"; echo "$out" | tail -20; status=1
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
