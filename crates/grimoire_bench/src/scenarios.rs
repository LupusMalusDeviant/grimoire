//! The two P0 baseline benches (Plan-0002 WP6.2 scope item 1) and the calibrated regression
//! injector (engine ADR-0010, "Vor der Umsetzung in WP6.2" / calibration Nachtrag).
//!
//! Reused near-verbatim from the WP6.1 noise spike (branch `p1/wp6.1-bench-spike`,
//! `spikes/bench-noise/src/lib.rs`), which measured these exact two bench shapes and calibrated
//! this exact injector against them (ADR-0010). What changes here is only the crate: this is the
//! real `grimoire_bench`, not throwaway spike code, so it carries doc comments, is part of the
//! workspace and its own contract tests, and its calibration *unit counts* are **not** copied
//! from the ADR's measurement table — engine ADR-0010's own closing note ("Offen für WP6.2
//! selbst") says why: the per-unit slope is bench- and host-dependent, so WP6.2 must calibrate
//! fresh, in the same CI job as the target measurement, every run (`scripts/calibrate_injection.sh`).
//!
//! `sim_step_600` is a small self-contained stand-in for the P0 demo scenario
//! (`grimoire_sim/tests/scenario/mod.rs` is private to that crate's test binary and not
//! reusable from here, same as in the spike): one exclusive integrate system plus a periodic
//! despawn/respawn keeps the shape (moving entities, one archetype change, a `Simulation::step`
//! call) without reproducing the golden scenario.

use std::hint::black_box;

use grimoire_core::impl_stable_hash;
use grimoire_ecs::{System, World, system_fn};
use grimoire_sim::{Simulation, TickInput};

/// Entities in the ECS query benchmark (`ecs_query_10k`; task scope: "ECS query over 10k
/// entities").
pub const ECS_ENTITIES: usize = 10_000;
/// Rounds per wall-clock sample of the ECS query benchmark.
pub const ECS_WALLCLOCK_ROUNDS: u32 = 2_000;
/// Rounds per Callgrind probe of the ECS query benchmark (matches engine ADR-0010's measurement).
pub const ECS_IR_ROUNDS: u32 = 100;

/// Entities in the simplified sim-step benchmark (`sim_step_600`).
pub const SIM_ENTITIES: usize = 2_000;
/// Ticks per wall-clock sample of the sim-step benchmark.
pub const SIM_WALLCLOCK_TICKS: u32 = 600;
/// Ticks per Callgrind probe of the sim-step benchmark (matches engine ADR-0010's measurement).
pub const SIM_IR_TICKS: u32 = 600;

/// Scenario name of the ECS query benchmark (contract §15.1 `BenchResult::scenario`).
pub const ECS_SCENARIO: &str = "ecs_query_10k";
/// Scenario name of the sim-step benchmark (contract §15.1 `BenchResult::scenario`).
pub const SIM_SCENARIO: &str = "sim_step_600";

#[derive(Clone)]
struct Pos {
    x: f32,
    y: f32,
}
impl_stable_hash!(Pos { x, y });

#[derive(Clone)]
struct Vel {
    x: f32,
    y: f32,
}
impl_stable_hash!(Vel { x, y });

/// Builds the 10k-entity world for the ECS query benchmark.
#[must_use]
pub fn build_ecs_world() -> World {
    let mut world = World::new();
    for i in 0..ECS_ENTITIES {
        let f = i as f32;
        world.spawn((Pos { x: f, y: -f }, Vel { x: 1.0, y: 0.5 }));
    }
    world
}

/// Runs `rounds + extra_rounds` write-query rounds over `world`. `extra_rounds` is the nominal
/// (uncalibrated) regression injection: whole extra rounds of identical work, inside the
/// measured region.
pub fn run_ecs_rounds(world: &mut World, rounds: u32, extra_rounds: u32) {
    for _ in 0..(rounds + extra_rounds) {
        for (pos, vel) in world.query_mut::<(&mut Pos, &Vel)>() {
            pos.x += vel.x;
            pos.y += vel.y;
        }
    }
}

/// Builds the simplified sim-step benchmark: `SIM_ENTITIES` moving entities and one exclusive
/// integrate-and-bounce system, plus a periodic despawn/respawn every 50 ticks so the schedule
/// exercises at least one structural change.
#[must_use]
pub fn build_sim(seed: u64) -> Simulation {
    let mut sim = Simulation::new(seed);
    for i in 0..SIM_ENTITIES {
        let f = i as f32;
        sim.world_mut().spawn((
            Pos {
                x: f % 400.0,
                y: -(f % 400.0),
            },
            Vel { x: 0.5, y: 0.25 },
        ));
    }
    sim.schedule_mut().add_system(integrate_system());
    sim.schedule_mut().add_system(recycle_system());
    sim
}

fn integrate_system() -> impl System {
    system_fn("integrate", |world: &mut World| {
        for (pos, vel) in world.query_mut::<(&mut Pos, &Vel)>() {
            pos.x += vel.x;
            pos.y += vel.y;
            if pos.x.abs() > 512.0 {
                pos.x = -pos.x.signum() * 512.0;
            }
            if pos.y.abs() > 512.0 {
                pos.y = -pos.y.signum() * 512.0;
            }
        }
    })
}

