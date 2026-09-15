//! Parallel scheduler building blocks (engine ADR-0006): executors, the executor on the world,
//! deferred commands and panic semantics.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use grimoire_core::{StableHasher, impl_stable_hash};
use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::AtomicBool;

use grimoire_ecs::{
    Access, CommandBuffer, Entity, Executor, PermutedExecutor, Schedule, SequentialExecutor, World,
    parallel_system_fn, system_fn,
};

#[derive(Clone, Debug, PartialEq)]
struct Pos {
    x: f32,
}
impl_stable_hash!(Pos { x });

#[derive(Clone, Debug, PartialEq)]
struct Vel {
    x: f32,
}
impl_stable_hash!(Vel { x });

#[derive(Clone, Debug, PartialEq)]
struct Score {
    value: u32,
}
impl_stable_hash!(Score { value });

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

#[test]
fn set_replaces_existing_components_only() {
    let mut world = World::new();
    let both = world.spawn((Pos { x: 1.0 }, Vel { x: 2.0 }));
    let only_pos = world.spawn((Pos { x: 3.0 },));
    let dead = world.spawn((Pos { x: 4.0 }, Vel { x: 5.0 }));
    world.despawn(dead);

    // The model: the same replacements done immediately where they apply.
    let mut model = World::new();
    let model_both = model.spawn((Pos { x: 1.0 }, Vel { x: 2.0 }));
    let model_only_pos = model.spawn((Pos { x: 3.0 },));
    let model_dead = model.spawn((Pos { x: 4.0 }, Vel { x: 5.0 }));
    model.despawn(model_dead);
    *model.get_mut::<Vel>(model_both).expect("has Vel") = Vel { x: 20.0 };
    *model.get_mut::<Pos>(model_only_pos).expect("has Pos") = Pos { x: 30.0 };

    let mut commands = CommandBuffer::new();
    commands.set(both, Vel { x: 20.0 });
    commands.set(only_pos, Vel { x: 99.0 });
    commands.set(only_pos, Pos { x: 30.0 });
    commands.set(dead, Vel { x: 99.0 });
    assert_eq!(commands.len(), 4);
    commands.apply(&mut world);

    assert!(commands.is_empty());
    assert_eq!(world.get::<Vel>(both), Some(&Vel { x: 20.0 }));
    assert_eq!(world.get::<Vel>(only_pos), None);
    assert_eq!(hash(&world), hash(&model));
}

#[test]
fn resource_commands_apply_in_recorded_order() {
    let mut world = World::new();
    let mut commands = CommandBuffer::new();
    commands.remove_resource::<Score>();
    commands.insert_resource(Score { value: 1 });
    commands.insert_resource(Score { value: 2 });
    commands.apply(&mut world);
    assert_eq!(world.resource::<Score>(), Some(&Score { value: 2 }));

    commands.remove_resource::<Score>();
    commands.apply(&mut world);
    assert_eq!(world.resource::<Score>(), None);

    let mut model = World::new();
    model.insert_resource(Score { value: 2 });
    model.remove_resource::<Score>();
    assert_eq!(hash(&world), hash(&model));
}

#[test]
fn append_keeps_order_and_empties_the_source() {
    let mut world = World::new();
    let entity = world.spawn((Pos { x: 0.0 },));
    let mut first = CommandBuffer::new();
    first.set(entity, Pos { x: 1.0 });
    let mut second = CommandBuffer::new();
    second.set(entity, Pos { x: 2.0 });
    second.spawn((Vel { x: 7.0 },));
    first.append(&mut second);
    assert!(second.is_empty());
    assert_eq!(first.len(), 3);
    first.apply(&mut world);
    assert_eq!(world.get::<Pos>(entity), Some(&Pos { x: 2.0 }));
    assert_eq!(world.query::<&Vel>().count(), 1);
}

#[test]
fn access_ignores_duplicate_declarations() {
    let once = Access::new()
        .read::<Pos>()
        .write::<Vel>()
        .read_resource::<Score>();
    let twice = once
        .clone()
        .read::<Pos>()
        .write::<Vel>()
        .read_resource::<Score>();
    assert_eq!(once, twice);
    assert_ne!(Access::new().read::<Pos>(), Access::new().write::<Pos>());
    assert_ne!(Access::new(), Access::new().structural());
}

/// Text of a panic payload created by `panic!` with a literal or a format string.
fn payload_text(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("<non-text payload>")
}

fn populated() -> World {
    let mut world = World::new();
    for i in 0..10 {
        world.spawn((Pos { x: i as f32 },));
    }
    world
}

fn bump(world: &mut World) {
    for pos in world.query_mut::<&mut Pos>() {
        pos.x += 1.0;
    }
}

fn double(world: &World, commands: &mut CommandBuffer) {
    for (entity, pos) in world.query::<(Entity, &Pos)>() {
        commands.set(entity, Pos { x: pos.x * 2.0 });
    }
}

fn spawn_vel(_: &World, commands: &mut CommandBuffer) {
    commands.spawn((Vel { x: 1.0 },));
}

