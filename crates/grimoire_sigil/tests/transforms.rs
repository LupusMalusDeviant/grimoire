//! WP5.2 gate: transforms, triggers, events, behaviors, the cascade cap, clears and flags, driven
//! through [`install`] in a real [`Simulation`] (Plan 0002 WP5.2, contract §11.3–§11.6).
//!
//! Every unit is hand-built from `docs/formats/sigil.md` §10 (`tests/support`). A single emitter
//! fires one bullet at local tick `0` (tick `0` of the simulation), so after `1 + n` steps that
//! bullet has seen `n` updates and its `age` is `n` — the clock every `time` trigger reads.

mod support;

use std::sync::Arc;

use grimoire_core::Vec2;
use grimoire_ecs::{PermutedExecutor, SequentialExecutor, World, system_fn};
use grimoire_sigil::{
    BehaviorId, BehaviorInput, BehaviorOutcome, BehaviorRegistry, BehaviorRegistryBuilder,
    BulletFlags, BulletMotion, BulletPool, BulletSpawn, ClearFilter, ClearRequest, DespawnCause,
    Emitter, EventId, EventRequest, SigilConfig, SigilContent, SigilLibrary, SigilUnit, UnitId,
    install,
};
use grimoire_sim::{SimRng, Simulation, TickInput};
use support::{
    NO_PROGRAM, ONCE, Sections, Timing, When, become_emitter, block, block_kind, bullet_type,
    burst, change_type, emitter, flags, program, reverse, script,
};

const UNIT: u64 = 11;

fn empty_registry() -> Arc<BehaviorRegistry> {
    BehaviorRegistryBuilder::new(1).build()
}

/// A simulation with `unit` installed against `registry`, generous bounds and capacity.
fn simulation(seed: u64, unit: SigilUnit, registry: Arc<BehaviorRegistry>) -> Simulation {
    let library = SigilLibrary::new(vec![unit], Arc::clone(&registry)).expect("library builds");
    let mut sim = Simulation::new(seed);
    install(
        &mut sim,
        library,
        registry,
        SigilConfig::new(8192, Vec2::new(-1.0e4, -1.0e4), Vec2::new(1.0e4, 1.0e4)),
    )
    .expect("install succeeds");
    sim
}

fn add_emitter(sim: &mut Simulation, index: u16, origin: Vec2, rotation: f32) {
    sim.world_mut().spawn((Emitter {
        unit: UnitId(UNIT),
        emitter: index,
        origin,
        rotation,
        started_at: 0,
    },));
}

fn run(sim: &mut Simulation, ticks: u32) {
    for _ in 0..ticks {
        sim.step(TickInput::default());
    }
}

