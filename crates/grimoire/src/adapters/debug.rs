//! Debug ↔ Sim/Render adapter: the facade's profiler (contract §9.1, §9.7, plan 0002 WP6.3,
//! PRD-0002 FR-12).
//!
//! `grimoire_debug` holds the clock-free data model ([`FrameProfile`], [`ScopeRegistry`]); this
//! module adds what needs the other crates: clock access through [`Clock`], the schedule observer
//! ([`ProfilerObserver`], contract §7.2), the subsystem scopes from system names, budgets per
//! subsystem ([`ProfilerBudgets`]) and the renderer's GPU time and bullet counters
//! ([`Profiler::record_stage_stats`]).
//!
//! The main loop of [`crate::AppBuilder::run`] uses a [`Profiler`] on every frame unless
//! [`crate::AppBuilder::profiler`] turned it off, and hands the result to
//! [`crate::GamePlugin::on_profile`]. A game with its own loop drives the same type:
//!
//! ```
//! use grimoire::adapters::debug::{Profiler, SCOPE_FRAME, SCOPE_SIM};
//! use grimoire::platform::{Clock, ManualClock};
//! use grimoire::prelude::*;
//! use std::time::Duration;
//!
//! let clock = ManualClock::new();
//! let mut sim = Simulation::new(1);
//! sim.schedule_mut().add_system(system_fn("sigil.update", |_world| {}));
//! let mut profiler = Profiler::default();
//!
//! profiler.begin_frame(0);
//! let started = clock.elapsed();
//! sim.step_observed(TickInput::default(), &mut profiler.observer(&clock));
//! profiler.record(SCOPE_SIM, clock.elapsed() - started);
//! profiler.record(SCOPE_FRAME, clock.elapsed() - started);
//!
//! let names: Vec<_> = profiler
//!     .profile()
//!     .scopes()
//!     .iter()
//!     .filter_map(|total| profiler.profile().scope_name(total.scope))
//!     .collect();
//! assert_eq!(names, ["sigil", "sim", "frame"]);
//! assert_eq!(profiler.budget("sigil"), Some(Duration::from_millis(1)));
//! ```

use std::time::Duration;

use grimoire_debug::{FrameProfile, ScopeId, ScopeRegistry, StatsFrame};
use grimoire_ecs::{StageInfo, SystemInfo, SystemObserver, World};
use grimoire_platform::Clock;
use grimoire_render::StageStats;
use grimoire_sigil::ContentEpoch;

use crate::plugin::FrameStats;

/// Whole frame: the loop's work from the frame's clock reading to the end of
/// [`crate::GamePlugin::on_frame`].
pub const SCOPE_FRAME: &str = "frame";
/// Sum of every `Simulation::step` of the frame.
pub const SCOPE_SIM: &str = "sim";
/// Render extraction: every plugin's `extract` and `extract_stage`.
pub const SCOPE_EXTRACT: &str = "extract";
/// Render CPU: the `render_stage` call, including submission and presentation.
pub const SCOPE_RENDER: &str = "render";
/// GPU time of the renderer's passes ([`StageStats::gpu_time`]), or a marked estimate without
/// timestamp queries ([`Profiler::record_stage_stats`]).
pub const SCOPE_GPU: &str = "gpu";
/// Systems whose name has no subsystem prefix ([`subsystem_scope`]).
pub const SCOPE_APP: &str = "app";

/// The scopes the main loop records itself, registered in this order by [`Profiler::new`] so they
/// keep the same [`ScopeId`]s (0 to 4) in every run. Subsystem scopes follow in the order they
/// first appear.
pub const LOOP_SCOPES: [&str; 5] = [
    SCOPE_FRAME,
    SCOPE_SIM,
    SCOPE_EXTRACT,
    SCOPE_RENDER,
    SCOPE_GPU,
];

/// Frame budget at 60 frames per second, rounded to whole nanoseconds.
const FRAME_BUDGET: Duration = Duration::from_nanos(16_666_667);

