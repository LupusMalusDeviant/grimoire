//! `Simulation` semantics: initial resources, step order, state hash, snapshots and `replay`.

use grimoire_core::impl_stable_hash;
use grimoire_ecs::system_fn;
use grimoire_sim::{InputFrame, InputLog, SimSeed, Simulation, Tick, TickInput, replay};

#[derive(Clone, Debug, Default, PartialEq)]
struct Trace {
    seen: Vec<(u64, u64, u32)>,
}
impl_stable_hash!(Trace { seen });

#[derive(Clone, Debug, PartialEq)]
struct Counter {
    value: u64,
}
impl_stable_hash!(Counter { value });

fn input(buttons: u32) -> TickInput {
    let mut input = TickInput::default();
    input.slots[0] = InputFrame {
        axes: [0; 4],
        buttons,
    };
    input
}

/// Simulation whose only system appends (tick, seed, buttons of slot 0) to the `Trace` resource
/// and counts the sum of all buttons in an entity.
fn traced(seed: u64) -> Simulation {
    let mut sim = Simulation::new(seed);
    sim.world_mut().insert_resource(Trace::default());
    sim.world_mut().spawn((Counter { value: 0 },));
    sim.schedule_mut().add_system(system_fn("trace", |world| {
        let tick = world.resource::<Tick>().map_or(u64::MAX, |tick| tick.0);
        let seed = world.resource::<SimSeed>().map_or(u64::MAX, |seed| seed.0);
        let buttons = world
            .resource::<TickInput>()
            .map_or(u32::MAX, |input| input.slots[0].buttons);
        if let Some(trace) = world.resource_mut::<Trace>() {
            trace.seen.push((tick, seed, buttons));
        }
        for (counter,) in world.query_mut::<(&mut Counter,)>() {
            counter.value += u64::from(buttons);
        }
    }));
    sim
}

#[test]
fn new_creates_the_simulation_resources() {
    let sim = Simulation::new(77);
    assert_eq!(sim.seed(), 77);
    assert_eq!(sim.tick(), 0);
    assert_eq!(sim.world().resource::<Tick>(), Some(&Tick(0)));
    assert_eq!(sim.world().resource::<SimSeed>(), Some(&SimSeed(77)));
    assert_eq!(
        sim.world().resource::<TickInput>(),
        Some(&TickInput::default())
    );
    assert_eq!(sim.world().entity_count(), 0);
}

#[test]
fn step_sets_input_runs_schedule_then_increments_tick() {
    let mut sim = traced(5);
    for buttons in [1, 2, 3] {
        sim.step(input(buttons));
    }
    assert_eq!(sim.tick(), 3);
    assert_eq!(sim.world().resource::<Tick>(), Some(&Tick(3)));
    assert_eq!(sim.world().resource::<TickInput>(), Some(&input(3)));
    assert_eq!(
        sim.world()
            .resource::<Trace>()
            .map(|trace| trace.seen.clone()),
        Some(vec![(0, 5, 1), (1, 5, 2), (2, 5, 3)])
    );
}

#[test]
fn step_overwrites_tampered_tick_and_seed() {
    let mut sim = Simulation::new(1);
    sim.schedule_mut().add_system(system_fn("tamper", |world| {
        world.insert_resource(Tick(999));
        world.insert_resource(SimSeed(999));
    }));
    sim.world_mut().remove_resource::<Tick>();
    sim.world_mut().remove_resource::<SimSeed>();
    sim.step(TickInput::default());
    sim.step(TickInput::default());
    assert_eq!(sim.tick(), 2);
    assert_eq!(sim.seed(), 1);
    assert_eq!(sim.world().resource::<Tick>(), Some(&Tick(2)));

    let mut observed = Simulation::new(1);
    observed
        .schedule_mut()
        .add_system(system_fn("record", |world| {
            let seed = world.resource::<SimSeed>().map_or(0, |seed| seed.0);
            world.insert_resource(Trace {
                seen: vec![(0, seed, 0)],
            });
            world.insert_resource(SimSeed(999));
        }));
    observed.step(TickInput::default());
    observed.step(TickInput::default());
    // The second step saw the real seed again although the first step overwrote it.
    assert_eq!(
        observed
            .world()
            .resource::<Trace>()
            .map(|trace| trace.seen.clone()),
        Some(vec![(0, 1, 0)])
    );
}

#[test]
fn state_hash_covers_tick_seed_and_world() {
    let a = traced(1);
    let b = traced(1);
    assert_eq!(a.state_hash(), b.state_hash());
    assert_ne!(a.state_hash(), traced(2).state_hash());

    let mut stepped = traced(1);
    stepped.step(TickInput::default());
    assert_ne!(stepped.state_hash(), a.state_hash());

    let mut with_input = traced(1);
    with_input.step(input(4));
    assert_ne!(with_input.state_hash(), stepped.state_hash());

    let mut moved = traced(1);
    moved.world_mut().spawn((Counter { value: 1 },));
    assert_ne!(moved.state_hash(), a.state_hash());
}

#[test]
fn snapshot_and_restore_continue_identically() {
    let mut sim = traced(9);
    for buttons in 0..5 {
        sim.step(input(buttons));
    }
    let snapshot = sim.snapshot();
    assert_eq!(snapshot.tick(), 5);
    assert_eq!(snapshot.seed(), 9);
    let hash_at_5 = sim.state_hash();

    for buttons in 5..10 {
        sim.step(input(buttons));
    }
    let hash_at_10 = sim.state_hash();

    sim.restore(&snapshot);
    assert_eq!(sim.tick(), 5);
    assert_eq!(sim.state_hash(), hash_at_5);

    let mut fresh = traced(12_345);
    fresh.restore(&snapshot);
    assert_eq!(fresh.seed(), 9);
    assert_eq!(fresh.state_hash(), hash_at_5);

    for buttons in 5..10 {
        sim.step(input(buttons));
        fresh.step(input(buttons));
    }
    assert_eq!(sim.state_hash(), hash_at_10);
    assert_eq!(fresh.state_hash(), hash_at_10);
    assert_eq!(snapshot.clone().tick(), 5);
}

fn log_of(buttons: impl IntoIterator<Item = u32>) -> InputLog {
    InputLog {
        seed: 3,
        tick_rate_hz: 60,
        frames: buttons.into_iter().map(input).collect(),
    }
}

#[test]
fn replay_records_checkpoints_and_the_final_tick() {
    let log = log_of(1..=5);
    let mut recorded = traced(3);
    let mut expected = Vec::new();
    for &frame in &log.frames {
        recorded.step(frame);
        expected.push((recorded.tick(), recorded.state_hash()));
    }

    assert_eq!(replay(&mut traced(3), &log, 1), expected);
    assert_eq!(
        replay(&mut traced(3), &log, 2),
        vec![expected[1], expected[3], expected[4]]
    );
    assert_eq!(replay(&mut traced(3), &log, 0), vec![expected[4]]);
    assert_eq!(replay(&mut traced(3), &log, 100), vec![expected[4]]);

    let four = log_of(1..=4);
    assert_eq!(
        replay(&mut traced(3), &four, 2),
        vec![expected[1], expected[3]]
    );
}

#[test]
fn replay_of_an_empty_log_records_the_current_state() {
    let mut sim = traced(3);
    let hash = sim.state_hash();
    assert_eq!(replay(&mut sim, &log_of([]), 10), vec![(0, hash)]);
}