fn pool(sim: &Simulation) -> &BulletPool {
    sim.world()
        .resource::<BulletPool>()
        .expect("pool installed")
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

/// Type `0` moves along +x at speed 1 and reverses at age 5.
#[test]
fn reverse_on_a_time_trigger_flips_the_heading_once() {
    let unit = Sections {
        bullet_types: vec![bullet_type(0, 0, 0)],
        emitters: vec![emitter(0, NO_PROGRAM, 0, ONCE, 1.0)],
        scripts: vec![script(0, None, &[reverse(When::Time(5))])],
        ..Sections::default()
    }
    .unit(UNIT);
    let mut sim = simulation(1, unit, empty_registry());
    add_emitter(&mut sim, 0, Vec2::ZERO, 0.0);

    run(&mut sim, 1 + 5);
    let bullet = pool(&sim).iter().next().expect("one bullet");
    assert_eq!(bullet.age(), 5);
    assert!(approx(bullet.position().x, 5.0), "{:?}", bullet.position());
    assert!(approx(bullet.velocity().x, -1.0), "reversed at age 5");

    run(&mut sim, 3);
    let bullet = pool(&sim).iter().next().expect("one bullet");
    assert!(approx(bullet.position().x, 2.0), "{:?}", bullet.position());
    assert!(approx(bullet.velocity().x, -1.0), "reverses only once");
}

/// Type `0` (smashable) turns into type `1` (grazeable, lifetime 3) after 2.5 units; the new
/// type's clock starts at the change, so it expires three updates later.
#[test]
fn change_type_on_distance_swaps_type_and_flags_and_restarts_the_clock() {
    let unit = Sections {
        bullet_types: vec![
            bullet_type(0, flags::SMASHABLE, 0),
            bullet_type(3, flags::GRAZEABLE, 1),
        ],
        emitters: vec![emitter(0, NO_PROGRAM, 0, ONCE, 1.0)],
        scripts: vec![script(0, None, &[change_type(When::Distance(2.5), 1)])],
        ..Sections::default()
    }
    .unit(UNIT);
    let mut sim = simulation(2, unit, empty_registry());
    add_emitter(&mut sim, 0, Vec2::ZERO, 0.0);

    run(&mut sim, 1 + 2);
    let bullet = pool(&sim).iter().next().expect("one bullet");
    assert_eq!(bullet.bullet_type(), 0, "2 units travelled, not yet 2.5");
    assert!(
        approx(pool(&sim).columns().state[0][0], 2.0),
        "distance slot"
    );

    run(&mut sim, 1);
    let bullet = pool(&sim).iter().next().expect("one bullet");
    assert_eq!(bullet.bullet_type(), 1);
    assert_eq!(bullet.flags(), BulletFlags::GRAZEABLE);
    assert_eq!(bullet.age(), 0, "the new type's clock starts at the change");

    run(&mut sim, 2);
    assert_eq!(pool(&sim).len(), 1, "lifetime 3 counts from the change");
    run(&mut sim, 1);
    assert!(pool(&sim).is_empty());
    let event = pool(&sim).events()[0];
    assert_eq!(event.cause, DespawnCause::Lifetime);
    assert_eq!(event.bullet_type, 1);
}

/// Type `0` bursts at age 3 into a four-shot ring of type `1` at half speed.
#[test]
fn burst_despawns_the_parent_and_spawns_one_level_deeper() {
    let unit = Sections {
        bullet_types: vec![bullet_type(0, 0, 0), bullet_type(0, 0, 1)],
        programs: vec![program(block(block_kind::RING, 4, [0.0; 6]), &[])],
        emitters: vec![emitter(0, NO_PROGRAM, 0, ONCE, 1.0)],
        scripts: vec![script(0, None, &[burst(When::Time(3), 1, 0, 0.5)])],
        ..Sections::default()
    }
    .unit(UNIT);
    let mut sim = simulation(3, unit, empty_registry());
    add_emitter(&mut sim, 0, Vec2::new(10.0, 0.0), 0.0);

    run(&mut sim, 1 + 3);
    let pool = pool(&sim);
    assert_eq!(pool.len(), 4);
    let events = pool.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].cause, DespawnCause::Transform);
    assert!(approx(events[0].position.x, 13.0));
    let columns = pool.columns();
    for bullet in pool.iter() {
        let slot = bullet.id().index() as usize;
        assert_eq!(bullet.bullet_type(), 1);
        assert_eq!(bullet.age(), 0, "sub-spawns move from the next tick on");
        assert!(approx(bullet.position().x, 13.0));
        assert!(approx(bullet.velocity().length(), 0.5));
        assert_eq!(columns.cascade[slot], 1);
        assert_eq!(
            columns.program[slot], 1,
            "the burst block's program, one-based"
        );
    }
}

/// Type `0` becomes sub-emitter `1` (a three-shot fan of type `1`) at age 2.
#[test]
fn become_emitter_fires_the_sub_emitter_volley_at_the_bullet() {
    let spread = 0.5f32;
    let unit = Sections {
        bullet_types: vec![bullet_type(0, 0, 0), bullet_type(0, 0, 1)],
        programs: vec![program(
            block(block_kind::FAN, 3, [spread, 0.0, 0.0, 0.0, 0.0, 0.0]),
            &[],
        )],
        emitters: vec![
            emitter(0, NO_PROGRAM, 0, ONCE, 1.0),
            emitter(1, 0, 1, ONCE, 0.7),
        ],
        scripts: vec![script(0, None, &[become_emitter(When::Time(2), 1)])],
        ..Sections::default()
    }
    .unit(UNIT);
    let mut sim = simulation(4, unit, empty_registry());
    add_emitter(&mut sim, 0, Vec2::ZERO, 1.0);

    run(&mut sim, 1 + 2);
    let pool = pool(&sim);
    assert_eq!(pool.events()[0].cause, DespawnCause::Transform);
    let mut angles: Vec<f32> = pool
        .iter()
        .map(|bullet| {
            assert_eq!(bullet.bullet_type(), 1);
            assert!(approx(bullet.velocity().length(), 0.7));
            bullet.velocity().angle()
        })
        .collect();
    angles.sort_by(f32::total_cmp);
    assert_eq!(angles.len(), 3);
    assert!(approx(angles[0], 1.0 - spread / 2.0), "{angles:?}");
    assert!(approx(angles[1], 1.0), "centred on the parent's heading");
    assert!(approx(angles[2], 1.0 + spread / 2.0), "{angles:?}");
    assert!(pool.columns().cascade[..].iter().all(|&depth| depth <= 1));
}

