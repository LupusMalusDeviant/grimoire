#!/usr/bin/env bash
# Calibrates the regression injector (Plan-0002 WP6.2 scope item 4; engine ADR-0010 "Vor der
# Umsetzung in WP6.2" / calibration Nachtrag): measures, on this runner in this same CI job, the
# marginal Ir cost of grimoire_bench::scenarios::run_calibration_units's fine-grained calibration
# unit, then solves for the unit count that puts the *measured* Ir delta within the target
# percentage of the *measured* baseline.
#
# Reused near-verbatim from the WP6.1 spike's own calibrate_injection.sh
# (spikes/bench-noise/scripts/calibrate_injection.sh, branch p1/wp6.1-bench-spike), which measured
# this same method against these same two benches (engine ADR-0010's WP6.2 addendum: all six
# calibration points landed within 0.00 percentage points of target). What changes here is the
# target percentages — 5/15/20, not the spike's 5/12/20 — because this task's own negative proof
# is specifically "+15% must be red" (not +12%), and the unit counts themselves: engine ADR-0010's
# closing note says explicitly that WP6.2 must calibrate fresh, not reuse the spike's measured
# unit counts (the per-unit slope is bench- and host-dependent).
#
# Method: two Callgrind runs establish the per-unit slope (baseline at units=0, a reference run at
# a large unit count), then one Callgrind run per target percentage verifies the result. Ir is an
# exact integer (Callgrind), so this stays in bash's 64-bit integer arithmetic end to end,
# matching the exact-integer gate rule (engine ADR-0010 §4.3) instead of introducing
# floating-point rounding for a calibration measurement that itself feeds that same gate.
#
# Usage: calibrate_injection.sh <ir_probe-bin> <bench: ecs|sim> <out-jsonl> <cg-out-dir>
set -euo pipefail

BIN="${1:?path to the ir_probe binary}"
BENCH="${2:?ecs|sim}"
OUT="${3:?output jsonl file}"
CG_DIR="${4:?directory for callgrind output files}"

mkdir -p "$CG_DIR"
: > "$OUT"

run_cg() {
  local units="$1" tag="$2"
  local cg_out="$CG_DIR/cg.${BENCH}.calibrate.${tag}.out"
  valgrind --tool=callgrind --callgrind-out-file="$cg_out" --quiet -- \
    "$BIN" "$BENCH" calibrated "$units" >&2
  local line
  line=$(grep '^summary:' "$cg_out" | tail -n1)
  if [[ -z "$line" ]]; then
    echo "no 'summary:' line in $cg_out" >&2
    exit 1
  fi
  echo "${line#summary: }"
}

echo "== calibration: $BENCH baseline (0 calibration units) ==" >&2
IR0=$(run_cg 0 baseline)
echo "== calibration: $BENCH baseline Ir = $IR0 ==" >&2

# Reference run: enough calibration units to move Ir by a clearly measurable amount, so the
# per-unit slope is precise (the calibration unit is deliberately tiny per call).
UREF=2000000
echo "== calibration: $BENCH reference run (units=$UREF) ==" >&2
IR_REF=$(run_cg "$UREF" ref)
DELTA_REF=$((IR_REF - IR0))
if [[ "$DELTA_REF" -le 0 ]]; then
  echo "calibration unit added no measurable Ir (baseline=$IR0 ref=$IR_REF at units=$UREF);" \
       "the compiler likely folded run_calibration_units away" >&2
  exit 1
fi
echo "== calibration: $BENCH slope = $DELTA_REF Ir / $UREF units ==" >&2

for PERCENT in 5 15 20; do
  # needed_units = round(IR0 * PERCENT * UREF / (100 * DELTA_REF))
  NUM=$((IR0 * PERCENT * UREF))
  DEN=$((100 * DELTA_REF))
  NEEDED_UNITS=$(( (NUM + DEN / 2) / DEN ))
  echo "== calibration: $BENCH target +${PERCENT}% -> $NEEDED_UNITS calibration units ==" >&2
  IR_FINAL=$(run_cg "$NEEDED_UNITS" "plus${PERCENT}")
  echo "== calibration: $BENCH +${PERCENT}% final Ir = $IR_FINAL ==" >&2
  printf '{"bench":"%s","target_percent":%s,"baseline_ir":%s,"reference_units":%s,"reference_ir":%s,"needed_units":%s,"final_ir":%s}\n' \
    "$BENCH" "$PERCENT" "$IR0" "$UREF" "$IR_REF" "$NEEDED_UNITS" "$IR_FINAL" >> "$OUT"
done

cat "$OUT"
