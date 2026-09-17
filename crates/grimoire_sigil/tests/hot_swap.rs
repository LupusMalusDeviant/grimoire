//! WP5.5 gate: hot swap with `replace_unit` and checked restore, in process without IPC
//! (Plan 0002 WP5.5, contract §11.8 "Tests").
//!
//! Units are hand-built from `docs/formats/sigil.md` §10 (`tests/support`). Unit `21` exists in
//! two versions with the same id; unit `22` is a bystander that no swap may touch.

mod support;

use std::sync::Arc;

use grimoire_core::{Vec2, hash_of};
use grimoire_ecs::{Entity, Executor, PermutedExecutor, SequentialExecutor};
use grimoire_sigil::{
    BehaviorRegistryBuilder, BulletPool, ContentEpoch, Emitter, SigilConfig, SigilContent,
    SigilError, SigilLibrary, SigilUnit, UnitId, install, replace_unit, restore_checked,
};
use grimoire_sim::{Simulation, TickInput};
use support::{
    NO_PROGRAM, Sections, Timing, When, block, block_kind, bullet_type, emitter, program,
};

const SWAPPED: u64 = 21;
const BYSTANDER: u64 = 22;

fn every(interval: u32) -> Timing {
    Timing {
        delay: 0,
        repeat: u32::MAX,
        interval,
    }
}

/// Version 1 of unit 21: one bullet type, a 4-shot ring every 10 ticks at speed 0.5.
fn v1() -> SigilUnit {
    Sections {
        bullet_types: vec![bullet_type(0, 0, 0)],
        programs: vec![program(block(block_kind::RING, 4, [0.0; 6]), &[])],
        emitters: vec![emitter(0, 0, 0, every(10), 0.5)],
        ..Sections::default()
    }
    .unit(SWAPPED)
}

/// Version 2 of unit 21: a second bullet type that reverses at age 3, fired as a 3-shot fan every
/// 10 ticks at speed 0.9.
fn v2() -> SigilUnit {
    Sections {
        bullet_types: vec![bullet_type(0, 0, 0), bullet_type(0, 0, 1)],
        programs: vec![
            program(block(block_kind::RING, 4, [0.0; 6]), &[]),
            program(
                block(block_kind::FAN, 3, [0.6, 0.0, 0.0, 0.0, 0.0, 0.0]),
                &[],
            ),
        ],
        emitters: vec![emitter(1, 1, 0, every(10), 0.9)],
        scripts: vec![support::script(1, None, &[support::reverse(When::Time(3))])],
        ..Sections::default()
    }
    .unit(SWAPPED)
}

/// Unit 22: a plain 2-shot ring every 5 ticks.
fn bystander() -> SigilUnit {
    Sections {
        bullet_types: vec![bullet_type(0, 0, 0)],
        programs: vec![program(block(block_kind::RING, 2, [0.0; 6]), &[])],
        emitters: vec![emitter(0, 0, 0, every(5), 0.3)],
        ..Sections::default()
    }
    .unit(BYSTANDER)
}

fn simulation(units: Vec<SigilUnit>, capacity: u32) -> Simulation {
    let registry = BehaviorRegistryBuilder::new(1).build();
    let library = SigilLibrary::new(units, Arc::clone(&registry)).expect("library builds");
    let mut sim = Simulation::new(5);
    install(
        &mut sim,
        library,
        registry,
        SigilConfig::new(capacity, Vec2::new(-1.0e4, -1.0e4), Vec2::new(1.0e4, 1.0e4)),
    )
    .expect("install succeeds");
    sim
}