/// Every 50th tick, despawns and respawns one entity so the world's archetype membership
/// actually changes (the one structural-change shape the real P0 scenario also exercises).
fn recycle_system() -> impl System {
    let mut tick: u32 = 0;
    system_fn("recycle", move |world: &mut World| {
        tick += 1;
        if !tick.is_multiple_of(50) {
            return;
        }
        if let Some((entity,)) = world.query::<(grimoire_ecs::Entity,)>().next() {
            world.despawn(entity);
        }
        let f = tick as f32;
        world.spawn((
            Pos {
                x: f % 400.0,
                y: -(f % 400.0),
            },
            Vel { x: 0.5, y: 0.25 },
        ));
    })
}

/// Runs `ticks` simulation steps, then `extra_ticks` more of a read-only pass over the world
/// that does not change state — the nominal (uncalibrated) regression injection.
pub fn run_sim_ticks(sim: &mut Simulation, ticks: u32, extra_ticks: u32) {
    for _ in 0..ticks {
        sim.step(TickInput::default());
    }
    if extra_ticks > 0 {
        let mut sum = 0.0f32;
        for _ in 0..extra_ticks {
            for (pos, vel) in sim.world().query::<(&Pos, &Vel)>() {
                sum += pos.x * vel.x + pos.y * vel.y;
            }
        }
        black_box(sum);
    }
}

/// Extra whole rounds/ticks for a nominal percentage regression, rounded up to the nearest unit.
/// Kept for parity with the spike and as a cheap smoke check; the calibrated injector below
/// (`run_calibration_units`) is what the production gate's calibration proof actually uses,
/// because this nominal version under-delivers its target percentage (engine ADR-0010, option 2
/// "Negativ": a nominal `+12%` measured only `+8.2%` `Ir` on `sim_step_600`).
#[must_use]
pub fn extra_units(base: u32, percent: u32) -> u32 {
    ((u64::from(base) * u64::from(percent)).div_ceil(100)) as u32
}

/// One iteration of the calibrated regression injector (engine ADR-0010, "Vor der Umsetzung in
/// WP6.2" / calibration Nachtrag): a homogeneous, arbitrarily fine-grained unit with no
/// per-round/per-tick fixed overhead, so its per-unit `Ir` cost is constant regardless of how
/// many units are injected. `scripts/calibrate_injection.sh` measures that per-unit cost on the
/// runner, in the same CI job as the target run, and solves the unit count for a target
/// percentage from the measurement — never from a hardcoded constant.
///
/// `black_box` on both the input and the output prevents the compiler from folding repeated
/// calls into a closed form (or removing the loop outright).
#[inline(never)]
fn calibration_unit(acc: f32) -> f32 {
    black_box(acc) * black_box(1.000_000_1) + black_box(0.000_000_1)
}

/// Runs `units` iterations of the calibration unit above and returns the (otherwise unused)
/// accumulator, so the loop cannot be optimized away.
pub fn run_calibration_units(units: u64) -> f32 {
    let mut acc = 1.0f32;
    for _ in 0..units {
        acc = calibration_unit(acc);
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_units_rounds_up_to_a_whole_unit() {
        assert_eq!(extra_units(2_000, 5), 100);
        assert_eq!(extra_units(2_000, 12), 240);
        assert_eq!(extra_units(100, 12), 12);
    }

    #[test]
    fn run_calibration_units_is_deterministic_for_a_given_count() {
        // Not a claim about Ir (only Callgrind on the CI runner measures that); just a sanity
        // check that the same unit count always does the same arithmetic.
        assert_eq!(run_calibration_units(1_000), run_calibration_units(1_000));
    }

    #[test]
    fn run_calibration_units_zero_is_a_no_op_accumulator() {
        assert_eq!(run_calibration_units(0), 1.0f32);
    }

    #[test]
    fn ecs_benchmark_body_runs_and_touches_every_entity() {
        let mut world = build_ecs_world();
        run_ecs_rounds(&mut world, 1, 0);
        assert_eq!(world.query::<&Pos>().count(), ECS_ENTITIES);
    }

    #[test]
    fn sim_benchmark_body_runs_and_keeps_entity_count_in_range() {
        let mut sim = build_sim(42);
        run_sim_ticks(&mut sim, 60, 0);
        // The recycle system despawns and respawns one entity every 50 ticks: population stays
        // exactly SIM_ENTITIES, this is not a determinism gate, just a sanity check.
        assert_eq!(sim.world().query::<&Pos>().count(), SIM_ENTITIES);
    }

    #[test]
    fn scenario_names_are_valid_bench_result_slugs() {
        // Contract §15.1: scenario matches [a-z0-9_]{1,64}.
        for name in [ECS_SCENARIO, SIM_SCENARIO] {
            assert!(!name.is_empty() && name.len() <= 64);
            assert!(
                name.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            );
        }
    }
}
