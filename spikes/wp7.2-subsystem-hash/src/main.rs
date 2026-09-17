//! Spike plan 0002 WP7.2 / OF-18.1: what do subsystem hashes cost, per system per tick versus
//! every N ticks, at 10,000 bullets?
//!
//! Measures the real implementation (`grimoire_sim::trace`, `SystemHasher`) on three scenarios:
//!
//! | Scenario | State | Systems |
//! |---|---|---|
//! | `sigil10k` | `grimoire_bench`'s `sigil_update_10k`: 10,000 active bullets with transforms | 5 `sigil.*` |
//! | `sigilchurn` | `sigil_churn_2k`: 10,000 active, 2,000 spawned and despawned per tick | 5 `sigil.*` |
//! | `parallel12k` | the parallel golden scenario of `grimoire_sim`: 12,000 entities, parallel stages | 8 |
//!
//! Each scenario runs [`MEASURED_TICKS`] ticks in one of these modes:
//!
//! - `build`: only the setup, subtracted from every other mode;
//! - `step`: plain `Simulation::step`;
//! - `noop`: `step_observed` with `NoopObserver`, the observer dispatch alone;
//! - `state_hash`: no step, one `state_hash` per tick, the cost of one world hash;
//! - `every_1`, `every_60`: `trace` with the state hash every tick or every 60 ticks;
//! - `per_system`: `trace` with `PER_SYSTEM_PER_TICK`;
//! - `every_60_systems`: `trace` every 60 ticks with system hashes at those ticks.
//!
//! Usage (the numbers only come from CI runners, engine ADR-0010):
//!
//! ```text
//! subsystem-hash-cost probe <scenario> <mode>   # one run, measured under Callgrind by scripts/measure.sh
//! subsystem-hash-cost params <scenario>         # systems, population, measured ticks
//! subsystem-hash-cost wallclock                 # median milliseconds per tick, trend only
//! subsystem-hash-cost check                     # correctness: traces match plain runs
//! ```

use std::hint::black_box;
use std::num::NonZeroU64;
use std::process::ExitCode;
use std::time::Instant;

use grimoire_bench::scenarios::{build_sigil_churn, build_sigil_update_10k};
use grimoire_ecs::{NoopObserver, StageMode};
use grimoire_sigil::BulletPool;
use grimoire_sim::{
    InputLog, Simulation, TickInput, TraceGranularity, first_divergence, replay, trace,
};

#[path = "../../../crates/grimoire_sim/tests/parallel_scenario/mod.rs"]
#[allow(dead_code, clippy::all)]
mod parallel_scenario;

/// Ticks every mode runs after the setup.
const MEASURED_TICKS: usize = 100;
/// Wall-clock repetitions per mode; the median is reported.
const WALLCLOCK_REPEATS: usize = 7;
/// Seed of the two Sigil scenarios, the one `grimoire_bench`'s probes use.
const SIGIL_SEED: u64 = 0xB5_11_C1_0C_C0_FF_EE_00;

const SCENARIOS: [&str; 3] = ["sigil10k", "sigilchurn", "parallel12k"];
const MODES: [&str; 7] = [
    "step",
    "noop",
    "state_hash",
    "every_1",
    "every_60",
    "per_system",
    "every_60_systems",
];

fn every(interval: u64) -> TraceGranularity {
    TraceGranularity::every(NonZeroU64::new(interval).expect("interval is not zero"))
}

fn build(scenario: &str) -> Option<Simulation> {
    match scenario {
        "sigil10k" => Some(build_sigil_update_10k(SIGIL_SEED)),
        "sigilchurn" => Some(build_sigil_churn(SIGIL_SEED)),
        "parallel12k" => Some(parallel_scenario::build_simulation(
            parallel_scenario::SEED,
            StageMode::Grouped,
        )),
        _ => None,
    }
}

fn log_for(sim: &Simulation, ticks: usize) -> InputLog {
    InputLog {
        seed: sim.seed(),
        tick_rate_hz: 60,
        frames: vec![TickInput::default(); ticks],
    }
}

/// Runs `mode` for `ticks` ticks on `sim`; `false` for an unknown mode.
fn run(sim: &mut Simulation, mode: &str, ticks: usize) -> bool {
    let log = log_for(sim, ticks);
    match mode {
        "build" => {}
        "step" => {
            for &input in &log.frames {
                sim.step(input);
            }
        }
        "noop" => {
            for &input in &log.frames {
                sim.step_observed(input, &mut NoopObserver);
            }
        }
        "state_hash" => {
            for _ in 0..ticks {
                black_box(sim.state_hash());
            }
        }
        "every_1" => {
            black_box(trace(sim, &log, every(1)));
        }
        "every_60" => {
            black_box(trace(sim, &log, every(60)));
        }
        "per_system" => {
            black_box(trace(sim, &log, TraceGranularity::PER_SYSTEM_PER_TICK));
        }
        "every_60_systems" => {
            black_box(trace(sim, &log, every(60).with_system_hashes()));
        }
        _ => return false,
    }
    black_box(sim.tick());
    true
}

