//! Systems, stages and the schedule (engine ADR-0006, which replaces the single-threaded
//! proposal ADR-0003).
//!
//! A [`Schedule`] is an ordered list of exclusive [`System`]s and [`ParallelSystem`]s. It splits
//! the list into stages from the list, the declared [`Access`] and the [`StageMode`] alone and
//! never reorders:
//!
//! - every exclusive system forms its own stage and runs with `&mut World`, exactly as in P0;
//! - a parallel system joins the open parallel stage unless it reads a component or resource that
//!   an earlier system of that stage writes, or an earlier system of the stage records structural
//!   commands; otherwise it starts a new stage.
//!
//! All systems of a parallel stage see the same unchanged `&World` and record into their own
//! [`CommandBuffer`]. They run through [`World::executor`]; after all have finished the buffers
//! are applied in list order, never in completion order. The result equals running every system
//! alone with its buffer applied right after it ([`StageMode::Isolated`]).

use std::any::{Any, TypeId};
use std::collections::BTreeMap;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

use crate::access::{Access, Declared};
use crate::command::CommandBuffer;
use crate::world::World;

/// A unit of simulation logic that runs exclusively with `&mut World`.
///
/// Every exclusive system forms its own stage. It needs no access declaration and no `Send`.
pub trait System {
    /// Human-readable name for diagnostics and ordering checks.
    fn name(&self) -> &str;
    /// Runs the system once against `world`.
    fn run(&mut self, world: &mut World);
}

struct FnSystem<F> {
    name: &'static str,
    function: F,
}

impl<F: FnMut(&mut World) + Send + 'static> System for FnSystem<F> {
    fn name(&self) -> &str {
        self.name
    }

    fn run(&mut self, world: &mut World) {
        (self.function)(world);
    }
}

/// Wraps a closure as a named exclusive [`System`].
pub fn system_fn<F: FnMut(&mut World) + Send + 'static>(name: &'static str, f: F) -> impl System {
    FnSystem { name, function: f }
}

/// A system that may run in a parallel stage: it reads the unchanged world and writes deferred
/// through its own [`CommandBuffer`] (engine ADR-0006, building block 3).
///
/// Everything the system reads or records must be covered by [`ParallelSystem::access`]; debug
/// builds check that. Commands become visible only after the stage, also to the recording system.
pub trait ParallelSystem: Send {
    /// Human-readable name for diagnostics and stage plans.
    fn name(&self) -> &str;
    /// Declared access; read exactly once, by [`Schedule::add_parallel_system`].
    fn access(&self) -> Access;
    /// Runs the system once against the unchanged `world`, recording into `commands`.
    fn run(&mut self, world: &World, commands: &mut CommandBuffer);
}

struct FnParallelSystem<F> {
    name: &'static str,
    access: Access,
    function: F,
}

impl<F: FnMut(&World, &mut CommandBuffer) + Send + 'static> ParallelSystem for FnParallelSystem<F> {
    fn name(&self) -> &str {
        self.name
    }

    fn access(&self) -> Access {
        self.access.clone()
    }

    fn run(&mut self, world: &World, commands: &mut CommandBuffer) {
        (self.function)(world, commands);
    }
}

/// Wraps a closure as a named [`ParallelSystem`] with the declared `access`.
pub fn parallel_system_fn<F>(name: &'static str, access: Access, f: F) -> impl ParallelSystem
where
    F: FnMut(&World, &mut CommandBuffer) + Send + 'static,
{
    FnParallelSystem {
        name,
        access,
        function: f,
    }
}

/// How a [`Schedule`] groups parallel systems into stages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StageMode {
    /// The stage rule of engine ADR-0006 (default).
    #[default]
    Grouped,
    /// Every system in its own stage: the reference semantics, in which every buffer is applied
    /// right after its system. [`StageMode::Grouped`] gives bit-identical states.
    Isolated,
}

