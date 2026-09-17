//! Wall-clock trend measurement (Plan-0002 WP6.2 scope item 2) for the two P0 baseline benches and
//! the Sigil -> Render extraction bench (`sigil_extract_10k`, Plan-0002 WP5.3).
//!
//! Wall clock is a **trend only** (engine ADR-0010: measured noise band 95-129% job-to-job on
//! shared Linux runners, "als scharfes Gate auf Linux ungeeignet") — this binary never feeds a
//! gate decision, it only emits `BenchResult` lines with `metric = "wall_time"` for the trend
//! branch (P-12). Sample count and warmup match the WP6.1 spike's own trimmed values
//! (`spikes/bench-noise/src/bin/wallclock.rs`: 2 warmup, 15 recorded), which already measured an
//! acceptably small effect from further warmup on these same two bench bodies.
//!
//! Usage: `wallclock --sha <40 hex> --os <os> --arch <arch> --logical-cpus <n> [--dirty]
//!   [--image <img>] [--cpu-model <model>] [--fingerprint k=v,...] [--run-id <id> --run-attempt <n>]`
//! Prints eight JSON Lines (`ecs_query_10k`, `sim_step_600`, `sigil_extract_10k`, `sigil_update_6k`,
//! `sigil_update_10k`, `sigil_churn_2k`, and since Plan 0002 WP3.6 `render_bullet_upload_10k` and
//! `render_light_cluster_256`, whose summed median per frame is printed against the 1.5 ms budget of
//! WP3.3) to stdout, and one human-readable budget line per Sigil
//! bench to stderr (median time per extraction against the plan's 0.5 ms budget, median time per
//! tick against the 1.0 ms budget of Plan 0002 WP5.4), so the CI log shows the numbers the plan
//! asks for without decoding JSON. The budget lines are trend output like everything here: a
//! millisecond budget is never a CI gate on shared runners (engine ADR-0010).

use std::collections::BTreeMap;
use std::process::ExitCode;
use std::time::Instant;

use grimoire_bench::scenarios::{
    BULLET_UPLOAD_SCENARIO, COLLIDE_BULLETS, COLLIDE_CLUSTER_SCENARIO, COLLIDE_ENEMIES,
    COLLIDE_TICK_BUDGET_MS, COLLIDE_UNIFORM_SCENARIO, COLLIDE_WALLCLOCK_TICKS, CollideLayout,
    ECS_ENTITIES, ECS_SCENARIO, ECS_WALLCLOCK_ROUNDS, EXTRACT_BULLETS, EXTRACT_SCENARIO,
    EXTRACT_WALLCLOCK_ROUNDS, LIGHT_CLUSTER_SCENARIO, RENDER_BULLETS, RENDER_POINT_LIGHTS,
    RENDER_UPLOAD_BUDGET_MS, RENDER_WALLCLOCK_FRAMES, SIGIL_CHURN_BULLETS, SIGIL_CHURN_PER_TICK,
    SIGIL_CHURN_SCENARIO, SIGIL_CHURN_WALLCLOCK_TICKS, SIGIL_ENTITIES, SIGIL_SCENARIO,
    SIGIL_TICK_BUDGET_MS, SIGIL_UPDATE_10K_BULLETS, SIGIL_UPDATE_10K_SCENARIO,
    SIGIL_UPDATE_10K_WALLCLOCK_TICKS, SIGIL_WALLCLOCK_TICKS, SIM_ENTITIES, SIM_SCENARIO,
    SIM_WALLCLOCK_TICKS, build_collide, build_ecs_world, build_render_cpu, build_sigil_churn,
    build_sigil_extract, build_sigil_update, build_sigil_update_10k, build_sim,
    run_bullet_upload_frames, run_collide_ticks, run_ecs_rounds, run_light_cluster_frames,
    run_sigil_extract_rounds, run_sigil_update_ticks, run_sim_ticks, sigil_update_fill_ticks,
};
use grimoire_bench::schema::{
    BenchResult, CommitRef, ExecutorInfo, ParamValue, RunKey, RunnerInfo, ValueOrigin, median,
};
use grimoire_ecs::{Executor, SequentialExecutor};
use grimoire_exec::ThreadPoolExecutor;
use grimoire_sim::Simulation;

