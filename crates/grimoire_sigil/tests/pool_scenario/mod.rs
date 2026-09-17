//! Hash-gate scenario of contract §11.7 (Plan 0002 WP5.2): more than three pool blocks,
//! behaviors, scatter, transforms, events and clears in one simulation.
//!
//! Shared by `tests/pool_determinism.rs` (sequential and permuted executors) and by
//! `grimoire_exec/tests/hash_gate.rs` (thread pools with 1, 2 and N threads), which includes this
//! file via `#[path]` (contract §11.7, PO decision V-1). Only the public API of `grimoire_sigil`
//! and the simulation crates is used.

#![allow(dead_code)] // each including test binary uses a different subset.

#[path = "../support/mod.rs"]
mod support;

use std::sync::Arc;

use grimoire_core::Vec2;
use grimoire_ecs::{World, system_fn};
use grimoire_sigil::{
    BehaviorId, BehaviorInput, BehaviorOutcome, BehaviorRegistry, BehaviorRegistryBuilder,
    BulletFlags, BulletMotion, BulletPool, ClearFilter, ClearRequest, Emitter, EventId,
    EventRequest, SigilConfig, SigilLibrary, SigilUnit, UnitId, install,
};
use grimoire_sim::{SimRng, Simulation, Tick, TickInput};
use support::{
    Sections, Timing, When, block, block_kind, bullet_type, burst, change_type, emitter, flags,
    modifier, modifier_kind, program, reverse, script,
};

/// Seed of the gate run.
pub const SEED: u64 = 0x0005_1675_2000_0002;
/// Ticks of the gate run.
pub const TOTAL_TICKS: u64 = 240;
/// A checkpoint hash is taken after every this many ticks.
pub const CHECKPOINT_EVERY: u64 = 20;
/// Frozen `state_hash` after [`TOTAL_TICKS`] with [`SEED`], equal for every executor and thread
/// count and on every platform.
pub const GOLDEN_POOL_FINAL_HASH: u64 = 0x0d30_40fb_0a5a_4ac9;

const UNIT: u64 = 0x5347;
const DRIFT: BehaviorId = BehaviorId(1);
const SPIN: BehaviorId = BehaviorId(2);
const FLARE: &str = "flare";

/// Bends the velocity by `params[0]` per tick; despawns at `age >= params[1]`.
fn drift(input: &BehaviorInput<'_>, motion: &mut BulletMotion, _: &mut SimRng) -> BehaviorOutcome {
    motion.velocity.y += input.params[0];
    if input.age as f32 >= input.params[1] {
        BehaviorOutcome::Despawn
    } else {
        BehaviorOutcome::Keep
    }
}

/// Turns the heading by a random amount from the block generator, toward the target if any.
fn spin(input: &BehaviorInput<'_>, motion: &mut BulletMotion, rng: &mut SimRng) -> BehaviorOutcome {
    let turn = (rng.next_f32() - 0.5) * 0.05;
    motion.velocity = motion.velocity + motion.velocity.perp() * turn;
    if let Some(target) = input.target {
        motion.state[0] = (target - motion.position).length();
    }
    BehaviorOutcome::Keep
}

/// The scenario's behavior registry (version `1`).
pub fn registry() -> Arc<BehaviorRegistry> {
    let mut builder = BehaviorRegistryBuilder::new(1);
    builder.register(DRIFT, "drift", drift).expect("unique id");
    builder.register(SPIN, "spin", spin).expect("unique id");
    builder.build()
}

