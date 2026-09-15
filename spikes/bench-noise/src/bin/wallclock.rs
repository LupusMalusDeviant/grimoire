//! K1 wall-clock harness for the WP6.1 / OF-17.3 noise spike (plan 0002 §3.2 step 7).
//!
//! Prints one JSON line per (bench, variant) to stdout and, if given, appends the same lines to
//! the file named in `argv[1]`. Metadata (`os`, `rep`, `sha`) comes from environment variables so
//! the CI workflow needs no templating inside this binary.
//!
//! Wall-clock time only measures these benchmarks; it never feeds simulation state
//! (`#[allow(clippy::disallowed_methods)]` is not needed here: this crate has no such lint).

use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::time::Instant;

use bench_noise::{
    ECS_WALLCLOCK_ROUNDS, SIM_WALLCLOCK_TICKS, VARIANTS, build_ecs_world, build_sim, extra_units,
    run_ecs_rounds, run_sim_ticks,
};
use serde::Serialize;

/// Unmeasured samples discarded before recording (cheap warmup, not the plan's 3 s: this spike
/// trims warmup to keep the ten-repetition matrix affordable, see the spike README).
const WARMUP_SAMPLES: u32 = 2;
/// Recorded samples per (bench, variant) (plan §3.2 uses 30; trimmed to 15, see README).
const SAMPLES: u32 = 15;

#[derive(Serialize)]
struct SampleRecord {
    os: String,
    rep: String,
    sha: String,
    bench: &'static str,
    variant: &'static str,
    percent: u32,
    base_units: u32,
    extra_units: u32,
    samples_ns: Vec<u128>,
}

fn env_or(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

fn measure_ecs(variant_percent: u32) -> (u32, u32, Vec<u128>) {
    let extra = extra_units(ECS_WALLCLOCK_ROUNDS, variant_percent);
    let mut world = build_ecs_world();
    for _ in 0..WARMUP_SAMPLES {
        run_ecs_rounds(&mut world, ECS_WALLCLOCK_ROUNDS, extra);
    }
    let mut samples = Vec::with_capacity(SAMPLES as usize);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        run_ecs_rounds(&mut world, ECS_WALLCLOCK_ROUNDS, extra);
        samples.push(start.elapsed().as_nanos());
    }
    (ECS_WALLCLOCK_ROUNDS, extra, samples)
}

fn measure_sim(variant_percent: u32) -> (u32, u32, Vec<u128>) {
    let extra = extra_units(SIM_WALLCLOCK_TICKS, variant_percent);
    let mut sim = build_sim(0xB5_11_C1_0C_C0_FF_EE_00);
    for _ in 0..WARMUP_SAMPLES {
        run_sim_ticks(&mut sim, SIM_WALLCLOCK_TICKS, extra);
    }
    let mut samples = Vec::with_capacity(SAMPLES as usize);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        run_sim_ticks(&mut sim, SIM_WALLCLOCK_TICKS, extra);
        samples.push(start.elapsed().as_nanos());
    }
    (SIM_WALLCLOCK_TICKS, extra, samples)
}

fn main() {
    let out_path = env::args().nth(1);
    let mut out_file = out_path.map(|path| {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("open wallclock output file")
    });

    let os = env_or("RUNNER_OS", "unknown");
    let rep = env_or("BENCH_NOISE_REP", "0");
    let sha = env_or("GITHUB_SHA", "unknown");

    for variant in VARIANTS {
        let (base, extra, samples) = measure_ecs(variant.percent);
        let record = SampleRecord {
            os: os.clone(),
            rep: rep.clone(),
            sha: sha.clone(),
            bench: "ecs_query_10k",
            variant: variant.name,
            percent: variant.percent,
            base_units: base,
            extra_units: extra,
            samples_ns: samples,
        };
        emit(&record, out_file.as_mut());
    }

    for variant in VARIANTS {
        let (base, extra, samples) = measure_sim(variant.percent);
        let record = SampleRecord {
            os: os.clone(),
            rep: rep.clone(),
            sha: sha.clone(),
            bench: "sim_step_600",
            variant: variant.name,
            percent: variant.percent,
            base_units: base,
            extra_units: extra,
            samples_ns: samples,
        };
        emit(&record, out_file.as_mut());
    }
}

fn emit(record: &SampleRecord, file: Option<&mut std::fs::File>) {
    let line = serde_json::to_string(record).expect("serialize sample record");
    println!("{line}");
    if let Some(file) = file {
        writeln!(file, "{line}").expect("append sample record");
    }
}
