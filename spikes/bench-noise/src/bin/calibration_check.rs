//! Verifies the calibration proof (ADR-0010 "Vor der Umsetzung in WP6.2" Nachtrag): reads every
//! `calibration-*.jsonl` line produced by `scripts/calibrate_injection.sh`, computes the actual
//! measured percentage against that same run's own baseline, prints a Markdown table, and fails
//! (non-zero exit) unless, for every (bench, target percent):
//!
//! - the measured percentage is within +/-1 percentage point of the target (this spike/ADR's own
//!   calibration accuracy bar), and
//! - the exact-integer gate ([`bench_noise::gate_decision`], plan §4.3, `w = 3%`) verdicts match
//!   what this proof exists to demonstrate: `+5%` never red, `+12%` and `+20%` always red.
//!
//! Usage: `calibration_check <results-dir>`

use std::env;
use std::fs;
use std::path::Path;

use bench_noise::{GateOutcome, gate_decision};
use serde::Deserialize;

#[derive(Deserialize)]
struct CalibrationLine {
    bench: String,
    target_percent: u64,
    baseline_ir: u64,
    final_ir: u64,
    needed_units: u64,
}

fn main() {
    let input_dir = env::args().nth(1).unwrap_or_else(|| "results".to_string());
    let mut lines = Vec::new();
    collect(Path::new(&input_dir), &mut lines);
    if lines.is_empty() {
        eprintln!("no calibration-*.jsonl lines found under {input_dir}");
        std::process::exit(2);
    }
    lines.sort_by(|a, b| (&a.bench, a.target_percent).cmp(&(&b.bench, b.target_percent)));

    println!("## WP6.1 / OF-17.3 calibration proof (ADR-0010 Nachtrag)\n");
    println!(
        "| Bench | Target % | Needed units | Baseline Ir | Final Ir | Measured % | \\|Δ\\| pp | Gate verdict | Expected | OK |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|");

    let mut failures = Vec::new();
    for line in &lines {
        // Hundredths-of-a-percent fixed point, exact-integer arithmetic (no float rounding),
        // matching the exact-integer ethos of the gate itself (plan §4.3).
        let delta = i128::from(line.final_ir) - i128::from(line.baseline_ir);
        let measured_x100 = delta * 10_000 / i128::from(line.baseline_ir);
        let measured_percent = measured_x100 as f64 / 100.0;
        let target_x100 = i128::from(line.target_percent) * 100;
        let diff_pp = (measured_x100 - target_x100).unsigned_abs() as f64 / 100.0;

        let verdict = match gate_decision(line.baseline_ir, line.final_ir, 3) {
            Ok(GateOutcome::Red) => "red",
            Ok(GateOutcome::Warn) => "warn",
            Ok(GateOutcome::Green) => "green",
            Err(_) => "error",
        };
        // ADR-0010's own negative-result percentages: +5% must never gate red, +12%/+20% must.
        let expect_red = line.target_percent >= 12;
        let expected = if expect_red { "red" } else { "not red" };
        let verdict_ok = (verdict == "red") == expect_red;
        let accuracy_ok = diff_pp <= 1.0;
        let ok = verdict_ok && accuracy_ok;
        if !ok {
            failures.push(format!(
                "{} +{}%: measured {measured_percent:.2}% (|Δ|={diff_pp:.2}pp), verdict={verdict} (expected {expected})",
                line.bench, line.target_percent
            ));
        }
        println!(
            "| {} | {} | {} | {} | {} | {:.2}% | {:.2} | {} | {} | {} |",
            line.bench,
            line.target_percent,
            line.needed_units,
            line.baseline_ir,
            line.final_ir,
            measured_percent,
            diff_pp,
            verdict,
            expected,
            if ok { "yes" } else { "NO" }
        );
    }

    if !failures.is_empty() {
        eprintln!("\ncalibration check FAILED:");
        for f in &failures {
            eprintln!("  - {f}");
        }
        std::process::exit(1);
    }
    println!("\nAll calibration targets within +/-1pp and gate verdicts as expected.");
}

fn collect(dir: &Path, out: &mut Vec<CalibrationLine>) {
    let Ok(entries) = fs::read_dir(dir) else {
        eprintln!("cannot read {}", dir.display());
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
            continue;
        }
        let is_calibration = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("calibration-") && n.ends_with(".jsonl"));
        if !is_calibration {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<CalibrationLine>(line) {
                Ok(record) => out.push(record),
                Err(err) => eprintln!("skip {}: {err}", path.display()),
            }
        }
    }
}
