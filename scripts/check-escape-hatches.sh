#!/usr/bin/env bash
# Rejects added lines that put off a fix or switch a check off: TODO-style
# markers and "workaround" comments, `#[ignore]` without a reason, a
# differential test ignored by a constant, a fixed or lowered proptest case
# count, a signed integer saturated in library code, and shellcheck directives
# without a reason. Only the lines a change adds are read, so existing code
# never fails it. Lint exceptions are clippy's job (`allow_attributes`).
#
# usage: check-escape-hatches.sh --staged     (the index against HEAD)
#        check-escape-hatches.sh BASE [HEAD]  (BASE to HEAD, or to the working tree)
set -euo pipefail

case "${1:-}" in
--staged) diff=(git diff --cached --no-color --no-ext-diff -U0 --) ;;
"")
  echo "usage: $0 --staged | BASE [HEAD]" >&2
  exit 2
  ;;
*) diff=(git diff --no-color --no-ext-diff -U0 "$@" --) ;;
esac

# Each rule: comment or code, a file pattern (a shell glob in which `*` also
# matches `/`), an extended regular expression for the added text, and what
# to do instead.
rules=(
  $'comment\t*\t(^|[^A-Za-z])(TODO|FIXME|HACK|XXX)([^A-Za-z]|$)\tFix it in this change. If it is out of scope, open an issue and describe what the code does now as a fact; better, add a test that fails until it is fixed.'
  $'comment\t*\t[Ww]ork-?around|[Ff]or now|[Qq]uick fix|[Bb]and-?aid|[Tt]emporary (fix|hack|patch)|[Uu]ntil .* (is|gets|are) fixed\tDo not explain away a workaround. Find the cause and fix it; if the cause is outside this repository, name the upstream behavior and link its issue.'
  $'code\t*.rs\t#\\[ignore\\]\tGive the reason: #[ignore = "why it does not run by default"]. A test that fails is fixed, not ignored.'
  $'code\t*.rs\twith_ignored_flag\\([[:space:]]*true\tIgnore a trial only on a condition: the oracle is missing (Oracle::locate) or its milestone is not reached. A failing comparison is a bug in the Rust code or in formal/lean.'
  $'code\t*.rs\t(cases:[[:space:]]*[0-9]|with_cases\\([[:space:]]*[0-9])\tTake the case count from dictum_testkit::oracle::cases() or proptest\'s PROPTEST_CASES default; a fixed count stops deep.yml from raising it, and a lower one hides failures.'
  $'code\tcrates/*/src/*.rs\tunwrap_or\\(i(8|16|32|64|128|size)::(MAX|MIN)\\)\tA rule value saturated at the bounds is a wrong result, not an overflow error. Use checked arithmetic and return EvalErrorKind::Overflow.'
  $'code\t*.sh\t#[[:space:]]*shellcheck[[:space:]]+disable=[A-Z0-9,]+[[:space:]]*$\tSay why after the directive: `# shellcheck disable=SC2016 # the backticks are literal`.'
  $'code\t*justfile\t#[[:space:]]*shellcheck[[:space:]]+disable=[A-Z0-9,]+[[:space:]]*$\tSay why after the directive.'
)

# The comment part of LINE in FILE, empty when it has none. A `#` that starts
# a Rust attribute such as `#[cfg]` is code.
comment_of() { # FILE LINE
  case $1 in
  *.rs | *.go) [[ $2 == *//* ]] && printf '%s' "//${2#*//}" ;;
  *.lean) [[ $2 == *--* ]] && printf '%s' "--${2#*--}" ;;
  *.sh | *.toml | *.yaml | *.yml | *.nix | justfile | *.tla | *.cfg)
    [[ $2 =~ (^|[[:space:]])#([^[!]|$) ]] && printf '%s' "#${2#*#}"
    ;;
  esac
  return 0
}

# Prose in docs may say "for now"; the scripts name the patterns they look for.
checked() { # FILE
  case $1 in
  docs/* | *.md | *.lock | *.snap | tests/conformance/*) return 1 ;;
  scripts/check-escape-hatches.sh | scripts/test-check-escape-hatches.sh) return 1 ;;
  esac
  return 0
}

found=0
file=""
line=0
while IFS= read -r row; do
  case $row in
  '+++ b/'*)
    file=${row#+++ b/}
    continue
    ;;
  '+++ '* | '--- '*) continue ;;
  '@@ '*)
    # @@ -a,b +c,d @@: the next added line is line c of the new file.
    line=${row#*+}
    line=${line%%[, ]*}
    continue
    ;;
  '+'*) ;;
  *) continue ;;
  esac
  text=${row#+}
  if checked "$file"; then
    comment=$(comment_of "$file" "$text")
    for rule in "${rules[@]}"; do
      IFS=$'\t' read -r kind glob pattern advice <<<"$rule"
      # shellcheck disable=SC2053 # the pattern is meant to glob
      [[ $file == $glob ]] || continue
      subject=$text
      [ "$kind" = comment ] && subject=$comment
      if [ -n "$subject" ] && grep -Eq -- "$pattern" <<<"$subject"; then
        printf '%s:%s: %s\n    %s\n' "$file" "$line" "${text#"${text%%[![:space:]]*}"}" "$advice" >&2
        found=$((found + 1))
      fi
    done
  fi
  line=$((line + 1))
done < <("${diff[@]}")

if ((found > 0)); then
  echo "$found added line(s) put off a fix or switch off a check." >&2
  exit 1
fi
