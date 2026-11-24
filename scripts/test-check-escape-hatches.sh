#!/usr/bin/env bash
# Tests check-escape-hatches.sh against a throwaway repository.
set -euo pipefail

check=$(cd "$(dirname "$0")" && pwd)/check-escape-hatches.sh
repo=$(mktemp -d)
trap 'rm -rf "$repo"' EXIT
cd "$repo"
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
git init -q -b main
git -c user.name=t -c user.email=t@example.com commit -q --allow-empty -m base
failures=0
expect() { # WANT(pass|fail) DESCRIPTION FILE CONTENT
  local want=$1 what=$2 status=0
  mkdir -p "$(dirname "$3")"
  printf '%s\n' "$4" >"$3"
  git add "$3"
  "$check" --staged >/dev/null 2>&1 || status=$?
  if { [ "$want" = pass ] && [ "$status" -ne 0 ]; } || { [ "$want" = fail ] && [ "$status" -ne 1 ]; }; then
    echo "FAIL: $what (exit $status)"
    failures=1
  fi
  git rm -q --cached "$3"
  rm -f "$3"
}

expect pass "plain code" crates/a/src/a.rs 'fn main() {}'
expect fail "a TODO comment" crates/a/src/a.rs 'fn main() {} // TODO: handle errors'
expect fail "a FIXME in Lean" formal/lean/A.lean 'theorem t : True := trivial -- FIXME'
expect fail "a workaround comment" crates/a/src/a.rs '// Workaround for the flaky test'
expect fail "for now in a justfile" justfile 'x: # for now'
expect pass "a word in a string, not a comment" crates/a/src/a.rs 'let s = "TODO list";'
expect pass "a flag in a string" crates/a/src/a.rs 'let s = "--for now";'
expect pass "an attribute is not a comment" crates/a/src/a.rs '#[cfg(test)]'
expect fail "an ignored test without a reason" crates/a/tests/a.rs '#[ignore]'
expect pass "an ignored test with a reason" crates/a/tests/a.rs '#[ignore = "wasm32-wasip1 has no threads"]'
expect fail "a trial ignored by a constant" crates/a/tests/drt.rs 'Trial::test("x", f).with_ignored_flag(true)'
expect pass "a trial ignored on a condition" crates/a/tests/drt.rs 'Trial::test("x", f).with_ignored_flag(oracle.is_err())'
expect fail "a fixed case count" crates/a/tests/p.rs 'TestRunner::new(Config { cases: 16, ..Config::default() })'
expect fail "a fixed case count in a proptest config" crates/a/tests/p.rs '#![proptest_config(ProptestConfig::with_cases(10))]'
expect pass "the case count from testkit" crates/a/tests/p.rs 'TestRunner::new(Config { cases: oracle::cases(), ..Config::default() })'
expect fail "a saturated value in library code" crates/a/src/a.rs 'let n = i64::try_from(x).unwrap_or(i64::MAX);'
expect pass "a saturated value in a test" crates/a/tests/a.rs 'let n = i64::try_from(x).unwrap_or(i64::MAX);'
expect pass "a saturated budget charge" crates/a/src/a.rs 'f.budget.step(u64::try_from(n).unwrap_or(u64::MAX))'
expect fail "a shellcheck directive without a reason" scripts/run.sh '# shellcheck disable=SC2086'
expect pass "a shellcheck directive with a reason" scripts/run.sh '# shellcheck disable=SC2086 # word splitting is wanted'
expect pass "prose in docs" docs/usage.md 'For now, only Linux is supported. TODO list'

# A range reports the line number in the new file.
printf 'a\nb\n' >f.rs
git add f.rs
git -c user.name=t -c user.email=t@example.com commit -q -m two
printf 'a\nb\n// TODO c\n' >f.rs
git add f.rs
git -c user.name=t -c user.email=t@example.com commit -q -m three
reported=$("$check" HEAD~1 HEAD 2>&1 || true)
if ! grep -q '^f.rs:3: // TODO c$' <<<"$reported"; then
  echo "FAIL: the line number of an added line"
  failures=1
fi

if [ "$failures" -ne 0 ]; then
  exit 1
fi
echo "check-escape-hatches.sh: all cases pass"
