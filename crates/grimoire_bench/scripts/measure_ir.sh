#!/usr/bin/env bash
# Measures Callgrind Ir for both P0 benches (Plan-0002 WP6.2 scope item 2): one baseline run per
# bench, no synthetic-variant sweep. Engine ADR-0010 already proved (two independent 10-run CI
# batches on ubuntu-24.04) that Ir's job-to-job and run-to-run noise band is exactly 0% for these
# two bench shapes, so unlike the WP6.1 noise spike's own measure_ir.sh
# (spikes/bench-noise/scripts/measure_ir.sh, branch p1/wp6.1-bench-spike, which swept four
# variants to *establish* that noise band), the production gate only needs the real candidate
# value once per push. Reuses that spike's proven technique of reading Callgrind's own `summary:`
# line rather than depending on gungraun (engine ADR-0010 WP6.2 addendum, "Entscheidung: roh
# bleibt, vorerst").
#
# Every emitted line is a contract-§15.1-compliant BenchResult, built by the `bench_result`
# binary (not by hand-formatted JSON here) so the one schema implementation is also the one place
# that can reject an invalid line.
#
# Usage: measure_ir.sh <ir_probe-bin> <bench_result-bin> <output-jsonl> <cg-out-dir>
# Env (all defaulted): GITHUB_SHA, GIT_DIRTY (1 for a dirty working tree), RUNNER_OS_LOWER,
#   RUNNER_ARCH_LOWER, RUNNER_IMAGE, CPU_MODEL, LOGICAL_CPUS, GITHUB_RUN_ID, GITHUB_RUN_ATTEMPT.
set -euo pipefail

IR_PROBE="${1:?path to the ir_probe binary}"
BENCH_RESULT="${2:?path to the bench_result binary}"
OUT="${3:?output JSONL file}"
CG_DIR="${4:?directory for callgrind output files}"

SHA="${GITHUB_SHA:-0000000000000000000000000000000000000000}"
OS="${RUNNER_OS_LOWER:-linux}"
ARCH="${RUNNER_ARCH_LOWER:-x86_64}"
IMAGE="${RUNNER_IMAGE:-}"
CPU_MODEL="${CPU_MODEL:-}"
LOGICAL_CPUS="${LOGICAL_CPUS:-$(nproc 2>/dev/null || echo 1)}"
VALGRIND_VERSION="$(valgrind --version 2>/dev/null | sed -n 's/^valgrind-//p')"
RUN_ID="${GITHUB_RUN_ID:-}"
RUN_ATTEMPT="${GITHUB_RUN_ATTEMPT:-}"

DIRTY_FLAG=()
if [[ "${GIT_DIRTY:-0}" == "1" ]]; then DIRTY_FLAG=(--dirty); fi

mkdir -p "$CG_DIR"
: > "$OUT"

measure_one() {
  local bin_arg="$1" scenario="$2"
  local cg_out="$CG_DIR/cg.${bin_arg}.baseline.out"
  echo "== callgrind: bench=$bin_arg (baseline) ==" >&2
  valgrind --tool=callgrind --callgrind-out-file="$cg_out" --quiet -- "$IR_PROBE" "$bin_arg" baseline
  local ir_line
  ir_line=$(grep '^summary:' "$cg_out" | tail -n1)
  if [[ -z "$ir_line" ]]; then
    echo "no 'summary:' line in $cg_out" >&2
    exit 1
  fi
  local ir="${ir_line#summary: }"
  echo "== $scenario Ir = $ir ==" >&2

  local params
  params="$("$IR_PROBE" "$bin_arg" params)"

  local run_args=()
  if [[ -n "$RUN_ID" && -n "$RUN_ATTEMPT" ]]; then
    run_args=(--run-id "$RUN_ID" --run-attempt "$RUN_ATTEMPT")
  fi
  local fingerprint_args=()
  if [[ -n "$VALGRIND_VERSION" ]]; then
    fingerprint_args=(--fingerprint "valgrind=$VALGRIND_VERSION")
  fi
  local image_args=()
  if [[ -n "$IMAGE" ]]; then image_args=(--image "$IMAGE"); fi
  local cpu_args=()
  if [[ -n "$CPU_MODEL" ]]; then cpu_args=(--cpu-model "$CPU_MODEL"); fi

  "$BENCH_RESULT" \
    --scenario "$scenario" --metric instructions --unit ir \
    --samples "$ir" \
    --sha "$SHA" "${DIRTY_FLAG[@]}" \
    --os "$OS" --arch "$ARCH" "${image_args[@]}" "${cpu_args[@]}" \
    --logical-cpus "$LOGICAL_CPUS" "${fingerprint_args[@]}" \
    --executor-kind sequential --executor-threads 1 \
    --params "$params" \
    --value-origin runner \
    "${run_args[@]}" \
    >>"$OUT"
}

measure_one ecs ecs_query_10k
measure_one sim sim_step_600
# Plan 0002 WP5.3: the Sigil -> Render extraction adapter over 10,000 bullets. Enters the gate in
# its current mode like the two P0 benches; until an accepted basis exists for it, `compare`
# reports it as "no accepted basis" without a verdict.
measure_one sigil_extract sigil_extract_10k
# Plan 0002 WP5.4: the `sigil.*` tick benches under the same gate. `sigil_update_6k` is the WP5.1
# micro-bench (six stacked modifiers, the trig-free hot path), `sigil_update_10k` holds 10,000
# active bullets with transforms, `sigil_churn_2k` spawns and despawns 2,000 bullets per tick.
measure_one sigil_update sigil_update_6k
measure_one sigil_update_10k sigil_update_10k
measure_one sigil_churn sigil_churn_2k
# Plan 0002 WP3.6: the render CPU benches — the bullet pass's preparation of 10,000 bullets and the
# clustered lighting's preparation of 256 point lights, both without a GPU
# (`grimoire_render::measurement`).
measure_one bullet_upload render_bullet_upload_10k
measure_one light_cluster render_light_cluster_256

cat "$OUT"
