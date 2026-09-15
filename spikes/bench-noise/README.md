# Spike: bench-noise (plan 0002 WP6.1, OF-17.3)

Throwaway code. Answers the open question OF-17.3: how noisy are benchmarks on GitHub-hosted
runners, and which gate metric is stable enough to break a CI run on a >10 % regression? See the
prepared proposal `docs/plans/0002-vorbereitung-of-17.3.md` and plan `0002-phase-p1-sichtbarer-kern.md`
WP6.1. This crate is never loaded by the engine and is not a workspace member (own empty
`[workspace]` table, like `spikes/look-dev` and `spikes/sigil-syntax`).

## What this measures

Two P0-shaped baselines, matching the plan's naming:

- `ecs_query_10k` — the write half of `grimoire_ecs/tests/timing.rs`
  (`query_mut::<(&mut Pos, &Vel)>` over 10,000 entities).
- `sim_step_600` — a small self-contained stand-in for the P0 demo scenario in
  `grimoire_sim/tests/scenario/mod.rs` (moving entities, one exclusive integrate system, a
  periodic despawn/respawn). That module is private to `grimoire_sim`'s own test binary; this
  spike asks how noisy a "sim step"-shaped benchmark is, not whether the real scenario is
  reproduced, so a faithful port was not worth the spike's time.

Two metrics:

- **K1 wall-clock** (`src/bin/wallclock.rs`): median of 15 timed samples per (bench, variant),
  after 2 warm-up samples, using `std::time::Instant`.
- **K4 instructions** (`src/bin/ir_probe.rs` + `scripts/measure_ir.sh`): one Callgrind run per
  (bench, variant), reading `Ir` back from Callgrind's own `summary:` line in its output file.
  Deliberately **not** `gungraun`/`iai-callgrind`: this spike only needs one number per run, and
  reading Callgrind's documented summary line avoids depending on an unreviewed Rust
  API/JSON-schema for a throwaway measurement. If K4 comes out gate-worthy, the ADR proposes
  `gungraun` for the real `grimoire_bench` in WP6.2, where its regression tooling and developer
  ergonomics are worth the dependency.

Four variants, injected as whole extra rounds/ticks of identical work inside the measured region
(same technique as the prepared proposal §3.1), read as a runtime parameter so one binary serves
every variant: `baseline` (+0 %), `plus5`, `plus12`, `plus20`.

## Deviations from the prepared proposal (cheapest sound option, stated here as instructed)

The proposal's full protocol (two measurement blocks on separate days, six candidates K1-K6,
21 bench/variant combinations, a two-commit build-path probe, an independent double-build check,
a cache-hostile variant, Windows/macOS wall-clock at full repetition) is sized for a dedicated,
multi-day measurement campaign. This spike answers the same question at a scope one session can
actually execute and watch through GitHub Actions, and trims:

- **One measurement window, not two blocks on separate days.** The proposal's two-block design
  (§3.0) exists to sample noise across different times of day. This spike's 10 Linux repetitions
  run from a single push. *Open point for the PO:* re-run the matrix on a different day if
  day-to-day drift (image updates, host churn) turns out to matter more than run-to-run noise.
- **+15 % dropped**, keeping baseline/+5 %/+12 %/+20 %. +12 % already exercises "just above the
  10 % gate" and +20 % "clearly above"; a fourth point between them adds cost without changing
  the answer to "is this metric sharp enough".
- **No cache-hostile variant.** The proposal itself calls this exploratory ("blinder Fleck"), not
  required to answer the gate-metric question.
- **No two-commit / independent-double-build check.** Cheap in principle, but needs a second
  `git worktree` checkout and build inside the job; the runtime-env-var injection above already
  gives every variant from a single build, so the marginal cost of also proving build-path
  stability did not fit this session's budget. *Open point for the PO / WP6.2.*
- **Windows and macOS: wall-clock only, `baseline` and `plus20` only, 3 repetitions instead of
  10.** Matches the hard rule "Windows and macOS only for the wall-clock comparison" and keeps
  concurrency modest while another PR's 3-OS CI runs in this repo.
- **`perf stat` (K5) probe skipped.** The proposal already expects it to fail on shared runners
  (§2 K5); skipping it saves a job without losing information the spike needs.
- **Warm-up trimmed** to 2 samples (not 3 s) and **15 wall-clock samples** per job (not 30), to
  keep the ten-repetition matrix affordable.

None of these trims touch the actual question: whether Callgrind's `Ir` is stable enough
(job-to-job and run-to-run) to gate a >10 % regression, with the wall-clock trend as the
fallback/companion metric plan 0002 §4.4 already expects to keep.

## Files

- `src/lib.rs` — benchmark bodies and the exact-integer gate decision (plan §4.3), unit-tested
  (plan §5's comparator self-test table) — tests run only in CI, never on the development machine.
- `src/bin/wallclock.rs`, `src/bin/ir_probe.rs` — the two measurement binaries.
- `src/bin/analyze.rs` — the comparator: reads every measurement file under a directory, computes
  the CV table and the injected-regression detection counts, prints Markdown and (optionally)
  writes a JSON report.
- `scripts/measure_ir.sh` — drives Valgrind/Callgrind over every (bench, variant) pair and emits
  one JSON line per run.

## Running

CI-only, per the hard rule on this spike: no local benchmarks, no local `cargo test`. Locally,
only `cargo fmt`, `cargo check` and `cargo clippy` are used to keep this compiling between pushes.
See `.github/workflows/spike-wp6.1-bench-noise.yml`, triggered on push to
`p1/wp6.1-bench-spike` only.
