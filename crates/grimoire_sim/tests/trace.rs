//! Subsystem hashes and divergence diagnosis (contract §8.5, OF-18.1, Plan 0002 WP7.2): recording
//! never changes a state hash, system hashes do not depend on stage mode or executor, and
//! `first_divergence` names the first diverging tick and system at every granularity.

mod parallel_scenario;

use std::num::NonZeroU64;
use std::sync::Arc;

use grimoire_core::impl_stable_hash;
use grimoire_ecs::{Executor, PermutedExecutor, SequentialExecutor, StageMode, system_fn};
use grimoire_sim::{
    HashTrace, InputFrame, InputLog, SystemHasher, Tick, TickInput, TraceGranularity,
    first_divergence, replay, system_hash, trace,
};

#[derive(Clone, Debug, PartialEq)]
struct Counter {
    value: u64,
}
impl_stable_hash!(Counter { value });

#[derive(Clone, Debug, PartialEq)]
struct Contacts {
    value: u64,
}
impl_stable_hash!(Contacts { value });

/// The step in which the faulty `collide.resolve` first behaves differently (`Tick` during the
/// step), so the first diverging state is the one after it: tick `FAULT_TICK + 1`.
const FAULT_TICK: u64 = 37;

fn every(interval: u64) -> TraceGranularity {
    TraceGranularity::every(NonZeroU64::new(interval).expect("interval is not zero"))
}

fn tick_of(world: &grimoire_ecs::World) -> u64 {
    world.resource::<Tick>().map_or(0, |tick| tick.0)
}

/// Three systems of three subsystems: `app.input` adds the buttons to `Counter`, `collide.resolve`
/// counts one contact per tick (two from [`FAULT_TICK`] on if `faulty`), `sigil.update` mixes both
/// into `Counter`. With `extra_system`, a fourth system `app.audit` runs last and changes nothing.
fn build(seed: u64, faulty: bool, extra_system: bool) -> grimoire_sim::Simulation {
    let mut sim = grimoire_sim::Simulation::new(seed);
    sim.world_mut().insert_resource(Counter { value: 0 });
    sim.world_mut().insert_resource(Contacts { value: 0 });
    sim.schedule_mut()
        .add_system(system_fn("app.input", |world| {
            let buttons = world
                .resource::<TickInput>()
                .map_or(0, |input| input.slots[0].buttons);
            if let Some(counter) = world.resource_mut::<Counter>() {
                counter.value += u64::from(buttons);
            }
        }))
        .add_system(system_fn("collide.resolve", move |world| {
            let extra = u64::from(faulty && tick_of(world) >= FAULT_TICK);
            if let Some(contacts) = world.resource_mut::<Contacts>() {
                contacts.value += 1 + extra;
            }
        }))
        .add_system(system_fn("sigil.update", |world| {
            let contacts = world.resource::<Contacts>().map_or(0, |c| c.value);
            if let Some(counter) = world.resource_mut::<Counter>() {
                counter.value = counter.value.wrapping_mul(31).wrapping_add(contacts);
            }
        }));
    if extra_system {
        sim.schedule_mut()
            .add_system(system_fn("app.audit", |_world| {}));
    }
    sim
}

fn log(seed: u64, ticks: u32) -> InputLog {
    InputLog {
        seed,
        tick_rate_hz: 60,
        frames: (0..ticks)
            .map(|tick| {
                let mut input = TickInput::default();
                input.slots[0] = InputFrame {
                    axes: [0; 4],
                    buttons: tick % 5,
                };
                input
            })
            .collect(),
    }
}

fn recorded(faulty: bool, granularity: TraceGranularity) -> HashTrace {
    let log = log(9, 60);
    trace(&mut build(log.seed, faulty, false), &log, granularity)
}

// --- recording --------------------------------------------------------------------------------

#[test]
fn a_trace_records_the_states_replay_reaches_at_every_granularity() {
    let log = log(9, 60);
    let replayed = replay(&mut build(9, false, false), &log, 1);
    for granularity in [
        TraceGranularity::PER_SYSTEM_PER_TICK,
        every(1),
        every(7),
        every(7).with_system_hashes(),
        every(600),
    ] {
        let traced = trace(&mut build(9, false, false), &log, granularity);
        assert_eq!(traced.granularity, granularity);
        assert_eq!(
            traced.system_names,
            vec!["app.input", "collide.resolve", "sigil.update"]
        );
        let start = &traced.checkpoints[0];
        assert_eq!(start.tick, 0);
        assert_eq!(start.state_hash, build(9, false, false).state_hash());
        assert!(start.system_hashes.is_empty());
        let interval = granularity.interval().get();
        let expected_ticks: Vec<u64> = std::iter::once(0)
            .chain((1..=60).filter(|tick| tick % interval == 0 || *tick == 60))
            .collect();
        let ticks: Vec<u64> = traced.checkpoints.iter().map(|c| c.tick).collect();
        assert_eq!(ticks, expected_ticks, "{granularity:?}");
        for checkpoint in &traced.checkpoints[1..] {
            let (_, hash) = replayed[checkpoint.tick as usize - 1];
            assert_eq!(checkpoint.state_hash, hash, "tick {}", checkpoint.tick);
            let systems = if granularity.has_system_hashes() {
                3
            } else {
                0
            };
            assert_eq!(checkpoint.system_hashes.len(), systems);
        }
    }
}

