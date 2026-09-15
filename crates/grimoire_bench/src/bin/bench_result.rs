//! Builds one contract-§15.1-compliant `BenchResult` JSON Lines line from CLI arguments and
//! prints it to stdout.
//!
//! Exists so that `scripts/measure_ir.sh` and `scripts/calibrate_injection.sh` — which must stay
//! in bash because they drive `valgrind --tool=callgrind` and read its `summary:` line (proven
//! approach from the WP6.1 spike, kept per engine ADR-0010's WP6.2 addendum) — never hand-format
//! JSON themselves. The spike's own `measure_ir.sh` used a `printf` template for its throwaway,
//! non-contract JSON; that is fine for a spike but would silently drift from contract §15.1 here
//! (wrong hex width, wrong key order, no size limits, no NaN check, ...). Routing every line
//! through [`grimoire_bench::schema::BenchResult::to_json_line`] means the one schema
//! implementation is also the one place that can produce an invalid line, and it refuses to.
//!
//! Usage (flags may repeat where noted; `key=value` pairs are comma-separated):
//! ```text
//! bench_result --scenario ecs_query_10k --metric instructions --unit ir \
//!   --samples 48211377 --sha <40 hex> [--dirty] \
//!   --os linux --arch x86_64 [--image ubuntu-24.04] [--cpu-model "..."] \
//!   --logical-cpus 4 [--fingerprint valgrind=3.22.0,...] \
//!   --executor-kind sequential --executor-threads 1 \
//!   [--params entities=10000,rounds=1] \
//!   --value-origin runner [--injected-regression-percent 0] \
//!   [--run-id <id> --run-attempt <n>]
//! ```

use std::collections::BTreeMap;
use std::process::ExitCode;

use grimoire_bench::schema::{
    BenchResult, CommitRef, ExecutorInfo, ParamValue, RunKey, RunnerInfo, ValueOrigin, median,
};

struct Args {
    scenario: Option<String>,
    metric: Option<String>,
    unit: Option<String>,
    samples: Vec<f64>,
    sha: Option<String>,
    dirty: bool,
    os: Option<String>,
    arch: Option<String>,
    image: Option<String>,
    cpu_model: Option<String>,
    logical_cpus: Option<u32>,
    fingerprint: BTreeMap<String, String>,
    executor_kind: Option<String>,
    executor_threads: Option<u32>,
    params: BTreeMap<String, ParamValue>,
    value_origin: Option<String>,
    injected_regression_percent: u32,
    run_id: Option<String>,
    run_attempt: Option<u32>,
}

fn parse_pairs(text: &str) -> BTreeMap<String, String> {
    text.split(',')
        .filter(|s| !s.is_empty())
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        scenario: None,
        metric: None,
        unit: None,
        samples: Vec::new(),
        sha: None,
        dirty: false,
        os: None,
        arch: None,
        image: None,
        cpu_model: None,
        logical_cpus: None,
        fingerprint: BTreeMap::new(),
        executor_kind: None,
        executor_threads: None,
        params: BTreeMap::new(),
        value_origin: None,
        injected_regression_percent: 0,
        run_id: None,
        run_attempt: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut next = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--scenario" => args.scenario = Some(next()?),
            "--metric" => args.metric = Some(next()?),
            "--unit" => args.unit = Some(next()?),
            "--samples" => {
                let raw = next()?;
                for part in raw.split(',') {
                    args.samples.push(
                        part.trim()
                            .parse::<f64>()
                            .map_err(|e| format!("invalid --samples entry {part:?}: {e}"))?,
                    );
                }
            }
            "--sha" => args.sha = Some(next()?),
            "--dirty" => args.dirty = true,
            "--os" => args.os = Some(next()?),
            "--arch" => args.arch = Some(next()?),
            "--image" => args.image = Some(next()?),
            "--cpu-model" => args.cpu_model = Some(next()?),
            "--logical-cpus" => {
                args.logical_cpus = Some(
                    next()?
                        .parse()
                        .map_err(|e| format!("invalid --logical-cpus: {e}"))?,
                );
            }
            "--fingerprint" => args.fingerprint = parse_pairs(&next()?),
            "--executor-kind" => args.executor_kind = Some(next()?),
            "--executor-threads" => {
                args.executor_threads = Some(
                    next()?
                        .parse()
                        .map_err(|e| format!("invalid --executor-threads: {e}"))?,
                );
            }
            "--params" => {
                for (k, v) in parse_pairs(&next()?) {
                    let value = v
                        .parse::<i64>()
                        .map_or_else(|_| ParamValue::Text(v.clone()), ParamValue::Int);
                    args.params.insert(k, value);
                }
            }
            "--value-origin" => args.value_origin = Some(next()?),
            "--injected-regression-percent" => {
                args.injected_regression_percent = next()?
                    .parse()
                    .map_err(|e| format!("invalid --injected-regression-percent: {e}"))?;
            }
            "--run-id" => args.run_id = Some(next()?),
            "--run-attempt" => {
                args.run_attempt = Some(
                    next()?
                        .parse()
                        .map_err(|e| format!("invalid --run-attempt: {e}"))?,
                );
            }
            other => return Err(format!("unknown flag {other:?}")),
        }
    }
    Ok(args)
}

fn build(args: Args) -> Result<BenchResult, String> {
    let samples = args.samples;
    let computed_median = median(&samples).ok_or("--samples must have at least one entry")?;
    let value_origin = match args.value_origin.as_deref() {
        Some("runner") | None => ValueOrigin::Runner,
        Some("reference") => ValueOrigin::Reference,
        Some("estimate") => ValueOrigin::Estimate,
        Some(other) => return Err(format!("unknown --value-origin {other:?}")),
    };
    let run = match (args.run_id, args.run_attempt) {
        (Some(id), Some(attempt)) => Some(RunKey { id, attempt }),
        (None, None) => None,
        _ => return Err("--run-id and --run-attempt must be given together".to_string()),
    };
    Ok(BenchResult {
        scenario: args.scenario.ok_or("--scenario is required")?,
        metric: args.metric.ok_or("--metric is required")?,
        unit: args.unit.ok_or("--unit is required")?,
        median: computed_median,
        samples,
        commit: CommitRef {
            sha: args.sha.ok_or("--sha is required")?,
            dirty: args.dirty,
        },
        runner: RunnerInfo {
            os: args.os.ok_or("--os is required")?,
            arch: args.arch.ok_or("--arch is required")?,
            image: args.image,
            cpu_model: args.cpu_model,
            logical_cpus: args.logical_cpus.ok_or("--logical-cpus is required")?,
            fingerprint: args.fingerprint,
        },
        executor: ExecutorInfo {
            kind: args.executor_kind.ok_or("--executor-kind is required")?,
            threads: args
                .executor_threads
                .ok_or("--executor-threads is required")?,
        },
        params: args.params,
        value_origin,
        injected_regression_percent: args.injected_regression_percent,
        run,
    })
}

fn main() -> ExitCode {
    let parsed = match parse_args().and_then(build) {
        Ok(result) => result,
        Err(err) => {
            eprintln!("bench_result: {err}");
            return ExitCode::FAILURE;
        }
    };
    match parsed.to_json_line() {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("bench_result: refused to write an invalid line: {err}");
            ExitCode::FAILURE
        }
    }
}
