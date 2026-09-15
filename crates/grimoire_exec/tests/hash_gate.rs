//! Hash gate of engine ADR-0006, building block 7: the engine's golden scenarios on real thread
//! pools with 1, 2 and N threads (`gate_executors`, N = 4 or `GRIMOIRE_GATE_THREADS`).
//!
//! Every checkpoint must equal the run without threads and the golden constants:
//!
//! - (a) the parallel form of the P0 scenario against the P0 run and `GOLDEN_FINAL_HASH`;
//! - (b) the parallel scenario against its isolated-stage run and `GOLDEN_PARALLEL_FINAL_HASH`;
//! - (c) the facade scenarios through `run_headless` and the headless frame loop;
//! - (d) random schedules on a 4-thread pool against isolated stages;
//! - (e) in debug builds, an undeclared read on a pool worker panics with the system name, and
//!   context-free blocks of a second world on a shared pool are never checked.
//!
//! The scenarios are included from the other crates' test directories, so no determinism crate
//! needs a dependency on this crate or on rayon.

#[path = "../../grimoire/tests/common/mod.rs"]
mod common;
#[path = "../../grimoire_sim/tests/parallel_scenario/mod.rs"]
mod parallel_scenario;
#[path = "../../grimoire_ecs/tests/support/random_schedule.rs"]
mod random_schedule;
#[path = "../../grimoire_sim/tests/scenario/mod.rs"]
mod scenario;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use common::{ParallelScenario, Scenario, bot_input};
use grimoire::prelude::*;
use grimoire_ecs::{Executor, SequentialExecutor, StageMode};
use grimoire_exec::{ThreadPoolExecutor, gate_executors};
use proptest::prelude::*;

/// Index of an executor in [`gate_executors`]: 1, 2 and N threads.
const ONE: usize = 0;
const TWO: usize = 1;
const N: usize = 2;

fn gate_executor(index: usize) -> (String, Arc<dyn Executor>) {
    gate_executors().swap_remove(index)
}

// ------------------------------------------------------------------ (a) P0 scenario

fn p0_reference() -> &'static Vec<(u64, u64)> {
    static REFERENCE: OnceLock<Vec<(u64, u64)>> = OnceLock::new();
    REFERENCE.get_or_init(|| {
        let mut sim = scenario::build_simulation(scenario::SEED);
        scenario::run_until(&mut sim, scenario::TOTAL_TICKS, |_| {})
    })
}

fn p0_gate(index: usize) {
    let (label, executor) = gate_executor(index);
    let mut sim = scenario::build_parallel_simulation(scenario::SEED, StageMode::Grouped);
    sim.world_mut().set_executor(executor);
    let hashes = scenario::run_until(&mut sim, scenario::TOTAL_TICKS, |_| {});
    assert_eq!(&hashes, p0_reference(), "P0 parallel form with {label}");
    assert_eq!(
        hashes.last().map(|&(_, hash)| hash),
        Some(scenario::GOLDEN_FINAL_HASH),
        "P0 golden with {label}"
    );
}

#[test]
fn p0_parallel_form_threads_1() {
    p0_gate(ONE);
}

#[test]
fn p0_parallel_form_threads_2() {
    p0_gate(TWO);
}

#[test]
fn p0_parallel_form_threads_n() {
    p0_gate(N);
}

// ------------------------------------------------------------------ (b) parallel scenario

fn parallel_reference() -> &'static Vec<(u64, u64)> {
    static REFERENCE: OnceLock<Vec<(u64, u64)>> = OnceLock::new();
    REFERENCE.get_or_init(|| {
        let mut sim =
            parallel_scenario::build_simulation(parallel_scenario::SEED, StageMode::Isolated);
        sim.world_mut().set_executor(Arc::new(SequentialExecutor));
        parallel_scenario::run_until(&mut sim, parallel_scenario::TOTAL_TICKS, |_| {})
    })
}

fn parallel_gate(index: usize) {
    let (label, executor) = gate_executor(index);
    let mut sim = parallel_scenario::build_simulation(parallel_scenario::SEED, StageMode::Grouped);
    sim.world_mut().set_executor(executor);
    let hashes = parallel_scenario::run_until(&mut sim, parallel_scenario::TOTAL_TICKS, |_| {});
    assert_eq!(
        &hashes,
        parallel_reference(),
        "parallel scenario with {label}: {}",
        parallel_scenario::checkpoint_report(&hashes)
    );
    assert_eq!(
        hashes.last().map(|&(_, hash)| hash),
        Some(parallel_scenario::GOLDEN_PARALLEL_FINAL_HASH),
        "parallel golden with {label}"
    );
}

