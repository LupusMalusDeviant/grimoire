//! P0 determinism gate (PRD-0018 FR-04, plan WP5.6).
//!
//! A demo simulation with at least 2 000 moving entities, a random spawner and despawner,
//! component inserts/removes and input-driven steering runs 10 000 ticks. The tests check:
//!
//! 1. two independent runs produce identical hash sequences (every 600 ticks and the final tick);
//! 2. a snapshot at tick 4 000, restored into a freshly built simulation, continues identically;
//! 3. `replay` over the recorded (and re-encoded) `InputLog` reproduces the hash sequence;
//! 4. the final hash equals a golden constant, which makes CI on every platform a
//!    cross-platform determinism check.

use std::sync::OnceLock;

use grimoire_core::math::dmath;
use grimoire_core::{Vec2, impl_stable_hash};
use grimoire_ecs::{CommandBuffer, Entity, With, Without, World, system_fn};
use grimoire_sim::{
    InputFrame, InputLog, SimRng, SimSeed, SimSnapshot, Simulation, Tick, TickInput, derive_rng,
    replay,
};

/// Final `Simulation::state_hash` of the demo scenario after 10 000 ticks.
///
/// This constant is the cross-platform determinism gate: CI runs this test on Windows, Linux and
/// macOS, and every platform must reproduce the value measured on the development machine
/// (Windows x86_64). A mismatch on one platform means floating-point or integer behaviour
/// diverges there and must be investigated, never papered over.
///
/// Renew it only when the outcome is supposed to change: a deliberate change of this scenario,
/// of `World::stable_hash` / `Simulation::state_hash`, of `StableHasher::ALGORITHM_VERSION`, of
/// `SimRng::ALGORITHM_VERSION` or of ECS ordering semantics. Mention the renewal and its reason in
/// the commit message.
const GOLDEN_FINAL_HASH: u64 = 654_972_031_850_314_100;

const SEED: u64 = 0xF1E2_D3C4_B5A6_9788;
const TICK_RATE_HZ: u32 = 60;
const TOTAL_TICKS: u64 = 10_000;
const HASH_EVERY: u64 = 600;
const SNAPSHOT_TICK: u64 = 4_000;

const MIN_ENTITIES: usize = 2_000;
const MAX_ENTITIES: usize = 3_000;
const DT: f32 = 1.0 / 60.0;
const ARENA_HALF: f32 = 512.0;
const MAX_SPEED: f32 = 120.0;
const STEER_ACCEL: f32 = 90.0;

const STREAM_INIT: u64 = 1;
const STREAM_AGITATE: u64 = 2;
const STREAM_CULL: u64 = 3;
const STREAM_SPAWN: u64 = 4;

#[derive(Clone)]
struct Position {
    value: Vec2,
}
impl_stable_hash!(Position { value });

#[derive(Clone)]
struct Velocity {
    value: Vec2,
}
impl_stable_hash!(Velocity { value });

#[derive(Clone)]
struct Lifetime {
    ticks_left: u32,
}
impl_stable_hash!(Lifetime { ticks_left });

/// Marker with data, inserted and removed at random so entities move between archetypes.
#[derive(Clone)]
struct Agitated {
    strength: f32,
}
impl_stable_hash!(Agitated { strength });

/// Scripted bot input: a pure function of the tick, integer arithmetic only.
fn scripted_input(tick: u64) -> TickInput {
    const DIRECTIONS: [(i16, i16); 6] = [
        (32_767, 0),
        (0, 32_767),
        (-32_767, 0),
        (0, -32_768),
        (23_170, 23_170),
        (0, 0),
    ];
    let (x, y) = DIRECTIONS[((tick / 240) % 6) as usize];
    let boost = u32::from(tick % 900 < 120);
    let purge = u32::from((2_000..2_300).contains(&(tick % 5_000)));
    let mut input = TickInput::default();
    input.slots[0] = InputFrame {
        axes: [x, y, 0, 0],
        buttons: boost | (purge << 1),
    };
    input.slots[1] = InputFrame {
        axes: [0, 0, ((tick % 1_000) as i16) - 500, 0],
        buttons: u32::from(tick % 1_300 < 200),
    };
    input
}

fn seed_and_tick(world: &World) -> (u64, u64) {
    let seed = world.resource::<SimSeed>().map_or(0, |seed| seed.0);
    let tick = world.resource::<Tick>().map_or(0, |tick| tick.0);
    (seed, tick)
}

fn current_input(world: &World) -> TickInput {
    world.resource::<TickInput>().copied().unwrap_or_default()
}

