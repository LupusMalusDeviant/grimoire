//! P1 additions to `grimoire_ecs` (contract §7 conformance, §7.1, §7.2, §7.3).

use std::any::Any;
use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

use grimoire_ecs::{Executor, QUERY_BLOCK_SIZE, Resource, Schedule, World, WorldSnapshot};

// ---- §7.1 -------------------------------------------------------------------------------------

/// `[0, B), [B, 2B), …`, `B = QUERY_BLOCK_SIZE`; last range shorter; `len == 0` yields none.
pub fn slice_block_ranges(len: usize) -> impl ExactSizeIterator<Item = Range<usize>> {
    let count = len.div_ceil(QUERY_BLOCK_SIZE);
    (0..count).map(move |index| {
        let start = index * QUERY_BLOCK_SIZE;
        start..(start + QUERY_BLOCK_SIZE).min(len)
    })
}

struct Slot<B, T> {
    block: Option<B>,
    output: Option<T>,
    panic: Option<Box<dyn Any + Send>>,
}

/// Generalisation of the crate-internal `run_blocks` (implemented to check the bounds).
pub fn run_blocks<B: Send, T: Send>(
    executor: &dyn Executor,
    blocks: Vec<B>,
    f: impl Fn(B) -> T + Sync,
) -> Vec<T> {
    if blocks.len() <= 1 {
        return blocks.into_iter().map(f).collect();
    }
    let f = &f;
    let mut slots: Vec<Slot<B, T>> = blocks
        .into_iter()
        .map(|block| Slot {
            block: Some(block),
            output: None,
            panic: None,
        })
        .collect();
    {
        let mut tasks: Vec<_> = slots
            .iter_mut()
            .map(|slot| {
                move || {
                    if let Some(block) = slot.block.take() {
                        match catch_unwind(AssertUnwindSafe(|| f(block))) {
                            Ok(output) => slot.output = Some(output),
                            Err(payload) => slot.panic = Some(payload),
                        }
                    }
                }
            })
            .collect();
        let mut refs: Vec<&mut (dyn FnMut() + Send)> = tasks
            .iter_mut()
            .map(|task| task as &mut (dyn FnMut() + Send))
            .collect();
        executor.run(&mut refs);
    }
    let mut outputs = Vec::with_capacity(slots.len());
    let mut first_panic = None;
    for slot in slots {
        if let Some(payload) = slot.panic {
            first_panic.get_or_insert(payload);
        } else if let Some(output) = slot.output {
            outputs.push(output);
        }
    }
    if let Some(payload) = first_panic {
        resume_unwind(payload);
    }
    outputs
}

// ---- §7.2 -------------------------------------------------------------------------------------

/// Only produced by `Schedule::run_observed` (§2 rule 13 exemption).
#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StageInfo {
    pub index: usize,
    pub exclusive: bool,
    pub first_system: usize,
    pub len: usize,
}

/// Only produced by `Schedule::run_observed` (§2 rule 13 exemption).
#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SystemInfo<'s> {
    pub index: usize,
    pub stage: usize,
    pub name: &'s str,
    pub parallel: bool,
}

pub trait SystemObserver {
    fn stage_started(&mut self, stage: StageInfo) {
        let _ = stage;
    }
    fn system_started(&mut self, system: SystemInfo<'_>) {
        let _ = system;
    }
    fn tasks_finished(&mut self, stage: StageInfo) {
        let _ = stage;
    }
    fn system_finished(&mut self, system: SystemInfo<'_>, world: &World) {
        let _ = (system, world);
    }
    fn stage_finished(&mut self, stage: StageInfo, world: &World) {
        let _ = (stage, world);
    }
}

#[derive(Default, Clone, Copy, Debug)]
pub struct NoopObserver;

impl SystemObserver for NoopObserver {}

/// Stand-in for the inherent method `Schedule::run_observed`.
pub trait ScheduleRunObserved {
    fn run_observed(&mut self, world: &mut World, observer: &mut dyn SystemObserver);
}

impl ScheduleRunObserved for Schedule {
    fn run_observed(&mut self, world: &mut World, observer: &mut dyn SystemObserver) {
        let _ = (world, observer);
        unimplemented!()
    }
}

// ---- §7.3 -------------------------------------------------------------------------------------

/// Stand-in for the inherent method `WorldSnapshot::resource`.
pub trait WorldSnapshotResource {
    fn resource<R: Resource>(&self) -> Option<&R>;
}

impl WorldSnapshotResource for WorldSnapshot {
    fn resource<R: Resource>(&self) -> Option<&R> {
        unimplemented!()
    }
}

// ---- §7 conformance ---------------------------------------------------------------------------

#[cfg(feature = "conformance")]
pub mod conformance {
    use super::{Executor, SystemObserver};

    pub fn executor(executor: &dyn Executor) {
        let _ = executor;
        unimplemented!()
    }

    pub fn system_observer(observer: &mut dyn SystemObserver) {
        let _ = observer;
        unimplemented!()
    }
}
