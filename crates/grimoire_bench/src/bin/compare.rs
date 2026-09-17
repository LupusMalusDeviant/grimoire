//! The comparator (Plan-0002 WP6.2 scope item 3): applies engine ADR-0010's exact-integer gate to
//! every `instructions`-metric candidate result against the matching line of an *accepted* basis
//! (never the last push, ADR-0010 §4.2), and reports (never gates on) `wall_time` results as
//! trend only.
//!
//! Exit codes follow ADR-0010 §4.3's table exactly: `0` for green or warn (a warning is an
//! annotation, not a failure), `2` for a measurement error (a zero value, or a candidate with no
//! comparable basis entry is reported but does not by itself raise this above 0 — see below), `3`
//! if any gated bench is red. The CI workflow (`.github/workflows/bench-gate.yml`, through
//! `.github/scripts/evaluate-bench-gate.sh`) decides whether exit `3` fails the job (WARN MODE vs.
//! HARD MODE, documented there; HARD MODE since plan 0002 M2); exit `2` always means something is
//! broken about the measurement itself and is never downgraded. `BENCH_GATE_MODE=hard` only
//! changes the wording of a red annotation.
//!
//! Usage:
//!   `compare --candidate <file.jsonl> --basis <file.jsonl>` — the real gate run.
//!   `compare --self-test` — proves the gate math on synthetic inputs (Plan-0002 WP6.2 scope
//!   item 3: "a self-test mode that runs the comparator on synthetic inputs in CI"), independent
//!   of any file or measurement; always run in CI regardless of WARN_MODE/HARD_MODE.

use std::fs;
use std::process::ExitCode;

use grimoire_bench::gate::{GateOutcome, gate_decision, warn_threshold_whole_percent};
use grimoire_bench::schema::BenchResult;

/// ADR-0010 measured `B = 0.0%` for Callgrind `Ir` (two independent 10-run CI batches, job-to-job
/// and run-to-run alike); every gate call in this binary uses that fixed noise band, which
/// resolves to `w = 3%` (see [`warn_threshold_whole_percent`]). Kept as a named constant (not
/// inlined as `0`) so the source of the number is traceable to the ADR, not to an unexplained
/// literal.
const IR_NOISE_BAND_PERCENT_X100: u32 = 0;

