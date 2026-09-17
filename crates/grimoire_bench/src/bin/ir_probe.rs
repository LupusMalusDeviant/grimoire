//! Runs one P0 bench body once, then exits. Meant to be invoked under
//! `valgrind --tool=callgrind`; the instruction count (`Ir`) is read back from Callgrind's own
//! `summary:` line in its output file by `scripts/measure_ir.sh` / `scripts/calibrate_injection.sh`,
//! not by this process. Deliberately does not depend on `gungraun`/`iai-callgrind`: engine
//! ADR-0010's WP6.2 addendum ("Entscheidung: roh bleibt, vorerst") measured both paths for these
//! same two benches and chose to keep reading Callgrind's own summary line directly.
//!
//! Plan 0002 WP5.3 adds a third bench body, `sigil_extract` (`sigil_extract_10k`: the Sigil -> Render
//! extraction adapter over 10,000 bullets), for `baseline` and `params`; the calibration proof
//! keeps calibrating the two P0 benches only. Plan 0002 WP5.4 adds three `sigil.*` tick bodies:
//! `sigil_update` (`sigil_update_6k`, the WP5.1 micro-bench, now measured), `sigil_update_10k`
//! (10,000 active bullets) and `sigil_churn` (`sigil_churn_2k`: 2,000 spawns and 2,000 despawns
//! per tick). Plan 0002 WP3.6 adds the render CPU bodies `bullet_upload` (`render_bullet_upload_10k`)
//! and `light_cluster` (`render_light_cluster_256`), WP6.5 the collision body `collide_uniform`
//! (10,000 bullets and 100 enemies per tick on one thread).
//!
//! Usage:
//!   `ir_probe <ecs|sim|sigil_extract> baseline` — the real, uninjected candidate measurement.
//!   `ir_probe <ecs|sim> calibrated <units>`   — baseline body plus `units` calibration units
//!                                               (see `grimoire_bench::scenarios::run_calibration_units`
//!                                               and `scripts/calibrate_injection.sh`).
//!   `ir_probe <ecs|sim|sigil_extract> params` — prints this bench's `BenchResult::params` as
//!                                               `key=value,key=value` and exits immediately,
//!                                               run *without* Valgrind. Exists so
//!                                               `scripts/measure_ir.sh` reads the entity/round
//!                                               counts from the one place that defines them
//!                                               ([`grimoire_bench::scenarios`]) instead of
//!                                               duplicating those constants in bash, where they
//!                                               could silently drift from the real bench body.

use std::hint::black_box;
use std::process::ExitCode;

use grimoire_bench::scenarios::{
    COLLIDE_BULLETS, COLLIDE_ENEMIES, COLLIDE_IR_TICKS, CollideLayout, ECS_ENTITIES, ECS_IR_ROUNDS,
    EXTRACT_BULLETS, EXTRACT_IR_ROUNDS, RENDER_BULLETS, RENDER_IR_FRAMES, RENDER_POINT_LIGHTS,
    SIGIL_CHURN_BULLETS, SIGIL_CHURN_IR_TICKS, SIGIL_CHURN_PER_TICK, SIGIL_ENTITIES,
    SIGIL_IR_TICKS, SIGIL_UPDATE_10K_BULLETS, SIGIL_UPDATE_10K_IR_TICKS, SIM_ENTITIES,
    SIM_IR_TICKS, build_collide, build_ecs_world, build_render_cpu, build_sigil_churn,
    build_sigil_extract, build_sigil_update, build_sigil_update_10k, build_sim,
    run_bullet_upload_frames, run_calibration_units, run_collide_ticks, run_ecs_rounds,
    run_light_cluster_frames, run_sigil_extract_rounds, run_sigil_update_ticks, run_sim_ticks,
    sigil_update_fill_ticks,
};

/// Every bench body `baseline` and `params` accept.
const BENCHES: &str = "'ecs', 'sim', 'sigil_extract', 'sigil_update', 'sigil_update_10k', \
                       'sigil_churn', 'bullet_upload', 'light_cluster' or 'collide_uniform'";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(bench), Some(mode)) = (args.next(), args.next()) else {
        eprintln!("usage: ir_probe <bench> baseline   (bench: {BENCHES})");
        eprintln!("       ir_probe <ecs|sim> calibrated <units>");
        eprintln!("       ir_probe <bench> params");
        return ExitCode::FAILURE;
    };

    match mode.as_str() {
        "params" => print_params(&bench),
        "baseline" => run_baseline(&bench),
        "calibrated" => {
            let Some(units_arg) = args.next() else {
                eprintln!("usage: ir_probe <ecs|sim> calibrated <units>");
                return ExitCode::FAILURE;
            };
            let Ok(units) = units_arg.parse::<u64>() else {
                eprintln!("invalid unit count {units_arg:?}");
                return ExitCode::FAILURE;
            };
            run_calibrated(&bench, units)
        }
        other => {
            eprintln!("unknown mode {other:?}, expected 'baseline' or 'calibrated'");
            ExitCode::FAILURE
        }
    }
}