fn system_count(sim: &mut Simulation) -> usize {
    sim.schedule_mut().system_names().len()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["probe", scenario, mode] => {
            let Some(mut sim) = build(scenario) else {
                eprintln!("unknown scenario {scenario:?}, expected one of {SCENARIOS:?}");
                return ExitCode::FAILURE;
            };
            if !run(&mut sim, mode, MEASURED_TICKS) {
                eprintln!("unknown mode {mode:?}, expected build or one of {MODES:?}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        ["params", scenario] => {
            let Some(mut sim) = build(scenario) else {
                eprintln!("unknown scenario {scenario:?}, expected one of {SCENARIOS:?}");
                return ExitCode::FAILURE;
            };
            let entities = sim.world().entity_count();
            let bullets = sim
                .world()
                .resource::<BulletPool>()
                .map_or(0, BulletPool::len);
            println!(
                "systems={} entities={entities} bullets={bullets} ticks={MEASURED_TICKS}",
                system_count(&mut sim)
            );
            ExitCode::SUCCESS
        }
        ["wallclock"] => {
            wallclock();
            ExitCode::SUCCESS
        }
        ["check"] => check(),
        _ => {
            eprintln!(
                "usage: subsystem-hash-cost probe <scenario> <build|mode> | params <scenario> | wallclock | check"
            );
            ExitCode::FAILURE
        }
    }
}

/// Median milliseconds per tick of every mode, [`WALLCLOCK_REPEATS`] fresh runs each. A trend on a
/// shared runner, never a budget proof (engine ADR-0010).
fn wallclock() {
    println!("| Scenario | Mode | Median ms/tick | Min | Max | vs. step |");
    println!("|---|---|---:|---:|---:|---:|");
    for scenario in SCENARIOS {
        let mut step_median = 0.0;
        for mode in MODES {
            let mut samples: Vec<f64> = (0..WALLCLOCK_REPEATS)
                .map(|_| {
                    let mut sim = build(scenario).expect("known scenario");
                    let started = Instant::now();
                    run(&mut sim, mode, MEASURED_TICKS);
                    started.elapsed().as_secs_f64() * 1_000.0 / MEASURED_TICKS as f64
                })
                .collect();
            samples.sort_by(f64::total_cmp);
            let median = samples[samples.len() / 2];
            if mode == "step" {
                step_median = median;
            }
            println!(
                "| {scenario} | {mode} | {median:.4} | {:.4} | {:.4} | {:.2}x |",
                samples[0],
                samples[samples.len() - 1],
                median / step_median
            );
        }
    }
}

/// Every traced mode reaches exactly the state hashes of a plain run, and the per-system trace of
/// a second build matches the first.
fn check() -> ExitCode {
    const TICKS: usize = 5;
    for scenario in SCENARIOS {
        let mut plain = build(scenario).expect("known scenario");
        let log = log_for(&plain, TICKS);
        let expected = replay(&mut plain, &log, 1);
        let mut traces = Vec::new();
        for granularity in [
            every(1),
            TraceGranularity::PER_SYSTEM_PER_TICK,
            every(2).with_system_hashes(),
        ] {
            let mut sim = build(scenario).expect("known scenario");
            let traced = trace(&mut sim, &log, granularity);
            for checkpoint in &traced.checkpoints[1..] {
                let hash = expected
                    .iter()
                    .find(|&&(tick, _)| tick == checkpoint.tick)
                    .map(|&(_, hash)| hash);
                if hash != Some(checkpoint.state_hash) {
                    eprintln!(
                        "{scenario}: {granularity:?} differs at tick {}",
                        checkpoint.tick
                    );
                    return ExitCode::FAILURE;
                }
            }
            traces.push(traced);
        }
        let mut again = build(scenario).expect("known scenario");
        let repeated = trace(&mut again, &log, TraceGranularity::PER_SYSTEM_PER_TICK);
        if let Some(divergence) = first_divergence(&traces[1], &repeated) {
            eprintln!("{scenario}: two identical runs diverge: {divergence}");
            return ExitCode::FAILURE;
        }
        println!(
            "{scenario}: ok, {} systems {:?}",
            repeated.system_names.len(),
            repeated.system_names
        );
    }
    ExitCode::SUCCESS
}