/// Unmeasured samples discarded before recording (matches the WP6.1 spike's trimmed warmup).
const WARMUP_SAMPLES: u32 = 2;
/// Recorded samples per bench (matches the WP6.1 spike's trimmed sample count).
const SAMPLES: u32 = 15;

struct Meta {
    sha: String,
    dirty: bool,
    os: String,
    arch: String,
    image: Option<String>,
    cpu_model: Option<String>,
    logical_cpus: u32,
    fingerprint: BTreeMap<String, String>,
    run: Option<RunKey>,
}

fn parse_meta() -> Result<Meta, String> {
    let mut sha = None;
    let mut dirty = false;
    let mut os = None;
    let mut arch = None;
    let mut image = None;
    let mut cpu_model = None;
    let mut logical_cpus = None;
    let mut fingerprint = BTreeMap::new();
    let mut run_id = None;
    let mut run_attempt = None;

    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut next = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--sha" => sha = Some(next()?),
            "--dirty" => dirty = true,
            "--os" => os = Some(next()?),
            "--arch" => arch = Some(next()?),
            "--image" => image = Some(next()?),
            "--cpu-model" => cpu_model = Some(next()?),
            "--logical-cpus" => {
                logical_cpus = Some(
                    next()?
                        .parse()
                        .map_err(|e| format!("invalid --logical-cpus: {e}"))?,
                );
            }
            "--fingerprint" => {
                fingerprint = next()?
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .filter_map(|pair| pair.split_once('='))
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
            }
            "--run-id" => run_id = Some(next()?),
            "--run-attempt" => {
                run_attempt = Some(
                    next()?
                        .parse()
                        .map_err(|e| format!("invalid --run-attempt: {e}"))?,
                );
            }
            other => return Err(format!("unknown flag {other:?}")),
        }
    }
    let run = match (run_id, run_attempt) {
        (Some(id), Some(attempt)) => Some(RunKey { id, attempt }),
        (None, None) => None,
        _ => return Err("--run-id and --run-attempt must be given together".to_string()),
    };
    Ok(Meta {
        sha: sha.ok_or("--sha is required")?,
        dirty,
        os: os.ok_or("--os is required")?,
        arch: arch.ok_or("--arch is required")?,
        image,
        cpu_model,
        logical_cpus: logical_cpus.ok_or("--logical-cpus is required")?,
        fingerprint,
        run,
    })
}

fn measure_ecs() -> Vec<f64> {
    let mut world = build_ecs_world();
    for _ in 0..WARMUP_SAMPLES {
        run_ecs_rounds(&mut world, ECS_WALLCLOCK_ROUNDS, 0);
    }
    let mut samples = Vec::with_capacity(SAMPLES as usize);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        run_ecs_rounds(&mut world, ECS_WALLCLOCK_ROUNDS, 0);
        samples.push(start.elapsed().as_nanos() as f64);
    }
    samples
}

fn measure_sim() -> Vec<f64> {
    let mut sim = build_sim(0xB5_11_C1_0C_C0_FF_EE_00);
    for _ in 0..WARMUP_SAMPLES {
        run_sim_ticks(&mut sim, SIM_WALLCLOCK_TICKS, 0);
    }
    let mut samples = Vec::with_capacity(SAMPLES as usize);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        run_sim_ticks(&mut sim, SIM_WALLCLOCK_TICKS, 0);
        samples.push(start.elapsed().as_nanos() as f64);
    }
    samples
}

fn measure_extract() -> Vec<f64> {
    let mut bench = build_sigil_extract(0xB5_11_C1_0C_C0_FF_EE_00);
    for _ in 0..WARMUP_SAMPLES {
        run_sigil_extract_rounds(&mut bench, EXTRACT_WALLCLOCK_ROUNDS, 0);
    }
    let mut samples = Vec::with_capacity(SAMPLES as usize);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        run_sigil_extract_rounds(&mut bench, EXTRACT_WALLCLOCK_ROUNDS, 0);
        samples.push(start.elapsed().as_nanos() as f64);
    }
    samples
}