#[test]
fn parallel_scenario_threads_1() {
    parallel_gate(ONE);
}

#[test]
fn parallel_scenario_threads_2() {
    parallel_gate(TWO);
}

#[test]
fn parallel_scenario_threads_n() {
    parallel_gate(N);
}

// ------------------------------------------------------------------ (c) facade scenarios

/// One 60 Hz tick per frame (see `grimoire/tests/headless.rs`).
const ONE_TICK: Duration = Duration::from_nanos(16_666_667);

fn builder(parallel: bool) -> AppBuilder {
    let builder = App::new(WindowConfig::default()).seed(33).hash_every(30);
    if parallel {
        builder.plugin(ParallelScenario { movers: 3_000 })
    } else {
        builder.plugin(Scenario { movers: 500 })
    }
}

fn facade_gate(index: usize) {
    let (label, executor) = gate_executor(index);
    for parallel in [false, true] {
        let default = builder(parallel).run_headless(600, &mut bot_input);
        let pooled = builder(parallel)
            .executor(Arc::clone(&executor))
            .run_headless(600, &mut bot_input);
        assert_eq!(
            pooled, default,
            "run_headless, parallel {parallel}, {label}"
        );

        let looped_default = builder(parallel)
            .run_headless_frames(300, ONE_TICK)
            .expect("headless frame loop runs");
        let looped_pooled = builder(parallel)
            .executor(Arc::clone(&executor))
            .run_headless_frames(300, ONE_TICK)
            .expect("headless frame loop runs");
        assert_eq!(
            looped_pooled.hashes, looped_default.hashes,
            "frame loop, parallel {parallel}, {label}"
        );
        assert_eq!(looped_pooled.final_hash, looped_default.final_hash);
    }
}

#[test]
fn facade_threads_1() {
    facade_gate(ONE);
}

#[test]
fn facade_threads_2() {
    facade_gate(TWO);
}

#[test]
fn facade_threads_n() {
    facade_gate(N);
}

// ------------------------------------------------------------------ (d) random schedules

fn four_threads() -> Arc<dyn Executor> {
    static POOL: OnceLock<Arc<dyn Executor>> = OnceLock::new();
    Arc::clone(POOL.get_or_init(|| Arc::new(ThreadPoolExecutor::new(4).expect("4-thread pool"))))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn random_schedules_on_four_threads_equal_isolated_stages(
        spec in random_schedule::schedule_spec(),
    ) {
        let reference =
            random_schedule::run_hashes(&spec, Arc::new(SequentialExecutor), StageMode::Isolated);
        let pooled = random_schedule::run_hashes(&spec, four_threads(), StageMode::Grouped);
        prop_assert_eq!(pooled, reference);
    }
}

// ------------------------------------------------------------------ pool usage and debug check

/// Blocks really run on the pool's named workers, not on the calling thread.
#[test]
fn blocks_run_on_pool_workers() {
    let mut world = World::new();
    for value in 0..(8 * grimoire_ecs::QUERY_BLOCK_SIZE as u64) {
        world.spawn((random_schedule::A { value },));
    }
    world.set_executor(Arc::new(ThreadPoolExecutor::new(4).expect("pool")));
    let names = Mutex::new(BTreeSet::new());
    let rows = world.par_blocks::<&random_schedule::A, _>(|block| {
        let name = std::thread::current().name().unwrap_or("").to_owned();
        names.lock().expect("unpoisoned").insert(name);
        block.len()
    });
    assert_eq!(rows.len(), 8);
    let names = names.into_inner().expect("unpoisoned");
    assert!(
        names.iter().all(|name| name.starts_with("grimoire-sim-")),
        "{names:?}"
    );
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "system `sneaky` reads component `hash_gate::random_schedule::B`")]
fn undeclared_read_on_a_worker_panics() {
    use random_schedule::{A, B};

    let mut world = World::new();
    for value in 0..(3 * grimoire_ecs::QUERY_BLOCK_SIZE as u64) {
        world.spawn((A { value }, B { value }));
    }
    world.set_executor(Arc::new(ThreadPoolExecutor::new(4).expect("pool")));
    let mut schedule = Schedule::new();
    schedule
        .add_parallel_system(parallel_system_fn(
            "honest",
            Access::new().read::<A>(),
            |world, _| {
                let _ = world.par_blocks::<&A, _>(|block| block.len());
            },
        ))
        .add_parallel_system(parallel_system_fn(
            "sneaky",
            Access::new().read::<A>(),
            |world, _| {
                let _ = world.par_blocks::<(Entity, &A), _>(|block| {
                    block
                        .filter(|(entity, _)| world.get::<B>(*entity).is_some())
                        .count()
                });
            },
        ));
    schedule.run(&mut world);
}

