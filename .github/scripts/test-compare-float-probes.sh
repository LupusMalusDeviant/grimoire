#!/usr/bin/env bash
# Self-test of compare-float-probes.sh: runs it on fixture probes and checks its exit status.
# The script's output is captured, so its ::error:: lines do not become annotations of a green run.
set -euo pipefail

script="$(cd "$(dirname "$0")" && pwd)/compare-float-probes.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
failures=0

platforms=(windows-x86_64 linux-x86_64 macos-aarch64)
core_ok='basic_hash=0x00000000000000aa\ndmath_hash=0x00000000000000bb\n'

# write_probe <dir> <kind> <target> <contents>
write_probe() {
  mkdir -p "$1"
  printf '%b' "$4" > "$1/$2-$3.txt"
}

# fixture <name> <core platforms...>: identical core probes, a different std-trig hash per platform.
fixture() {
  local dir="$work/$1"
  shift
  mkdir -p "$dir"
  local index=0
  for target in "$@"; do
    write_probe "$dir" core "$target" "$core_ok"
    write_probe "$dir" std-trig "$target" "std_trig_hash=0x000000000000000${index}\n"
    index=$((index + 1))
  done
}

# expect <name> <exit status> <TESTS_RESULT>
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

fixture core-divergent "${platforms[@]}"
write_probe "$work/core-divergent" core macos-aarch64 'basic_hash=0x00000000000000aa\ndmath_hash=0x00000000000000cc\n'
expect core-divergent 1 success

fixture core-unreadable "${platforms[@]}"
write_probe "$work/core-unreadable" core linux-x86_64 'basic_hash=0x00000000000000aa\n'
expect core-unreadable 1 success

fixture missing-after-green windows-x86_64 linux-x86_64
expect missing-after-green 1 success

fixture missing-after-red windows-x86_64 linux-x86_64
expect missing-after-red 0 failure

mkdir -p "$work/none-after-green" "$work/none-after-red"
expect none-after-green 1 success
expect none-after-red 0 failure

for target in "${platforms[@]}"; do
  write_probe "$work/nested/float-probe-$target" core "$target" "$core_ok"
done
expect nested 0 success

if [ "$failures" -ne 0 ]; then
  echo "${failures} self-test case(s) failed"
  exit 1
fi
echo "all self-test cases passed"
