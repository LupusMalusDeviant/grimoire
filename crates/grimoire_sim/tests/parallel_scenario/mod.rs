//! Parallel golden scenario (engine ADR-0006, building block 7), shared by
//! `tests/parallel_determinism.rs` and the thread-pool hash gate of `grimoire_exec` (which
//! includes this file via `#[path]`).
//!
//! About 12 000 entities in three archetypes run 1 500 ticks through a multi-member parallel
//! stage with deferred component and resource writes, block random streams, `f32` reductions
//! folded over blocks, a stage split by a read-after-write and structural stage boundaries:
//!
//! | System | Kind | Work |
//! |--------|------|------|
//! | `hazard` | exclusive | moves the `Hazard` resource with `dmath` trigonometry |
//! | `sense` | parallel | block random turns (`derive_block_rng`), `set::<Heading>` in block order |
//! | `heat` | parallel | heats entities inside the hazard (`dmath::hypot`), `set::<Heat>` |
//! | `census` | parallel | block-order `f32` sum and `dmath::min`/`max`, `insert_resource::<Census>` |
//! | `tag` | parallel, structural | reads `Heat` (new stage), inserts and removes `Marked` |
//! | `steer` | exclusive | `par_blocks_mut` over velocities, headings and `Option<&Marked>` |
//! | `integrate` | exclusive | `par_blocks_mut` movement, collects expired entities per block |
//! | `emit` | parallel, structural | refills the population from `Census` |
//!
//! [`GOLDEN_PARALLEL_FINAL_HASH`] is measured with [`StageMode::Isolated`] and the sequential
//! executor; grouped stages under every executor and thread count must reproduce every
//! checkpoint.

// Each including test binary uses a different subset of this module.
#![allow(dead_code)]

use grimoire_core::math::dmath;
use grimoire_core::{Vec2, impl_stable_hash};
use grimoire_ecs::{
    Access, CommandBuffer, Entity, StageMode, With, Without, World, parallel_system_fn, system_fn,
};
use grimoire_sim::{SimRng, SimSeed, Simulation, Tick, derive_block_rng, derive_rng};

/// Final `Simulation::state_hash` of the parallel scenario after [`TOTAL_TICKS`] ticks.
///
/// Measured with `StageMode::Isolated` and the `SequentialExecutor` on Windows x86_64 (debug).
/// Provisional until the CI test matrix reproduces it on Windows, Linux and macOS. Grouped stages
/// with every executor, including thread pools of 1, 2 and N threads, must reproduce it.
///
/// Renew it only when the outcome is supposed to change: a deliberate change of this scenario,
/// of `QUERY_BLOCK_SIZE` or the block rule, of `derive_block_rng`, of the hash layout or of an
/// algorithm version. Mention the renewal and its reason in the commit message.
pub const GOLDEN_PARALLEL_FINAL_HASH: u64 = 12_942_142_408_096_405_486;

pub const SEED: u64 = 0x5EED_0006_A11E_1234;
pub const TOTAL_TICKS: u64 = 1_500;
pub const HASH_EVERY: u64 = 100;
/// Target population; the initial population has exactly this size.
pub const POPULATION: usize = 12_000;
const MAX_POPULATION: usize = POPULATION + 1_000;
const MAX_SPAWNS_PER_TICK: usize = 48;
const DT: f32 = 1.0 / 60.0;
const ARENA_HALF: f32 = 800.0;

const STREAM_INIT: u64 = 1;
const STREAM_SENSE: u64 = 2;
const STREAM_EMIT: u64 = 3;

#[derive(Clone)]
pub struct Position {
    pub value: Vec2,
}
impl_stable_hash!(Position { value });

#[derive(Clone)]
pub struct Velocity {
    pub value: Vec2,
}
impl_stable_hash!(Velocity { value });

#[derive(Clone)]
pub struct Heading {
    pub angle: f32,
}
impl_stable_hash!(Heading { angle });

/// Heat gathered inside the hazard; entities without it never heat up.
#[derive(Clone)]
pub struct Heat {
    pub value: f32,
}
impl_stable_hash!(Heat { value });

#[derive(Clone)]
pub struct Lifetime {
    pub ticks_left: u32,
}
impl_stable_hash!(Lifetime { ticks_left });