#[test]
fn system_hashes_follow_the_world_after_each_system() {
    let mut sim = build(9, false, false);
    let mut hasher = SystemHasher::new();
    sim.step_observed(TickInput::default(), &mut hasher);
    let hashes = hasher.hashes().to_vec();
    assert_eq!(hashes.len(), 3);
    // `sigil.update` is the last system, and after the step only `Tick` changes: rewriting it
    // with the value it had during the step restores exactly the world the last hash saw.
    sim.world_mut().insert_resource(Tick(0));
    assert_eq!(hashes[2], system_hash(sim.world()));
    // `app.input` saw no buttons, so it changed nothing; `collide.resolve` and `sigil.update` did.
    assert_ne!(hashes[0], hashes[1]);
    assert_ne!(hashes[1], hashes[2]);

    hasher.clear();
    assert!(hasher.hashes().is_empty());
}

#[test]
fn recording_system_hashes_never_changes_a_state_hash() {
    let log = log(3, 40);
    let plain = replay(&mut build(3, false, false), &log, 1);
    let mut observed = build(3, false, false);
    let traced = trace(&mut observed, &log, TraceGranularity::PER_SYSTEM_PER_TICK);
    let traced_hashes: Vec<(u64, u64)> = traced.checkpoints[1..]
        .iter()
        .map(|checkpoint| (checkpoint.tick, checkpoint.state_hash))
        .collect();
    assert_eq!(traced_hashes, plain);
    assert_eq!(observed.state_hash(), plain.last().expect("40 ticks").1);
}

#[test]
fn an_empty_log_records_only_the_start() {
    let empty = log(9, 0);
    let traced = trace(
        &mut build(9, false, false),
        &empty,
        TraceGranularity::PER_SYSTEM_PER_TICK,
    );
    assert_eq!(traced.checkpoints.len(), 1);
    assert_eq!(traced.checkpoints[0].tick, 0);
}

#[test]
fn system_hashes_do_not_depend_on_stage_mode_or_executor() {
    const TICKS: u32 = 12;
    let log = InputLog {
        seed: parallel_scenario::SEED,
        tick_rate_hz: 60,
        frames: vec![TickInput::default(); TICKS as usize],
    };
    let run = |mode: StageMode, executor: Arc<dyn Executor>| {
        let mut sim = parallel_scenario::build_simulation(parallel_scenario::SEED, mode);
        sim.world_mut().set_executor(executor);
        trace(&mut sim, &log, TraceGranularity::PER_SYSTEM_PER_TICK)
    };
    let reference = run(StageMode::Isolated, Arc::new(SequentialExecutor));
    assert_eq!(reference.system_names.len(), 8);
    assert!(
        reference.checkpoints[1..]
            .iter()
            .all(|checkpoint| checkpoint.system_hashes.len() == 8)
    );
    for (mode, executor) in [
        (
            StageMode::Grouped,
            Arc::new(SequentialExecutor) as Arc<dyn Executor>,
        ),
        (StageMode::Grouped, Arc::new(PermutedExecutor::new(1))),
        (StageMode::Grouped, Arc::new(PermutedExecutor::reversed())),
        (StageMode::Isolated, Arc::new(PermutedExecutor::new(2))),
    ] {
        let candidate = run(mode, executor);
        assert_eq!(candidate, reference, "{mode:?}");
        assert_eq!(first_divergence(&reference, &candidate), None);
    }
}

// --- diagnosis --------------------------------------------------------------------------------

#[test]
fn identical_runs_have_no_divergence() {
    let granularity = TraceGranularity::PER_SYSTEM_PER_TICK;
    assert_eq!(
        first_divergence(&recorded(false, granularity), &recorded(false, granularity)),
        None
    );
}

#[test]
fn per_system_per_tick_names_the_exact_tick_and_system() {
    let granularity = TraceGranularity::PER_SYSTEM_PER_TICK;
    let divergence = first_divergence(&recorded(false, granularity), &recorded(true, granularity))
        .expect("the faulty run diverges");
    assert_eq!(divergence.tick, FAULT_TICK + 1);
    assert_eq!(divergence.last_matching_tick, Some(FAULT_TICK));
    assert!(divergence.is_exact());
    let system = divergence.system.as_ref().expect("system hashes present");
    assert_eq!(system.index, 1);
    assert_eq!(system.name, "collide.resolve");
    assert_eq!(system.subsystem(), "collide");
    assert_eq!(
        divergence.to_string(),
        "first divergence in the step to tick 38, in system `collide.resolve` (subsystem `collide`)"
    );
}