/// Budgets per subsystem from PRD-0002 (performance budgets, desktop reference at 60 FPS) and
/// PRD-0004 (10,000 bullets): the defaults of [`ProfilerBudgets::default`].
///
/// | Scope | Budget | Source |
/// |-------|--------|--------|
/// | `frame` | 16.67 ms | 60 FPS |
/// | `sim` | 4 ms | PRD-0002 "Sim gesamt" |
/// | `sigil` | 1 ms | PRD-0004 NFR, simulation share of 10k bullets |
/// | `collide` | 1.5 ms | PRD-0002 and PRD-0004, collision |
/// | `extract` | 0.5 ms | PRD-0004 NFR, render extraction |
/// | `render` | 3 ms | PRD-0002 "Render-CPU" |
/// | `gpu` | 8 ms | PRD-0002 "GPU-Frame" |
/// | `audio` | 1 ms | PRD-0002 "Audio-Mix" |
pub const DEFAULT_BUDGETS: [(&str, Duration); 8] = [
    (SCOPE_FRAME, FRAME_BUDGET),
    (SCOPE_SIM, Duration::from_millis(4)),
    ("sigil", Duration::from_millis(1)),
    ("collide", Duration::from_micros(1_500)),
    (SCOPE_EXTRACT, Duration::from_micros(500)),
    (SCOPE_RENDER, Duration::from_millis(3)),
    (SCOPE_GPU, Duration::from_millis(8)),
    ("audio", Duration::from_millis(1)),
];

/// The profiler scope of a system: the part of its name before the first `.` (contract §9.1,
/// §9.7: `sigil.update` → `sigil`).
///
/// A name without a dot, with an empty prefix, or whose prefix is one of the [`LOOP_SCOPES`]
/// counts to [`SCOPE_APP`], so a system can never inflate a scope the loop measures itself.
///
/// ```
/// use grimoire::adapters::debug::subsystem_scope;
///
/// assert_eq!(subsystem_scope("collide.broadphase"), "collide");
/// assert_eq!(subsystem_scope("walk"), "app");
/// assert_eq!(subsystem_scope("sim.helper"), "app");
/// ```
#[must_use]
pub fn subsystem_scope(system_name: &str) -> &str {
    match system_name.split_once('.') {
        Some((prefix, _)) if !prefix.is_empty() && !LOOP_SCOPES.contains(&prefix) => prefix,
        _ => SCOPE_APP,
    }
}

/// Budgets per scope name. [`ProfilerBudgets::default`] holds [`DEFAULT_BUDGETS`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfilerBudgets {
    entries: Vec<(String, Duration)>,
}

impl Default for ProfilerBudgets {
    fn default() -> Self {
        let mut budgets = Self::none();
        for (scope, budget) in DEFAULT_BUDGETS {
            budgets.set(scope, Some(budget));
        }
        budgets
    }
}

impl ProfilerBudgets {
    /// No budget for any scope.
    #[must_use]
    pub fn none() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Sets (`Some`) or removes (`None`) the budget of `scope`.
    pub fn set(&mut self, scope: &str, budget: Option<Duration>) -> &mut Self {
        let position = self.entries.iter().position(|(name, _)| name == scope);
        match (position, budget) {
            (Some(index), Some(budget)) => self.entries[index].1 = budget,
            (Some(index), None) => {
                self.entries.remove(index);
            }
            (None, Some(budget)) => self.entries.push((scope.to_owned(), budget)),
            (None, None) => {}
        }
        self
    }

    /// [`ProfilerBudgets::set`] for builder chains.
    #[must_use]
    pub fn with(mut self, scope: &str, budget: Option<Duration>) -> Self {
        self.set(scope, budget);
        self
    }

    /// The budget of `scope`, if any.
    #[must_use]
    pub fn get(&self, scope: &str) -> Option<Duration> {
        self.entries
            .iter()
            .find(|(name, _)| name == scope)
            .map(|&(_, budget)| budget)
    }

