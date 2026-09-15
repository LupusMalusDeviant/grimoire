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

use std::process::ExitCode;

use bench_noise::{
    ECS_IR_ROUNDS, SIM_IR_TICKS, VARIANTS, build_ecs_world, build_sim, extra_units, run_ecs_rounds,
    run_sim_ticks,
};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(bench), Some(variant_name)) = (args.next(), args.next()) else {
        eprintln!("usage: ir_probe <ecs|sim> <baseline|plus5|plus12|plus20>");
        return ExitCode::FAILURE;
    };
    let Some(variant) = VARIANTS.iter().find(|v| v.name == variant_name) else {
        eprintln!("unknown variant {variant_name:?}");
        return ExitCode::FAILURE;
    };

    match bench.as_str() {
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
