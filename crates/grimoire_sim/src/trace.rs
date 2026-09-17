//! Subsystem hashes and divergence diagnosis (contract §8.5, OF-18.1, Plan 0002 WP7.2).
//!
//! A [`HashTrace`] records a run's state hashes at chosen ticks and, if asked, the world hash
//! after every system of those ticks. [`trace`] replays an [`InputLog`] and records one;
//! [`SystemHasher`] is the [`SystemObserver`] behind it, usable by any driver that steps a
//! [`Simulation`] itself. [`first_divergence`] compares two traces and names the first tick whose
//! hashes differ and, where the trace allows it, the first system after which they differ and its
//! subsystem.
//!
//! The world hash after a system is taken in `SystemObserver::system_finished`: for an exclusive
//! system right after it ran, for a parallel system right after its command buffer was applied.
//! By the stage rule of engine ADR-0006 that state is bit-identical to the state after the system
//! under `StageMode::Isolated`, so system hashes depend neither on the stage mode nor on the
//! executor (contract §7.2). Recording never changes [`Simulation::state_hash`].

use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroU64;

use grimoire_core::StableHasher;
use grimoire_ecs::{SystemInfo, SystemObserver, World};

use crate::input::InputLog;
use crate::simulation::Simulation;

/// Recording density of a [`HashTrace`] (OF-18.1).
///
/// Checkpoints are the ticks `t` with `t % interval == 0` (counted like [`Simulation::tick`]
/// after the step, the convention of [`crate::replay`]), plus the start and the end of the run.
/// With system hashes, every checkpoint after a step also holds the world hash after each system
/// of that step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TraceGranularity {
    interval: NonZeroU64,
    system_hashes: bool,
}

impl TraceGranularity {
    /// A checkpoint after every tick, each with the world hash after every system: the exact
    /// first diverging tick and system, at the highest cost.
    pub const PER_SYSTEM_PER_TICK: Self = Self {
        interval: NonZeroU64::MIN,
        system_hashes: true,
    };

    /// The state hash every `interval` ticks, without system hashes.
    #[must_use]
    pub const fn every(interval: NonZeroU64) -> Self {
        Self {
            interval,
            system_hashes: false,
        }
    }

    /// The same checkpoints, each additionally with the world hash after every system.
    #[must_use]
    pub const fn with_system_hashes(self) -> Self {
        Self {
            interval: self.interval,
            system_hashes: true,
        }
    }

    /// Ticks between two regular checkpoints.
    #[must_use]
    pub const fn interval(self) -> NonZeroU64 {
        self.interval
    }

    /// Whether checkpoints hold system hashes.
    #[must_use]
    pub const fn has_system_hashes(self) -> bool {
        self.system_hashes
    }
}

/// The hashes recorded at one tick of a [`HashTrace`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceCheckpoint {
    /// [`Simulation::tick`] when the hashes were taken: after the step, or the start tick for the
    /// first checkpoint.
    pub tick: u64,
    /// [`Simulation::state_hash`] at `tick`.
    pub state_hash: u64,
    /// World hash ([`system_hash`]) after each system of the step that led to `tick`, indexed by
    /// system list position. Empty without system hashes and for the start checkpoint.
    pub system_hashes: Vec<u64>,
}

impl TraceCheckpoint {
    /// A checkpoint from its fields, for example read back from a stored golden master.
    #[must_use]
    pub const fn new(tick: u64, state_hash: u64, system_hashes: Vec<u64>) -> Self {
        Self {
            tick,
            state_hash,
            system_hashes,
        }
    }
}

/// The recorded hashes of one run; see the module documentation.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashTrace {
    /// How densely the run was recorded.
    pub granularity: TraceGranularity,
    /// Names of the schedule's systems in list order ([`grimoire_ecs::Schedule::system_names`]).
    pub system_names: Vec<String>,
    /// Checkpoints in strictly ascending tick order.
    pub checkpoints: Vec<TraceCheckpoint>,
}

