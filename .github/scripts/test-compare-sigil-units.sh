#!/usr/bin/env bash
# Self-test of compare-sigil-units.sh: runs it on fixture artifacts and checks its exit status.
# The script's output is captured, so its ::error:: lines do not become annotations of a green run.
set -euo pipefail

script="$(cd "$(dirname "$0")" && pwd)/compare-sigil-units.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
failures=0

platforms=(windows-latest ubuntu-latest macos-latest)

# artifact <case> <platform>: one platform's build output with two units and a report.
artifact() {
  local root="$work/$1/$2"
  mkdir -p "$root/units/corpus" "$root/units/reference" "$root/reports"
  printf 'GRIMSIGL\001\000\000\000unit-a' > "$root/units/corpus/a.unit"
  printf 'GRIMSIGL\001\000\000\000unit-b' > "$root/units/reference/b.unit"
  printf '{"ok": true}\n' > "$root/reports/corpus.json"
}

# fixture <case> <platforms...>
fixture() {
  local name="$1"
  shift
  mkdir -p "$work/$name"
  for platform in "$@"; do
    artifact "$name" "$platform"
  done
}

# expect <case> <exit status> <TESTS_RESULT>
expect() {
  local name="$1" want="$2" result="$3" got=0
  TESTS_RESULT="$result" GITHUB_STEP_SUMMARY="$work/$name.summary" \
    bash "$script" "$work/$name" "self-test" > "$work/$name.log" 2>&1 || got=$?
  if [ "$got" -eq "$want" ]; then
    echo "ok   ${name} (exit ${got})"
  else
    echo "FAIL ${name}: exit ${got}, expected ${want}"
    sed 's/^::/  [annotation] /' "$work/$name.log"
    failures=$((failures + 1))
  fi
}

fixture identical "${platforms[@]}"
expect identical 0 success

fixture one-byte-differs "${platforms[@]}"
printf 'GRIMSIGL\001\000\000\000unit-B' > "$work/one-byte-differs/macos-latest/units/reference/b.unit"
expect one-byte-differs 1 success

fixture report-differs "${platforms[@]}"
printf '{"ok": false}\n' > "$work/report-differs/ubuntu-latest/reports/corpus.json"
expect report-differs 1 success

fixture missing-file "${platforms[@]}"
rm "$work/missing-file/windows-latest/units/corpus/a.unit"
expect missing-file 1 success

fixture extra-file "${platforms[@]}"
printf 'extra' > "$work/extra-file/ubuntu-latest/units/corpus/c.unit"
expect extra-file 1 success

fixture no-units "${platforms[@]}"
rm -r "$work/no-units/macos-latest/units"
expect no-units 1 success

fixture missing-after-green windows-latest ubuntu-latest
expect missing-after-green 1 success

fixture missing-after-red windows-latest ubuntu-latest
expect missing-after-red 0 failure

fixture differs-after-red windows-latest ubuntu-latest
printf 'x' >> "$work/differs-after-red/ubuntu-latest/units/corpus/a.unit"
expect differs-after-red 1 failure

mkdir -p "$work/none-after-green" "$work/none-after-red"
expect none-after-green 1 success
expect none-after-red 0 failure
expect absent-directory 1 success

if [ "$failures" -ne 0 ]; then
  echo "${failures} self-test case(s) failed"
  exit 1
fi
echo "all self-test cases passed"