/// Inserted by `tag` on hot entities and removed once they cooled down.
#[derive(Clone)]
pub struct Marked {
    pub since: u64,
}
impl_stable_hash!(Marked { since });

/// Moving danger zone, written by the exclusive `hazard` system.
#[derive(Clone)]
pub struct Hazard {
    pub center: Vec2,
    pub radius: f32,
}
impl_stable_hash!(Hazard { center, radius });

/// Reduction over all moving entities, written by `census`.
#[derive(Clone)]
pub struct Census {
    pub count: u32,
    pub sum_x: f32,
    pub min_speed: f32,
    pub max_speed: f32,
}
impl_stable_hash!(Census {
    count,
    sum_x,
    min_speed,
    max_speed
});

fn seed_and_tick(world: &World) -> (u64, u64) {
    let seed = world.resource::<SimSeed>().map_or(0, |seed| seed.0);
    let tick = world.resource::<Tick>().map_or(0, |tick| tick.0);
    (seed, tick)
}

/// Draws a new agent without heat; `offset_x` shifts its start position.
fn agent(rng: &mut SimRng, offset_x: f32) -> (Position, Velocity, Heading, Lifetime) {
    let x = rng.range_f32(-ARENA_HALF, ARENA_HALF) * 0.9 + offset_x;
    let y = rng.range_f32(-ARENA_HALF, ARENA_HALF) * 0.9;
    let angle = rng.range_f32(0.0, dmath::TAU);
    let speed = rng.range_f32(10.0, 60.0);
    let ticks_left = rng.range_u32(200, 2_000);
    (
        Position {
            value: Vec2::new(x, y),
        },
        Velocity {
            value: Vec2::from_angle(angle) * speed,
        },
        Heading { angle },
        Lifetime { ticks_left },
    )
}

fn hazard(world: &mut World) {
    let tick = world.resource::<Tick>().map_or(0, |tick| tick.0);
    let phase = tick as f32 * 0.013;
    let center = Vec2::new(dmath::cos(phase) * 300.0, dmath::sin(phase * 1.7) * 300.0);
    let radius = 150.0 + dmath::sin(tick as f32 * 0.05) * 50.0;
    world.insert_resource(Hazard { center, radius });
}

fn sense(world: &World, commands: &mut CommandBuffer) {
    let (seed, tick) = seed_and_tick(world);
    let Some(hazard) = world.resource::<Hazard>() else {
        return;
    };
    let turns = world.par_blocks::<(Entity, &Position, &Velocity), _>(|block| {
        let mut rng = derive_block_rng(seed, tick, STREAM_SENSE, block.index() as u64);
        let mut turns = Vec::new();
        for (entity, position, velocity) in block {
            if !rng.chance(0.04) {
                continue;
            }
            let away = dmath::atan2(
                position.value.y - hazard.center.y,
                position.value.x - hazard.center.x,
            );
            let drift = dmath::atan2(velocity.value.y, velocity.value.x);
            let jitter = rng.range_f32(-0.6, 0.6);
            turns.push((entity, away * 0.7 + drift * 0.3 + jitter));
        }
        turns
    });
    for (entity, angle) in turns.into_iter().flatten() {
        commands.set(entity, Heading { angle });
    }
}

fn heat(world: &World, commands: &mut CommandBuffer) {
    let (_, tick) = seed_and_tick(world);
    let Some(hazard) = world.resource::<Hazard>() else {
        return;
    };
    let decay = tick.is_multiple_of(10);
    let updates = world.par_blocks::<(Entity, &Position, &Heat), _>(|block| {
        let mut updates = Vec::new();
        for (entity, position, heat) in block {
            let distance = dmath::hypot(
                position.value.x - hazard.center.x,
                position.value.y - hazard.center.y,
            );
            if distance < hazard.radius {
                let gain = (hazard.radius - distance) / hazard.radius;
                updates.push((entity, heat.value + gain));
            } else if decay && heat.value > 0.0 {
                let cooled = heat.value * 0.5;
                updates.push((entity, if cooled < 0.01 { 0.0 } else { cooled }));
            }
        }
        updates
    });
    for (entity, value) in updates.into_iter().flatten() {
        commands.set(entity, Heat { value });
    }
}

