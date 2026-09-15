//! Shared benchmark bodies and gate math for the WP6.1 / OF-17.3 noise spike (plan 0002).
//!
//! Throwaway spike code (plan 0002 §"Versuchsprotokoll WP6.1"): not a member of the engine
//! workspace, not held to its determinism lints, never loaded by the runtime, and never
//! promoted as-is — WP6.2 builds the real `grimoire_bench` from whatever this spike finds.
//!
//! Two P0-shaped baselines, simplified for a throwaway measurement of noise rather than a
//! faithful reproduction of engine behaviour:
//!
//! - [`build_ecs_world`] / [`run_ecs_rounds`]: the write-query half of
//!   `grimoire_ecs/tests/timing.rs` (`query_mut::<(&mut Pos, &Vel)>` over 10,000 entities).
//! - [`build_sim`] / [`run_sim_ticks`]: a small self-contained stand-in for the P0 demo scenario
//!   in `grimoire_sim/tests/scenario/mod.rs`. That module is private to the `grimoire_sim` test
//!   binary and not reusable from here; re-deriving its exact spawner/despawner/steering
//!   behaviour would spend spike time on fidelity the noise question does not need. One exclusive
//!   integrate system plus a periodic despawn/respawn keeps the shape (moving entities, one
//!   archetype change, a `Simulation::step` call) without reproducing the golden scenario.

use std::hint::black_box;

use grimoire_core::impl_stable_hash;
use grimoire_ecs::{System, World, system_fn};
use grimoire_sim::{Simulation, TickInput};

/// Entities in the ECS query benchmark (plan 0002 WP6.1 bench B1, `ecs_query_10k_write`).
pub const ECS_ENTITIES: usize = 10_000;
/// Rounds per wall-clock sample of the ECS query benchmark (matches `timing.rs`).
pub const ECS_WALLCLOCK_ROUNDS: u32 = 2_000;
/// Rounds per Callgrind probe of the ECS query benchmark (plan 0002 WP6.1 §3.1, bench B1).
pub const ECS_IR_ROUNDS: u32 = 100;

/// Entities in the simplified sim-step benchmark (plan 0002 WP6.1 bench B3, `sim_demo_600`).
pub const SIM_ENTITIES: usize = 2_000;
/// Ticks per wall-clock sample of the sim-step benchmark (plan 0002 WP6.1 §3.1, bench B3).
pub const SIM_WALLCLOCK_TICKS: u32 = 600;
/// Ticks per Callgrind probe of the sim-step benchmark (plan 0002 WP6.1 §3.1, bench B3).
pub const SIM_IR_TICKS: u32 = 600;

/// One nominal regression variant: extra whole rounds/ticks of identical work inside the
/// measured region (plan 0002 WP6.1 §3.1), named after the percentage it targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Variant {
    pub name: &'static str,
    pub percent: u32,
}

/// The variant set this spike measures. The prepared proposal (`0002-vorbereitung-of-17.3.md`
/// §3.1/§3.2) also lists +15% and a "cache-hostile" access-pattern variant; both are cut here
/// (cheapest sound option, stated in the spike README and the final report) because +12/+20%
/// already probes "just above the 10% gate" and "clearly above it", and the cache-hostile
/// variant is explicitly exploratory in the proposal ("Blinder Fleck"), not required to answer
/// which metric can gate a >10% regression.
pub const VARIANTS: &[Variant] = &[
    Variant {
        name: "baseline",
        percent: 0,
    },
    Variant {
        name: "plus5",
        percent: 5,
    },
    Variant {
        name: "plus12",
        percent: 12,
    },
    Variant {
        name: "plus20",
        percent: 20,
    },
];

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

/// Builds the 10k-entity world for the ECS query benchmark (`grimoire_ecs/tests/timing.rs`).
pub fn build_ecs_world() -> World {
    let mut world = World::new();
    for i in 0..ECS_ENTITIES {
        let f = i as f32;
        world.spawn((Pos { x: f, y: -f }, Vel { x: 1.0, y: 0.5 }));
    }
    world
}

/// Runs `rounds + extra_rounds` write-query rounds over `world`. `extra_rounds` is the
/// regression injection: whole extra rounds of the identical work, inside the measured region
/// (plan 0002 WP6.1 §3.1).
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
/// that does not change state — the B3 regression injection of plan 0002 WP6.1 §3.1.
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