/// Diagnostic view of one stage of a [`Schedule`]; see [`Schedule::stages`].
///
/// `Display` renders `{exclusive|parallel} [{names}] ({reason})`, for example
/// ``parallel [census, drag] (reads component `game::Pos`, written by `integrate`)``.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Stage<'s> {
    /// Whether the stage is a single exclusive system.
    pub exclusive: bool,
    /// Names of the systems of the stage, in list order.
    pub systems: Vec<&'s str>,
    /// Why the stage begins here: `exclusive system`, `first stage`,
    /// `follows an exclusive system`, `isolated stage mode`,
    /// `` `a` records structural commands ``, ``reads component `T`, written by `a` `` or
    /// ``reads resource `R`, written by `a` ``. Type names come from `std::any::type_name` and
    /// serve diagnostics only.
    pub reason: String,
}

impl fmt::Display for Stage<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = if self.exclusive {
            "exclusive"
        } else {
            "parallel"
        };
        write!(f, "{kind} [{}] ({})", self.systems.join(", "), self.reason)
    }
}

/// Schedule-local registration numbers of declared types, in first-declaration order.
#[derive(Default)]
struct Registry {
    numbers: BTreeMap<TypeId, u32>,
    names: Vec<&'static str>,
}

impl Registry {
    fn number(&mut self, declared: &Declared) -> u32 {
        if let Some(&number) = self.numbers.get(&declared.type_id) {
            return number;
        }
        let Ok(number) = u32::try_from(self.names.len()) else {
            panic!("a schedule cannot declare more than u32::MAX types");
        };
        self.names.push(declared.name);
        self.numbers.insert(declared.type_id, number);
        number
    }
}

/// Declared access of one parallel system as sorted registration numbers.
#[derive(Default)]
struct Resolved {
    reads: Vec<u32>,
    writes: Vec<u32>,
    resource_reads: Vec<u32>,
    resource_writes: Vec<u32>,
    structural: bool,
}

struct ParallelEntry {
    system: Box<dyn ParallelSystem>,
    resolved: Resolved,
    /// Owned by the schedule and reused every tick.
    commands: CommandBuffer,
    /// Panic payload of the last run, taken after the stage.
    panic: Option<Box<dyn Any + Send>>,
}

enum Entry {
    Exclusive(Box<dyn System>),
    Parallel(ParallelEntry),
}

impl Entry {
    fn name(&self) -> &str {
        match self {
            Entry::Exclusive(system) => system.name(),
            Entry::Parallel(entry) => entry.system.name(),
        }
    }
}

/// Entries `start..end` form one stage.
struct StageRange {
    start: usize,
    end: usize,
    exclusive: bool,
    reason: String,
}

/// Writers of the open parallel stage while stages are planned.
#[derive(Default)]
struct OpenStage {
    /// First writer (entry index) per component registration number.
    component_writers: BTreeMap<u32, usize>,
    /// First writer (entry index) per resource registration number.
    resource_writers: BTreeMap<u32, usize>,
    /// First member that records structural commands.
    structural: Option<usize>,
}

/// Ordered list of systems, split into stages (engine ADR-0006).
///
/// Systems keep the order in which they were added; the schedule never reorders them. Stages are
/// planned when systems are added or the mode changes, never while running. A schedule of only
/// exclusive systems runs exactly like the P0 loop and allocates nothing per tick.
///
/// ```
/// use grimoire_core::impl_stable_hash;
/// use grimoire_ecs::{Access, Entity, Schedule, World, parallel_system_fn, system_fn};
///
/// #[derive(Clone)]
/// struct Pos { x: f32 }
/// impl_stable_hash!(Pos { x });
///
/// let mut schedule = Schedule::new();
/// schedule
///     .add_system(system_fn("integrate", |world| {
///         for pos in world.query_mut::<&mut Pos>() {
///             pos.x += 1.0;
///         }
///     }))
///     .add_parallel_system(parallel_system_fn(
///         "clamp",
///         Access::new().read::<Pos>().write::<Pos>(),
///         |world, commands| {
///             for (entity, pos) in world.query::<(Entity, &Pos)>() {
///                 if pos.x > 1.5 {
///                     commands.set(entity, Pos { x: 1.5 });
///                 }
///             }
///         },
///     ));
/// let plan: Vec<String> = schedule.stages().iter().map(ToString::to_string).collect();
/// assert_eq!(
///     plan,
///     [
///         "exclusive [integrate] (exclusive system)",
///         "parallel [clamp] (follows an exclusive system)",
///     ]
/// );
///
/// let mut world = World::new();
/// world.spawn((Pos { x: 1.0 },));
/// schedule.run(&mut world);
/// schedule.run(&mut world);
/// assert_eq!(world.query::<&Pos>().next().map(|pos| pos.x), Some(1.5));
/// ```
#[derive(Default)]
pub struct Schedule {
    entries: Vec<Entry>,
    stages: Vec<StageRange>,
    components: Registry,
    resources: Registry,
    mode: StageMode,
}