    /// Every budget as `(scope, budget)`, in the order they were first set.
    pub fn iter(&self) -> impl Iterator<Item = (&str, Duration)> {
        self.entries
            .iter()
            .map(|(name, budget)| (name.as_str(), *budget))
    }
}

/// The facade's profiler: a [`FrameProfile`] with scope ids keyed by name, budgets per scope and
/// clock access through a borrowed [`Clock`] (contract §9.7).
///
/// Durations come only from the clock the caller passes, so a run with a manual clock records
/// deterministic values. Nothing here reaches the simulation: [`ProfilerObserver`] reads, it never
/// writes, and no state hash changes (contract §7.2, §8.4).
#[derive(Clone, Debug)]
pub struct Profiler {
    registry: ScopeRegistry,
    profile: FrameProfile,
    budgets: ProfilerBudgets,
    /// Scratch list of the scopes (and their system counts) of the parallel stage being observed.
    stage_scopes: Vec<(ScopeId, u32)>,
}

impl Default for Profiler {
    fn default() -> Self {
        Self::new(ProfilerBudgets::default())
    }
}

impl Profiler {
    /// Creates a profiler with `budgets` and registers the [`LOOP_SCOPES`] in their fixed order.
    #[must_use]
    pub fn new(budgets: ProfilerBudgets) -> Self {
        let mut profiler = Self {
            registry: ScopeRegistry::new(),
            profile: FrameProfile::default(),
            budgets,
            stage_scopes: Vec::new(),
        };
        for scope in LOOP_SCOPES {
            let _ = profiler.scope_id(scope);
        }
        profiler
    }

    /// Starts frame `frame`: clears the previous frame's totals and counters.
    pub fn begin_frame(&mut self, frame: u64) {
        self.profile.begin(frame);
    }

    /// The id of the scope `name`, registering it (with its budget) on first use. `None` only once
    /// 65,536 scope names are registered; recording under a further name is then skipped.
    pub fn scope_id(&mut self, name: &str) -> Option<ScopeId> {
        if let Some(scope) = self.registry.get(name) {
            return Some(scope);
        }
        let scope = self.registry.register(name)?;
        if let Some(budget) = self.budgets.get(name) {
            self.profile.set_budget(scope, Some(budget));
        }
        Some(scope)
    }

    /// Adds a measured `duration` to the scope `name`.
    pub fn record(&mut self, name: &str, duration: Duration) {
        if let Some(scope) = self.scope_id(name) {
            self.profile.record(scope, name, duration);
        }
    }

    /// Adds a fallback `duration` without a real measurement to the scope `name`; the scope's
    /// total for this frame is marked as an estimate.
    pub fn record_estimate(&mut self, name: &str, duration: Duration) {
        if let Some(scope) = self.scope_id(name) {
            self.profile.record_estimate(scope, name, duration);
        }
    }

    /// Runs `work` and records the time it took on `clock` under `name`.
    pub fn measure<T>(&mut self, clock: &dyn Clock, name: &str, work: impl FnOnce() -> T) -> T {
        let started = clock.elapsed();
        let result = work();
        self.record(name, clock.elapsed().saturating_sub(started));
        result
    }

    /// A schedule observer that records every system's time under its [`subsystem_scope`], for
    /// `Simulation::step_observed` (see [`ProfilerObserver`]).
    pub fn observer<'a>(&'a mut self, clock: &'a dyn Clock) -> ProfilerObserver<'a> {
        ProfilerObserver {
            profiler: self,
            clock,
            mark: Duration::ZERO,
            task_phase: Duration::ZERO,
        }
    }

    /// Adds `value` to the counter `name` for this frame.
    pub fn add_counter(&mut self, name: &'static str, value: u64) {
        self.profile.add_counter(name, value);
    }