fn spawn_agent(world: &mut World, rng: &mut SimRng) {
    let x = rng.range_f32(-ARENA_HALF, ARENA_HALF);
    let y = rng.range_f32(-ARENA_HALF, ARENA_HALF);
    let heading = rng.range_f32(0.0, dmath::TAU);
    let speed = rng.range_f32(20.0, 80.0);
    let ticks_left = rng.range_u32(300, 1_500);
    world.spawn((
        Position {
            value: Vec2::new(x, y),
        },
        Velocity {
            value: Vec2::from_angle(heading) * speed,
        },
        Lifetime { ticks_left },
    ));
}

/// Slot 0 axes steer every entity, button 0 boosts; a tick-driven swirl rotates velocities.
fn steer(world: &mut World) {
    let (_, tick) = seed_and_tick(world);
    let pilot = current_input(world).slots[0];
    let push = Vec2::new(pilot.axis(0), pilot.axis(1)) * (STEER_ACCEL * DT);
    let swirl = dmath::sin(tick as f32 * 0.01) * 0.02;
    let max_speed = if pilot.is_pressed(0) {
        MAX_SPEED * 1.5
    } else {
        MAX_SPEED
    };
    for (velocity, agitated) in world.query_mut::<(&mut Velocity, Option<&Agitated>)>() {
        let spin = swirl + agitated.map_or(0.0, |agitated| agitated.strength * 0.01);
        let turned = (velocity.value + push).rotate(spin);
        velocity.value = if turned.length_squared() > max_speed * max_speed {
            turned.normalize_or_zero() * max_speed
        } else {
            turned
        };
    }
}

/// Slot 1 button 0 agitates random entities; while released, agitation wears off at random.
fn agitate(world: &mut World) {
    let (seed, tick) = seed_and_tick(world);
    let mut rng = derive_rng(seed, tick, STREAM_AGITATE);
    let mut commands = CommandBuffer::new();
    if current_input(world).slots[1].is_pressed(0) {
        for (entity, ()) in world.query::<(Entity, Without<Agitated>)>() {
            if rng.chance(0.01) {
                let strength = rng.range_f32(0.5, 2.0);
                commands.insert(entity, Agitated { strength });
            }
        }
    } else {
        for (entity, ()) in world.query::<(Entity, With<Agitated>)>() {
            if rng.chance(0.05) {
                commands.remove::<Agitated>(entity);
            }
        }
    }
    commands.apply(world);
}

/// Moves entities and wraps them around the square arena.
fn integrate(world: &mut World) {
    for (position, velocity) in world.query_mut::<(&mut Position, &Velocity)>() {
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
    }
}

/// Ages entities; expired ones, a few random ones and (while slot 0 button 1 is held) many in
/// the right half of the arena are despawned.
fn age_and_cull(world: &mut World) {
    let (seed, tick) = seed_and_tick(world);
    let purge = current_input(world).slots[0].is_pressed(1);
    let mut rng = derive_rng(seed, tick, STREAM_CULL);
    let mut commands = CommandBuffer::new();
    for (entity, lifetime, position) in world.query_mut::<(Entity, &mut Lifetime, &Position)>() {
        lifetime.ticks_left = lifetime.ticks_left.saturating_sub(1);
        let random_death = rng.chance(0.000_5);
        let purged = purge && position.value.x > 0.0 && rng.chance(0.02);
        if lifetime.ticks_left == 0 || random_death || purged {
            commands.despawn(entity);
        }
    }
    commands.apply(world);
}

/// Refills the population to at least `MIN_ENTITIES` plus a random surplus, capped.
fn spawn(world: &mut World) {
    let (seed, tick) = seed_and_tick(world);
    let mut rng = derive_rng(seed, tick, STREAM_SPAWN);
    let count = world.entity_count();
    let wanted = MIN_ENTITIES.saturating_sub(count) + rng.range_u32(0, 4) as usize;
    let room = MAX_ENTITIES.saturating_sub(count);
    for _ in 0..wanted.min(room) {
        spawn_agent(world, &mut rng);
    }
}

fn build_simulation(seed: u64) -> Simulation {
    let mut sim = Simulation::new(seed);
    let world = sim.world_mut();
    world.register_component::<Position>();
    world.register_component::<Velocity>();
    world.register_component::<Lifetime>();
    world.register_component::<Agitated>();
    let mut rng = derive_rng(seed, 0, STREAM_INIT);
    for _ in 0..MIN_ENTITIES {
        spawn_agent(world, &mut rng);
    }
    sim.schedule_mut()
        .add_system(system_fn("steer", steer))
        .add_system(system_fn("agitate", agitate))
        .add_system(system_fn("integrate", integrate))
        .add_system(system_fn("age_and_cull", age_and_cull))
        .add_system(system_fn("spawn", spawn));
    sim
}