fn census(world: &World, commands: &mut CommandBuffer) {
    let blocks = world.par_blocks::<(&Position, &Velocity), _>(|block| {
        let mut count = 0u32;
        let mut sum_x = 0.0f32;
        let mut min_speed = f32::MAX;
        let mut max_speed = 0.0f32;
        for (position, velocity) in block {
            let speed = dmath::hypot(velocity.value.x, velocity.value.y);
            count += 1;
            sum_x += position.value.x;
            min_speed = dmath::min(min_speed, speed);
            max_speed = dmath::max(max_speed, speed);
        }
        (count, sum_x, min_speed, max_speed)
    });
    let mut total = Census {
        count: 0,
        sum_x: 0.0,
        min_speed: f32::MAX,
        max_speed: 0.0,
    };
    for (count, sum_x, min_speed, max_speed) in blocks {
        total.count += count;
        total.sum_x += sum_x;
        total.min_speed = dmath::min(total.min_speed, min_speed);
        total.max_speed = dmath::max(total.max_speed, max_speed);
    }
    commands.insert_resource(total);
}

fn tag(world: &World, commands: &mut CommandBuffer) {
    let (_, tick) = seed_and_tick(world);
    let mut marks = world.par_blocks::<(Entity, &Heat, Without<Marked>), _>(|block| {
        let mut local = CommandBuffer::new();
        for (entity, heat, ()) in block {
            if heat.value > 2.5 {
                local.insert(entity, Marked { since: tick });
            }
        }
        local
    });
    for local in &mut marks {
        commands.append(local);
    }
    let cooled = world.par_blocks::<(Entity, &Heat, With<Marked>), _>(|block| {
        block
            .filter(|(_, heat, ())| heat.value < 0.25)
            .map(|(entity, _, ())| entity)
            .collect::<Vec<_>>()
    });
    for entity in cooled.into_iter().flatten() {
        commands.remove::<Marked>(entity);
    }
}

fn steer(world: &mut World) {
    world.par_blocks_mut::<(&mut Velocity, &Heading, Option<&Marked>), _>(|block| {
        for (velocity, heading, marked) in block {
            let speed = if marked.is_some() { 90.0 } else { 45.0 };
            let target = Vec2::from_angle(heading.angle) * speed;
            velocity.value = velocity.value * 0.9 + target * 0.1;
        }
    });
}

fn integrate(world: &mut World) {
    let expired =
        world.par_blocks_mut::<(Entity, &mut Position, &Velocity, &mut Lifetime), _>(|block| {
            let mut expired = Vec::new();
            for (entity, position, velocity, lifetime) in block {
                let mut next = position.value + velocity.value * DT;
                if next.x > ARENA_HALF {
                    next.x -= 2.0 * ARENA_HALF;
                } else if next.x < -ARENA_HALF {
                    next.x += 2.0 * ARENA_HALF;
                }
                if next.y > ARENA_HALF {
                    next.y -= 2.0 * ARENA_HALF;
                } else if next.y < -ARENA_HALF {
                    next.y += 2.0 * ARENA_HALF;
                }
                position.value = next;
                lifetime.ticks_left = lifetime.ticks_left.saturating_sub(1);
                if lifetime.ticks_left == 0 {
                    expired.push(entity);
                }
            }
            expired
        });
    for entity in expired.into_iter().flatten() {
        world.despawn(entity);
    }
}

fn emit(world: &World, commands: &mut CommandBuffer) {
    let (seed, tick) = seed_and_tick(world);
    let mut rng = derive_rng(seed, tick, STREAM_EMIT);
    let mean_x = world.resource::<Census>().map_or(0.0, |census| {
        if census.count == 0 {
            0.0
        } else {
            census.sum_x / census.count as f32
        }
    });
    let count = world.entity_count();
    let missing = POPULATION.saturating_sub(count);
    let wanted = (missing + rng.range_u32(0, 4) as usize)
        .min(MAX_SPAWNS_PER_TICK)
        .min(MAX_POPULATION.saturating_sub(count));
    for _ in 0..wanted {
        let warm = rng.range_u32(0, 3) != 0;
        let (position, velocity, heading, lifetime) = agent(&mut rng, mean_x * 0.1);
        if warm {
            commands.spawn((position, velocity, heading, Heat { value: 0.0 }, lifetime));
        } else {
            commands.spawn((position, velocity, heading, lifetime));
        }
    }
}