impl HashTrace {
    /// A trace from its fields, for example read back from a stored golden master.
    #[must_use]
    pub const fn new(
        granularity: TraceGranularity,
        system_names: Vec<String>,
        checkpoints: Vec<TraceCheckpoint>,
    ) -> Self {
        Self {
            granularity,
            system_names,
            checkpoints,
        }
    }
}

/// Hash of `world` alone, as [`SystemHasher`] takes it after a system: a fresh `StableHasher` fed
/// with [`World::stable_hash`]. Unlike [`Simulation::state_hash`] it contains neither tick nor
/// seed, which do not change inside a step.
#[must_use]
pub fn system_hash(world: &World) -> u64 {
    let mut hasher = StableHasher::new();
    world.stable_hash(&mut hasher);
    hasher.finish()
}

/// [`SystemObserver`] that records [`system_hash`] after every system of a schedule run.
///
/// The hash of the system at list position `i` lands at index `i` of [`SystemHasher::hashes`].
/// [`SystemHasher::clear`] before each step keeps the buffer's allocation. Read-only like every
/// observer: it never changes the world or any hash (contract §7.2).
#[derive(Debug, Default, Clone)]
pub struct SystemHasher {
    hashes: Vec<u64>,
}

impl SystemHasher {
    /// An empty hasher.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Hashes recorded since the last [`SystemHasher::clear`], indexed by system list position.
    /// A system that did not finish (a panic) leaves its slot and every later one at `0`.
    #[must_use]
    pub fn hashes(&self) -> &[u64] {
        &self.hashes
    }

    /// Forgets the recorded hashes and keeps the allocation.
    pub fn clear(&mut self) {
        self.hashes.clear();
    }
}

impl SystemObserver for SystemHasher {
    fn system_finished(&mut self, system: SystemInfo<'_>, world: &World) {
        if self.hashes.len() <= system.index {
            self.hashes.resize(system.index + 1, 0);
        }
        self.hashes[system.index] = system_hash(world);
    }
}

/// Steps `sim` once per frame of `log` and records a [`HashTrace`] with `granularity`.
///
/// The first checkpoint is the state before the first step; a checkpoint follows every step that
/// leaves [`Simulation::tick`] a multiple of the interval, and the last step always gets one.
/// Steps between checkpoints run without an observer. Like [`crate::replay`], `sim` must be in
/// the state the log was recorded from; the seed is not checked here. The resulting states and
/// hashes are exactly those of [`crate::replay`].
pub fn trace(sim: &mut Simulation, log: &InputLog, granularity: TraceGranularity) -> HashTrace {
    let system_names = sim
        .schedule_mut()
        .system_names()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let interval = granularity.interval().get();
    let mut checkpoints = vec![TraceCheckpoint::new(
        sim.tick(),
        sim.state_hash(),
        Vec::new(),
    )];
    let mut hasher = SystemHasher::new();
    let last = log.frames.len().checked_sub(1);
    for (index, &input) in log.frames.iter().enumerate() {
        let recorded = sim.tick().wrapping_add(1).is_multiple_of(interval) || Some(index) == last;
        if !recorded {
            sim.step(input);
            continue;
        }
        let system_hashes = if granularity.has_system_hashes() {
            hasher.clear();
            sim.step_observed(input, &mut hasher);
            hasher.hashes().to_vec()
        } else {
            sim.step(input);
            Vec::new()
        };
        checkpoints.push(TraceCheckpoint::new(
            sim.tick(),
            sim.state_hash(),
            system_hashes,
        ));
    }
    HashTrace::new(granularity, system_names, checkpoints)
}

/// The system [`first_divergence`] names.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DivergentSystem {
    /// Position of the system in the schedule's system list.
    pub index: usize,
    /// Name of the system.
    pub name: String,
}

impl DivergentSystem {
    /// The subsystem the system belongs to: the part of its name before the first `.`
    /// (`"sigil"` for `"sigil.update"`), or the whole name if it has none. The facade's profiler
    /// groups systems the same way.
    #[must_use]
    pub fn subsystem(&self) -> &str {
        self.name.split('.').next().unwrap_or(&self.name)
    }
}