/// Extra whole rounds/ticks for a nominal percentage regression, rounded to the nearest unit
/// (plan 0002 WP6.1 §3.1: "additional whole rounds of the same work").
pub fn extra_units(base: u32, percent: u32) -> u32 {
    ((u64::from(base) * u64::from(percent)).div_ceil(100)) as u32
}

/// Outcome of comparing one candidate measurement against an accepted basis (plan 0002 WP6.1
/// §4.3): the exitcode a real gate would use, plus whether it is a warning-only crossing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateOutcome {
    /// Exitcode 0, no warning: candidate within `warn_percent` of the basis, or an improvement.
    Green,
    /// Exitcode 0, with an annotation: `warn_percent < delta <= 10%`.
    Warn,
    /// Exitcode 3: `delta > 10%`, checked as an exact integer ratio so the boundary never hangs
    /// on floating-point rounding (plan 0002 WP6.1 §4.3): `10 * candidate > 11 * baseline`.
    Red,
}

/// Measurement-error outcome (plan 0002 WP6.1 §4.3, exitcode 2): a value of zero or the basis
/// missing makes any ratio meaningless.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GateError;

/// Compares `candidate` against `baseline` (both in the same unit: nanoseconds or instructions).
/// `warn_percent` is a whole-percent threshold (the plan's `w`, e.g. 3).
///
/// # Errors
/// Returns [`GateError`] (exitcode 2) if either value is zero: a zero basis or measurement is
/// never a valid ratio, per plan 0002 WP6.1 §4.3 ("fehlender Bench, Wert 0 oder NaN").
pub fn gate_decision(
    baseline: u64,
    candidate: u64,
    warn_percent: u32,
) -> Result<GateOutcome, GateError> {
    if baseline == 0 || candidate == 0 {
        return Err(GateError);
    }
    // Red: candidate / baseline > 1.10, as an exact integer comparison (plan §4.3).
    if 10u128 * u128::from(candidate) > 11u128 * u128::from(baseline) {
        return Ok(GateOutcome::Red);
    }
    if candidate <= baseline {
        return Ok(GateOutcome::Green);
    }
    // Warn: (candidate - baseline) / baseline > warn_percent / 100, exact integer comparison.
    let delta = candidate - baseline;
    if 100u128 * u128::from(delta) > u128::from(warn_percent) * u128::from(baseline) {
        Ok(GateOutcome::Warn)
    } else {
        Ok(GateOutcome::Green)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Plan 0002 WP6.1 §5, "Unit-Tests des Vergleichers", run only in CI (never locally: the
    // hard rule on this spike forbids `cargo test` on the development machine).
    #[test]
    fn plus_15_percent_is_red() {
        assert_eq!(gate_decision(100_000, 115_000, 3), Ok(GateOutcome::Red));
    }

    #[test]
    fn exactly_10_percent_is_green_with_warning() {
        // "> 10%" is the rule (plan §4.3): exactly +10% must not trip the red gate.
        assert_eq!(gate_decision(100_000, 110_000, 3), Ok(GateOutcome::Warn));
    }

    #[test]
    fn just_over_10_percent_is_red() {
        assert_eq!(gate_decision(100_000, 110_010, 3), Ok(GateOutcome::Red));
    }

    #[test]
    fn plus_2_percent_under_warn_threshold_is_plain_green() {
        assert_eq!(gate_decision(100_000, 102_000, 3), Ok(GateOutcome::Green));
    }

    #[test]
    fn plus_4_percent_over_warn_threshold_warns() {
        assert_eq!(gate_decision(100_000, 104_000, 3), Ok(GateOutcome::Warn));
    }

    #[test]
    fn minus_20_percent_is_green() {
        assert_eq!(gate_decision(100_000, 80_000, 3), Ok(GateOutcome::Green));
    }

    #[test]
    fn zero_candidate_or_baseline_is_a_measurement_error() {
        assert_eq!(gate_decision(0, 100, 3), Err(GateError));
        assert_eq!(gate_decision(100, 0, 3), Err(GateError));
    }

    #[test]
    fn extra_units_rounds_up_to_a_whole_unit() {
        assert_eq!(extra_units(2_000, 5), 100);
        assert_eq!(extra_units(2_000, 12), 240);
        assert_eq!(extra_units(100, 12), 12);
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
}