    /// Records what a `render_stage` call reports, after the loop measured it as
    /// [`SCOPE_RENDER`] taking `render_time` (contract §9.7):
    ///
    /// - [`SCOPE_GPU`]: [`StageStats::gpu_time`] when the renderer measured it with timestamp
    ///   queries. Otherwise `render_time`, marked as an estimate: without timestamp queries the
    ///   only visible GPU cost is the time the CPU spent submitting and presenting, which includes
    ///   waits for the GPU but also presentation (for example vertical sync).
    /// - The counters `bullets_drawn`, `bullets_rejected_palette_space` and
    ///   `bullets_rejected_invalid`.
    pub fn record_stage_stats(&mut self, stats: &StageStats, render_time: Duration) {
        match stats.gpu_time {
            Some(gpu_time) => self.record(SCOPE_GPU, gpu_time),
            None => self.record_estimate(SCOPE_GPU, render_time),
        }
        self.add_counter("bullets_drawn", u64::from(stats.bullets_drawn));
        self.add_counter(
            "bullets_rejected_palette_space",
            u64::from(stats.bullets_rejected_palette_space),
        );
        self.add_counter(
            "bullets_rejected_invalid",
            u64::from(stats.bullets_rejected_invalid),
        );
    }

    /// Sets (`Some`) or removes (`None`) the budget of the scope `name`, also for a scope already
    /// registered.
    pub fn set_budget(&mut self, name: &str, budget: Option<Duration>) {
        self.budgets.set(name, budget);
        if let Some(scope) = self.registry.get(name) {
            self.profile.set_budget(scope, budget);
        }
    }

    /// The budget of the scope `name`, if any.
    #[must_use]
    pub fn budget(&self, name: &str) -> Option<Duration> {
        self.budgets.get(name)
    }

    /// The totals of the current frame.
    #[must_use]
    pub fn profile(&self) -> &FrameProfile {
        &self.profile
    }

    /// The scope names and their ids.
    #[must_use]
    pub fn registry(&self) -> &ScopeRegistry {
        &self.registry
    }

    /// Adds `duration` to `scope` under its registered name.
    fn record_id(&mut self, scope: ScopeId, duration: Duration, estimate: bool) {
        let Some(name) = self.registry.name(scope) else {
            return;
        };
        if estimate {
            self.profile.record_estimate(scope, name, duration);
        } else {
            self.profile.record(scope, name, duration);
        }
    }
}

/// Schedule observer of a [`Profiler`] (contract §7.2, §9.7): records every exclusive system's run
/// and, for a parallel stage, its task phase and each buffer application under the system's
/// [`subsystem_scope`].
///
/// The time of a single parallel system is not observable (contract §7.2). A parallel stage's task
/// phase is recorded under its scope when all systems of the stage share one; otherwise it is
/// split over the stage's scopes in proportion to their number of systems and recorded as an
/// estimate.
///
/// It only reads the clock and the system names: the world is never touched, and state hashes are
/// the same with and without it.
pub struct ProfilerObserver<'a> {
    profiler: &'a mut Profiler,
    clock: &'a dyn Clock,
    /// Clock reading at the last event.
    mark: Duration,
    /// Task phase of the parallel stage being observed.
    task_phase: Duration,
}

impl std::fmt::Debug for ProfilerObserver<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProfilerObserver")
            .field("mark", &self.mark)
            .field("task_phase", &self.task_phase)
            .finish_non_exhaustive()
    }
}

impl ProfilerObserver<'_> {
    /// Time since the last event; moves the mark to now.
    fn lap(&mut self) -> Duration {
        let now = self.clock.elapsed();
        let spent = now.saturating_sub(self.mark);
        self.mark = now;
        spent
    }
}