/// A self-bursting type is legal for the decoder when no primary emitter reaches it; spawned by
/// hand at depth 2, its children reach the maximum depth 3 and its grandchildren are dropped.
#[test]
fn sub_spawns_beyond_the_maximum_depth_are_dropped_and_counted() {
    let unit = Sections {
        bullet_types: vec![bullet_type(0, 0, 0), bullet_type(0, 0, 1)],
        programs: vec![program(block(block_kind::RING, 2, [0.0; 6]), &[])],
        emitters: vec![emitter(1, NO_PROGRAM, 0, ONCE, 1.0)],
        scripts: vec![script(0, None, &[burst(When::Time(1), 0, 0, 1.0)])],
        ..Sections::default()
    }
    .unit(UNIT);
    let mut sim = simulation(5, unit, empty_registry());
    let content = sim
        .world()
        .resource::<SigilContent>()
        .cloned()
        .expect("content installed");
    sim.world_mut()
        .resource_mut::<BulletPool>()
        .expect("pool installed")
        .spawn(
            &content,
            BulletSpawn::new(UnitId(UNIT), 0, Vec2::ZERO, 0.0, 1.0).with_cascade(2),
        )
        .expect("spawn at depth 2");

    run(&mut sim, 1);
    assert_eq!(pool(&sim).len(), 2);
    assert!(pool(&sim).columns().cascade.iter().all(|&depth| depth == 3));
    assert_eq!(pool(&sim).dropped_spawns(), 0);

    run(&mut sim, 1);
    assert!(pool(&sim).is_empty(), "both depth-3 bullets burst");
    assert_eq!(
        pool(&sim).dropped_spawns(),
        4,
        "their depth-4 children are dropped"
    );
}

/// `event` triggers fire in every tick an `EventRequest` for them is live, and only then.
#[test]
fn event_requests_fire_event_triggers_for_one_tick() {
    let phase_end = EventId::from_name("phase_end");
    let unit = Sections {
        bullet_types: vec![bullet_type(0, 0, 0)],
        emitters: vec![emitter(0, NO_PROGRAM, 0, ONCE, 1.0)],
        scripts: vec![script(0, None, &[reverse(When::Event(phase_end.0))])],
        ..Sections::default()
    }
    .unit(UNIT);
    let mut sim = simulation(6, unit, empty_registry());
    add_emitter(&mut sim, 0, Vec2::ZERO, 0.0);
    run(&mut sim, 3);
    let velocity = |sim: &Simulation| pool(sim).iter().next().expect("bullet").velocity().x;
    assert!(approx(velocity(&sim), 1.0));

    sim.world_mut().spawn((EventRequest {
        event: EventId::from_name("other"),
    },));
    run(&mut sim, 1);
    assert!(approx(velocity(&sim), 1.0), "another event does not fire");
    assert_eq!(sim.world().query::<&EventRequest>().count(), 0, "consumed");

    sim.world_mut().spawn((EventRequest { event: phase_end },));
    sim.world_mut().spawn((EventRequest { event: phase_end },));
    run(&mut sim, 1);
    assert!(approx(velocity(&sim), -1.0), "two requests still fire once");
    assert_eq!(sim.world().query::<&EventRequest>().count(), 0);

    run(&mut sim, 2);
    assert!(approx(velocity(&sim), -1.0), "no request, no trigger");
}

/// Adds `params[0]` to the velocity's y component and despawns at `age >= params[1]`.
fn drift(input: &BehaviorInput<'_>, motion: &mut BulletMotion, _: &mut SimRng) -> BehaviorOutcome {
    motion.velocity.y += input.params[0];
    if input.age as f32 >= input.params[1] {
        BehaviorOutcome::Despawn
    } else {
        BehaviorOutcome::Keep
    }
}