fn add_emitter(sim: &mut Simulation, unit: u64, index: u16) -> Entity {
    sim.world_mut().spawn((Emitter {
        unit: UnitId(unit),
        emitter: index,
        origin: Vec2::ZERO,
        rotation: 0.0,
        started_at: 0,
    },))
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

fn epoch(sim: &Simulation) -> ContentEpoch {
    sim.world()
        .resource::<SigilContent>()
        .expect("content installed")
        .epoch()
}

fn unit_index(sim: &Simulation, unit: u64) -> u16 {
    sim.world()
        .resource::<SigilContent>()
        .and_then(|content| content.library().unit_index(UnitId(unit)))
        .expect("unit loaded")
}

fn count_of(sim: &Simulation, unit: u64) -> usize {
    let index = unit_index(sim, unit);
    pool(sim).iter().filter(|b| b.unit_index() == index).count()
}

#[test]
fn a_swap_takes_effect_at_the_next_tick_and_restarts_the_unit() {
    let mut sim = simulation(vec![v1(), bystander()], 256);
    let swapped_emitter = add_emitter(&mut sim, SWAPPED, 0);
    let bystander_emitter = add_emitter(&mut sim, BYSTANDER, 0);
    run(&mut sim, 15);
    let old_bullets = count_of(&sim, SWAPPED);
    let bystanders = count_of(&sim, BYSTANDER);
    assert_eq!(old_bullets, 8, "two v1 volleys");
    let events_before = pool(&sim).events().to_vec();
    let before = epoch(&sim);

    let report = replace_unit(&mut sim, v2()).expect("swap succeeds");
    assert_eq!(report.unit, UnitId(SWAPPED));
    assert_eq!(report.effective_tick, 15);
    assert_eq!(report.restarted_emitters, 1);
    assert_eq!(report.despawned_bullets, old_bullets as u32);
    assert_eq!(report.epoch, epoch(&sim));
    assert_eq!(report.epoch.swaps, before.swaps + 1);
    assert_ne!(report.epoch.manifest_hash, before.manifest_hash);

    assert_eq!(
        count_of(&sim, SWAPPED),
        0,
        "every bullet of the old layout is gone"
    );
    assert_eq!(
        count_of(&sim, BYSTANDER),
        bystanders,
        "other units are untouched"
    );
    assert_eq!(
        pool(&sim).events(),
        events_before.as_slice(),
        "no despawn events"
    );
    let started_at = |sim: &Simulation, entity| {
        sim.world()
            .get::<Emitter>(entity)
            .expect("emitter alive")
            .started_at
    };
    assert_eq!(started_at(&sim, swapped_emitter), 15);
    assert_eq!(started_at(&sim, bystander_emitter), 0);

    // Tick 15 runs with version 2: its emitter fires volley 0 of the new program at once.
    run(&mut sim, 1);
    let index = unit_index(&sim, SWAPPED);
    let fresh: Vec<_> = pool(&sim)
        .iter()
        .filter(|b| b.unit_index() == index)
        .collect();
    assert_eq!(fresh.len(), 3, "a 3-shot fan from v2");
    for bullet in &fresh {
        assert_eq!(bullet.bullet_type(), 1);
        assert!((bullet.velocity().length() - 0.9).abs() < 1e-4);
    }
    // The new unit's transform runs too, so the runtime cache follows the swap.
    let heading = fresh[0].velocity();
    let id = fresh[0].id();
    run(&mut sim, 3);
    let reversed = pool(&sim).get(id).expect("still alive").velocity();
    assert!((reversed + heading).length() < 1e-4, "reversed at age 3");
}

#[test]
fn state_hash_tells_epochs_apart_when_the_pool_is_identical() {
    let mut sim = simulation(vec![v1(), bystander()], 64);
    add_emitter(&mut sim, BYSTANDER, 0);
    run(&mut sim, 8);
    let pool_hash = hash_of(pool(&sim));
    let state_hash = sim.state_hash();
    let before = epoch(&sim);

    // A byte-identical unit is a swap too: same manifest, one more swap.
    let report = replace_unit(&mut sim, v1()).expect("swap succeeds");
    assert_eq!(
        (report.despawned_bullets, report.restarted_emitters),
        (0, 0)
    );
    assert_eq!(report.epoch.manifest_hash, before.manifest_hash);
    assert_eq!(report.epoch.swaps, 1);
    assert_eq!(hash_of(pool(&sim)), pool_hash);
    assert_ne!(sim.state_hash(), state_hash);
}

#[test]
fn swaps_always_move_the_epoch_forward() {
    let mut sim = simulation(vec![v1(), bystander()], 64);
    let installed = epoch(&sim);
    let first = replace_unit(&mut sim, v2()).expect("swap succeeds").epoch;
    let second = replace_unit(&mut sim, v2()).expect("swap succeeds").epoch;
    assert_ne!(first, second, "two swaps of the same unit");
    assert_eq!(first.manifest_hash, second.manifest_hash);
    let back = replace_unit(&mut sim, v1()).expect("swap succeeds").epoch;
    assert_eq!(
        back.manifest_hash, installed.manifest_hash,
        "A -> B -> A: same manifest"
    );
    assert_ne!(back, installed, "but a different epoch");
    assert_eq!(back.swaps, 3);
}

#[test]
fn restore_checked_rejects_a_foreign_epoch_and_restores_the_own_one() {
    let mut sim = simulation(vec![v1(), bystander()], 256);
    add_emitter(&mut sim, SWAPPED, 0);
    add_emitter(&mut sim, BYSTANDER, 0);
    run(&mut sim, 12);
    let before_swap = sim.snapshot();
    let installed = epoch(&sim);
    replace_unit(&mut sim, v2()).expect("swap succeeds");
    run(&mut sim, 6);
    assert!(!pool(&sim).is_empty(), "active bullets at snapshot time");

    // Foreign epoch: an error, and nothing changes.
    let hash = sim.state_hash();
    let tick = sim.tick();
    assert_eq!(
        restore_checked(&mut sim, &before_swap),
        Err(SigilError::ContentEpochMismatch {
            snapshot: Some(installed),
            loaded: Some(epoch(&sim)),
        })
    );
    assert_eq!((sim.state_hash(), sim.tick()), (hash, tick));

    // A snapshot without Sigil content is foreign too.
    let bare = Simulation::new(5).snapshot();
    assert_eq!(
        restore_checked(&mut sim, &bare),
        Err(SigilError::ContentEpochMismatch {
            snapshot: None,
            loaded: Some(epoch(&sim)),
        })
    );
    assert_eq!(sim.state_hash(), hash);

    // Own epoch: snapshot -> restore -> N ticks is bit-identical with active bullets.
    let after_swap = sim.snapshot();
    run(&mut sim, 20);
    let continued = sim.state_hash();
    restore_checked(&mut sim, &after_swap).expect("same epoch restores");
    assert_eq!(sim.state_hash(), hash);
    run(&mut sim, 20);
    assert_eq!(sim.state_hash(), continued);
}

#[test]
fn a_rejected_swap_changes_nothing() {
    let mut sim = simulation(vec![v1(), bystander()], 64);
    add_emitter(&mut sim, SWAPPED, 0);
    run(&mut sim, 11);
    let hash = sim.state_hash();

    let stranger = Sections {
        bullet_types: vec![bullet_type(0, 0, 0)],
        emitters: vec![emitter(0, NO_PROGRAM, 0, every(1), 1.0)],
        ..Sections::default()
    }
    .unit(99);
    assert_eq!(
        replace_unit(&mut sim, stranger),
        Err(SigilError::UnknownUnit(UnitId(99)))
    );
    assert_eq!(sim.state_hash(), hash);

    let unregistered = Sections {
        bullet_types: vec![bullet_type(0, 0, 0)],
        emitters: vec![emitter(0, NO_PROGRAM, 0, every(1), 1.0)],
        behavior_refs: vec![77],
        ..Sections::default()
    }
    .unit(SWAPPED);
    assert_eq!(
        replace_unit(&mut sim, unregistered),
        Err(SigilError::UnknownBehavior {
            unit: UnitId(SWAPPED),
            behavior: grimoire_sigil::BehaviorId(77),
        })
    );
    assert_eq!(sim.state_hash(), hash);

    let mut bare = Simulation::new(5);
    let bare_hash = bare.state_hash();
    assert_eq!(replace_unit(&mut bare, v1()), Err(SigilError::NotInstalled));
    assert_eq!(bare.state_hash(), bare_hash);
}

/// Unit 21 with `emitters` emitters, each a 400-shot ring every tick of bullets living 3 ticks
/// (more than three pool blocks in steady state with three emitters).
fn wide(emitters: usize) -> SigilUnit {
    Sections {
        bullet_types: vec![bullet_type(3, 0, 0)],
        programs: vec![program(block(block_kind::RING, 400, [0.0; 6]), &[])],
        emitters: vec![emitter(0, 0, 0, every(1), 0.2); emitters],
        ..Sections::default()
    }
    .unit(SWAPPED)
}

/// Runs the shrink-and-regrow session with `executor` and an optional game-spawned emitter with an
/// index no version of the unit has. Returns the per-tick state hashes, the final pool hash and
/// the reports of both swaps.
fn shrink_and_regrow(
    executor: Arc<dyn Executor>,
    invalid_emitter: bool,
) -> (Vec<u64>, u64, [grimoire_sigil::SwapReport; 2]) {
    let mut sim = simulation(vec![wide(3)], 4_096);
    sim.world_mut().set_executor(executor);
    for index in 0..3 {
        add_emitter(&mut sim, SWAPPED, index);
    }
    if invalid_emitter {
        add_emitter(&mut sim, SWAPPED, 5);
    }
    let mut hashes = Vec::new();
    let mut run_hashed = |sim: &mut Simulation, ticks: u32| {
        for _ in 0..ticks {
            sim.step(TickInput::default());
            hashes.push(sim.state_hash());
        }
    };
    run_hashed(&mut sim, 12);
    assert!(
        pool(&sim).slot_count() as usize > 3 * grimoire_ecs::QUERY_BLOCK_SIZE,
        "more than three pool blocks"
    );
    let shrink = replace_unit(&mut sim, wide(1)).expect("shrinking swap succeeds");
    run_hashed(&mut sim, 12);
    let regrow = replace_unit(&mut sim, wide(3)).expect("regrowing swap succeeds");
    run_hashed(&mut sim, 12);
    let pool_hash = hash_of(pool(&sim));
    (hashes, pool_hash, [shrink, regrow])
}

#[test]
fn shrinking_and_regrowing_a_unit_is_executor_independent_and_never_panics() {
    let (reference, pool_hash, [shrink, regrow]) =
        shrink_and_regrow(Arc::new(SequentialExecutor), true);
    assert_eq!(
        shrink.restarted_emitters, 1,
        "emitters 1 and 2 become inactive"
    );
    assert_eq!(
        regrow.restarted_emitters, 3,
        "and restart when the unit grows back"
    );
    assert!(regrow.effective_tick > shrink.effective_tick);

    for executor in [
        Arc::new(PermutedExecutor::new(1)) as Arc<dyn Executor>,
        Arc::new(PermutedExecutor::new(2)),
        Arc::new(PermutedExecutor::reversed()),
    ] {
        assert_eq!(shrink_and_regrow(executor, true).0, reference);
    }

    // The game-spawned emitter with index 5 never fires: without it the pool is identical.
    let (_, without_invalid, _) = shrink_and_regrow(Arc::new(SequentialExecutor), false);
    assert_eq!(without_invalid, pool_hash);
}
