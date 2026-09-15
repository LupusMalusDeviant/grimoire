#!/usr/bin/env bash
# Calibrates the regression injector (ADR-0010 "Vor der Umsetzung in WP6.2" / calibration open
# point; plan 0002 WP6.1 §3.1: "der Anteil wird lokal so kalibriert, dass `Ir` um den Nennwert
# steigt, ... vor dem ersten Lauf festgeschrieben").
#
# The spike's nominal "+N whole rounds/ticks" injection mixes fixed per-round/per-tick overhead
# into the delta, so a nominal +12% landed at only +8.2% measured Ir for sim_step_600 (ADR-0010
# option 2, "Negativ"). This script instead measures, on this runner in this same job, the
# marginal Ir cost of bench_noise's fine-grained calibration unit (a homogeneous unit with no
# fixed per-round overhead), then solves for the unit count that puts the *measured* Ir delta
# within the target percentage of the *measured* baseline.
#
# Method: two Callgrind runs establish the per-unit slope (baseline at units=0, a reference run at
# a large unit count), then one Callgrind run per target percentage verifies the result. Ir is an
# exact integer (Callgrind; ADR-0010: "Rauschen ist ... exakt null"), so this stays in bash's
# 64-bit integer arithmetic end to end, matching the exact-integer gate rule (plan §4.3) instead
# of introducing floating-point rounding for a throwaway measurement.
#
# Usage: calibrate_injection.sh <ir_probe-binary> <bench: ecs|sim> <out-jsonl> <cg-out-dir>
# Targets 5, 12 and 20 percent (ADR-0010's own negative-result percentages).
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
# per-unit slope is precise (the calibration unit is deliberately tiny per call, see lib.rs).
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

for PERCENT in 5 12 20; do
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
