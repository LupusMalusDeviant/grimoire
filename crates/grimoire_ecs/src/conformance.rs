//! Conformance suites for the trait contracts of this crate (contract §2 rule 12), behind the
//! non-default feature `conformance`.
//!
//! Every implementation of [`crate::Executor`] and [`crate::SystemObserver`] calls the matching
//! function from its own tests. The suites check only properties every implementation, including
//! [`crate::SequentialExecutor`] and [`crate::NoopObserver`], must satisfy; implementation-specific
//! behaviour belongs in that implementation's own tests.

use std::array;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use grimoire_core::{StableHasher, impl_stable_hash};

use crate::{
    Access, Executor, Schedule, StageInfo, SystemInfo, SystemObserver, World, parallel_system_fn,
    system_fn,
};

/// Checks the [`Executor`] contract (§7): every task runs exactly once, [`Executor::run`] returns
/// only after every task has finished, a task's panic is never swallowed, and a nested
/// [`Executor::run`] call from inside a task completes instead of deadlocking.
pub fn executor(executor: &dyn Executor) {
    let counts: [AtomicU32; 4] = array::from_fn(|_| AtomicU32::new(0));
    {
        let counts = &counts;
        let mut tasks: Vec<Box<dyn FnMut() + Send + '_>> = (0..counts.len())
            .map(|i| -> Box<dyn FnMut() + Send + '_> {
                Box::new(move || {
                    counts[i].fetch_add(1, Ordering::SeqCst);
                })
            })
            .collect();
        let mut refs: Vec<&mut (dyn FnMut() + Send)> = tasks
            .iter_mut()
            .map(|task| task.as_mut() as &mut (dyn FnMut() + Send))
            .collect();
        executor.run(&mut refs);
        // `run` has returned: every count must already reflect exactly one execution.
        for (index, count) in counts.iter().enumerate() {
            assert_eq!(
                count.load(Ordering::SeqCst),
                1,
                "task {index} did not run exactly once"
            );
        }
    }

    let panicked = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let mut task: Box<dyn FnMut() + Send> =
            Box::new(|| panic!("grimoire_ecs::conformance::executor probe panic"));
        let mut refs: [&mut (dyn FnMut() + Send); 1] = [task.as_mut()];
        executor.run(&mut refs);
    }));
    assert!(
        panicked.is_err(),
        "Executor::run must not swallow a task panic"
    );

    let nested_ran = AtomicBool::new(false);
    {
        let mut outer: Box<dyn FnMut() + Send> = Box::new(|| {
            let mut inner: Box<dyn FnMut() + Send> = Box::new(|| {
                nested_ran.store(true, Ordering::SeqCst);
            });
            let mut refs: [&mut (dyn FnMut() + Send); 1] = [inner.as_mut()];
            executor.run(&mut refs);
        });
        let mut refs: [&mut (dyn FnMut() + Send); 1] = [outer.as_mut()];
        executor.run(&mut refs);
    }
    assert!(
        nested_ran.load(Ordering::SeqCst),
        "a nested Executor::run call from inside a task must not deadlock"
    );
}

#[derive(Clone)]
struct Value {
    n: i32,
}
impl_stable_hash!(Value { n });

/// One exclusive system followed by one parallel (structural) system; fixed so the expected
/// [`SystemObserver`] call log in [`system_observer`] is stable.
fn scenario() -> Schedule {
    let mut schedule = Schedule::new();
    schedule
        .add_system(system_fn("conformance.exclusive", |world| {
            world.spawn((Value { n: 1 },));
        }))
        .add_parallel_system(parallel_system_fn(
            "conformance.parallel",
            Access::new().structural(),
            |_world, commands| {
                commands.spawn((Value { n: 2 },));
            },
        ));
    schedule
}

struct Recorder<'a> {
    log: Vec<String>,
    inner: &'a mut dyn SystemObserver,
}