/// Types: `0` ring bullet (grazeable, lifetime 110, drift, becomes `3` after 3 u), `1` scatter
/// bullet (smashable, lifetime 70, spin, bursts into `2` at 45 t), `2` shard (lifetime 30), `3`
/// ember (grazeable, lifetime 60, reverses on the `flare` event).
fn unit() -> SigilUnit {
    let every_tick = Timing {
        delay: 0,
        repeat: u32::MAX,
        interval: 1,
    };
    let every_other = Timing {
        delay: 3,
        repeat: u32::MAX,
        interval: 2,
    };
    let flare = EventId::from_name(FLARE).0;
    Sections {
        bullet_types: vec![
            bullet_type(110, flags::GRAZEABLE, 0),
            bullet_type(70, flags::SMASHABLE, 1),
            bullet_type(30, 0, 2),
            bullet_type(60, flags::GRAZEABLE | flags::REFLECTABLE, 3),
        ],
        programs: vec![
            program(
                block(block_kind::RING, 40, [0.05, 0.0, 0.0, 0.0, 0.0, 0.0]),
                &[modifier(modifier_kind::ACCELERATE, 0, 0, [0.001, 0.2, 0.0])],
            ),
            program(
                block(block_kind::SCATTER, 14, [2.0, 0.0, 0.4, 0.0, 0.0, 0.0]),
                &[modifier(modifier_kind::SINE_OFFSET, 0, 0, [0.3, 16.0, 0.0])],
            ),
            program(
                block(block_kind::SCATTER, 3, [0.9, 0.0, 0.2, 0.0, 0.0, 0.0]),
                &[],
            ),
        ],
        emitters: vec![
            emitter(0, 0, 0, every_tick, 0.08),
            emitter(1, 1, 0, every_other, 0.1),
        ],
        scripts: vec![
            script(
                0,
                Some((DRIFT.0, &[0.0004, 1.0e6])),
                &[change_type(When::Distance(3.0), 3)],
            ),
            script(1, Some((SPIN.0, &[])), &[burst(When::Time(45), 2, 2, 0.12)]),
            script(3, None, &[reverse(When::Event(flare))]),
        ],
        behavior_refs: vec![DRIFT.0, SPIN.0],
    }
    .unit(UNIT)
}

/// Builds the scenario: a game system before the `sigil.*` stages raises `flare` every 23 ticks
/// and requests a clear every 61 ticks (grazeable bullets) and every 97 ticks (all shards).
pub fn build_simulation(seed: u64) -> Simulation {
    let registry = registry();
    let library = SigilLibrary::new(vec![unit()], Arc::clone(&registry)).expect("library builds");
    let mut sim = Simulation::new(seed);
    sim.schedule_mut()
        .add_system(system_fn("scenario.director", |world: &mut World| {
            let tick = world.resource::<Tick>().copied().unwrap_or_default().0;
            if tick % 23 == 22 {
                world.spawn((EventRequest {
                    event: EventId::from_name(FLARE),
                },));
            }
            if tick % 61 == 60 {
                world.spawn((ClearRequest {
                    filter: ClearFilter::AnyFlags(BulletFlags::GRAZEABLE),
                },));
            }
            if tick % 97 == 96 {
                world.spawn((ClearRequest {
                    filter: ClearFilter::Type(UnitId(UNIT), 2),
                },));
            }
        }));
    install(
        &mut sim,
        library,
        registry,
        SigilConfig::new(8192, Vec2::new(-400.0, -400.0), Vec2::new(400.0, 400.0)),
    )
    .expect("install succeeds");
    sim.world_mut()
        .insert_resource(grimoire_sigil::AimTarget(Some(Vec2::new(12.0, -30.0))));
    for (index, origin, rotation) in [
        (0u16, Vec2::new(-20.0, 0.0), 0.0f32),
        (0, Vec2::new(20.0, 0.0), 0.7),
        (1, Vec2::new(0.0, 15.0), 1.9),
        (1, Vec2::new(0.0, -15.0), -1.1),
    ] {
        sim.world_mut().spawn((Emitter {
            unit: UnitId(UNIT),
            emitter: index,
            origin,
            rotation,
            started_at: 0,
        },));
    }
    sim
}

/// Steps `sim` to `total` ticks, returning `(tick, state_hash)` after every
/// [`CHECKPOINT_EVERY`] ticks; `observe` sees the simulation after each step.
pub fn run_until(
    sim: &mut Simulation,
    total: u64,
    mut observe: impl FnMut(&Simulation),
) -> Vec<(u64, u64)> {
    let mut checkpoints = Vec::new();
    while sim.tick() < total {
        sim.step(TickInput::default());
        observe(sim);
        if sim.tick().is_multiple_of(CHECKPOINT_EVERY) {
            checkpoints.push((sim.tick(), sim.state_hash()));
        }
    }
    checkpoints
}

/// Highest slot count the pool reached, for the "more than three blocks" precondition.
pub fn slot_count(sim: &Simulation) -> u32 {
    sim.world()
        .resource::<BulletPool>()
        .map_or(0, BulletPool::slot_count)
}