impl fmt::Debug for Schedule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Schedule")
            .field("systems", &self.system_names())
            .finish()
    }
}

impl Schedule {
    /// Creates an empty schedule.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends the exclusive `system` after all previously added systems.
    pub fn add_system(&mut self, system: impl System + 'static) -> &mut Self {
        self.entries.push(Entry::Exclusive(Box::new(system)));
        self.rebuild_stages();
        self
    }

    /// Appends the parallel `system` after all previously added systems.
    ///
    /// Reads [`ParallelSystem::access`] once and resolves it into schedule-local registration
    /// numbers (one sequence for components and one for resources, in first-declaration order).
    /// No world is read or changed.
    pub fn add_parallel_system(&mut self, system: impl ParallelSystem + 'static) -> &mut Self {
        let access = system.access();
        let mut resolved = Resolved {
            structural: access.structural,
            ..Resolved::default()
        };
        for declared in &access.components {
            let number = self.components.number(declared);
            if declared.write {
                resolved.writes.push(number);
            } else {
                resolved.reads.push(number);
            }
        }
        for declared in &access.resources {
            let number = self.resources.number(declared);
            if declared.write {
                resolved.resource_writes.push(number);
            } else {
                resolved.resource_reads.push(number);
            }
        }
        for list in [
            &mut resolved.reads,
            &mut resolved.writes,
            &mut resolved.resource_reads,
            &mut resolved.resource_writes,
        ] {
            list.sort_unstable();
            list.dedup();
        }
        self.entries.push(Entry::Parallel(ParallelEntry {
            system: Box::new(system),
            resolved,
            commands: CommandBuffer::new(),
            panic: None,
        }));
        self.rebuild_stages();
        self
    }

    /// Selects how parallel systems are grouped (default [`StageMode::Grouped`]).
    pub fn set_stage_mode(&mut self, mode: StageMode) -> &mut Self {
        self.mode = mode;
        self.rebuild_stages();
        self
    }

    /// The current [`StageMode`].
    #[must_use]
    pub fn stage_mode(&self) -> StageMode {
        self.mode
    }

