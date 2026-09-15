#!/usr/bin/env bash
# K4 driver for the WP6.1 / OF-17.3 noise spike (plan 0002 §3.2 step 4): runs the ir_probe
# binary once per (bench, variant) pair under Valgrind/Callgrind and emits one JSON line per run,
# reading the instruction count back from Callgrind's own `summary:` line — see the spike README
# for why this spike reads that line directly instead of depending on gungraun/iai-callgrind.
#
# Usage: measure_ir.sh <ir_probe-binary> <output-file> <cg-out-dir>
# Env: RUNNER_OS, BENCH_NOISE_REP, GITHUB_SHA (metadata copied into every JSON line, defaulted).
set -euo pipefail

BIN="${1:?path to the ir_probe binary}"
OUT="${2:?output JSONL file}"
CG_DIR="${3:?directory for callgrind output files}"

OS="${RUNNER_OS:-unknown}"
REP="${BENCH_NOISE_REP:-0}"
SHA="${GITHUB_SHA:-unknown}"

mkdir -p "$CG_DIR"
: > "$OUT"

BENCHES=(ecs sim)
BENCH_NAMES=(ecs_query_10k sim_step_600)
VARIANTS=(baseline plus5 plus12 plus20)
PERCENTS=(0 5 12 20)

for i in "${!BENCHES[@]}"; do
  bench="${BENCHES[$i]}"
  bench_name="${BENCH_NAMES[$i]}"
  for j in "${!VARIANTS[@]}"; do
    variant="${VARIANTS[$j]}"
    percent="${PERCENTS[$j]}"
    cg_out="$CG_DIR/cg.${bench}.${variant}.rep${REP}.out"
    echo "== callgrind: bench=$bench variant=$variant ==" >&2
    valgrind --tool=callgrind --callgrind-out-file="$cg_out" --quiet -- "$BIN" "$bench" "$variant"
    ir_line=$(grep '^summary:' "$cg_out" | tail -n1)
    if [[ -z "$ir_line" ]]; then
      echo "no 'summary:' line in $cg_out" >&2
      exit 1
    fi
    ir="${ir_line#summary: }"
    printf '{"os":"%s","rep":"%s","sha":"%s","bench":"%s","variant":"%s","percent":%s,"ir":%s}\n' \
      "$OS" "$REP" "$SHA" "$bench_name" "$variant" "$percent" "$ir" >>"$OUT"
  done
done

cat "$OUT"