/// Nudges the heading by a random amount from the block generator.
fn jitter(_: &BehaviorInput<'_>, motion: &mut BulletMotion, rng: &mut SimRng) -> BehaviorOutcome {
    let turn = (rng.next_f32() - 0.5) * 0.02;
    motion.velocity = motion.velocity + motion.velocity.perp() * turn;
    motion.state[0] += 1.0;
    BehaviorOutcome::Keep
}

const DRIFT: BehaviorId = BehaviorId(7);
const JITTER: BehaviorId = BehaviorId(9);

fn registry(drift_first: bool) -> Arc<BehaviorRegistry> {
    let mut builder = BehaviorRegistryBuilder::new(3);
    if drift_first {
        builder.register(DRIFT, "drift", drift).expect("unique");
        builder.register(JITTER, "jitter", jitter).expect("unique");
    } else {
        builder.register(JITTER, "jitter", jitter).expect("unique");
        builder.register(DRIFT, "drift", drift).expect("unique");
    }
    builder.build()
}

#[test]
fn a_bound_behavior_runs_every_tick_and_can_despawn() {
    let unit = Sections {
        bullet_types: vec![bullet_type(0, 0, 0)],
        emitters: vec![emitter(0, NO_PROGRAM, 0, ONCE, 1.0)],
        scripts: vec![script(0, Some((DRIFT.0, &[0.25, 4.0])), &[])],
        behavior_refs: vec![DRIFT.0, JITTER.0],
        ..Sections::default()
    }
    .unit(UNIT);
    let mut sim = simulation(7, unit, registry(true));
    add_emitter(&mut sim, 0, Vec2::ZERO, 0.0);

    run(&mut sim, 1 + 3);
    let bullet = pool(&sim).iter().next().expect("bullet");
    assert!(approx(bullet.velocity().y, 0.75), "{:?}", bullet.velocity());

    run(&mut sim, 1);
    assert!(pool(&sim).is_empty());
    assert_eq!(pool(&sim).events()[0].cause, DespawnCause::Behavior);
}

/// A unit that uses both behaviors on more than one pool block (1,024 slots each).
fn behavior_unit() -> SigilUnit {
    let forever = Timing {
        delay: 0,
        repeat: u32::MAX,
        interval: 1,
    };
    Sections {
        bullet_types: vec![bullet_type(0, 0, 0), bullet_type(0, 0, 1)],
        programs: vec![
            program(block(block_kind::RING, 48, [0.0; 6]), &[]),
            program(
                block(block_kind::SCATTER, 32, [1.0, 0.0, 0.3, 0.0, 0.0, 0.0]),
                &[],
            ),
        ],
        emitters: vec![
            emitter(0, 0, 0, forever, 0.3),
            emitter(1, 1, 0, forever, 0.4),
        ],
        scripts: vec![
            script(0, Some((DRIFT.0, &[0.001, 1.0e6])), &[]),
            script(1, Some((JITTER.0, &[])), &[reverse(When::Time(12))]),
        ],
        behavior_refs: vec![DRIFT.0, JITTER.0],
    }
    .unit(UNIT)
}

fn per_tick_hashes(sim: &mut Simulation, ticks: u32) -> Vec<u64> {
    (0..ticks)
        .map(|_| {
            sim.step(TickInput::default());
            sim.state_hash()
        })
        .collect()
}

/// Contract §11.5 and plan WP5.2: two worlds with the same registry entries, registered in a
/// different order, stepped with a different block execution order, hash identically.
#[test]
fn two_worlds_with_reordered_registries_and_executors_hash_identically() {
    assert_eq!(registry(true).fingerprint(), registry(false).fingerprint());

    let mut first = simulation(8, behavior_unit(), registry(true));
    add_emitter(&mut first, 0, Vec2::ZERO, 0.0);
    add_emitter(&mut first, 1, Vec2::new(5.0, 5.0), 0.5);
    first.world_mut().set_executor(Arc::new(SequentialExecutor));

    let mut second = simulation(8, behavior_unit(), registry(false));
    add_emitter(&mut second, 0, Vec2::ZERO, 0.0);
    add_emitter(&mut second, 1, Vec2::new(5.0, 5.0), 0.5);
    second
        .world_mut()
        .set_executor(Arc::new(PermutedExecutor::reversed()));

    let first_hashes = per_tick_hashes(&mut first, 40);
    assert!(
        pool(&first).slot_count() as usize > 2 * grimoire_ecs::QUERY_BLOCK_SIZE,
        "the gate must span several pool blocks"
    );
    assert_eq!(first_hashes, per_tick_hashes(&mut second, 40));
}

