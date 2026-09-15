//! Comparator and CV report for the WP6.1 / OF-17.3 noise spike (plan 0002 §3.3-§3.5).
//!
//! Reads every `*.json`/`*.jsonl` file under `argv[1]` (recursively — `gh run download` nests
//! artifacts in per-artifact folders), groups the lines by (bench, os, variant), and for each
//! (bench, os) with a `baseline` group:
//!
//! - the per-job value (K1: median of that job's wall-clock samples; K4: the job's single `Ir`);
//! - job-to-job spread of the baseline group: robust CV (`1.4826 * MAD / median`), classic CV
//!   (stdev/mean) and range/median, all in percent (plan §3.3 point 1);
//! - the noise band `B`: the largest symmetric log-ratio `|ln(v_i / v_j)|` between any two
//!   baseline jobs, expressed back as a percentage via `e^d - 1` (plan §3.3 point 2);
//! - for each regression variant, how many of its jobs the exact-integer gate
//!   ([`bench_noise::gate_decision`]) would flag red against the baseline median as the accepted
//!   basis (plan §3.3 point 3, §4.3);
//! - the plan's §3.5 classification (`Gate-fähig` / `... mit Unschärfe` / `Warn-fähig` /
//!   `Nur Trend`) per (bench, os, metric).
//!
//! Prints a Markdown report to stdout (pipe into `$GITHUB_STEP_SUMMARY`) and, if `argv[2]` is
//! given, also writes the full structured result as JSON.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::Path;

use bench_noise::{GateOutcome, gate_decision};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize)]
struct RawLine {
    os: String,
    rep: String,
    bench: String,
    variant: String,
    #[serde(default)]
    samples_ns: Vec<u128>,
    #[serde(default)]
    ir: Option<u64>,
}

/// One job's value for one metric: the median of its wall-clock samples, or its single `Ir`.
#[derive(Clone, Copy, Debug, Serialize)]
struct JobValue {
    rep: u32,
    value: f64,
}

#[derive(Serialize)]
struct MetricReport {
    bench: String,
    os: String,
    metric: &'static str,
    jobs: usize,
    baseline_median: f64,
    robust_cv_percent: f64,
    classic_cv_percent: f64,
    range_over_median_percent: f64,
    noise_band_b_percent: f64,
    warn_threshold_w_percent: f64,
    variants: Vec<VariantReport>,
    classification: &'static str,
}

#[derive(Serialize)]
struct VariantReport {
    variant: String,
    jobs: usize,
    red: usize,
    warn: usize,
    green: usize,
    errors: usize,
    per_job: Vec<JobOutcome>,
}

#[derive(Serialize)]
struct JobOutcome {
    rep: u32,
    ratio_percent: f64,
    outcome: &'static str,
}