/// The first difference between two traces; see [`first_divergence`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Divergence {
    /// First tick present in both traces whose hashes differ.
    pub tick: u64,
    /// Latest earlier tick present in both traces whose state hashes match, `None` if there is
    /// none (for example if the start states already differ).
    pub last_matching_tick: Option<u64>,
    /// The first system, in list order, whose hash differs at `tick`. `None` if either trace has
    /// no system hashes there or the two schedules list different systems.
    pub system: Option<DivergentSystem>,
}

impl Divergence {
    /// Whether the difference certainly arose in the step to [`Divergence::tick`]: the state one
    /// tick earlier matched. Only then is [`Divergence::system`] the system in which it arose;
    /// otherwise the cause lies in a step after `last_matching_tick` and the system is merely the
    /// first one whose hash differs at `tick`.
    #[must_use]
    pub fn is_exact(&self) -> bool {
        self.last_matching_tick
            .is_some_and(|last| last.checked_add(1) == Some(self.tick))
    }
}

impl fmt::Display for Divergence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_exact() {
            write!(f, "first divergence in the step to tick {}", self.tick)?;
        } else {
            match self.last_matching_tick {
                Some(last) => write!(
                    f,
                    "first divergence after tick {last}, detected at tick {}",
                    self.tick
                )?,
                None => write!(
                    f,
                    "divergence at tick {} without a matching tick before it",
                    self.tick
                )?,
            }
        }
        match (&self.system, self.is_exact()) {
            (Some(system), true) => write!(
                f,
                ", in system `{}` (subsystem `{}`)",
                system.name,
                system.subsystem()
            ),
            (Some(system), false) => write!(
                f,
                "; at tick {} the first differing system is `{}` (subsystem `{}`)",
                self.tick,
                system.name,
                system.subsystem()
            ),
            (None, _) => Ok(()),
        }
    }
}

/// Compares `candidate` with `reference` and returns the first divergence, or `None` if every
/// tick present in both traces has identical hashes.
///
/// Only ticks present in both traces are compared, in ascending order; a tick differs if its
/// state hashes differ or both checkpoints hold system hashes that differ. At the first such
/// tick, the named system is the first list position whose system hash differs, provided both
/// traces hold system hashes there and list the same system names. Traces of different lengths
/// or granularities can be compared; a shorter trace simply ends the comparison early.
#[must_use]
pub fn first_divergence(reference: &HashTrace, candidate: &HashTrace) -> Option<Divergence> {
    let reference_by_tick: BTreeMap<u64, &TraceCheckpoint> = reference
        .checkpoints
        .iter()
        .map(|checkpoint| (checkpoint.tick, checkpoint))
        .collect();
    let mut last_matching_tick = None;
    for checkpoint in &candidate.checkpoints {
        let Some(expected) = reference_by_tick.get(&checkpoint.tick) else {
            continue;
        };
        let comparable_systems = !expected.system_hashes.is_empty()
            && !checkpoint.system_hashes.is_empty()
            && reference.system_names == candidate.system_names;
        let first_system = if comparable_systems {
            expected
                .system_hashes
                .iter()
                .zip(&checkpoint.system_hashes)
                .position(|(want, got)| want != got)
        } else {
            None
        };
        let systems_differ = !expected.system_hashes.is_empty()
            && !checkpoint.system_hashes.is_empty()
            && expected.system_hashes != checkpoint.system_hashes;
        if expected.state_hash == checkpoint.state_hash && !systems_differ {
            last_matching_tick = Some(checkpoint.tick);
            continue;
        }
        let system = first_system.and_then(|index| {
            candidate
                .system_names
                .get(index)
                .map(|name| DivergentSystem {
                    index,
                    name: name.clone(),
                })
        });
        return Some(Divergence {
            tick: checkpoint.tick,
            last_matching_tick,
            system,
        });
    }
    None
}