/// A clear requested by a system that runs before the `sigil.*` stages takes effect in the same
/// tick (PRD-0004 FR-12: at most one tick) and reports each bullet through the event hook.
#[test]
fn a_type_filtered_clear_applies_within_the_requesting_tick() {
    let forever = Timing {
        delay: 0,
        repeat: u32::MAX,
        interval: 1,
    };
    let unit = Sections {
        bullet_types: vec![bullet_type(0, 0, 0), bullet_type(0, 0, 1)],
        programs: vec![program(block(block_kind::RING, 3, [0.0; 6]), &[])],
        emitters: vec![
            emitter(0, 0, 0, forever, 0.5),
            emitter(1, 0, 0, forever, 0.5),
        ],
        ..Sections::default()
    }
    .unit(UNIT);
    let registry = empty_registry();
    let library = SigilLibrary::new(vec![unit], Arc::clone(&registry)).expect("library builds");
    let mut sim = Simulation::new(9);
    const CLEAR_TICK: u64 = 5;
    sim.schedule_mut()
        .add_system(system_fn("game.bomb", |world: &mut World| {
            let tick = world
                .resource::<grimoire_sim::Tick>()
                .copied()
                .unwrap_or_default();
            if tick.0 == CLEAR_TICK {
                world.spawn((ClearRequest {
                    filter: ClearFilter::Type(UnitId(UNIT), 1),
                },));
            }
        }));
    install(
        &mut sim,
        library,
        registry,
        SigilConfig::new(1024, Vec2::new(-1.0e4, -1.0e4), Vec2::new(1.0e4, 1.0e4)),
    )
    .expect("install succeeds");
    add_emitter(&mut sim, 0, Vec2::ZERO, 0.0);
    add_emitter(&mut sim, 1, Vec2::ZERO, 0.0);

    run(&mut sim, CLEAR_TICK as u32);
    let before = pool(&sim).iter().filter(|b| b.bullet_type() == 1).count();
    assert_eq!(before, 15);

    run(&mut sim, 1);
    let pool = pool(&sim);
    let cleared: Vec<_> = pool
        .events()
        .iter()
        .filter(|event| event.cause == DespawnCause::Clear)
        .collect();
    assert_eq!(
        cleared.len(),
        15 + 3,
        "every type-1 bullet, including this tick's volley"
    );
    assert!(cleared.iter().all(|event| event.bullet_type == 1));
    assert_eq!(pool.iter().filter(|b| b.bullet_type() == 1).count(), 0);
    assert_eq!(pool.iter().filter(|b| b.bullet_type() == 0).count(), 18);
    assert_eq!(sim.world().query::<&ClearRequest>().count(), 0);
}

/// Flags are stored per bullet and hashed (PRD-0004 FR-04: data only in P1).
#[test]
fn bullet_flags_are_stored_and_hashed() {
    let hash_with = |bullet_flags: u8| {
        let unit = Sections {
            bullet_types: vec![bullet_type(0, bullet_flags, 0)],
            emitters: vec![emitter(0, NO_PROGRAM, 0, ONCE, 1.0)],
            ..Sections::default()
        }
        .unit(UNIT);
        let mut sim = simulation(10, unit, empty_registry());
        add_emitter(&mut sim, 0, Vec2::ZERO, 0.0);
        run(&mut sim, 2);
        let stored = pool(&sim).iter().next().expect("bullet").flags();
        assert_eq!(stored, BulletFlags(bullet_flags));
        // The unit's content hash differs too; compare the pool alone to isolate the column.
        grimoire_core::hash_of(pool(&sim))
    };
    let all = flags::SMASHABLE | flags::REFLECTABLE | flags::ENV_ACTIVE | flags::GRAZEABLE;
    assert_ne!(hash_with(0), hash_with(all));
    assert_ne!(hash_with(flags::ENV_ACTIVE), hash_with(flags::REFLECTABLE));
}

