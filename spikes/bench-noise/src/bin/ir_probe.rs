//! K4 instruction-count probe for the WP6.1 / OF-17.3 noise spike (plan 0002 §3.2 step 4).
//!
//! Runs exactly one (bench, variant) combination once and exits. Meant to be invoked under
//! `valgrind --tool=callgrind`; the instruction count (`Ir`) is read back from Callgrind's own
//! `summary:` line in its output file (see `scripts/measure_ir.sh`), not from this process.
//! Deliberately does not depend on `gungraun`/`iai-callgrind`: reading Callgrind's own summary
//! line keeps this throwaway spike to one well-documented Valgrind output format instead of an
//! unreviewed Rust API/JSON-schema dependency (cheapest sound option, see the spike README).
//!
//! Usage: `ir_probe <ecs|sim> <baseline|plus5|plus12|plus20>`
//!         `ir_probe <ecs|sim> calibrated <units>` (ADR-0010 calibration Nachtrag: runs the
//!         bench's own baseline body plus `units` calibration units, see
//!         `bench_noise::run_calibration_units` and `scripts/calibrate_injection.sh`).

use std::hint::black_box;
use std::process::ExitCode;

use bench_noise::{
    ECS_IR_ROUNDS, SIM_IR_TICKS, VARIANTS, Variant, build_ecs_world, build_sim, extra_units,
    run_calibration_units, run_ecs_rounds, run_sim_ticks,
};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(bench), Some(mode)) = (args.next(), args.next()) else {
        eprintln!("usage: ir_probe <ecs|sim> <baseline|plus5|plus12|plus20>");
        eprintln!("       ir_probe <ecs|sim> calibrated <units>");
        return ExitCode::FAILURE;
    };

    if mode == "calibrated" {
        let Some(units_arg) = args.next() else {
            eprintln!("usage: ir_probe <ecs|sim> calibrated <units>");
            return ExitCode::FAILURE;
        };
        let Ok(units) = units_arg.parse::<u64>() else {
            eprintln!("invalid unit count {units_arg:?}");
            return ExitCode::FAILURE;
        };
        return run_calibrated(&bench, units);
    }

    let Some(variant) = VARIANTS.iter().find(|v| v.name == mode) else {
        eprintln!("unknown variant {mode:?}, expected a variant name or 'calibrated'");
        return ExitCode::FAILURE;
    };
    run_nominal(&bench, variant)
}

fn run_nominal(bench: &str, variant: &Variant) -> ExitCode {
    match bench {
        "ecs" => {
            let extra = extra_units(ECS_IR_ROUNDS, variant.percent);
            let mut world = build_ecs_world();
            run_ecs_rounds(&mut world, ECS_IR_ROUNDS, extra);
        }
        "sim" => {
            let extra = extra_units(SIM_IR_TICKS, variant.percent);
            let mut sim = build_sim(0xB5_11_C1_0C_C0_FF_EE_00);
            run_sim_ticks(&mut sim, SIM_IR_TICKS, extra);
        }
        other => {
            eprintln!("unknown bench {other:?}, expected 'ecs' or 'sim'");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

/// Calibrated injection (ADR-0010 calibration Nachtrag): runs the bench's own baseline body
/// unmodified (0 nominal extra rounds/ticks), then `units` fine-grained calibration units inside
/// the same measured region. `scripts/calibrate_injection.sh` measures this binary's `Ir` at
/// `units=0` and at a large reference unit count on the same runner, solves the per-unit slope,
/// and picks `units` so the resulting `Ir` delta lands within +/-1 percentage point of the target
/// percentage of that same baseline.
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