#[test]
fn every_n_ticks_names_only_the_window() {
    let divergence = first_divergence(&recorded(false, every(10)), &recorded(true, every(10)))
        .expect("the faulty run diverges");
    assert_eq!(divergence.tick, 40);
    assert_eq!(divergence.last_matching_tick, Some(30));
    assert!(!divergence.is_exact());
    assert_eq!(divergence.system, None);
    assert_eq!(
        divergence.to_string(),
        "first divergence after tick 30, detected at tick 40"
    );
}

#[test]
fn every_n_ticks_with_system_hashes_names_the_first_differing_system_but_not_as_the_cause() {
    let granularity = every(10).with_system_hashes();
    let divergence = first_divergence(&recorded(false, granularity), &recorded(true, granularity))
        .expect("the faulty run diverges");
    assert_eq!(divergence.tick, 40);
    assert_eq!(divergence.last_matching_tick, Some(30));
    assert!(!divergence.is_exact());
    // The state already differed when tick 40's step began, so the first system differs too.
    let system = divergence.system.as_ref().expect("system hashes present");
    assert_eq!(system.name, "app.input");
    assert_eq!(
        divergence.to_string(),
        "first divergence after tick 30, detected at tick 40; at tick 40 the first differing \
         system is `app.input` (subsystem `app`)"
    );
}

#[test]
fn a_per_system_reference_narrows_a_coarse_candidate_only_to_common_ticks() {
    let divergence = first_divergence(
        &recorded(false, TraceGranularity::PER_SYSTEM_PER_TICK),
        &recorded(true, every(10)),
    )
    .expect("the faulty run diverges");
    assert_eq!(
        (divergence.tick, divergence.last_matching_tick),
        (40, Some(30))
    );
    assert_eq!(divergence.system, None);

    // The other way round, re-recording the candidate per system per tick up to the detected
    // tick, gives the exact answer against the detailed reference.
    let reference = recorded(false, TraceGranularity::PER_SYSTEM_PER_TICK);
    let mut prefix = log(9, 60);
    prefix.frames.truncate(divergence.tick as usize);
    let narrowed = trace(
        &mut build(9, true, false),
        &prefix,
        TraceGranularity::PER_SYSTEM_PER_TICK,
    );
    let exact = first_divergence(&reference, &narrowed).expect("still diverges");
    assert!(exact.is_exact());
    assert_eq!(exact.tick, FAULT_TICK + 1);
    assert_eq!(
        exact.system.map(|system| system.name),
        Some("collide.resolve".to_string())
    );
}

/// The two-stage diagnosis ADR-0018 proposes: detect every N ticks, then replay both runs plainly
/// to the last matching checkpoint and record only the window up to the detecting checkpoint per
/// system per tick.
#[test]
fn detecting_every_n_ticks_then_tracing_only_the_window_per_system_finds_the_exact_system() {
    let log = log(9, 60);
    let detected = first_divergence(&recorded(false, every(10)), &recorded(true, every(10)))
        .expect("the faulty run diverges");
    let start = detected.last_matching_tick.expect("tick 30 matched") as usize;
    let end = detected.tick as usize;
    let window = |faulty: bool| {
        let mut sim = build(log.seed, faulty, false);
        let before = InputLog {
            seed: log.seed,
            tick_rate_hz: log.tick_rate_hz,
            frames: log.frames[..start].to_vec(),
        };
        replay(&mut sim, &before, 0);
        let inside = InputLog {
            seed: log.seed,
            tick_rate_hz: log.tick_rate_hz,
            frames: log.frames[start..end].to_vec(),
        };
        trace(&mut sim, &inside, TraceGranularity::PER_SYSTEM_PER_TICK)
    };
    let reference = window(false);
    assert_eq!(reference.checkpoints.first().map(|c| c.tick), Some(30));
    assert_eq!(reference.checkpoints.len(), 11, "start plus ticks 31 to 40");
    let exact = first_divergence(&reference, &window(true)).expect("the window diverges");
    assert!(exact.is_exact());
    assert_eq!(exact.tick, FAULT_TICK + 1);
    assert_eq!(exact.last_matching_tick, Some(FAULT_TICK));
    assert_eq!(
        exact.system.map(|system| system.name),
        Some("collide.resolve".to_string())
    );
}

#[test]
fn different_schedules_name_no_system() {
    let log = log(9, 60);
    let granularity = TraceGranularity::PER_SYSTEM_PER_TICK;
    let reference = trace(&mut build(9, false, false), &log, granularity);
    let candidate = trace(&mut build(9, true, true), &log, granularity);
    let divergence = first_divergence(&reference, &candidate).expect("diverges");
    assert_eq!(divergence.system, None);
}

#[test]
fn a_different_start_state_diverges_at_the_start_without_a_matching_tick() {
    let log = log(9, 20);
    let reference = trace(&mut build(9, false, false), &log, every(5));
    let candidate = trace(&mut build(10, false, false), &log, every(5));
    let divergence = first_divergence(&reference, &candidate).expect("seeds differ");
    assert_eq!(divergence.tick, 0);
    assert_eq!(divergence.last_matching_tick, None);
    assert!(!divergence.is_exact());
    assert_eq!(
        divergence.to_string(),
        "divergence at tick 0 without a matching tick before it"
    );
}