/// Samples `frames` preparations of one render CPU bench body, after the usual warm-up.
fn measure_render(
    run: fn(&mut grimoire_bench::scenarios::RenderCpuBench, u32, u32) -> usize,
) -> Vec<f64> {
    let mut bench = build_render_cpu();
    for _ in 0..WARMUP_SAMPLES {
        run(&mut bench, RENDER_WALLCLOCK_FRAMES, 0);
    }
    let mut samples = Vec::with_capacity(SAMPLES as usize);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        run(&mut bench, RENDER_WALLCLOCK_FRAMES, 0);
        samples.push(start.elapsed().as_nanos() as f64);
    }
    samples
}

/// Samples `ticks` simulation steps of an already built, steady `sim`, after the usual warm-up.
fn measure_collide(layout: CollideLayout, executor: &dyn Executor) -> Vec<f64> {
    let mut bench = build_collide(layout);
    for _ in 0..WARMUP_SAMPLES {
        run_collide_ticks(&mut bench, executor, COLLIDE_WALLCLOCK_TICKS, 0);
    }
    let mut samples = Vec::with_capacity(SAMPLES as usize);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        run_collide_ticks(&mut bench, executor, COLLIDE_WALLCLOCK_TICKS, 0);
        samples.push(start.elapsed().as_nanos() as f64);
    }
    samples
}

fn measure_sigil_ticks(mut sim: Simulation, ticks: u32) -> Vec<f64> {
    for _ in 0..WARMUP_SAMPLES {
        run_sigil_update_ticks(&mut sim, ticks, 0);
    }
    let mut samples = Vec::with_capacity(SAMPLES as usize);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        run_sigil_update_ticks(&mut sim, ticks, 0);
        samples.push(start.elapsed().as_nanos() as f64);
    }
    samples
}

fn result_for(
    meta: &Meta,
    scenario: &str,
    samples: Vec<f64>,
    params: BTreeMap<String, ParamValue>,
) -> BenchResult {
    result_on(meta, scenario, samples, params, &SequentialExecutor)
}

/// Like [`result_for`], recording `executor`: `sequential` for one thread, `thread_pool` with its
/// thread count otherwise (contract §15.1).
fn result_on(
    meta: &Meta,
    scenario: &str,
    samples: Vec<f64>,
    params: BTreeMap<String, ParamValue>,
    executor: &dyn Executor,
) -> BenchResult {
    let threads = executor.threads();
    let computed_median = median(&samples).unwrap_or(0.0);
    BenchResult {
        scenario: scenario.to_string(),
        metric: "wall_time".to_string(),
        unit: "ns".to_string(),
        median: computed_median,
        samples,
        commit: CommitRef {
            sha: meta.sha.clone(),
            dirty: meta.dirty,
        },
        runner: RunnerInfo {
            os: meta.os.clone(),
            arch: meta.arch.clone(),
            image: meta.image.clone(),
            cpu_model: meta.cpu_model.clone(),
            logical_cpus: meta.logical_cpus,
            fingerprint: meta.fingerprint.clone(),
        },
        executor: ExecutorInfo {
            kind: if threads == 1 {
                "sequential"
            } else {
                "thread_pool"
            }
            .to_string(),
            threads: threads as u32,
        },
        params,
        value_origin: ValueOrigin::Runner,
        injected_regression_percent: 0,
        run: meta.run.clone(),
    }
}

