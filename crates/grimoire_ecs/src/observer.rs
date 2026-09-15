//! Read-only observation of a running [`crate::Schedule`] (contract §7.2, additive P1 extension).
//!
//! [`SystemObserver`] lets a caller (the facade's profiler) measure stages and systems without
//! changing the stage plan, the world or any hash: [`crate::Schedule::run`] is exactly
//! [`crate::Schedule::run_observed`] with a [`NoopObserver`]. All calls happen on the calling
//! thread of `run_observed`, never on a worker and never from inside a task or block.

use crate::world::World;

/// Diagnostic view of one stage while it runs; see [`SystemObserver`].
///
/// Has no public constructor (contract §2 rule 13): only [`crate::Schedule::run_observed`]
/// creates one.
#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StageInfo {
    /// Position of the stage in [`crate::Schedule::stages`].
    pub index: usize,
    /// Whether the stage is a single exclusive system.
    pub exclusive: bool,
    /// List index (see [`crate::Schedule::system_names`]) of the first system of the stage.
    pub first_system: usize,
    /// Number of systems in the stage.
    pub len: usize,
}

/// Diagnostic view of one system while it runs; see [`SystemObserver`].
///
/// Has no public constructor (contract §2 rule 13): only [`crate::Schedule::run_observed`]
/// creates one.
#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SystemInfo<'s> {
    /// Position of the system in the schedule's system list.
    pub index: usize,
    /// Index of the stage the system belongs to.
    pub stage: usize,
    /// Name of the system.
    pub name: &'s str,
    /// Whether the system is a [`crate::ParallelSystem`] (`false` for an exclusive
    /// [`crate::System`]).
    pub parallel: bool,
}

/// Read-only observer of a [`crate::Schedule::run_observed`] call.
///
/// Every method has a no-op default so implementers only override what they measure. An
/// implementation never needs `Send`/`Sync`: it runs only on the thread that called
/// `run_observed` (contract §7.2).
///
/// Call order:
/// - exclusive stage: [`Self::stage_started`] → [`Self::system_started`] → the system runs →
///   [`Self::system_finished`] → [`Self::stage_finished`];
/// - parallel stage: [`Self::stage_started`] → every task runs → [`Self::tasks_finished`] → for
///   each system in list order, its command buffer is applied, then [`Self::system_finished`] →
///   [`Self::stage_finished`].
///
/// If a task of a parallel stage panics, that stage gets none of [`Self::tasks_finished`],
/// [`Self::system_finished`] or [`Self::stage_finished`]. If applying a buffer panics, systems
/// whose buffer already applied still get [`Self::system_finished`]; the panicking system, every
/// later one and the stage do not. In an exclusive stage, [`Self::system_started`] without a
/// matching [`Self::system_finished`]/[`Self::stage_finished`] marks a panic there.
pub trait SystemObserver {
    /// A stage begins.
    fn stage_started(&mut self, stage: StageInfo) {
        let _ = stage;
    }
    /// An exclusive system is about to run (never called for parallel stages).
    fn system_started(&mut self, system: SystemInfo<'_>) {
        let _ = system;
    }
    /// Every task of a parallel stage has finished, before any command buffer is applied.
    fn tasks_finished(&mut self, stage: StageInfo) {
        let _ = stage;
    }
    /// A system has finished: for an exclusive system, right after it ran; for a parallel system,
    /// right after its command buffer was applied.
    fn system_finished(&mut self, system: SystemInfo<'_>, world: &World) {
        let _ = (system, world);
    }
    /// A stage has finished.
    fn stage_finished(&mut self, stage: StageInfo, world: &World) {
        let _ = (stage, world);
    }
}

/// Observer with no effect; used by [`crate::Schedule::run`].
#[derive(Clone, Copy, Default, Debug)]
pub struct NoopObserver;

impl SystemObserver for NoopObserver {}

#[cfg(all(test, feature = "conformance"))]
mod conformance_tests {
    use super::NoopObserver;

    #[test]
    fn noop_observer_is_conformant() {
        crate::conformance::system_observer(&mut NoopObserver);
    }
}