#[test]
fn a_panicking_stage_applies_none_of_its_commands() {
    let armed = Arc::new(AtomicBool::new(true));
    let trigger = Arc::clone(&armed);
    let mut schedule = Schedule::new();
    schedule
        .add_system(system_fn("bump", bump))
        .add_parallel_system(parallel_system_fn(
            "double",
            Access::new().read::<Pos>().write::<Pos>(),
            double,
        ))
        .add_parallel_system(parallel_system_fn("boom", Access::new(), move |_, _| {
            if trigger.swap(false, Ordering::Relaxed) {
                panic!("boom");
            }
        }))
        .add_parallel_system(parallel_system_fn(
            "spawn_vel",
            Access::new().structural(),
            spawn_vel,
        ));
    assert_eq!(schedule.stages().len(), 2);

    let mut world = populated();
    let result = catch_unwind(AssertUnwindSafe(|| schedule.run(&mut world)));
    let payload = result.expect_err("the stage panics");
    assert_eq!(payload_text(payload.as_ref()), "boom");
    assert!(!armed.load(Ordering::Relaxed));

    // Only the exclusive stage before the panicking stage is applied.
    let mut after_bump = populated();
    bump(&mut after_bump);
    assert_eq!(hash(&world), hash(&after_bump));

    // The next run starts from empty buffers: nothing of the failed stage is applied late.
    schedule.run(&mut world);
    let mut reference = Schedule::new();
    reference
        .add_system(system_fn("bump", bump))
        .add_parallel_system(parallel_system_fn(
            "double",
            Access::new().read::<Pos>().write::<Pos>(),
            double,
        ))
        .add_parallel_system(parallel_system_fn(
            "spawn_vel",
            Access::new().structural(),
            spawn_vel,
        ));
    reference.run(&mut after_bump);
    assert_eq!(hash(&world), hash(&after_bump));
    assert_eq!(world.query::<&Vel>().count(), 1);
}

#[test]
fn the_panic_of_the_lowest_list_index_wins_under_any_executor() {
    for executor in [
        Arc::new(SequentialExecutor) as Arc<dyn Executor>,
        Arc::new(PermutedExecutor::reversed()),
        Arc::new(PermutedExecutor::new(5)),
    ] {
        let mut schedule = Schedule::new();
        schedule
            .add_parallel_system(parallel_system_fn("calm", Access::new(), |_, _| {}))
            .add_parallel_system(parallel_system_fn("first", Access::new(), |_, _| {
                panic!("first")
            }))
            .add_parallel_system(parallel_system_fn("second", Access::new(), |_, _| {
                panic!("second")
            }));
        let mut world = World::new();
        world.set_executor(executor);
        let payload = catch_unwind(AssertUnwindSafe(|| schedule.run(&mut world)))
            .expect_err("the stage panics");
        assert_eq!(payload_text(payload.as_ref()), "first");
    }
}

#[test]
fn a_panicking_exclusive_system_propagates_unchanged() {
    let mut schedule = Schedule::new();
    schedule.add_system(system_fn("explode", |_| panic!("exclusive {}", 7)));
    let mut world = World::new();
    let payload =
        catch_unwind(AssertUnwindSafe(|| schedule.run(&mut world))).expect_err("the system panics");
    assert_eq!(payload_text(payload.as_ref()), "exclusive 7");
}

#[test]
fn parallel_stages_see_the_world_before_the_stage() {
    // Both systems read the counter before either buffer is applied.
    #[derive(Clone)]
    struct Counter {
        value: u32,
    }
    impl_stable_hash!(Counter { value });

    let mut world = World::new();
    world.insert_resource(Counter { value: 1 });
    world.set_executor(Arc::new(PermutedExecutor::reversed()));
    let mut schedule = Schedule::new();
    for name in ["left", "right"] {
        schedule.add_parallel_system(parallel_system_fn(
            name,
            Access::new()
                .read_resource::<Counter>()
                .write_resource::<Counter>(),
            move |world, commands| {
                let value = world
                    .resource::<Counter>()
                    .map_or(0, |counter| counter.value);
                let step = if name == "left" { 10 } else { 100 };
                commands.insert_resource(Counter {
                    value: value + step,
                });
            },
        ));
    }
    // A later reader of a resource written in the stage starts a new stage.
    assert_eq!(schedule.stages().len(), 2);
    schedule.run(&mut world);
    assert_eq!(world.resource::<Counter>().map(|c| c.value), Some(111));
}

// ------------------------------------------------------------------ recovery after an unwind

/// Arms the panicking `Drop` of [`Fragile`] for exactly one drop.
static FRAGILE_ARMED: AtomicBool = AtomicBool::new(false);

/// Resource whose replaced value panics on drop while armed, i.e. while a buffer is applied.
#[derive(Clone)]
struct Fragile {
    value: u32,
}
impl_stable_hash!(Fragile { value });