impl SystemObserver for ProfilerObserver<'_> {
    fn stage_started(&mut self, stage: StageInfo) {
        self.mark = self.clock.elapsed();
        if !stage.exclusive {
            self.profiler.stage_scopes.clear();
            self.task_phase = Duration::ZERO;
        }
    }

    fn system_started(&mut self, _system: SystemInfo<'_>) {
        self.mark = self.clock.elapsed();
    }

    fn tasks_finished(&mut self, _stage: StageInfo) {
        self.task_phase = self.lap();
    }

    fn system_finished(&mut self, system: SystemInfo<'_>, _world: &World) {
        // Exclusive: the system's run. Parallel: the application of its command buffer.
        let spent = self.lap();
        let Some(scope) = self.profiler.scope_id(subsystem_scope(system.name)) else {
            return;
        };
        self.profiler.record_id(scope, spent, false);
        if system.parallel {
            match self
                .profiler
                .stage_scopes
                .iter_mut()
                .find(|(existing, _)| *existing == scope)
            {
                Some((_, systems)) => *systems += 1,
                None => self.profiler.stage_scopes.push((scope, 1)),
            }
        }
    }

    fn stage_finished(&mut self, stage: StageInfo, _world: &World) {
        if stage.exclusive {
            return;
        }
        let task_phase = self.task_phase;
        let stage_scopes = std::mem::take(&mut self.profiler.stage_scopes);
        match stage_scopes.as_slice() {
            [] => {}
            [(scope, _)] => self.profiler.record_id(*scope, task_phase, false),
            shared => {
                let systems: u32 = shared.iter().map(|&(_, count)| count).sum();
                let nanos = task_phase.as_nanos();
                let mut assigned = 0u128;
                for (index, &(scope, count)) in shared.iter().enumerate() {
                    let mut share = nanos * u128::from(count) / u128::from(systems.max(1));
                    if index + 1 == shared.len() {
                        // The rounding remainder goes to the last scope, so the shares add up.
                        share = nanos - assigned;
                    }
                    assigned += share;
                    let share = Duration::from_nanos(u64::try_from(share).unwrap_or(u64::MAX));
                    self.profiler.record_id(scope, share, true);
                }
            }
        }
        // Hand the (cleared) allocation back for the next parallel stage.
        let mut stage_scopes = stage_scopes;
        stage_scopes.clear();
        self.profiler.stage_scopes = stage_scopes;
    }
}