fn read_lines(path: &str) -> Result<Vec<BenchResult>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            BenchResult::from_json_line(line).map_err(|e| format!("{path}: invalid line: {e}"))
        })
        .collect()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--self-test") {
        return self_test();
    }

    let mut candidate_path = None;
    let mut basis_path = None;
    let mut it = args.into_iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--candidate" => candidate_path = it.next(),
            "--basis" => basis_path = it.next(),
            other => {
                eprintln!("compare: unknown flag {other:?}");
                return ExitCode::from(2);
            }
        }
    }
    let (Some(candidate_path), Some(basis_path)) = (candidate_path, basis_path) else {
        eprintln!("usage: compare --candidate <file.jsonl> --basis <file.jsonl>");
        eprintln!("       compare --self-test");
        return ExitCode::from(2);
    };

    let candidates = match read_lines(&candidate_path) {
        Ok(lines) => lines,
        Err(err) => {
            eprintln!("compare: {err}");
            return ExitCode::from(2);
        }
    };
    if candidates.is_empty() {
        eprintln!("compare: {candidate_path} has no result lines");
        return ExitCode::from(2);
    }

    // A missing or empty basis is not a measurement error: the trend branch (P-12) has no
    // accepted basis at all until the first "accept baseline" workflow_dispatch run. Every
    // candidate is then reported without a verdict, and the run stays green.
    let basis = match fs::metadata(&basis_path) {
        Ok(_) => match read_lines(&basis_path) {
            Ok(lines) => lines,
            Err(err) => {
                eprintln!("compare: {err}");
                return ExitCode::from(2);
            }
        },
        Err(_) => {
            println!(
                "No accepted basis file at {basis_path} yet — reporting candidates only, no verdict."
            );
            Vec::new()
        }
    };

    println!("## Benchmark gate (engine ADR-0010)\n");
    println!("| Scenario | Metric | Basis | Candidate | Delta | Verdict |");
    println!("|---|---|---|---|---|---|");

    // Only the wording of a red annotation depends on the mode; whether exit 3 fails the job is
    // decided by `.github/scripts/evaluate-bench-gate.sh`, which sets this variable.
    let hard_mode = std::env::var("BENCH_GATE_MODE").is_ok_and(|mode| mode == "hard");
    let mut any_error = false;
    let mut any_red = false;
    for candidate in &candidates {
        if candidate.metric != "instructions" {
            // wall_time (or any future non-gated metric): trend only, never gated.
            println!(
                "| {} | {} | — | {} {} | — | trend only |",
                candidate.scenario, candidate.metric, candidate.median, candidate.unit
            );
            continue;
        }
        let Some(basis_entry) = basis.iter().find(|b| candidate.comparable_to(b)) else {
            println!(
                "| {} | {} | none | {} {} | — | no accepted basis |",
                candidate.scenario, candidate.metric, candidate.median, candidate.unit
            );
            continue;
        };
        if candidate.commit.dirty {
            println!(
                "| {} | {} | {} {} | {} {} | — | dirty commit, no verdict |",
                candidate.scenario,
                candidate.metric,
                basis_entry.median,
                basis_entry.unit,
                candidate.median,
                candidate.unit
            );
            continue;
        }
        let baseline_ir = basis_entry.median as u64;
        let candidate_ir = candidate.median as u64;
        let warn_percent = warn_threshold_whole_percent(IR_NOISE_BAND_PERCENT_X100);
        let verdict = gate_decision(baseline_ir, candidate_ir, warn_percent);
        let delta_percent = if baseline_ir == 0 {
            f64::NAN
        } else {
            100.0 * (candidate_ir as f64 - baseline_ir as f64) / baseline_ir as f64
        };
        let verdict_text = match verdict {
            Ok(GateOutcome::Green) => "green",
            Ok(GateOutcome::Warn) => {
                println!(
                    "::warning title=Benchmark warning::{} {} is +{delta_percent:.2}% over the accepted basis (warn threshold {warn_percent}%, red threshold 10%)",
                    candidate.scenario, candidate.metric
                );
                "warn"
            }
            Ok(GateOutcome::Red) => {
                any_red = true;
                if hard_mode {
                    println!(
                        "::error title=Benchmark regression (HARD MODE)::{} {} is +{delta_percent:.2}% over the accepted basis, over the 10% ADR-0010 gate (HARD MODE: fails the build)",
                        candidate.scenario, candidate.metric
                    );
                } else {
                    println!(
                        "::warning title=Benchmark regression (WARN MODE)::{} {} is +{delta_percent:.2}% over the accepted basis, over the 10% ADR-0010 gate (WARN MODE: not failing the build)",
                        candidate.scenario, candidate.metric
                    );
                }
                "RED"
            }
            Err(_) => {
                eprintln!(
                    "compare: measurement error for {} {} (baseline={baseline_ir}, candidate={candidate_ir})",
                    candidate.scenario, candidate.metric
                );
                any_error = true;
                "ERROR"
            }
        };
        println!(
            "| {} | {} | {baseline_ir} {} | {candidate_ir} {} | {delta_percent:.2}% | {verdict_text} |",
            candidate.scenario, candidate.metric, basis_entry.unit, candidate.unit
        );
    }

    // Priority order matches how the CI workflow step reads these codes (see the module doc
    // comment): a measurement error is reported ahead of a graded red verdict, since it means
    // something about the measurement itself is broken, not that the gate has an opinion yet.
    if any_error {
        ExitCode::from(2)
    } else if any_red {
        ExitCode::from(3)
    } else {
        ExitCode::SUCCESS
    }
}

/// Synthetic self-test (Plan-0002 WP6.2 scope item 3): proves the exact-integer gate on inputs
/// that need no measurement, run under Callgrind or otherwise. Includes the exact negative proof
/// the task scope calls for: an injected +15% regression must yield red.
fn self_test() -> ExitCode {
    // (baseline, candidate, warn_percent, expected)
    let cases: &[(u64, u64, u32, GateOutcome)] = &[
        (100_000, 100_000, 3, GateOutcome::Green),
        (100_000, 102_000, 3, GateOutcome::Green),
        (100_000, 104_000, 3, GateOutcome::Warn),
        (100_000, 110_000, 3, GateOutcome::Warn), // exactly +10%: warn, not red
        (100_000, 110_010, 3, GateOutcome::Red),  // just over +10%: red
        (100_000, 115_000, 3, GateOutcome::Red),  // the task's own negative proof: +15% -> red
        (100_000, 120_000, 3, GateOutcome::Red),
        (100_000, 80_000, 3, GateOutcome::Green),
    ];

    println!("## Comparator self-test (synthetic inputs, engine ADR-0010)\n");
    println!("| Baseline | Candidate | Warn % | Expected | Got | OK |");
    println!("|---|---|---|---|---|---|");
    let mut failures = 0u32;
    for &(baseline, candidate, warn_percent, expected) in cases {
        let got = gate_decision(baseline, candidate, warn_percent);
        let ok = got == Ok(expected);
        if !ok {
            failures += 1;
        }
        println!(
            "| {baseline} | {candidate} | {warn_percent} | {expected:?} | {got:?} | {} |",
            if ok { "yes" } else { "NO" }
        );
    }

    if failures > 0 {
        eprintln!("compare --self-test: {failures} case(s) failed");
        return ExitCode::FAILURE;
    }
    println!("\nAll comparator self-test cases passed.");
    ExitCode::SUCCESS
}
