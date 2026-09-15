//! `gungraun`-based benchmark harness for the WP6.1 / OF-17.3 spike's "gungraun vs. raw Callgrind
//! line" comparison (ADR-0010 "Vor der Umsetzung in WP6.2", Folge-Entscheidung: "WP6.2
//! entscheidet gungraun/iai-callgrind gegen eine eigene, schlanke Callgrind-Ansteuerung wie in
//! diesem Spike").
//!
//! Exercises the same two P0-shaped bodies as `src/bin/ir_probe.rs` (`ecs_query_10k`,
//! `sim_step_600`), `baseline` and the nominal `+12%` variant, so the CI job that runs this file
//! alongside `scripts/measure_ir.sh` on the same runner/image (`.github/workflows/…`, job
//! `gungraun-vs-raw`) can compare gungraun's own setup cost, CI time and job-to-job stability
//! against this spike's raw-Callgrind-summary-line approach.
//!
//! World/sim construction is passed as the `#[bench::id(...)]` argument expression rather than
//! built inside the benchmarked function body. Per gungraun's docs (`setup_and_teardown.md`,
//! `library_benchmarks/quickstart.md`'s "function calls are fine too"), argument expressions run
//! *before* Callgrind's default entry point (the annotated function itself), so their cost is
//! excluded from the measured `Ir` — unlike `ir_probe`, which counts the whole process including
//! world/sim construction. Whether that alone narrows the nominal-vs-measured percentage gap that
//! motivated the calibration injector (see `lib.rs`) is exactly one of the things this comparison
//! measures; it does not replace the calibration proof, since WP6.2 needs the injector to work
//! the same way regardless of which tool it ends up using.

use std::hint::black_box;

use bench_noise::{
    ECS_IR_ROUNDS, SIM_IR_TICKS, build_ecs_world, build_sim, extra_units, run_ecs_rounds,
    run_sim_ticks,
};
use grimoire_ecs::World;
use grimoire_sim::Simulation;
use gungraun::prelude::*;

const SIM_SEED: u64 = 0xB5_11_C1_0C_C0_FF_EE_00;

#[library_benchmark]
#[bench::baseline(build_ecs_world())]
fn bench_ecs_query_10k_baseline(mut world: World) -> World {
    run_ecs_rounds(&mut world, ECS_IR_ROUNDS, 0);
    black_box(world)
}

#[library_benchmark]
#[bench::plus12(build_ecs_world())]
fn bench_ecs_query_10k_plus12(mut world: World) -> World {
    let extra = extra_units(ECS_IR_ROUNDS, 12);
    run_ecs_rounds(&mut world, ECS_IR_ROUNDS, extra);
    black_box(world)
}

#[library_benchmark]
#[bench::baseline(build_sim(SIM_SEED))]
fn bench_sim_step_600_baseline(mut sim: Simulation) -> Simulation {
    run_sim_ticks(&mut sim, SIM_IR_TICKS, 0);
    black_box(sim)
}

#[library_benchmark]
#[bench::plus12(build_sim(SIM_SEED))]
fn bench_sim_step_600_plus12(mut sim: Simulation) -> Simulation {
    let extra = extra_units(SIM_IR_TICKS, 12);
    run_sim_ticks(&mut sim, SIM_IR_TICKS, extra);
    black_box(sim)
}

library_benchmark_group!(
    name = ecs_query_10k,
    benchmarks = [bench_ecs_query_10k_baseline, bench_ecs_query_10k_plus12]
);

library_benchmark_group!(
    name = sim_step_600,
    benchmarks = [bench_sim_step_600_baseline, bench_sim_step_600_plus12]
);

main!(library_benchmark_groups = [ecs_query_10k, sim_step_600]);
