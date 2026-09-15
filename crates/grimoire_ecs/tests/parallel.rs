//! Parallel scheduler building blocks (engine ADR-0006): executors, the executor on the world,
//! deferred commands and panic semantics.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use grimoire_core::{StableHasher, impl_stable_hash};
use grimoire_ecs::{Executor, PermutedExecutor, SequentialExecutor, World};

#[derive(Clone, Debug, PartialEq)]
struct Pos {
    x: f32,
}
impl_stable_hash!(Pos { x });

fn hash(world: &World) -> u64 {
    let mut hasher = StableHasher::new();
    world.stable_hash(&mut hasher);
    hasher.finish()
}

/// Runs `count` tasks through `executor` and returns the task indices in execution order.
fn execution_order(executor: &dyn Executor, count: usize) -> Vec<usize> {
    let log = std::sync::Mutex::new(Vec::new());
    {
        let mut tasks: Vec<_> = (0..count)
            .map(|index| {
                let log = &log;
                move || log.lock().expect("unpoisoned").push(index)
            })
            .collect();
        let mut refs: Vec<&mut (dyn FnMut() + Send)> = tasks
            .iter_mut()
            .map(|task| task as &mut (dyn FnMut() + Send))
            .collect();
        executor.run(&mut refs);
    }
    log.into_inner().expect("unpoisoned")
}

#[test]
fn sequential_executor_runs_tasks_in_index_order() {
    assert_eq!(execution_order(&SequentialExecutor, 5), [0, 1, 2, 3, 4]);
    assert_eq!(SequentialExecutor.threads(), 1);
}

#[test]
fn permuted_executor_runs_every_task_once_in_a_reproducible_order() {
    let first = PermutedExecutor::new(17);
    let second = PermutedExecutor::new(17);
    let mut orders = Vec::new();
    for _ in 0..4 {
        let order = execution_order(&first, 9);
        assert_eq!(order, execution_order(&second, 9));
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..9).collect::<Vec<_>>());
        orders.push(order);
    }
    // The permutation changes with the call counter and is not the identity every time.
    assert!(orders.windows(2).any(|pair| pair[0] != pair[1]));
    assert!(
        orders
            .iter()
            .any(|order| *order != (0..9).collect::<Vec<_>>())
    );
    assert_ne!(
        execution_order(&PermutedExecutor::new(1), 9),
        execution_order(&PermutedExecutor::new(2), 9)
    );
    assert_eq!(first.threads(), 1);
}

#[test]
fn reversed_executor_runs_from_last_to_first() {
    let executor = PermutedExecutor::reversed();
    assert_eq!(execution_order(&executor, 4), [3, 2, 1, 0]);
    assert_eq!(execution_order(&executor, 4), [3, 2, 1, 0]);
    assert!(execution_order(&executor, 0).is_empty());
}

#[test]
fn nested_runs_inside_a_task_finish() {
    for executor in [
        Arc::new(SequentialExecutor) as Arc<dyn Executor>,
        Arc::new(PermutedExecutor::new(4)),
        Arc::new(PermutedExecutor::reversed()),
    ] {
        let counter = AtomicUsize::new(0);
        let mut outer: Vec<_> = (0..3)
            .map(|_| {
                let executor = &executor;
                let counter = &counter;
                move || {
                    let mut inner: Vec<_> = (0..4)
                        .map(|_| {
                            move || {
                                counter.fetch_add(1, Ordering::Relaxed);
                            }
                        })
                        .collect();
                    let mut refs: Vec<&mut (dyn FnMut() + Send)> = inner
                        .iter_mut()
                        .map(|task| task as &mut (dyn FnMut() + Send))
                        .collect();
                    executor.run(&mut refs);
                }
            })
            .collect();
        let mut refs: Vec<&mut (dyn FnMut() + Send)> = outer
            .iter_mut()
            .map(|task| task as &mut (dyn FnMut() + Send))
            .collect();
        executor.run(&mut refs);
        drop(refs);
        drop(outer);
        assert_eq!(counter.load(Ordering::Relaxed), 12);
    }
}

#[test]
fn world_defaults_to_the_sequential_executor() {
    let world = World::new();
    assert_eq!(world.executor().threads(), 1);
}

/// Executor that reports a thread count and counts its `run` calls.
#[derive(Debug, Default)]
struct CountingExecutor {
    calls: AtomicUsize,
}

impl Executor for CountingExecutor {
    fn threads(&self) -> usize {
        7
    }

    fn run(&self, tasks: &mut [&mut (dyn FnMut() + Send)]) {
        self.calls.fetch_add(1, Ordering::Relaxed);
        SequentialExecutor.run(tasks);
    }
}

#[test]
fn executor_is_not_simulation_state() {
    let mut plain = World::new();
    let mut with_executor = World::new();
    with_executor.set_executor(Arc::new(CountingExecutor::default()));
    for world in [&mut plain, &mut with_executor] {
        world.spawn((Pos { x: 1.5 },));
        world.insert_resource(Pos { x: -2.0 });
    }
    assert_eq!(with_executor.executor().threads(), 7);
    assert_eq!(hash(&plain), hash(&with_executor));

    // A snapshot does not carry the executor; restoring keeps the current one.
    let snapshot = with_executor.snapshot();
    plain.restore(&snapshot);
    assert_eq!(plain.executor().threads(), 1);
    assert_eq!(hash(&plain), hash(&with_executor));

    with_executor.spawn((Pos { x: 3.0 },));
    with_executor.restore(&plain.snapshot());
    assert_eq!(with_executor.executor().threads(), 7);
    assert_eq!(hash(&plain), hash(&with_executor));
}

#[test]
fn world_stays_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<World>();
    assert_send_sync::<SequentialExecutor>();
    assert_send_sync::<PermutedExecutor>();
}