    /// The stage plan with the reason each stage begins, in execution order.
    #[must_use]
    pub fn stages(&self) -> Vec<Stage<'_>> {
        self.stages
            .iter()
            .map(|range| Stage {
                exclusive: range.exclusive,
                systems: self.entries[range.start..range.end]
                    .iter()
                    .map(Entry::name)
                    .collect(),
                reason: range.reason.clone(),
            })
            .collect()
    }

    /// Plans the stages in one forward pass over the entries; see the module documentation.
    fn rebuild_stages(&mut self) {
        self.stages.clear();
        let mut open: Option<OpenStage> = None;
        for index in 0..self.entries.len() {
            let Entry::Parallel(entry) = &self.entries[index] else {
                open = None;
                self.stages.push(StageRange {
                    start: index,
                    end: index + 1,
                    exclusive: true,
                    reason: String::from("exclusive system"),
                });
                continue;
            };
            let resolved = &entry.resolved;
            let reason = match &open {
                None if self.stages.is_empty() => Some(String::from("first stage")),
                None => Some(String::from("follows an exclusive system")),
                Some(_) if self.mode == StageMode::Isolated => {
                    Some(String::from("isolated stage mode"))
                }
                Some(OpenStage {
                    structural: Some(member),
                    ..
                }) => Some(format!(
                    "`{}` records structural commands",
                    self.entries[*member].name()
                )),
                Some(stage) => self.conflict(resolved, stage),
            };
            if let Some(reason) = reason {
                open = Some(OpenStage::default());
                self.stages.push(StageRange {
                    start: index,
                    end: index + 1,
                    exclusive: false,
                    reason,
                });
            } else if let Some(last) = self.stages.last_mut() {
                last.end = index + 1;
            }
            if let Some(stage) = &mut open {
                for &number in &resolved.writes {
                    stage.component_writers.entry(number).or_insert(index);
                }
                for &number in &resolved.resource_writes {
                    stage.resource_writers.entry(number).or_insert(index);
                }
                if resolved.structural && stage.structural.is_none() {
                    stage.structural = Some(index);
                }
            }
        }
    }

    /// The first declared read (components first, ascending registration number) that a member
    /// of the open stage writes, as a stage reason.
    fn conflict(&self, resolved: &Resolved, stage: &OpenStage) -> Option<String> {
        let component = resolved.reads.iter().find_map(|number| {
            stage.component_writers.get(number).map(|&writer| {
                format!(
                    "reads component `{}`, written by `{}`",
                    self.components.names[*number as usize],
                    self.entries[writer].name()
                )
            })
        });
        component.or_else(|| {
            resolved.resource_reads.iter().find_map(|number| {
                stage.resource_writers.get(number).map(|&writer| {
                    format!(
                        "reads resource `{}`, written by `{}`",
                        self.resources.names[*number as usize],
                        self.entries[writer].name()
                    )
                })
            })
        })
    }

    /// Runs every stage once, in order.
    ///
    /// # Panics
    ///
    /// Propagates the panic of a system. If systems of a parallel stage panic, the remaining
    /// tasks of the stage finish, every buffer of the stage is discarded and the payload of the
    /// system with the lowest list index is resumed: no command of the stage is applied, earlier
    /// stages of the call stay applied. Panics of exclusive systems and while applying commands
    /// are not transactional.
    pub fn run(&mut self, world: &mut World) {
        for stage in 0..self.stages.len() {
            let (start, end, exclusive) = {
                let range = &self.stages[stage];
                (range.start, range.end, range.exclusive)
            };
            if exclusive {
                if let Entry::Exclusive(system) = &mut self.entries[start] {
                    system.run(world);
                }
                continue;
            }
            run_parallel_stage(&mut self.entries[start..end], world);
        }
    }

    /// Names of all systems in list order.
    #[must_use]
    pub fn system_names(&self) -> Vec<&str> {
        self.entries.iter().map(Entry::name).collect()
    }

    /// Number of systems of both kinds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the schedule contains no systems.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Runs the parallel systems `entries` of one stage and applies their buffers in list order.
fn run_parallel_stage(entries: &mut [Entry], world: &mut World) {
    {
        let shared: &World = world;
        let mut tasks: Vec<_> = entries
            .iter_mut()
            .filter_map(|entry| match entry {
                Entry::Parallel(entry) => Some(entry),
                Entry::Exclusive(_) => None,
            })
            .map(|entry| move || run_task(entry, shared))
            .collect();
        if let [task] = tasks.as_mut_slice() {
            task();
        } else {
            let mut refs: Vec<&mut (dyn FnMut() + Send)> = tasks
                .iter_mut()
                .map(|task| task as &mut (dyn FnMut() + Send))
                .collect();
            shared.executor().run(&mut refs);
        }
    }
    let mut first_panic = None;
    for entry in entries.iter_mut() {
        if let Entry::Parallel(entry) = entry
            && let Some(payload) = entry.panic.take()
        {
            first_panic.get_or_insert(payload);
        }
    }
    if let Some(payload) = first_panic {
        for entry in entries.iter_mut() {
            if let Entry::Parallel(entry) = entry {
                entry.commands.clear();
            }
        }
        resume_unwind(payload);
    }
    for entry in entries.iter_mut() {
        if let Entry::Parallel(entry) = entry {
            entry.commands.apply(world);
        }
    }
}

/// Runs one parallel system and stores its panic instead of unwinding through the executor.
fn run_task(entry: &mut ParallelEntry, world: &World) {
    let ParallelEntry {
        system,
        commands,
        panic,
        ..
    } = entry;
    let result = catch_unwind(AssertUnwindSafe(|| {
        system.run(world, commands);
    }));
    if let Err(payload) = result {
        *panic = Some(payload);
    }
}
