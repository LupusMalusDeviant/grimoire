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
| `src/scenarios.rs` | The two P0 baseline bench bodies (`ecs_query_10k`, `sim_step_600`) and the calibrated regression injector. |
| `src/bin/ir_probe.rs` | Runs one bench body once; meant to be invoked under `valgrind --tool=callgrind` by `scripts/measure_ir.sh` / `scripts/calibrate_injection.sh`. |
| `src/bin/wallclock.rs` | Wall-clock trend measurement (never gated) for both benches; prints `BenchResult` lines. |
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
and its header comment for the WARN MODE → HARD MODE switch. In short:

1. **`gate`**: builds the release binaries, measures wall-clock samples (trend) and `Ir` (gate)
   for both benches, fetches the accepted basis from the `bench-trends` branch (P-12), and prints
   a verdict. Currently **WARN MODE**: a red verdict is annotated but never fails the job.
2. **`calibration-proof`**: calibrates the regression injector fresh on this runner (engine
   ADR-0010: the per-unit slope is bench- and host-dependent, never copied from a prior
   measurement) and proves +5% never gates red while +15%/+20% always do — this task's own
   negative proof, backed by a real Callgrind measurement, not just a unit test on synthetic
   numbers.
3. **`publish-trend`** (push to `main` only): appends this run's `BenchResult` lines to
   `bench-trends:trend/results.jsonl`.

The accepted basis (`bench-trends:accepted-basis.jsonl`) changes only through the
`bench-accept-baseline` workflow (`workflow_dispatch`), never automatically from a push.