/// Two worlds share one pool. A pool worker waits inside a parallel system of the first world
/// (its access context is on the worker's stack) and meanwhile runs a block of a `par_blocks`
/// call that the second world issues outside any schedule. That block has no context and must
/// not be checked against the waiting system's declaration.
#[cfg(debug_assertions)]
#[test]
fn context_free_blocks_on_a_shared_pool_inherit_no_foreign_context() {
    use std::sync::Barrier;
    use std::sync::mpsc;

    use random_schedule::{A, B};

    const TIMEOUT: Duration = Duration::from_secs(30);

    #[derive(Clone)]
    struct Gravity {
        value: u64,
    }
    grimoire_core::impl_stable_hash!(Gravity { value });

    fn world(executor: &Arc<dyn Executor>) -> World {
        let mut world = World::new();
        for value in 0..(2 * grimoire_ecs::QUERY_BLOCK_SIZE as u64) {
            world.spawn((A { value }, B { value }));
        }
        world.insert_resource(Gravity { value: 1 });
        world.set_executor(Arc::clone(executor));
        world
    }

    for _ in 0..3 {
        let pool: Arc<dyn Executor> = Arc::new(ThreadPoolExecutor::new(2).expect("2-thread pool"));
        let mut first = world(&pool);
        let second = world(&pool);

        // Both blocks of `waiting` meet at the barrier, so they run on the two workers at once.
        // Block 0 then returns and its worker waits in the join for block 1, which blocks until
        // the second world is done. The only free worker is the one waiting inside `waiting`.
        let barrier = Barrier::new(2);
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let go_tx = Mutex::new(go_tx);
        let done_rx = Mutex::new(done_rx);

        let mut schedule = Schedule::new();
        schedule
            .add_parallel_system(parallel_system_fn(
                "waiting",
                Access::new().read::<A>(),
                move |world, _| {
                    let _ = world.par_blocks::<&A, _>(|block| {
                        barrier.wait();
                        if block.index() == 0 {
                            go_tx
                                .lock()
                                .expect("unpoisoned")
                                .send(())
                                .expect("receiver");
                        } else {
                            let _ = done_rx.lock().expect("unpoisoned").recv_timeout(TIMEOUT);
                        }
                        block.len()
                    });
                },
            ))
            .add_parallel_system(parallel_system_fn(
                "idle",
                Access::new().read::<A>(),
                |_, _| {},
            ));
        assert_eq!(schedule.stages().len(), 1);

        let second = &second;
        let outcome = std::thread::scope(|scope| {
            let other = scope.spawn(move || {
                go_rx
                    .recv_timeout(TIMEOUT)
                    .expect("block 0 of `waiting` ran");
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    second.par_blocks::<(Entity, &A), _>(|block| {
                        let gravity = second.resource::<Gravity>().map_or(0, |g| g.value);
                        let with_b = block
                            .filter(|(entity, _)| second.get::<B>(*entity).is_some())
                            .count();
                        gravity * with_b as u64
                    })
                }));
                done_tx.send(()).expect("receiver");
                outcome
            });
            schedule.run(&mut first);
            other.join().expect("second thread")
        });
        match outcome {
            Ok(rows) => assert_eq!(rows, vec![grimoire_ecs::QUERY_BLOCK_SIZE as u64; 2]),
            Err(payload) => {
                let text = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                    .unwrap_or_default();
                panic!("context-free blocks were checked against a foreign system: {text}");
            }
        }
    }
}
