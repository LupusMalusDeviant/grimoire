# grimoire_bench

Benchmarks and the JSON-schema types for bench results (engine contract §15.1), plus the
regression gate from engine ADR-0010 (Plan-0002 WP6.2).

Outside the determinism set (engine ADR-0008, contract §1, §3): this is the only engine crate
allowed to read the wall clock, and it may depend on any runtime crate and on `grimoire_exec`. It
deliberately carries no `clippy.toml`. Nothing depends on it.

## What lives here

| Module / binary | Purpose |
|---|---|
| `src/schema.rs` | The `BenchResult` JSON Lines schema v1 (contract §15.1): strict read/write, size limits, median recomputation, hex-encoded hashes. |
| `src/gate.rs` | The regression gate (engine ADR-0010 §4.3): the exact-integer `10*candidate > 11*baseline` rule for red, and the sliding warning threshold. |
| `src/scenarios.rs` | The two P0 baseline bench bodies (`ecs_query_10k`, `sim_step_600`), the calibrated regression injector, the WP5.3 extraction body `sigil_extract_10k` (the facade's Sigil -> Render adapter over 10,000 bullets) and the WP5.4 Sigil tick bodies: `sigil_update_6k` (the WP5.1 micro-bench, six stacked modifiers), `sigil_update_10k` (10,000 active bullets with transforms) and `sigil_churn_2k` (2,000 spawns and 2,000 despawns per tick at 10,000 active), the WP3.6 render CPU bodies `render_bullet_upload_10k` and `render_light_cluster_256`, and the WP6.5 collision bodies `collide_uniform` and `collide_cluster` (10,000 bullets and 100 enemy entities per tick through `rebuild_par`, `overlapping_batch` and `graze_ring`; spread over a 64 x 64 arena, or every bullet in the four grid cells around the graze centre). All are measured by `wallclock`; `ir_probe` measures every one except `collide_cluster`, which stays a trend (contract §14). |
| `src/curtain.rs`, `fixtures/` | The WP6.6 benchmark scene "Vollvorhang" (PRD-0004): three compiled Sigil units (sources next to them, kept current by `grimoire_sigilc`'s `tests/unit_fixtures.rs` and part of the WP4.4 platform identity gate) mixing every modifier type, the blocks ring, spiral, fan, wave, line and scatter, and the transforms burst, reverse and change_type. Steady at about 10,000 live bullets, measured per tick in four phases: simulation, extraction, collision (as the §9.6 adapter will build the grid) and render CPU (`render_stage` on an offscreen renderer, the software adapter in CI). Wall-clock trend only. |
| `src/bin/ir_probe.rs` | Runs one bench body once; meant to be invoked under `valgrind --tool=callgrind` by `scripts/measure_ir.sh` / `scripts/calibrate_injection.sh`. |
| `src/bin/wallclock.rs` | Wall-clock trend measurement (never gated) for every bench above; prints `BenchResult` lines, plus budget lines on stderr: the per-extraction median against the plan's 0.5 ms budget and the per-tick medians of the Sigil tick benches against the 1.0 ms budget (sequential executor), and both collision benches per tick on one thread and on a `grimoire_exec::ThreadPoolExecutor` with N threads against the 1.5 ms budget, and the four full-curtain phases per tick against the P1 stress-test budgets (simulation 1.0 ms, extraction 0.5 ms, collision 1.5 ms, render CPU 3 ms). The render phase needs a GPU adapter; with `GRIMOIRE_REQUIRE_GPU_ADAPTER=1` (set in CI together with `GRIMOIRE_GPU_ADAPTER=software`) a missing adapter fails the run instead of dropping the phase. |
| `src/bin/bench_result.rs` | Builds one contract-compliant `BenchResult` line from CLI flags, so the bash scripts driving Callgrind never hand-format JSON. |
| `src/bin/compare.rs` | The comparator: gates `instructions`-metric candidates against an accepted basis, reports `wall_time` as trend only. `--self-test` proves the gate math on synthetic inputs. |
| `src/bin/calibration_check.rs` | Verifies the calibration proof produced by `scripts/calibrate_injection.sh` (accuracy within ±1 percentage point, correct red/not-red verdicts). |

## Why Callgrind `Ir`, not wall clock, is the gate metric

Engine ADR-0010 measured wall-clock noise of 95-129% job-to-job on shared Linux CI runners for
these exact two benches — more than ten times the 10% threshold it is meant to gate. Callgrind
instruction counts (`Ir`), on the same runners, measured **exactly 0%** noise across two
independent 10-run batches. `Ir` is therefore the hard gate metric for these single-threaded P0
benches; wall clock stays a trend, on every OS, never a pass/fail signal. See
`docs/adr/0010-benchmark-strategie.md` for the full measurement and the decision.

## Running locally

Building the bench binaries is fine locally (`cargo build --release -p grimoire_bench --bins`),
but **do not run the benches or Callgrind locally** — per this repo's contribution rules,
benchmark and Callgrind measurements only happen on the CI runner
(`.github/workflows/bench-gate.yml`), so every accepted number is comparable across runs. Unit
tests (`cargo test -p grimoire_bench`) are fine and expected before every commit (contract §2
rule 3): they cover the schema, the gate math (including the negative proof that an injected
+15% regression yields red) and the bench bodies' own sanity checks — none of them touch a real
clock or Valgrind.

## The gate in CI

See `.github/workflows/bench-gate.yml` for the full pipeline (build, measure, compare, publish)
and its header comment for the mode switch `BENCH_GATE_MODE`. The workflow runs on every pull
request, every push to `main`, nightly and on demand. In short:

1. **`gate`**: builds the release binaries, measures wall-clock samples (trend) and `Ir` (gate)
   for every bench, fetches the accepted basis from the `bench-trends` branch (P-12), and
   evaluates the verdict with `.github/scripts/evaluate-bench-gate.sh`. **HARD MODE since plan
   0002 M2**: more than +10% `Ir` on any scenario with an accepted basis fails the job. A scenario
   without an accepted basis gets no verdict until `bench-accept-baseline` accepts one.
2. **`calibration-proof`**: calibrates the regression injector fresh on this runner (engine
   ADR-0010: the per-unit slope is bench- and host-dependent, never copied from a prior
   measurement) and proves +5% never gates red while +15%/+20% always do, backed by a real
   Callgrind measurement, not just a unit test on synthetic numbers.
3. **`regression-proof`**: the self-test job of plan 0002 WP6.2. It measures every bench without
   injection (basis and repeat) and with `GRIMOIRE_BENCH_INJECT_REGRESSION=1` (a calibrated +5% and
   +15% on `ecs_query_10k` and `sim_step_600`, see `scripts/measure_ir.sh`), then runs the `gate`
   job's evaluation script in HARD MODE (`.github/scripts/prove-bench-gate.sh`). It is green only if
   +15% fails that evaluation with exactly the two injected scenarios red, while +5% and the repeat
   pass.
4. **`publish-trend`** (push to `main` only): appends this run's `BenchResult` lines to
   `bench-trends:trend/results.jsonl` whenever the measurement is valid, including a red verdict.

The accepted basis (`bench-trends:accepted-basis.jsonl`) changes only through the
`bench-accept-baseline` workflow (`workflow_dispatch`), never automatically from a push. A
regression that is merged on purpose turns the `gate` job on `main` red once; its trend lines are
still published, so accepting that commit as the new basis clears the gate.