fn main() -> ExitCode {
    let meta = match parse_meta() {
        Ok(meta) => meta,
        Err(err) => {
            eprintln!("wallclock: {err}");
            return ExitCode::FAILURE;
        }
    };

    let ecs_params = BTreeMap::from([
        ("entities".to_string(), ParamValue::Int(ECS_ENTITIES as i64)),
        (
            "rounds".to_string(),
            ParamValue::Int(i64::from(ECS_WALLCLOCK_ROUNDS)),
        ),
    ]);
    let sim_params = BTreeMap::from([
        ("entities".to_string(), ParamValue::Int(SIM_ENTITIES as i64)),
        (
            "ticks".to_string(),
            ParamValue::Int(i64::from(SIM_WALLCLOCK_TICKS)),
        ),
    ]);

    let extract_params = BTreeMap::from([
        (
            "bullets".to_string(),
            ParamValue::Int(i64::from(EXTRACT_BULLETS)),
        ),
        (
            "rounds".to_string(),
            ParamValue::Int(i64::from(EXTRACT_WALLCLOCK_ROUNDS)),
        ),
    ]);

    let int = |value: u32| ParamValue::Int(i64::from(value));
    let update_6k_params = BTreeMap::from([
        ("bullets".to_string(), int(SIGIL_ENTITIES)),
        ("ticks".to_string(), int(SIGIL_WALLCLOCK_TICKS)),
    ]);
    let update_10k_params = BTreeMap::from([
        ("bullets".to_string(), int(SIGIL_UPDATE_10K_BULLETS)),
        ("ticks".to_string(), int(SIGIL_UPDATE_10K_WALLCLOCK_TICKS)),
    ]);
    let churn_params = BTreeMap::from([
        ("bullets".to_string(), int(SIGIL_CHURN_BULLETS)),
        ("per_tick".to_string(), int(SIGIL_CHURN_PER_TICK)),
        ("ticks".to_string(), int(SIGIL_CHURN_WALLCLOCK_TICKS)),
    ]);

    let update_6k = {
        let mut sim = build_sigil_update(0xB5_11_C1_0C_C0_FF_EE_00);
        run_sigil_update_ticks(&mut sim, sigil_update_fill_ticks(), 0);
        measure_sigil_ticks(sim, SIGIL_WALLCLOCK_TICKS)
    };
    let update_10k = measure_sigil_ticks(
        build_sigil_update_10k(0xB5_11_C1_0C_C0_FF_EE_00),
        SIGIL_UPDATE_10K_WALLCLOCK_TICKS,
    );
    let churn = measure_sigil_ticks(
        build_sigil_churn(0xB5_11_C1_0C_C0_FF_EE_00),
        SIGIL_CHURN_WALLCLOCK_TICKS,
    );

    let results = [
        result_for(&meta, ECS_SCENARIO, measure_ecs(), ecs_params),
        result_for(&meta, SIM_SCENARIO, measure_sim(), sim_params),
        result_for(&meta, EXTRACT_SCENARIO, measure_extract(), extract_params),
        result_for(&meta, SIGIL_SCENARIO, update_6k, update_6k_params),
        result_for(
            &meta,
            SIGIL_UPDATE_10K_SCENARIO,
            update_10k,
            update_10k_params,
        ),
        result_for(&meta, SIGIL_CHURN_SCENARIO, churn, churn_params),
        result_for(
            &meta,
            BULLET_UPLOAD_SCENARIO,
            measure_render(run_bullet_upload_frames),
            BTreeMap::from([
                ("bullets".to_string(), int(RENDER_BULLETS)),
                ("frames".to_string(), int(RENDER_WALLCLOCK_FRAMES)),
            ]),
        ),
        result_for(
            &meta,
            LIGHT_CLUSTER_SCENARIO,
            measure_render(run_light_cluster_frames),
            BTreeMap::from([
                ("lights".to_string(), int(RENDER_POINT_LIGHTS)),
                ("bullets".to_string(), int(RENDER_BULLETS)),
                ("frames".to_string(), int(RENDER_WALLCLOCK_FRAMES)),
            ]),
        ),
    ];
    // Plan 0002 WP5.3 and WP5.4 budgets, as trend lines for the log (never a gate, engine
    // ADR-0010).
    let per_extraction_ms = results[2].median / f64::from(EXTRACT_WALLCLOCK_ROUNDS) / 1.0e6;
    eprintln!(
        "{EXTRACT_SCENARIO}: median {per_extraction_ms:.4} ms per extraction of {EXTRACT_BULLETS} bullets (plan budget 0.5 ms, wall clock, trend only)"
    );
    for (result, ticks, what) in [
        (
            &results[3],
            SIGIL_WALLCLOCK_TICKS,
            "6,000 active bullets, six stacked modifiers",
        ),
        (
            &results[4],
            SIGIL_UPDATE_10K_WALLCLOCK_TICKS,
            "10,000 active bullets",
        ),
        (
            &results[5],
            SIGIL_CHURN_WALLCLOCK_TICKS,
            "10,000 active bullets, 2,000 spawns and 2,000 despawns per tick",
        ),
    ] {
        let per_tick_ms = result.median / f64::from(ticks) / 1.0e6;
        let verdict = if per_tick_ms <= SIGIL_TICK_BUDGET_MS {
            "within"
        } else {
            "OVER"
        };
        eprintln!(
            "{}: median {per_tick_ms:.4} ms per tick, {what}, sequential (budget {SIGIL_TICK_BUDGET_MS:.1} ms: {verdict}; wall clock, trend only)",
            result.scenario
        );
    }

    // Plan 0002 WP3.3/WP3.6: bullet upload plus clustering against the render-CPU share of 1.5 ms.
    let per_frame_ms =
        |result: &BenchResult| result.median / f64::from(RENDER_WALLCLOCK_FRAMES) / 1.0e6;
    let (upload_ms, cluster_ms) = (per_frame_ms(&results[6]), per_frame_ms(&results[7]));
    let render_ms = upload_ms + cluster_ms;
    let verdict = if render_ms <= RENDER_UPLOAD_BUDGET_MS {
        "within"
    } else {
        "OVER"
    };
    eprintln!(
        "{BULLET_UPLOAD_SCENARIO} + {LIGHT_CLUSTER_SCENARIO}: median {upload_ms:.4} + {cluster_ms:.4} = {render_ms:.4} ms per frame, {RENDER_BULLETS} bullets and {RENDER_POINT_LIGHTS} lights (budget {RENDER_UPLOAD_BUDGET_MS:.1} ms: {verdict}; wall clock, trend only)"
    );

    // Plan 0002 WP6.5 (contract §14): both collision benches on one thread and on N threads, against
    // the collision budget of 1.5 ms per tick.
    let pool_threads = std::thread::available_parallelism()
        .map_or(2, std::num::NonZeroUsize::get)
        .max(2);
    let pool = match ThreadPoolExecutor::new(pool_threads) {
        Ok(pool) => pool,
        Err(err) => {
            eprintln!("wallclock: thread pool with {pool_threads} threads: {err}");
            return ExitCode::FAILURE;
        }
    };
    let collide_params = BTreeMap::from([
        ("bullets".to_string(), int(COLLIDE_BULLETS)),
        ("enemies".to_string(), int(COLLIDE_ENEMIES)),
        ("ticks".to_string(), int(COLLIDE_WALLCLOCK_TICKS)),
    ]);
    let mut collide_results = Vec::new();
    for (scenario, layout) in [
        (COLLIDE_UNIFORM_SCENARIO, CollideLayout::Uniform),
        (COLLIDE_CLUSTER_SCENARIO, CollideLayout::Cluster),
    ] {
        for executor in [&SequentialExecutor as &dyn Executor, &pool] {
            let result = result_on(
                &meta,
                scenario,
                measure_collide(layout, executor),
                collide_params.clone(),
                executor,
            );
            let per_tick_ms = result.median / f64::from(COLLIDE_WALLCLOCK_TICKS) / 1.0e6;
            let verdict = if per_tick_ms <= COLLIDE_TICK_BUDGET_MS {
                "within"
            } else {
                "OVER"
            };
            eprintln!(
                "{scenario}: median {per_tick_ms:.4} ms per tick, {COLLIDE_BULLETS} bullets and {COLLIDE_ENEMIES} enemies, {} thread(s) (budget {COLLIDE_TICK_BUDGET_MS:.1} ms: {verdict}; wall clock, trend only)",
                executor.threads()
            );
            collide_results.push(result);
        }
    }

    for result in results.iter().chain(&collide_results) {
        match result.to_json_line() {
            Ok(line) => println!("{line}"),
            Err(err) => {
                eprintln!("wallclock: refused to write an invalid line: {err}");
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}