/// The frame values of a debug-protocol `Stats` message or a profiler export (contract §13) for
/// `stats` and the content epoch `content` (zero without Sigil content). `fps` is converted with
/// `as f32` (contract §13).
///
/// A plugin reads the epoch in `extract` with
/// `world.resource::<SigilContent>().map(SigilContent::epoch)` and keeps it for
/// [`crate::GamePlugin::on_profile`], which sees no world.
#[must_use]
pub fn stats_frame(stats: &FrameStats, content: Option<ContentEpoch>) -> StatsFrame {
    let mut frame = StatsFrame::default();
    frame.frame = stats.frame;
    frame.sim_tick = stats.sim_tick;
    frame.ticks_this_frame = stats.ticks_this_frame;
    frame.alpha = stats.alpha;
    frame.frame_time = stats.frame_time;
    frame.fps = stats.fps as f32;
    frame.dropped_time = stats.dropped_time;
    if let Some(epoch) = content {
        frame.content_swaps = epoch.swaps;
        frame.content_manifest = epoch.manifest_hash.0;
    }
    frame
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use grimoire_ecs::{Access, CommandBuffer, Schedule, parallel_system_fn, system_fn};

    use super::*;

    /// Advances by one microsecond on every reading, so every observed interval is exactly 1 µs.
    #[derive(Default)]
    struct SteppingClock {
        micros: AtomicU64,
    }

    impl Clock for SteppingClock {
        fn elapsed(&self) -> Duration {
            Duration::from_micros(self.micros.fetch_add(1, Ordering::Relaxed) + 1)
        }
    }

    const MICRO: Duration = Duration::from_micros(1);

    fn names(profiler: &Profiler) -> Vec<(String, Duration, u32, bool)> {
        profiler
            .profile()
            .scopes()
            .iter()
            .map(|total| {
                (
                    profiler
                        .profile()
                        .scope_name(total.scope)
                        .unwrap_or_default()
                        .to_owned(),
                    total.total,
                    total.calls,
                    total.estimate,
                )
            })
            .collect()
    }

    #[test]
    fn subsystem_scopes_follow_the_prefix_and_protect_loop_scopes() {
        assert_eq!(subsystem_scope("sigil.update"), "sigil");
        assert_eq!(subsystem_scope("collide.broadphase.graze"), "collide");
        assert_eq!(subsystem_scope("walk"), SCOPE_APP);
        assert_eq!(subsystem_scope(".hidden"), SCOPE_APP);
        for scope in LOOP_SCOPES {
            assert_eq!(subsystem_scope(&format!("{scope}.x")), SCOPE_APP);
        }
    }

    #[test]
    fn loop_scopes_get_fixed_ids_and_default_budgets() {
        let profiler = Profiler::default();
        for (index, scope) in LOOP_SCOPES.into_iter().enumerate() {
            assert_eq!(
                profiler.registry().get(scope),
                Some(ScopeId(u16::try_from(index).unwrap_or(u16::MAX)))
            );
        }
        assert_eq!(profiler.budget(SCOPE_GPU), Some(Duration::from_millis(8)));
        assert_eq!(
            profiler.budget("collide"),
            Some(Duration::from_micros(1_500))
        );
        assert_eq!(profiler.budget("unknown"), None);
        assert!(profiler.profile().scopes().is_empty());
    }

    #[test]
    fn budgets_can_be_replaced_removed_and_changed_later() {
        let budgets = ProfilerBudgets::default()
            .with(SCOPE_SIM, Some(Duration::from_millis(2)))
            .with(SCOPE_GPU, None)
            .with("ai", Some(Duration::from_micros(250)));
        assert_eq!(budgets.iter().count(), DEFAULT_BUDGETS.len());
        let mut profiler = Profiler::new(budgets);
        profiler.begin_frame(0);
        profiler.record(SCOPE_SIM, Duration::from_millis(3));
        profiler.record(SCOPE_GPU, Duration::from_millis(30));
        profiler.record("ai", Duration::from_micros(100));
        let sim = profiler.profile().scopes()[0];
        assert_eq!(sim.budget, Some(Duration::from_millis(2)));
        assert!(sim.over_budget());
        assert!(
            !profiler.profile().scopes()[1].over_budget(),
            "no GPU budget"
        );
        profiler.set_budget("ai", Some(Duration::from_micros(50)));
        assert!(profiler.profile().scopes()[2].over_budget());
        assert!(ProfilerBudgets::none().get(SCOPE_FRAME).is_none());
    }

    #[test]
    fn exclusive_systems_are_recorded_under_their_subsystem() {
        let clock = SteppingClock::default();
        let mut profiler = Profiler::default();
        let mut schedule = Schedule::new();
        schedule
            .add_system(system_fn("sigil.update", |_| {}))
            .add_system(system_fn("walk", |_| {}))
            .add_system(system_fn("sigil.clear", |_| {}));
        let mut world = World::new();
        profiler.begin_frame(0);
        schedule.run_observed(&mut world, &mut profiler.observer(&clock));
        assert_eq!(
            names(&profiler),
            [
                ("sigil".to_owned(), 2 * MICRO, 2, false),
                ("app".to_owned(), MICRO, 1, false)
            ]
        );
        assert_eq!(
            profiler.profile().scopes()[0].budget,
            Some(Duration::from_millis(1))
        );
    }

    #[test]
    fn a_parallel_stage_of_one_subsystem_records_task_phase_and_buffers_as_measured() {
        let clock = SteppingClock::default();
        let mut profiler = Profiler::default();
        let mut schedule = Schedule::new();
        let noop = |_: &World, _: &mut CommandBuffer| {};
        schedule
            .add_parallel_system(parallel_system_fn("collide.a", Access::new(), noop))
            .add_parallel_system(parallel_system_fn("collide.b", Access::new(), noop));
        assert_eq!(schedule.stages().len(), 1, "both systems share one stage");
        let mut world = World::new();
        profiler.begin_frame(0);
        schedule.run_observed(&mut world, &mut profiler.observer(&clock));
        // One recording per buffer application plus the task phase, all measured.
        assert_eq!(
            names(&profiler),
            [("collide".to_owned(), 3 * MICRO, 3, false)]
        );
    }

    /// Like [`SteppingClock`], but from the second reading on (the end of a parallel stage's task
    /// phase, when that stage runs first) everything is 299 µs later: a 300 µs task phase.
    #[derive(Default)]
    struct JumpyClock {
        readings: AtomicU64,
    }

    impl Clock for JumpyClock {
        fn elapsed(&self) -> Duration {
            let reading = self.readings.fetch_add(1, Ordering::Relaxed);
            let jump = if reading >= 1 { 299 } else { 0 };
            Duration::from_micros(reading + jump)
        }
    }

    #[test]
    fn a_mixed_parallel_stage_splits_its_task_phase_as_an_estimate() {
        let clock = JumpyClock::default();
        let mut profiler = Profiler::default();
        let mut schedule = Schedule::new();
        let noop = |_: &World, _: &mut CommandBuffer| {};
        schedule
            .add_parallel_system(parallel_system_fn("sigil.a", Access::new(), noop))
            .add_parallel_system(parallel_system_fn("collide.b", Access::new(), noop))
            .add_parallel_system(parallel_system_fn("sigil.c", Access::new(), noop))
            .add_system(system_fn("sigil.after", |_| {}));
        let mut world = World::new();
        profiler.begin_frame(0);
        schedule.run_observed(&mut world, &mut profiler.observer(&clock));
        // Task phase 300 µs: sigil has two of the three systems (200 µs), collide one (the
        // remainder, 100 µs); plus 1 µs per buffer application and 1 µs for `sigil.after`.
        assert_eq!(
            names(&profiler),
            [
                ("sigil".to_owned(), Duration::from_micros(203), 4, true),
                ("collide".to_owned(), Duration::from_micros(101), 2, true),
            ]
        );
    }

    #[test]
    fn the_profiler_observer_passes_the_system_observer_conformance_suite() {
        let clock = SteppingClock::default();
        let mut profiler = Profiler::default();
        grimoire_ecs::conformance::system_observer(&mut profiler.observer(&clock));
    }

    #[test]
    fn record_stage_stats_prefers_measured_gpu_time_and_marks_the_fallback() {
        let mut profiler = Profiler::default();
        let mut stats = StageStats::default();
        stats.bullets_drawn = 7;
        stats.bullets_rejected_invalid = 2;
        profiler.begin_frame(0);
        profiler.record_stage_stats(&stats, Duration::from_millis(2));
        let gpu = profiler.profile().scopes()[0];
        assert_eq!(
            (gpu.total, gpu.estimate),
            (Duration::from_millis(2), true),
            "without timestamp queries the render time stands in, marked"
        );
        assert_eq!(
            profiler.profile().counters(),
            [
                ("bullets_drawn", 7),
                ("bullets_rejected_palette_space", 0),
                ("bullets_rejected_invalid", 2)
            ]
        );
        profiler.begin_frame(1);
        stats.gpu_time = Some(Duration::from_micros(4_200));
        profiler.record_stage_stats(&stats, Duration::from_millis(2));
        let gpu = profiler.profile().scopes()[0];
        assert_eq!(
            (gpu.total, gpu.estimate),
            (Duration::from_micros(4_200), false)
        );
    }

    #[test]
    fn measure_records_the_work_and_returns_its_value() {
        let clock = SteppingClock::default();
        let mut profiler = Profiler::default();
        profiler.begin_frame(0);
        let value = profiler.measure(&clock, "ai", || 42);
        assert_eq!(value, 42);
        assert_eq!(names(&profiler), [("ai".to_owned(), MICRO, 1, false)]);
    }
}