/// Every transform kind and trigger kind in one cascade: seeds (behavior `drift`) become an
/// emitter of shards; shards change into embers after a distance; embers reverse on an event and
/// burst into dust. Depth 2.
fn cascade_unit() -> SigilUnit {
    let phase = EventId::from_name("phase").0;
    let volley = Timing {
        delay: 0,
        repeat: 3,
        interval: 20,
    };
    Sections {
        bullet_types: vec![
            bullet_type(0, flags::SMASHABLE, 0),
            bullet_type(0, flags::GRAZEABLE, 1),
            bullet_type(0, flags::GRAZEABLE, 2),
            bullet_type(90, 0, 3),
        ],
        programs: vec![
            program(
                block(block_kind::RING, 6, [0.2, 0.0, 0.0, 0.0, 0.0, 0.0]),
                &[],
            ),
            program(
                block(block_kind::FAN, 5, [1.2, 0.0, 0.0, 0.0, 0.0, 0.0]),
                &[support::modifier(
                    support::modifier_kind::ROTATE,
                    0,
                    0,
                    [0.01, 0.0, 0.0],
                )],
            ),
            program(
                block(block_kind::SCATTER, 3, [0.8, 0.0, 0.5, 0.0, 0.0, 0.0]),
                &[],
            ),
        ],
        emitters: vec![emitter(0, 0, 0, volley, 0.05), emitter(1, 1, 1, ONCE, 0.12)],
        scripts: vec![
            script(
                0,
                Some((DRIFT.0, &[0.0005, 1.0e6])),
                &[become_emitter(When::Time(30), 1)],
            ),
            script(1, None, &[change_type(When::Distance(2.0), 2)]),
            script(
                2,
                None,
                &[
                    reverse(When::Event(phase)),
                    burst(When::Time(25), 3, 2, 0.08),
                ],
            ),
        ],
        behavior_refs: vec![DRIFT.0, JITTER.0],
    }
    .unit(UNIT)
}

fn cascade_simulation(seed: u64) -> Simulation {
    let mut sim = simulation(seed, cascade_unit(), registry(true));
    sim.schedule_mut()
        .add_system(system_fn("game.phase", |world: &mut World| {
            let tick = world
                .resource::<grimoire_sim::Tick>()
                .copied()
                .unwrap_or_default();
            if tick.0 % 37 == 36 {
                world.spawn((EventRequest {
                    event: EventId::from_name("phase"),
                },));
            }
        }));
    add_emitter(&mut sim, 0, Vec2::ZERO, 0.0);
    add_emitter(&mut sim, 0, Vec2::new(-6.0, 3.0), 1.3);
    sim
}

#[test]
fn snapshot_restore_is_bit_identical_in_the_middle_of_a_cascade() {
    let mut sim = cascade_simulation(12);
    run(&mut sim, 70);
    let types: std::collections::BTreeSet<u16> =
        pool(&sim).iter().map(|b| b.bullet_type()).collect();
    assert!(
        types.len() >= 2,
        "snapshot must see several generations, saw {types:?}"
    );
    let snapshot = sim.snapshot();

    let mut continued = sim;
    run(&mut continued, 60);

    let mut restored = cascade_simulation(999);
    restored.restore(&snapshot);
    run(&mut restored, 60);
    assert_eq!(continued.state_hash(), restored.state_hash());
}

/// Frozen `state_hash` after 150 ticks of [`cascade_simulation`] (seed `2026`): every transform
/// kind, every trigger kind, a behavior, events and scatter sub-spawns, equal on every platform.
/// If this fails after an *intentional* runtime change, the assertion prints the new value; a
/// renewal is a Product Owner decision (contract §2b).
const GOLDEN_CASCADE_HASH: u64 = 0xba8a_f2d0_284f_b962;

#[test]
fn golden_hash_cascade_unit() {
    let mut sim = cascade_simulation(2026);
    run(&mut sim, 150);
    let dust = pool(&sim).iter().filter(|b| b.bullet_type() == 3).count();
    assert!(dust > 0, "the cascade must reach its last generation");
    let hash = sim.state_hash();
    assert_eq!(
        hash, GOLDEN_CASCADE_HASH,
        "cascade-unit state hash changed; if intentional, GOLDEN_CASCADE_HASH = {hash:#018x}"
    );
}