fn main() {
    let mut args = env::args().skip(1);
    let input_dir = args.next().unwrap_or_else(|| "results".to_string());
    let output_json = args.next();

    let mut lines = Vec::new();
    collect_lines(Path::new(&input_dir), &mut lines);
    eprintln!("read {} measurement lines from {input_dir}", lines.len());

    // (bench, os, metric) -> variant name -> job values
    let mut grouped: BTreeMap<(String, String, &'static str), BTreeMap<String, Vec<JobValue>>> =
        BTreeMap::new();

    for line in &lines {
        let rep: u32 = line.rep.parse().unwrap_or(0);
        if !line.samples_ns.is_empty() {
            let mut samples: Vec<f64> = line.samples_ns.iter().map(|&n| n as f64).collect();
            let value = median(&mut samples);
            grouped
                .entry((line.bench.clone(), line.os.clone(), "wallclock_ns"))
                .or_default()
                .entry(line.variant.clone())
                .or_default()
                .push(JobValue { rep, value });
        }
        if let Some(ir) = line.ir {
            grouped
                .entry((line.bench.clone(), line.os.clone(), "instructions_ir"))
                .or_default()
                .entry(line.variant.clone())
                .or_default()
                .push(JobValue {
                    rep,
                    value: ir as f64,
                });
        }
    }

    let mut reports = Vec::new();
    for ((bench, os, metric), variants) in &grouped {
        let Some(baseline_jobs) = variants.get("baseline") else {
            continue;
        };
        if baseline_jobs.is_empty() {
            continue;
        }
        let mut baseline_values: Vec<f64> = baseline_jobs.iter().map(|j| j.value).collect();
        let baseline_median = median(&mut baseline_values);
        let mean = mean(&baseline_values);
        let mad = mad(&baseline_values, baseline_median);
        let robust_cv = if baseline_median > 0.0 {
            1.4826 * mad / baseline_median * 100.0
        } else {
            f64::NAN
        };
        let sd = stdev(&baseline_values, mean);
        let classic_cv = if mean > 0.0 {
            sd / mean * 100.0
        } else {
            f64::NAN
        };
        let (min, max) = min_max(&baseline_values);
        let range_over_median = if baseline_median > 0.0 {
            (max - min) / baseline_median * 100.0
        } else {
            f64::NAN
        };
        let noise_band_b = log_ratio_band(&baseline_values) * 100.0;
        let warn_w = if noise_band_b <= 1.0 {
            3.0
        } else {
            (3.0 * noise_band_b).min(9.0)
        };
        let warn_w_units = warn_w.round().max(0.0) as u32;

        let mut variant_reports = Vec::new();
        for (variant_name, jobs) in variants {
            if variant_name == "baseline" {
                continue;
            }
            let mut red = 0;
            let mut warn = 0;
            let mut green = 0;
            let mut errors = 0;
            let mut per_job = Vec::new();
            for job in jobs {
                let ratio_percent = (job.value / baseline_median - 1.0) * 100.0;
                let outcome =
                    match gate_decision(baseline_median as u64, job.value as u64, warn_w_units) {
                        Ok(GateOutcome::Red) => {
                            red += 1;
                            "red"
                        }
                        Ok(GateOutcome::Warn) => {
                            warn += 1;
                            "warn"
                        }
                        Ok(GateOutcome::Green) => {
                            green += 1;
                            "green"
                        }
                        Err(_) => {
                            errors += 1;
                            "error"
                        }
                    };
                per_job.push(JobOutcome {
                    rep: job.rep,
                    ratio_percent,
                    outcome,
                });
            }
            variant_reports.push(VariantReport {
                variant: variant_name.clone(),
                jobs: jobs.len(),
                red,
                warn,
                green,
                errors,
                per_job,
            });
        }

        let classification = classify(noise_band_b, &variant_reports);

        reports.push(MetricReport {
            bench: bench.clone(),
            os: os.clone(),
            metric,
            jobs: baseline_jobs.len(),
            baseline_median,
            robust_cv_percent: robust_cv,
            classic_cv_percent: classic_cv,
            range_over_median_percent: range_over_median,
            noise_band_b_percent: noise_band_b,
            warn_threshold_w_percent: warn_w,
            variants: variant_reports,
            classification,
        });
    }

    print_markdown(&reports);

    if let Some(path) = output_json {
        let json = serde_json::to_string_pretty(&reports).expect("serialize report");
        fs::write(&path, json).unwrap_or_else(|e| panic!("write {path}: {e}"));
    }
}

/// Plan 0002 WP6.1 §3.5 classification, restricted to what this trimmed spike measured: no
/// +15% variant, so "gate-fähig mit ausgewiesener Unschärfe" here only needs +12% to be optional
/// per the plan's own relaxation ("+12% darf fehlen").
fn classify(noise_band_b_percent: f64, variants: &[VariantReport]) -> &'static str {
    let find = |name: &str| variants.iter().find(|v| v.variant == name);
    let plus5_all_green = find("plus5").is_some_and(|v| v.jobs > 0 && v.red == 0);
    let plus12_all_red = find("plus12").is_some_and(|v| v.jobs > 0 && v.red == v.jobs);
    let plus20_all_red = find("plus20").is_some_and(|v| v.jobs > 0 && v.red == v.jobs);
    let plus20_most_red = find("plus20").is_some_and(|v| v.jobs > 0 && v.red * 10 >= v.jobs * 8);

    if noise_band_b_percent <= 1.0 && plus5_all_green && plus12_all_red && plus20_all_red {
        "Gate-fähig (Pilot)"
    } else if noise_band_b_percent <= 3.3 && plus5_all_green && plus20_all_red {
        "Gate-fähig mit ausgewiesener Unschärfe"
    } else if noise_band_b_percent <= 10.0 && plus20_most_red {
        "Warn-fähig"
    } else {
        "Nur Trend"
    }
}