/// Steps `sim` with the scripted input until `end`, recording checkpoints like `replay` does.
fn run_until(
    sim: &mut Simulation,
    end: u64,
    mut on_step: impl FnMut(&Simulation),
) -> Vec<(u64, u64)> {
    let mut hashes = Vec::new();
    while sim.tick() < end {
        sim.step(scripted_input(sim.tick()));
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

struct Recording {
    hashes: Vec<(u64, u64)>,
    log: InputLog,
    snapshot: SimSnapshot,
    snapshot_hash: u64,
    min_entities: usize,
    max_entities: usize,
    max_agitated: usize,
    despawns_seen: bool,
}

fn record() -> Recording {
    let mut sim = build_simulation(SEED);
    let mut log = InputLog {
        seed: SEED,
        tick_rate_hz: TICK_RATE_HZ,
        frames: Vec::new(),
    };
    let mut snapshot = None;
    let mut min_entities = usize::MAX;
    let mut max_entities = 0;
    let mut max_agitated = 0;
    let mut despawns_seen = false;
    let hashes = run_until(&mut sim, TOTAL_TICKS, |sim| {
        let tick = sim.tick();
        log.frames.push(scripted_input(tick - 1));
        let world = sim.world();
        min_entities = min_entities.min(world.entity_count());
        max_entities = max_entities.max(world.entity_count());
        if tick.is_multiple_of(50) {
            max_agitated = max_agitated.max(world.query::<(With<Agitated>,)>().count());
            despawns_seen |= world
                .query::<(Entity,)>()
                .any(|(entity,)| entity.generation() > 0);
        }
        if tick == SNAPSHOT_TICK {
            snapshot = Some((sim.snapshot(), sim.state_hash()));
        }
    });
    let (snapshot, snapshot_hash) = snapshot.expect("the run passes the snapshot tick");
    Recording {
        hashes,
        log,
        snapshot,
        snapshot_hash,
        min_entities,
        max_entities,
        max_agitated,
        despawns_seen,
    }
}

fn reference() -> &'static Recording {
    static REFERENCE: OnceLock<Recording> = OnceLock::new();
    REFERENCE.get_or_init(record)
}

#[test]
fn scenario_exercises_the_engine() {
    let reference = reference();
    assert!(
        reference.min_entities >= MIN_ENTITIES,
        "{}",
        reference.min_entities
    );
    assert!(reference.max_entities <= MAX_ENTITIES);
    assert!(reference.max_entities > MIN_ENTITIES);
    assert!(reference.max_agitated > 0);
    assert!(reference.despawns_seen);
    assert_eq!(reference.log.frames.len(), TOTAL_TICKS as usize);
    let ticks: Vec<u64> = reference.hashes.iter().map(|&(tick, _)| tick).collect();
    let expected: Vec<u64> = (1..=16)
        .map(|n| n * HASH_EVERY)
        .chain([TOTAL_TICKS])
        .collect();
    assert_eq!(ticks, expected);
}

#[test]
fn double_run_gives_identical_hash_sequences() {
    let mut second = build_simulation(SEED);
    let hashes = run_until(&mut second, TOTAL_TICKS, |_| {});
    assert_eq!(hashes, reference().hashes);
}

#[test]
fn restored_snapshot_continues_identically() {
    let reference = reference();
    let mut restored = build_simulation(SEED);
    restored.restore(&reference.snapshot);
    assert_eq!(restored.tick(), SNAPSHOT_TICK);
    assert_eq!(restored.state_hash(), reference.snapshot_hash);

    let hashes = run_until(&mut restored, TOTAL_TICKS, |_| {});
    let expected: Vec<(u64, u64)> = reference
        .hashes
        .iter()
        .copied()
        .filter(|&(tick, _)| tick > SNAPSHOT_TICK)
        .collect();
    assert_eq!(hashes, expected);
}

#[test]
fn replay_of_the_recorded_log_reproduces_the_run() {
    let reference = reference();
    let decoded = InputLog::from_bytes(&reference.log.to_bytes()).expect("valid log");
    assert_eq!(decoded, reference.log);
    let mut sim = build_simulation(decoded.seed);
    assert_eq!(replay(&mut sim, &decoded, HASH_EVERY), reference.hashes);
}

#[test]
fn final_hash_matches_golden_value() {
    let &(tick, hash) = reference()
        .hashes
        .last()
        .expect("at least the final checkpoint");
    assert_eq!(tick, TOTAL_TICKS);
    assert_eq!(hash, GOLDEN_FINAL_HASH, "measured final hash: {hash}");
}
