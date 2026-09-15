//! # grimoire_exec
//!
//! Thread-pool executor of the Grimoire simulation (engine ADR-0006, building block 6).
//!
//! This crate is the only thread source of the simulation. It lies outside the determinism
//! crates and carries no determinism `clippy.toml`; no determinism crate depends on it or on
//! rayon, which CI checks. rayon is an implementation detail: no rayon type appears in the API.
//!
//! [`ThreadPoolExecutor`] implements [`grimoire_ecs::Executor`] on its own rayon pool with a fixed
//! number of workers. Stages, blocks and all results are merged by `grimoire_ecs` in task order,
//! so every state hash is the same for any thread count. Game binaries pass it to
//! `grimoire::AppBuilder::executor`; the game harness and the engine's hash gate use
//! [`gate_executors`].
//!
//! ```
//! use std::sync::Arc;
//!
//! use grimoire_ecs::{Executor, World};
//! use grimoire_exec::ThreadPoolExecutor;
//!
//! let mut world = World::new();
//! world.set_executor(Arc::new(ThreadPoolExecutor::new(2)?));
//! assert_eq!(world.executor().threads(), 2);
//! # Ok::<(), grimoire_exec::ExecError>(())
//! ```

use std::sync::Arc;

use grimoire_ecs::Executor;
use rayon::prelude::*;

/// Errors of this crate.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ExecError {
    /// A pool needs at least one worker thread.
    #[error("thread count must be at least 1")]
    ZeroThreads,
    /// The thread pool could not be built; the text comes from the pool implementation.
    #[error("thread pool could not be built: {0}")]
    Pool(String),
}

/// [`Executor`] on its own thread pool with exactly `threads` workers, never the global pool.
///
/// [`Executor::run`] hands every task to the pool and blocks the calling thread until all tasks
/// have finished. A nested call from a worker of the same pool runs its tasks on that pool
/// without deadlocking. A panic of a task is propagated to the caller of `run` after the other
/// tasks finished; tasks created by `grimoire_ecs` catch their own panics beforehand.
#[derive(Debug)]
pub struct ThreadPoolExecutor {
    pool: rayon::ThreadPool,
    threads: usize,
}

impl ThreadPoolExecutor {
    /// Builds a pool with exactly `threads` workers named `grimoire-sim-{index}`.
    ///
    /// # Errors
    ///
    /// [`ExecError::ZeroThreads`] if `threads` is 0, [`ExecError::Pool`] if the operating system
    /// refuses to create the threads.
    pub fn new(threads: usize) -> Result<Self, ExecError> {
        if threads == 0 {
            return Err(ExecError::ZeroThreads);
        }
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|index| format!("grimoire-sim-{index}"))
            .build()
            .map_err(|error| ExecError::Pool(error.to_string()))?;
        Ok(Self { pool, threads })
    }
}

impl Executor for ThreadPoolExecutor {
    fn threads(&self) -> usize {
        self.threads
    }

    fn run(&self, tasks: &mut [&mut (dyn FnMut() + Send)]) {
        self.pool.install(|| {
            // One task per rayon job: tasks of a stage or of query blocks are coarse.
            tasks.par_iter_mut().with_max_len(1).for_each(|task| task());
        });
    }
}

/// Environment variable overriding `N`, the largest thread count of the hash gate.
pub const GATE_THREADS_ENV: &str = "GRIMOIRE_GATE_THREADS";

/// `N` of the hash gate when [`GATE_THREADS_ENV`] is not set.
const DEFAULT_GATE_THREADS: usize = 4;

/// Executors of the hash gate (engine ADR-0006, building block 7): pools with 1, 2 and N
/// threads, labelled `threads-1`, `threads-2` and `threads-{N}`.
///
/// `N` is 4 unless [`GATE_THREADS_ENV`] is set; it never depends on the machine's core count.
/// Test helper for the engine gate and game harnesses.
///
/// # Panics
///
/// If [`GATE_THREADS_ENV`] is set but not an integer of at least 3, or if a pool cannot be built.
#[must_use]
pub fn gate_executors() -> Vec<(String, Arc<dyn Executor>)> {
    let value = std::env::var(GATE_THREADS_ENV).ok();
    let largest = match parse_gate_threads(value.as_deref()) {
        Ok(threads) => threads,
        Err(message) => panic!("{message}"),
    };
    [1, 2, largest]
        .into_iter()
        .map(|threads| {
            let executor = match ThreadPoolExecutor::new(threads) {
                Ok(executor) => executor,
                Err(error) => panic!("hash gate pool with {threads} threads: {error}"),
            };
            (
                format!("threads-{threads}"),
                Arc::new(executor) as Arc<dyn Executor>,
            )
        })
        .collect()
}

