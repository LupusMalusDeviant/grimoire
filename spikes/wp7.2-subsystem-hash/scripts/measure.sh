#!/usr/bin/env bash
# Spike plan 0002 WP7.2 / OF-18.1: Callgrind instruction counts (Ir, engine ADR-0010's noise-free
# metric) of every mode of `subsystem-hash-cost` on every scenario, as a Markdown table.
#
# Each mode runs in its own process under Callgrind; the `build` mode (setup only) is subtracted
# and the rest divided by the measured ticks, so every value is instructions per simulated tick.
#
# Usage: measure.sh <subsystem-hash-cost binary> [callgrind output directory]
set -euo pipefail

BIN="${1:?path to the subsystem-hash-cost binary}"
CG_DIR="${2:-cg-out}"
mkdir -p "$CG_DIR"

MODES=(build step noop state_hash every_1 every_60 per_system every_60_systems)
SCENARIOS=(sigil10k sigilchurn parallel12k)

ir_of() {
  local scenario="$1" mode="$2" out
  out="$CG_DIR/cg.${scenario}.${mode}.out"
  valgrind --tool=callgrind --callgrind-out-file="$out" --quiet -- "$BIN" probe "$scenario" "$mode"
  local ir
  ir="$(sed -n 's/^summary: //p' "$out" | tail -n1)"
  if [[ -z "$ir" ]]; then
    echo "no 'summary:' line in $out" >&2
    exit 1
  fi
  echo "$ir"
}

echo "### Callgrind Ir per tick"
echo
echo "| Scenario | Systems | Mode | Ir per tick | vs. step | Ir per tick above step |"
echo "|---|---:|---|---:|---:|---:|"
derived=()
for scenario in "${SCENARIOS[@]}"; do
  params="$("$BIN" params "$scenario")"
  ticks="$(sed -n 's/.*ticks=\([0-9]*\).*/\1/p' <<<"$params")"
  systems="$(sed -n 's/.*systems=\([0-9]*\).*/\1/p' <<<"$params")"
  echo "== $scenario: $params ==" >&2
  declare -A IR=()
  for mode in "${MODES[@]}"; do
    IR[$mode]="$(ir_of "$scenario" "$mode")"
    echo "   $mode Ir=${IR[$mode]}" >&2
  done
  step=$(awk -v s="${IR[step]}" -v b="${IR[build]}" -v t="$ticks" 'BEGIN { printf "%.0f", (s - b) / t }')
  for mode in "${MODES[@]:1}"; do
    awk -v scenario="$scenario" -v systems="$systems" -v mode="$mode" -v ir="${IR[$mode]}" \
      -v build="${IR[build]}" -v ticks="$ticks" -v step="$step" 'BEGIN {
        per = (ir - build) / ticks
        above = (mode == "state_hash") ? per : per - step
        printf "| %s | %d | %s | %.0f | %.3fx | %.0f |\n", scenario, systems, mode, per, per / step, above
      }'
  done
  hash=$(awk -v s="${IR[state_hash]}" -v b="${IR[build]}" -v t="$ticks" 'BEGIN { printf "%.0f", (s - b) / t }')
  per_system=$(awk -v s="${IR[per_system]}" -v b="${IR[build]}" -v t="$ticks" 'BEGIN { printf "%.0f", (s - b) / t }')
  derived+=("$(awk -v scenario="$scenario" -v systems="$systems" -v step="$step" -v hash="$hash" \
    -v ps="$per_system" 'BEGIN {
      extra = ps - step
      printf "| %s | %d | %.0f | %.0f (%.1f%% of a step) | %.2f | %.1f%% | %.1f%% | %.2f%% | %.2f%% |", \
        scenario, systems, step, hash, 100 * hash / step, extra / hash, 100 * extra / step, \
        100 * hash / step, 100 * hash / step / 60, 100 * (systems + 1) * hash / step / 600
    }')")
  unset IR
done

echo
echo "### Derived per tick"
echo
echo "Columns: plain step; one state hash; world hashes the per-system trace adds per tick (≈ systems + 1);"
echo "overhead per system per tick; state hash every tick; every 60 ticks; every 600 ticks with system hashes."
echo
echo "| Scenario | Systems | Step Ir | State hash Ir | Hashes added per tick | Per system per tick | Every tick | Every 60 | Every 600 + systems |"
echo "|---|---:|---:|---:|---:|---:|---:|---:|---:|"
printf '%s\n' "${derived[@]}"
