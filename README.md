# bench-trends

Measurement history and the accepted basis for the engine benchmark gate (engine ADR-0010, P-12).

- `trend/results.jsonl`: every push-to-main measurement (contract §15.1 `BenchResult` lines,
  `wall_time` and `instructions` alike), appended by the `bench-gate` CI job
  (`.github/workflows/bench-gate.yml`). Never edited by hand.
- `accepted-basis.jsonl`: the basis the gate compares candidates against (engine ADR-0010 §4.2:
  never the last push). Changed **only** by the `bench-accept-baseline` workflow
  (`workflow_dispatch`, `.github/workflows/bench-accept-baseline.yml`), never automatically.

This branch is orphaned from `main` on purpose: it holds measurement data, not engine source
history, and grows independently of it.