impl Drop for Fragile {
    fn drop(&mut self) {
        if self.value == 1 && FRAGILE_ARMED.swap(false, Ordering::SeqCst) {
            panic!("dropping the replaced resource panics");
        }
    }
}

/// Stage `[replace, spawn]`: a panic while `replace`'s buffer is applied skips `spawn`'s buffer.
fn fragile_stage() -> Schedule {
    let mut schedule = Schedule::new();
    schedule
        .add_parallel_system(parallel_system_fn(
            "replace",
            Access::new().write_resource::<Fragile>(),
            |_, commands| commands.insert_resource(Fragile { value: 2 }),
        ))
        .add_parallel_system(parallel_system_fn(
            "spawn",
            Access::new().structural(),
            spawn_vel,
        ));
    schedule
}

#[test]
fn a_panic_while_applying_leaves_no_commands_for_the_run_after_restore() {
    let mut schedule = fragile_stage();
    assert_eq!(schedule.stages().len(), 1);
    let mut world = World::new();
    world.insert_resource(Fragile { value: 1 });
    let snapshot = world.snapshot();

    FRAGILE_ARMED.store(true, Ordering::SeqCst);
    let payload = catch_unwind(AssertUnwindSafe(|| schedule.run(&mut world)))
        .expect_err("applying the stage panics");
    FRAGILE_ARMED.store(false, Ordering::SeqCst);
    assert_eq!(
        payload_text(payload.as_ref()),
        "dropping the replaced resource panics"
    );

    // Documented recovery: restore, then run the same schedule again.
    world.restore(&snapshot);
    schedule.run(&mut world);

    let mut reference = World::new();
    reference.restore(&snapshot);
    fragile_stage().run(&mut reference);
    assert_eq!(reference.entity_count(), 1);
    assert_eq!(world.entity_count(), 1, "a stale spawn was applied late");
    assert_eq!(hash(&world), hash(&reference));
}

/// Runs every task, then unwinds once while armed, like an executor that breaks its contract.
struct UnwindingExecutor {
    armed: AtomicBool,
}

impl Executor for UnwindingExecutor {
    fn threads(&self) -> usize {
        1
    }

    fn run(&self, tasks: &mut [&mut (dyn FnMut() + Send)]) {
        for task in tasks.iter_mut() {
            task();
        }
        if self.armed.swap(false, Ordering::SeqCst) {
            panic!("executor unwinds after its tasks");
        }
    }
}

#[test]
fn an_unwinding_executor_leaves_no_commands_for_the_run_after_restore() {
    let build = || {
        let mut schedule = Schedule::new();
        schedule
            .add_parallel_system(parallel_system_fn(
                "double",
                Access::new().read::<Pos>().write::<Pos>(),
                double,
            ))
            .add_parallel_system(parallel_system_fn(
                "spawn_vel",
                Access::new().structural(),
                spawn_vel,
            ));
        schedule
    };
    let mut schedule = build();
    assert_eq!(schedule.stages().len(), 1);
    let mut world = populated();
    world.set_executor(Arc::new(UnwindingExecutor {
        armed: AtomicBool::new(true),
    }));
    let snapshot = world.snapshot();

    let payload = catch_unwind(AssertUnwindSafe(|| schedule.run(&mut world)))
        .expect_err("the executor unwinds");
    assert_eq!(
        payload_text(payload.as_ref()),
        "executor unwinds after its tasks"
    );

    world.restore(&snapshot);
    schedule.run(&mut world);

    let mut reference = populated();
    build().run(&mut reference);
    assert_eq!(
        world.query::<&Vel>().count(),
        1,
        "a stale spawn was applied late"
    );
    assert_eq!(hash(&world), hash(&reference));
}

#[test]
fn an_unwinding_executor_leaves_no_stale_system_panic_behind() {
    let armed = Arc::new(AtomicBool::new(true));
    let trigger = Arc::clone(&armed);
    let mut schedule = Schedule::new();
    schedule
        .add_parallel_system(parallel_system_fn("boom", Access::new(), move |_, _| {
            if trigger.swap(false, Ordering::SeqCst) {
                panic!("boom");
            }
        }))
        .add_parallel_system(parallel_system_fn(
            "spawn_vel",
            Access::new().structural(),
            spawn_vel,
        ));
    let mut world = World::new();
    world.set_executor(Arc::new(UnwindingExecutor {
        armed: AtomicBool::new(true),
    }));
    let snapshot = world.snapshot();

    // The executor's own unwind wins; the system's stored panic is never taken.
    let payload = catch_unwind(AssertUnwindSafe(|| schedule.run(&mut world)))
        .expect_err("the executor unwinds");
    assert_eq!(
        payload_text(payload.as_ref()),
        "executor unwinds after its tasks"
    );
    assert!(!armed.load(Ordering::SeqCst));

    // No system panics now, so the run must neither resume the old payload nor spawn twice.
    world.restore(&snapshot);
    schedule.run(&mut world);
    assert_eq!(world.query::<&Vel>().count(), 1);
}