/// Builds the parallel scenario with the given stage mode.
pub fn build_simulation(seed: u64, mode: StageMode) -> Simulation {
    let mut sim = Simulation::new(seed);
    let world = sim.world_mut();
    world.register_component::<Position>();
    world.register_component::<Velocity>();
    world.register_component::<Heading>();
    world.register_component::<Heat>();
    world.register_component::<Lifetime>();
    world.register_component::<Marked>();
    world.insert_resource(Hazard {
        center: Vec2::ZERO,
        radius: 0.0,
    });
    world.insert_resource(Census {
        count: 0,
        sum_x: 0.0,
        min_speed: 0.0,
        max_speed: 0.0,
    });
    let mut rng = derive_rng(seed, 0, STREAM_INIT);
    for index in 0..POPULATION {
        let (position, velocity, heading, lifetime) = agent(&mut rng, 0.0);
        if index % 3 == 0 {
            world.spawn((position, velocity, heading, lifetime));
        } else {
            world.spawn((position, velocity, heading, Heat { value: 0.0 }, lifetime));
        }
    }
    sim.schedule_mut()
        .add_system(system_fn("hazard", hazard))
        .add_parallel_system(parallel_system_fn(
            "sense",
            Access::new()
                .read::<Position>()
                .read::<Velocity>()
                .write::<Heading>()
                .read_resource::<Tick>()
                .read_resource::<SimSeed>()
                .read_resource::<Hazard>(),
            sense,
        ))
        .add_parallel_system(parallel_system_fn(
            "heat",
            Access::new()
                .read::<Position>()
                .read::<Heat>()
                .write::<Heat>()
                .read_resource::<Tick>()
                .read_resource::<SimSeed>()
                .read_resource::<Hazard>(),
            heat,
        ))
        .add_parallel_system(parallel_system_fn(
            "census",
            Access::new()
                .read::<Position>()
                .read::<Velocity>()
                .write_resource::<Census>(),
            census,
        ))
        .add_parallel_system(parallel_system_fn(
            "tag",
            Access::new()
                .read::<Heat>()
                .read_resource::<Tick>()
                .read_resource::<SimSeed>()
                .structural(),
            tag,
        ))
        .add_system(system_fn("steer", steer))
        .add_system(system_fn("integrate", integrate))
        .add_parallel_system(parallel_system_fn(
            "emit",
            Access::new()
                .read_resource::<Tick>()
                .read_resource::<SimSeed>()
                .read_resource::<Census>()
                .structural(),
            emit,
        ))
        .set_stage_mode(mode);
    sim
}

/// Steps `sim` until `end`, recording `(tick, state_hash)` every [`HASH_EVERY`] ticks and at the
/// end; `on_step` observes the simulation after every step.
pub fn run_until(
    sim: &mut Simulation,
    end: u64,
    mut on_step: impl FnMut(&Simulation),
) -> Vec<(u64, u64)> {
    let mut hashes = Vec::new();
    while sim.tick() < end {
        sim.step(grimoire_sim::TickInput::default());
        on_step(sim);
        if sim.tick().is_multiple_of(HASH_EVERY) {
            hashes.push((sim.tick(), sim.state_hash()));
        }
    }
    if hashes.last().map(|&(tick, _)| tick) != Some(sim.tick()) {
        hashes.push((sim.tick(), sim.state_hash()));
    }
    hashes
}

/// Failure text: the final hash, then one `tick hash` line per checkpoint.
pub fn checkpoint_report(hashes: &[(u64, u64)]) -> String {
    let final_hash = hashes.last().map_or(0, |&(_, hash)| hash);
    let lines: Vec<String> = hashes
        .iter()
        .map(|&(tick, hash)| format!("  {tick:>6} {hash:#018x}"))
        .collect();
    format!(
        "measured final hash: {final_hash}; checkpoints (tick, state_hash):\n{}",
        lines.join("\n")
    )
}