/// `N` of the hash gate from the value of [`GATE_THREADS_ENV`], if set.
fn parse_gate_threads(value: Option<&str>) -> Result<usize, String> {
    let Some(value) = value else {
        return Ok(DEFAULT_GATE_THREADS);
    };
    match value.trim().parse::<usize>() {
        Ok(threads) if threads >= 3 => Ok(threads),
        _ => Err(format!(
            "{GATE_THREADS_ENV} must be an integer of at least 3, got `{value}`"
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn run_all(executor: &dyn Executor, tasks: &mut [Box<dyn FnMut() + Send + '_>]) {
        let mut refs: Vec<&mut (dyn FnMut() + Send)> = tasks
            .iter_mut()
            .map(|task| &mut **task as &mut (dyn FnMut() + Send))
            .collect();
        executor.run(&mut refs);
    }

    #[test]
    fn zero_threads_are_rejected() {
        assert!(matches!(
            ThreadPoolExecutor::new(0),
            Err(ExecError::ZeroThreads)
        ));
        assert_eq!(
            ExecError::ZeroThreads.to_string(),
            "thread count must be at least 1"
        );
    }

    #[test]
    fn thread_count_is_reported() {
        for threads in [1, 3] {
            let executor = ThreadPoolExecutor::new(threads).expect("pool");
            assert_eq!(executor.threads(), threads);
        }
    }

    #[test]
    fn every_task_runs_exactly_once() {
        for threads in [1, 2, 4] {
            let executor = ThreadPoolExecutor::new(threads).expect("pool");
            for count in [1, 7, 1_000] {
                let mut runs = vec![0usize; count];
                {
                    let mut tasks: Vec<Box<dyn FnMut() + Send + '_>> = runs
                        .iter_mut()
                        .map(|slot| Box::new(move || *slot += 1) as Box<dyn FnMut() + Send + '_>)
                        .collect();
                    run_all(&executor, &mut tasks);
                }
                assert!(
                    runs.iter().all(|&runs| runs == 1),
                    "{threads} threads, {count} tasks"
                );
            }
        }
    }

    #[test]
    fn nested_runs_finish() {
        for threads in [1, 4] {
            let executor = ThreadPoolExecutor::new(threads).expect("pool");
            let counter = AtomicUsize::new(0);
            {
                let mut outer: Vec<Box<dyn FnMut() + Send + '_>> = (0..6)
                    .map(|_| {
                        let executor = &executor;
                        let counter = &counter;
                        Box::new(move || {
                            let mut inner: Vec<Box<dyn FnMut() + Send + '_>> = (0..5)
                                .map(|_| {
                                    Box::new(move || {
                                        counter.fetch_add(1, Ordering::Relaxed);
                                    })
                                        as Box<dyn FnMut() + Send + '_>
                                })
                                .collect();
                            run_all(executor, &mut inner);
                        }) as Box<dyn FnMut() + Send + '_>
                    })
                    .collect();
                run_all(&executor, &mut outer);
            }
            assert_eq!(counter.load(Ordering::Relaxed), 30, "{threads} threads");
        }
    }

    #[test]
    fn a_task_panic_propagates() {
        let executor = ThreadPoolExecutor::new(2).expect("pool");
        let finished = AtomicUsize::new(0);
        let result = catch_unwind(AssertUnwindSafe(|| {
            let mut tasks: Vec<Box<dyn FnMut() + Send + '_>> = (0..8)
                .map(|index| {
                    let finished = &finished;
                    Box::new(move || {
                        if index == 3 {
                            panic!("task {index}");
                        }
                        finished.fetch_add(1, Ordering::Relaxed);
                    }) as Box<dyn FnMut() + Send + '_>
                })
                .collect();
            run_all(&executor, &mut tasks);
        }));
        let payload = result.expect_err("the panic reaches the caller");
        assert_eq!(
            payload.downcast_ref::<String>().map(String::as_str),
            Some("task 3")
        );
        // The pool stays usable.
        let mut again: Vec<Box<dyn FnMut() + Send + '_>> = vec![Box::new(|| {
            finished.fetch_add(1, Ordering::Relaxed);
        })];
        run_all(&executor, &mut again);
        assert!(finished.load(Ordering::Relaxed) >= 1);
    }

    #[test]
    fn gate_thread_count_defaults_to_four() {
        assert_eq!(parse_gate_threads(None), Ok(4));
        assert_eq!(parse_gate_threads(Some("6")), Ok(6));
        assert_eq!(parse_gate_threads(Some(" 3 ")), Ok(3));
    }

    #[test]
    fn invalid_gate_thread_counts_are_rejected() {
        for value in ["2", "0", "x", ""] {
            let error = parse_gate_threads(Some(value)).expect_err(value);
            assert!(error.starts_with("GRIMOIRE_GATE_THREADS must be an integer of at least 3"));
        }
    }

    #[test]
    fn gate_executors_have_one_two_and_n_threads() {
        // Only meaningful without an override in the environment; the parser is tested above.
        if std::env::var_os(GATE_THREADS_ENV).is_some() {
            return;
        }
        let executors = gate_executors();
        let labels: Vec<&str> = executors.iter().map(|(label, _)| label.as_str()).collect();
        assert_eq!(labels, ["threads-1", "threads-2", "threads-4"]);
        let threads: Vec<usize> = executors
            .iter()
            .map(|(_, executor)| executor.threads())
            .collect();
        assert_eq!(threads, [1, 2, 4]);
    }
}