impl SystemObserver for Recorder<'_> {
    fn stage_started(&mut self, stage: StageInfo) {
        self.log.push(format!(
            "stage_started({}, {})",
            stage.index, stage.exclusive
        ));
        self.inner.stage_started(stage);
    }
    fn system_started(&mut self, system: SystemInfo<'_>) {
        self.log.push(format!("system_started({})", system.name));
        self.inner.system_started(system);
    }
    fn tasks_finished(&mut self, stage: StageInfo) {
        self.log.push(format!("tasks_finished({})", stage.index));
        self.inner.tasks_finished(stage);
    }
    fn system_finished(&mut self, system: SystemInfo<'_>, world: &World) {
        self.log.push(format!("system_finished({})", system.name));
        self.inner.system_finished(system, world);
    }
    fn stage_finished(&mut self, stage: StageInfo, world: &World) {
        self.log.push(format!("stage_finished({})", stage.index));
        self.inner.stage_finished(stage, world);
    }
}

fn state_hash(world: &World) -> u64 {
    let mut hasher = StableHasher::new();
    world.stable_hash(&mut hasher);
    hasher.finish()
}

/// Checks the [`SystemObserver`] contract (§7.2) against `observer`: the call order per stage
/// kind, the two documented panic cases, and that neither the call order nor the presence of
/// `observer` changes [`World::stable_hash`].
///
/// `observer` receives every call the fixed scenario produces, so a real observer (for example
/// the facade's profiler) can be checked with its own instance.
pub fn system_observer(observer: &mut dyn SystemObserver) {
    // Call order, both stage kinds.
    let mut world = World::new();
    let log = {
        let mut recorder = Recorder {
            log: Vec::new(),
            inner: observer,
        };
        scenario().run_observed(&mut world, &mut recorder);
        recorder.log
    };
    assert_eq!(
        log,
        vec![
            "stage_started(0, true)".to_owned(),
            "system_started(conformance.exclusive)".to_owned(),
            "system_finished(conformance.exclusive)".to_owned(),
            "stage_finished(0)".to_owned(),
            "stage_started(1, false)".to_owned(),
            "tasks_finished(1)".to_owned(),
            "system_finished(conformance.parallel)".to_owned(),
            "stage_finished(1)".to_owned(),
        ],
        "observed call order does not match contract §7.2"
    );

    // The observer must not change any hash: the same scenario run with `NoopObserver` and with
    // `observer` must be bit-identical.
    let mut baseline = World::new();
    scenario().run(&mut baseline);
    assert_eq!(
        state_hash(&baseline),
        state_hash(&world),
        "a SystemObserver must not change the state hash"
    );

    // A panicking task of a parallel stage: the stage gets neither `tasks_finished` nor
    // `system_finished` nor `stage_finished`.
    let panic_log = {
        let mut schedule = Schedule::new();
        schedule.add_parallel_system(parallel_system_fn(
            "conformance.panicking_task",
            Access::new(),
            |_world, _commands| panic!("grimoire_ecs::conformance::system_observer task probe"),
        ));
        let mut world = World::new();
        let mut recorder = Recorder {
            log: Vec::new(),
            inner: observer,
        };
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            schedule.run_observed(&mut world, &mut recorder);
        }));
        assert!(result.is_err(), "a task panic must propagate");
        recorder.log
    };
    assert_eq!(
        panic_log,
        vec!["stage_started(0, false)".to_owned()],
        "a panicking task must not deliver tasks_finished, system_finished or stage_finished"
    );

    // A panicking exclusive system: `system_started` without a matching `system_finished`/
    // `stage_finished` marks the panic.
    let panic_log = {
        let mut schedule = Schedule::new();
        schedule.add_system(system_fn("conformance.panicking_system", |_world| {
            panic!("grimoire_ecs::conformance::system_observer system probe")
        }));
        let mut world = World::new();
        let mut recorder = Recorder {
            log: Vec::new(),
            inner: observer,
        };
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            schedule.run_observed(&mut world, &mut recorder);
        }));
        assert!(result.is_err(), "a system panic must propagate");
        recorder.log
    };
    assert_eq!(
        panic_log,
        vec![
            "stage_started(0, true)".to_owned(),
            "system_started(conformance.panicking_system)".to_owned(),
        ],
        "a panicking exclusive system must not deliver system_finished or stage_finished"
    );
}