fn collect_lines(dir: &Path, out: &mut Vec<RawLine>) {
    let Ok(entries) = fs::read_dir(dir) else {
        eprintln!("cannot read {}", dir.display());
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_lines(&path, out);
            continue;
        }
        let is_json_like = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e == "json" || e == "jsonl");
        if !is_json_like {
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
            match serde_json::from_str::<RawLine>(line) {
                Ok(record) => out.push(record),
                Err(err) => {
                    // Not every JSON file in the artifact tree is a measurement line (fingerprints,
                    // this tool's own report); only warn if it at least looked like an object.
                    if let Ok(Value::Object(_)) = serde_json::from_str::<Value>(line) {
                        eprintln!("skip {}: {err}", path.display());
                    }
                }
            }
        }
    }
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).expect("no NaN in measurements"));
    let n = values.len();
    if n == 0 {
        return f64::NAN;
    }
    if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

fn mad(values: &[f64], center: f64) -> f64 {
    let mut deviations: Vec<f64> = values.iter().map(|v| (v - center).abs()).collect();
    median(&mut deviations)
}

fn stdev(values: &[f64], mean: f64) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let variance =
        values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (values.len() - 1) as f64;
    variance.sqrt()
}

fn min_max(values: &[f64]) -> (f64, f64) {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for &v in values {
        min = min.min(v);
        max = max.max(v);
    }
    (min, max)
}

/// Largest symmetric log-ratio between any two values (plan §3.3 point 2): `d = |ln(x / y)|`,
/// returned as a fraction (multiply by 100 for percent, or take `e^d - 1` for the plan's percent
/// convention on asymmetric changes).
fn log_ratio_band(values: &[f64]) -> f64 {
    let mut max_d = 0.0f64;
    for (i, &a) in values.iter().enumerate() {
        for &b in &values[i + 1..] {
            if a <= 0.0 || b <= 0.0 {
                continue;
            }
            let d = (a / b).ln().abs();
            max_d = max_d.max(d);
        }
    }
    max_d.exp() - 1.0
}

fn print_markdown(reports: &[MetricReport]) {
    println!("## WP6.1 / OF-17.3 bench-noise spike: coefficient-of-variation report\n");
    println!(
        "| Bench | OS | Metric | Jobs | Basis (median) | Robust CV | Classic CV | Range/Median | Noise band B | w | Classification |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    for r in reports {
        println!(
            "| {} | {} | {} | {} | {:.3} | {:.2}% | {:.2}% | {:.2}% | {:.2}% | {:.1}% | {} |",
            r.bench,
            r.os,
            r.metric,
            r.jobs,
            r.baseline_median,
            r.robust_cv_percent,
            r.classic_cv_percent,
            r.range_over_median_percent,
            r.noise_band_b_percent,
            r.warn_threshold_w_percent,
            r.classification
        );
    }
    println!("\n### Injected-regression detection (candidate vs. baseline median)\n");
    println!("| Bench | OS | Metric | Variant | Jobs | Red (>10%) | Warn | Green | Errors |");
    println!("|---|---|---|---|---|---|---|---|---|");
    for r in reports {
        for v in &r.variants {
            println!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                r.bench, r.os, r.metric, v.variant, v.jobs, v.red, v.warn, v.green, v.errors
            );
        }
    }
}