fn print_params(bench: &str) -> ExitCode {
    match bench {
        "ecs" => println!("entities={ECS_ENTITIES},rounds={ECS_IR_ROUNDS}"),
        "sim" => println!("entities={SIM_ENTITIES},ticks={SIM_IR_TICKS}"),
        "sigil_extract" => println!("bullets={EXTRACT_BULLETS},rounds={EXTRACT_IR_ROUNDS}"),
        "sigil_update" => println!("bullets={SIGIL_ENTITIES},ticks={SIGIL_IR_TICKS}"),
        "sigil_update_10k" => {
            println!("bullets={SIGIL_UPDATE_10K_BULLETS},ticks={SIGIL_UPDATE_10K_IR_TICKS}");
        }
        "sigil_churn" => println!(
            "bullets={SIGIL_CHURN_BULLETS},per_tick={SIGIL_CHURN_PER_TICK},ticks={SIGIL_CHURN_IR_TICKS}"
        ),
        "bullet_upload" => println!("bullets={RENDER_BULLETS},frames={RENDER_IR_FRAMES}"),
        "light_cluster" => println!(
            "lights={RENDER_POINT_LIGHTS},bullets={RENDER_BULLETS},frames={RENDER_IR_FRAMES}"
        ),
        "collide_uniform" => {
            println!(
                "bullets={COLLIDE_BULLETS},enemies={COLLIDE_ENEMIES},ticks={COLLIDE_IR_TICKS}"
            );
        }
        other => {
            eprintln!("unknown bench {other:?}, expected {BENCHES}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

fn run_baseline(bench: &str) -> ExitCode {
    match bench {
        "ecs" => {
            let mut world = build_ecs_world();
            run_ecs_rounds(&mut world, ECS_IR_ROUNDS, 0);
        }
        "sim" => {
            let mut sim = build_sim(0xB5_11_C1_0C_C0_FF_EE_00);
            run_sim_ticks(&mut sim, SIM_IR_TICKS, 0);
        }
        "sigil_extract" => {
            let mut bench = build_sigil_extract(0xB5_11_C1_0C_C0_FF_EE_00);
            black_box(run_sigil_extract_rounds(&mut bench, EXTRACT_IR_ROUNDS, 0));
        }
        "sigil_update" => {
            let mut sim = build_sigil_update(0xB5_11_C1_0C_C0_FF_EE_00);
            run_sigil_update_ticks(&mut sim, sigil_update_fill_ticks(), 0);
            run_sigil_update_ticks(&mut sim, SIGIL_IR_TICKS, 0);
            black_box(sim.tick());
        }
        "sigil_update_10k" => {
            let mut sim = build_sigil_update_10k(0xB5_11_C1_0C_C0_FF_EE_00);
            run_sigil_update_ticks(&mut sim, SIGIL_UPDATE_10K_IR_TICKS, 0);
            black_box(sim.tick());
        }
        "sigil_churn" => {
            let mut sim = build_sigil_churn(0xB5_11_C1_0C_C0_FF_EE_00);
            run_sigil_update_ticks(&mut sim, SIGIL_CHURN_IR_TICKS, 0);
            black_box(sim.tick());
        }
        "bullet_upload" => {
            let mut bench = build_render_cpu();
            black_box(run_bullet_upload_frames(&mut bench, RENDER_IR_FRAMES, 0));
        }
        "light_cluster" => {
            let mut bench = build_render_cpu();
            black_box(run_light_cluster_frames(&mut bench, RENDER_IR_FRAMES, 0));
        }
        // Plan 0002 WP6.5: single-threaded only, as the Callgrind gate requires (engine ADR-0010);
        // `collide_cluster` and the N-thread runs stay wall-clock trends (contract §14).
        "collide_uniform" => {
            let mut bench = build_collide(CollideLayout::Uniform);
            black_box(run_collide_ticks(
                &mut bench,
                &grimoire_ecs::SequentialExecutor,
                COLLIDE_IR_TICKS,
                0,
            ));
        }
        other => {
            eprintln!("unknown bench {other:?}, expected {BENCHES}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

/// Calibrated injection (engine ADR-0010 calibration Nachtrag): runs the bench's own baseline
/// body unmodified, then `units` fine-grained calibration units inside the same measured region.
fn run_calibrated(bench: &str, units: u64) -> ExitCode {
    match bench {
        "ecs" => {
            let mut world = build_ecs_world();
            run_ecs_rounds(&mut world, ECS_IR_ROUNDS, 0);
            black_box(run_calibration_units(units));
        }
        "sim" => {
            let mut sim = build_sim(0xB5_11_C1_0C_C0_FF_EE_00);
            run_sim_ticks(&mut sim, SIM_IR_TICKS, 0);
            black_box(run_calibration_units(units));
        }
        other => {
            eprintln!("unknown bench {other:?}, expected 'ecs' or 'sim'");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
