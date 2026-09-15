//! Executors run the tasks of parallel stages and data-parallel queries (engine ADR-0006,
//! building block 6).
//!
//! `grimoire_ecs` never creates threads. It hands a slice of tasks to an [`Executor`] and merges
//! all results by task index itself, so no state or hash depends on the executor, the thread
//! count, the executing thread or the completion order. Thread pools live in `grimoire_exec`,
//! outside the determinism crates.

use std::sync::atomic::{AtomicU64, Ordering};

/// Runs a batch of tasks, possibly concurrently.
///
/// Contract:
///
/// - [`Executor::run`] runs every task exactly once and returns only when all have finished.
/// - Order, concurrency and executing thread are unspecified; results never pass through the
///   executor.
/// - A nested call of [`Executor::run`] from inside a task must not deadlock.
/// - Tasks created by `grimoire_ecs` catch their own panics. An executor must never swallow a
///   panic of a task.
pub trait Executor: Send + Sync {
    /// Worker threads of this executor. Diagnostics only: it never influences stages, blocks or
    /// simulation state.
    fn threads(&self) -> usize;

    /// Runs every task exactly once and returns when all have finished.
    fn run(&self, tasks: &mut [&mut (dyn FnMut() + Send)]);
}

/// Runs tasks in index order on the calling thread; the default executor of every
/// [`World`](crate::World).
#[derive(Clone, Copy, Debug, Default)]
pub struct SequentialExecutor;

impl Executor for SequentialExecutor {
    fn threads(&self) -> usize {
        1
    }

    fn run(&self, tasks: &mut [&mut (dyn FnMut() + Send)]) {
        for task in tasks.iter_mut() {
            task();
        }
    }
}

/// Test executor: runs tasks on the calling thread in a permuted order.
///
/// [`PermutedExecutor::new`] derives a new permutation for every call of [`Executor::run`] from
/// the seed and a call counter (Fisher–Yates driven by SplitMix64 mixing, integer arithmetic
/// only), so a run is reproducible for a given seed and sequence of calls.
/// [`PermutedExecutor::reversed`] always runs from the last task to the first. Code whose result
/// depends on the completion order of tasks shows up as a hash mismatch against
/// [`SequentialExecutor`] without needing real threads.
#[derive(Debug)]
pub struct PermutedExecutor {
    seed: u64,
    calls: AtomicU64,
    reversed: bool,
}

impl PermutedExecutor {
    /// Executor with permutations derived from `seed` and the number of previous calls.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            calls: AtomicU64::new(0),
            reversed: false,
        }
    }

    /// Executor that always runs tasks from last to first.
    #[must_use]
    pub fn reversed() -> Self {
        Self {
            seed: 0,
            calls: AtomicU64::new(0),
            reversed: true,
        }
    }
}

/// SplitMix64 output function applied to `value + γ`.
const fn mix(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

impl Executor for PermutedExecutor {
    fn threads(&self) -> usize {
        1
    }

    fn run(&self, tasks: &mut [&mut (dyn FnMut() + Send)]) {
        let mut order: Vec<usize> = (0..tasks.len()).collect();
        if self.reversed {
            order.reverse();
        } else {
            let call = self.calls.fetch_add(1, Ordering::Relaxed);
            let mut state = mix(self.seed ^ mix(call));
            for last in (1..order.len()).rev() {
                state = mix(state);
                // `last + 1` fits in u64 and the remainder is at most `last`, so both casts are
                // lossless.
                let pick = (state % (last as u64 + 1)) as usize;
                order.swap(last, pick);
            }
        }
        for index in order {
            (tasks[index])();
        }
    }
}
