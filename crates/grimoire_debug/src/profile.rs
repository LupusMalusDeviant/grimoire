//! Profiler data model (contract §13 "Profiler-Datenmodell", plan 0002 WP6.3).
//!
//! Everything here works on durations the caller measured: this module never reads a clock
//! (contract §13). The facade measures with the platform clock and drives [`FrameProfile`]
//! (contract §9.7); a game that runs its own loop can do the same with [`ScopeRegistry`] and
//! [`ScopeTimer`].

use std::time::Duration;

use crate::generated::debug_protocol::{Stats, StatsCounter, StatsScope};

/// Most scopes and most counters one `Stats` message carries (contract §13: `Vec≤64`).
const MAX_STATS_ENTRIES: usize = 64;

/// Longest scope or counter name one `Stats` message carries, in bytes (contract §13: `Str≤64`).
const MAX_STATS_NAME_BYTES: usize = 64;

/// Identifies one profiler scope. The caller assigns ids (the facade: contract §9.7), for example
/// through a [`ScopeRegistry`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScopeId(pub u16);

/// The totals of one scope in the current frame of a [`FrameProfile`].
///
/// Only [`FrameProfile`] creates these (contract §2 rule 13); the name is available through
/// [`FrameProfile::scope_name`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeTotal {
    /// The scope these totals belong to.
    pub scope: ScopeId,
    /// Sum of every duration recorded for the scope this frame, saturating.
    pub total: Duration,
    /// Number of recordings this frame, saturating.
    pub calls: u32,
    /// Budget of the scope ([`FrameProfile::set_budget`]), if any. Added by WP6.3.
    pub budget: Option<Duration>,
    /// `true` if at least one recording this frame was a fallback value without a real
    /// measurement ([`FrameProfile::record_estimate`]), for example GPU time without timestamp
    /// queries. Added by WP6.3.
    pub estimate: bool,
}

impl ScopeTotal {
    /// Whether the scope has a budget and its total exceeds it.
    #[must_use]
    pub fn over_budget(&self) -> bool {
        self.budget.is_some_and(|budget| self.total > budget)
    }
}

/// Frame values of a [`Stats`] message that a [`FrameProfile`] does not hold itself (contract
/// §13). `grimoire_debug` has no edge to the facade or the simulation, so the caller passes them.
///
/// Build it from [`StatsFrame::default`] and field assignment (contract §2 rule 13).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StatsFrame {
    /// Index of the frame, starting at 0.
    pub frame: u64,
    /// Simulation tick count after the frame's ticks.
    pub sim_tick: u64,
    /// Ticks simulated during the frame.
    pub ticks_this_frame: u32,
    /// Interpolation factor of the frame.
    pub alpha: f32,
    /// Real time since the previous frame.
    pub frame_time: Duration,
    /// Frames per second (the facade converts its `f64` with `as f32`).
    pub fps: f32,
    /// Total real time the fixed timestep has discarded.
    pub dropped_time: Duration,
    /// Number of Sigil unit swaps applied so far (contract §11.8).
    pub content_swaps: u32,
    /// Current content-manifest hash (contract §11.8).
    pub content_manifest: u64,
}

/// A scope's name and budget, kept across frames.
#[derive(Clone, Debug)]
struct ScopeEntry {
    scope: ScopeId,
    name: String,
}

/// Per-frame profiler totals: durations per scope and named counters (contract §13).
///
/// - [`FrameProfile::begin`] starts a frame and clears the totals and counters, but keeps the
///   name table and the budgets.
/// - [`FrameProfile::record`] copies the name only when a [`ScopeId`] appears for the first time;
///   for a known id the first name stays ([`FrameProfile::scope_name`]). Once the vectors have
///   grown, recording does not allocate.
/// - [`FrameProfile::scopes`] lists the scopes in the order of their first recording this frame.
///
/// ```
/// use std::time::Duration;
/// use grimoire_debug::{FrameProfile, ScopeId, StatsFrame};
///
/// let mut profile = FrameProfile::default();
/// profile.set_budget(ScopeId(0), Some(Duration::from_millis(4)));
/// profile.begin(7);
/// profile.record(ScopeId(0), "sim", Duration::from_millis(3));
/// profile.record(ScopeId(0), "sim", Duration::from_millis(2));
/// profile.add_counter("bullets_drawn", 1200);
///
/// let sim = profile.scope(ScopeId(0)).unwrap();
/// assert_eq!((sim.total, sim.calls), (Duration::from_millis(5), 2));
/// assert!(sim.over_budget());
///
/// let stats = profile.to_stats(&StatsFrame::default());
/// assert_eq!(stats.scopes[0].budget_ns, 4_000_000);
/// ```
#[derive(Clone, Debug, Default)]
pub struct FrameProfile {
    frame: u64,
    scopes: Vec<ScopeTotal>,
    names: Vec<ScopeEntry>,
    budgets: Vec<(ScopeId, Duration)>,
    counters: Vec<(&'static str, u64)>,
}

impl FrameProfile {
    /// Starts frame `frame`: clears the scope totals and counters of the previous frame. Names and
    /// budgets stay.
    pub fn begin(&mut self, frame: u64) {
        self.frame = frame;
        self.scopes.clear();
        self.counters.clear();
    }

    /// The frame passed to the most recent [`FrameProfile::begin`] (0 before the first).
    #[must_use]
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// Adds a measured `duration` to `scope`. `name` is copied only the first time `scope` ever
    /// appears.
    pub fn record(&mut self, scope: ScopeId, name: &str, duration: Duration) {
        self.record_with(scope, name, duration, false);
    }

    /// Like [`FrameProfile::record`], but marks the scope's total for this frame as an estimate: a
    /// fallback value without a real measurement (contract §13, `estimate`). Added by WP6.3.
    pub fn record_estimate(&mut self, scope: ScopeId, name: &str, duration: Duration) {
        self.record_with(scope, name, duration, true);
    }

    fn record_with(&mut self, scope: ScopeId, name: &str, duration: Duration, estimate: bool) {
        if !self.names.iter().any(|entry| entry.scope == scope) {
            self.names.push(ScopeEntry {
                scope,
                name: name.to_owned(),
            });
        }
        if let Some(total) = self.scopes.iter_mut().find(|total| total.scope == scope) {
            total.total = total.total.saturating_add(duration);
            total.calls = total.calls.saturating_add(1);
            total.estimate |= estimate;
            return;
        }
        let budget = self.budget(scope);
        self.scopes.push(ScopeTotal {
            scope,
            total: duration,
            calls: 1,
            budget,
            estimate,
        });
    }

    /// Sets (`Some`) or removes (`None`) the budget of `scope`. Budgets survive
    /// [`FrameProfile::begin`] and also apply to a total already recorded this frame. Added by
    /// WP6.3.
    pub fn set_budget(&mut self, scope: ScopeId, budget: Option<Duration>) {
        let position = self.budgets.iter().position(|&(id, _)| id == scope);
        match (position, budget) {
            (Some(index), Some(budget)) => self.budgets[index].1 = budget,
            (Some(index), None) => {
                self.budgets.remove(index);
            }
            (None, Some(budget)) => self.budgets.push((scope, budget)),
            (None, None) => {}
        }
        if let Some(total) = self.scopes.iter_mut().find(|total| total.scope == scope) {
            total.budget = budget;
        }
    }

    /// The budget of `scope`, if one is set. Added by WP6.3.
    #[must_use]
    pub fn budget(&self, scope: ScopeId) -> Option<Duration> {
        self.budgets
            .iter()
            .find(|&&(id, _)| id == scope)
            .map(|&(_, budget)| budget)
    }

    /// Adds `value` to the counter `name` for this frame, saturating. A name recorded twice in one
    /// frame keeps one entry at the position of its first recording.
    pub fn add_counter(&mut self, name: &'static str, value: u64) {
        if let Some(counter) = self
            .counters
            .iter_mut()
            .find(|(existing, _)| *existing == name)
        {
            counter.1 = counter.1.saturating_add(value);
            return;
        }
        self.counters.push((name, value));
    }

    /// The scope totals of this frame, in the order of their first recording.
    #[must_use]
    pub fn scopes(&self) -> &[ScopeTotal] {
        &self.scopes
    }

    /// The totals of `scope` this frame, if it was recorded. Added by WP6.3.
    #[must_use]
    pub fn scope(&self, scope: ScopeId) -> Option<&ScopeTotal> {
        self.scopes.iter().find(|total| total.scope == scope)
    }

    /// The name first recorded for `scope`, in this or any earlier frame.
    #[must_use]
    pub fn scope_name(&self, scope: ScopeId) -> Option<&str> {
        self.names
            .iter()
            .find(|entry| entry.scope == scope)
            .map(|entry| entry.name.as_str())
    }

    /// The counters of this frame, in the order of their first recording.
    #[must_use]
    pub fn counters(&self) -> &[(&'static str, u64)] {
        &self.counters
    }

    /// Builds a `Stats` message payload from this frame's totals and `frame`.
    ///
    /// Never fails and truncates deterministically (contract §13): the first 64 scopes and the
    /// first 64 counters in order of first recording, every name cut at a UTF-8 character
    /// boundary to at most 64 bytes. Durations are nanoseconds, `u64::MAX` on overflow; a missing
    /// budget (and a budget of zero) is `budget_ns = 0`.
    #[must_use]
    pub fn to_stats(&self, frame: &StatsFrame) -> Stats {
        let scopes = self
            .scopes
            .iter()
            .take(MAX_STATS_ENTRIES)
            .map(|total| StatsScope {
                scope: total.scope.0,
                name: truncated_name(self.scope_name(total.scope).unwrap_or_default()),
                total_ns: saturating_nanos(total.total),
                calls: total.calls,
                budget_ns: total.budget.map_or(0, saturating_nanos),
                estimate: total.estimate,
            })
            .collect();
        let counters = self
            .counters
            .iter()
            .take(MAX_STATS_ENTRIES)
            .map(|&(name, value)| StatsCounter {
                name: truncated_name(name),
                value,
            })
            .collect();
        Stats {
            frame: frame.frame,
            sim_tick: frame.sim_tick,
            ticks_this_frame: frame.ticks_this_frame,
            alpha: frame.alpha,
            frame_time_ns: saturating_nanos(frame.frame_time),
            fps: frame.fps,
            dropped_time_ns: saturating_nanos(frame.dropped_time),
            content_swaps: frame.content_swaps,
            content_manifest: frame.content_manifest,
            scopes,
            counters,
        }
    }
}

/// Nanoseconds of `duration`, `u64::MAX` if they do not fit.
fn saturating_nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

/// `name` cut at a UTF-8 character boundary to at most [`MAX_STATS_NAME_BYTES`] bytes.
fn truncated_name(name: &str) -> String {
    let mut end = name.len().min(MAX_STATS_NAME_BYTES);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_owned()
}

/// Assigns [`ScopeId`]s to scope names in the order the names are first registered (Scope-API,
/// plan 0002 WP6.3).
///
/// The facade keys its table by subsystem prefix (contract §9.7); a game with its own loop can
/// use the same type for its own scopes.
#[derive(Clone, Debug, Default)]
pub struct ScopeRegistry {
    names: Vec<String>,
}

impl ScopeRegistry {
    /// Creates an empty registry. Identical to [`ScopeRegistry::default`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The id of `name`, registering it with the next free id on first use. `None` once all
    /// 65,536 ids are taken; a name registered before stays reachable. Allocates only on first
    /// use of a name.
    pub fn register(&mut self, name: &str) -> Option<ScopeId> {
        if let Some(scope) = self.get(name) {
            return Some(scope);
        }
        let id = u16::try_from(self.names.len()).ok()?;
        self.names.push(name.to_owned());
        Some(ScopeId(id))
    }

    /// The id of `name`, if it is registered.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<ScopeId> {
        self.names
            .iter()
            .position(|existing| existing == name)
            .and_then(|index| u16::try_from(index).ok())
            .map(ScopeId)
    }

    /// The name registered for `scope`.
    #[must_use]
    pub fn name(&self, scope: ScopeId) -> Option<&str> {
        self.names.get(usize::from(scope.0)).map(String::as_str)
    }

    /// Number of registered names.
    #[must_use]
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether no name is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// A scope measurement in progress (Scope-API, plan 0002 WP6.3): the scope and the caller's clock
/// reading at its start.
///
/// `grimoire_debug` reads no clock (contract §13), so both ends take the reading from the caller:
///
/// ```
/// use std::time::Duration;
/// use grimoire_debug::{FrameProfile, ScopeRegistry, ScopeTimer};
///
/// let mut registry = ScopeRegistry::new();
/// let mut profile = FrameProfile::default();
/// let ai = registry.register("ai").unwrap();
/// profile.begin(0);
/// let timer = ScopeTimer::start(ai, Duration::from_micros(100)); // clock.elapsed()
/// // ... the measured work ...
/// let spent = timer.stop(&mut profile, "ai", Duration::from_micros(350)); // clock.elapsed()
/// assert_eq!(spent, Duration::from_micros(250));
/// assert_eq!(profile.scope(ai).unwrap().total, spent);
/// ```
#[must_use = "a scope timer records nothing until it is stopped"]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeTimer {
    scope: ScopeId,
    started_at: Duration,
}

impl ScopeTimer {
    /// Starts measuring `scope` at the clock reading `now`.
    pub fn start(scope: ScopeId, now: Duration) -> Self {
        Self {
            scope,
            started_at: now,
        }
    }

    /// The scope being measured.
    #[must_use]
    pub fn scope(&self) -> ScopeId {
        self.scope
    }

    /// The clock reading passed to [`ScopeTimer::start`].
    #[must_use]
    pub fn started_at(&self) -> Duration {
        self.started_at
    }

    /// Time from the start to the clock reading `now`; zero if `now` lies before the start.
    #[must_use]
    pub fn elapsed(&self, now: Duration) -> Duration {
        now.saturating_sub(self.started_at)
    }

    /// Records the time from the start to `now` in `profile` under `name` and returns it.
    pub fn stop(self, profile: &mut FrameProfile, name: &str, now: Duration) -> Duration {
        let elapsed = self.elapsed(now);
        profile.record(self.scope, name, elapsed);
        elapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn begin_clears_totals_and_counters_but_keeps_names_and_budgets() {
        let mut profile = FrameProfile::default();
        profile.set_budget(ScopeId(3), Some(MS));
        profile.begin(1);
        profile.record(ScopeId(3), "render", MS);
        profile.add_counter("bullets_drawn", 5);
        profile.begin(2);
        assert_eq!(profile.frame(), 2);
        assert!(profile.scopes().is_empty());
        assert!(profile.counters().is_empty());
        assert_eq!(profile.scope_name(ScopeId(3)), Some("render"));
        assert_eq!(profile.budget(ScopeId(3)), Some(MS));
        profile.record(ScopeId(3), "render", 2 * MS);
        assert_eq!(profile.scopes()[0].budget, Some(MS));
        assert!(profile.scopes()[0].over_budget());
    }

    #[test]
    fn the_first_name_of_a_scope_wins_and_order_follows_first_recording() {
        let mut profile = FrameProfile::default();
        profile.begin(0);
        profile.record(ScopeId(9), "sigil", MS);
        profile.record(ScopeId(2), "sim", MS);
        profile.record(ScopeId(9), "renamed", MS);
        let order: Vec<u16> = profile.scopes().iter().map(|total| total.scope.0).collect();
        assert_eq!(order, [9, 2]);
        assert_eq!(profile.scope_name(ScopeId(9)), Some("sigil"));
        assert_eq!(profile.scope(ScopeId(9)).map(|total| total.calls), Some(2));
        assert_eq!(profile.scope_name(ScopeId(1)), None);
    }

    #[test]
    fn totals_and_calls_saturate() {
        let mut profile = FrameProfile::default();
        profile.begin(0);
        profile.record(ScopeId(0), "frame", Duration::MAX);
        profile.record(ScopeId(0), "frame", Duration::MAX);
        assert_eq!(profile.scopes()[0].total, Duration::MAX);
        let stats = profile.to_stats(&StatsFrame::default());
        assert_eq!(stats.scopes[0].total_ns, u64::MAX);
    }

    #[test]
    fn an_estimate_marks_the_whole_frame_total() {
        let mut profile = FrameProfile::default();
        profile.begin(0);
        profile.record(ScopeId(4), "gpu", MS);
        assert!(!profile.scopes()[0].estimate);
        profile.record_estimate(ScopeId(4), "gpu", MS);
        profile.record(ScopeId(4), "gpu", MS);
        assert!(profile.scopes()[0].estimate);
        profile.begin(1);
        profile.record(ScopeId(4), "gpu", MS);
        assert!(!profile.scopes()[0].estimate, "estimate is per frame");
    }

    #[test]
    fn set_budget_updates_removes_and_reaches_the_current_frame() {
        let mut profile = FrameProfile::default();
        profile.begin(0);
        profile.record(ScopeId(1), "sim", 3 * MS);
        assert_eq!(profile.scopes()[0].budget, None);
        profile.set_budget(ScopeId(1), Some(4 * MS));
        assert_eq!(profile.scopes()[0].budget, Some(4 * MS));
        profile.set_budget(ScopeId(1), Some(2 * MS));
        assert!(profile.scopes()[0].over_budget());
        profile.set_budget(ScopeId(1), None);
        assert_eq!(profile.budget(ScopeId(1)), None);
        assert!(!profile.scopes()[0].over_budget());
        profile.set_budget(ScopeId(8), None);
        assert_eq!(profile.budget(ScopeId(8)), None);
    }

    #[test]
    fn counters_with_the_same_name_merge_at_their_first_position() {
        let mut profile = FrameProfile::default();
        profile.begin(0);
        profile.add_counter("a", 1);
        profile.add_counter("b", 2);
        profile.add_counter("a", u64::MAX);
        assert_eq!(profile.counters(), [("a", u64::MAX), ("b", 2)]);
    }

    #[test]
    fn to_stats_carries_frame_values_budgets_and_estimates() {
        let mut profile = FrameProfile::default();
        profile.set_budget(ScopeId(0), Some(Duration::from_nanos(16_666_667)));
        profile.set_budget(ScopeId(1), Some(Duration::ZERO));
        profile.begin(12);
        profile.record(ScopeId(0), "frame", Duration::from_nanos(1_500));
        profile.record_estimate(ScopeId(1), "gpu", Duration::from_nanos(700));
        profile.add_counter("bullets_drawn", 42);
        let frame = StatsFrame {
            frame: 12,
            sim_tick: 99,
            ticks_this_frame: 2,
            alpha: 0.25,
            frame_time: Duration::from_nanos(16_000_000),
            fps: 60.0,
            dropped_time: Duration::MAX,
            content_swaps: 3,
            content_manifest: 0xDEAD_BEEF,
        };
        let stats = profile.to_stats(&frame);
        assert_eq!(
            (stats.frame, stats.sim_tick, stats.ticks_this_frame),
            (12, 99, 2)
        );
        assert_eq!((stats.alpha, stats.fps), (0.25, 60.0));
        assert_eq!(
            (stats.frame_time_ns, stats.dropped_time_ns),
            (16_000_000, u64::MAX)
        );
        assert_eq!(
            (stats.content_swaps, stats.content_manifest),
            (3, 0xDEAD_BEEF)
        );
        assert_eq!(stats.scopes.len(), 2);
        assert_eq!(
            (
                stats.scopes[0].name.as_str(),
                stats.scopes[0].total_ns,
                stats.scopes[0].budget_ns,
                stats.scopes[0].estimate
            ),
            ("frame", 1_500, 16_666_667, false)
        );
        assert_eq!(
            (stats.scopes[1].budget_ns, stats.scopes[1].estimate),
            (0, true),
            "a zero budget travels as `none`"
        );
        assert_eq!(stats.counters[0].name, "bullets_drawn");
        assert_eq!(stats.counters[0].value, 42);
    }

    #[test]
    fn to_stats_truncates_to_64_entries_and_64_byte_names_and_round_trips() {
        // Contract §13 "Weitere Prüfungen": 65 scopes, 65 counters and a 65-byte name with a
        // multi-byte character at the boundary.
        // Counter names are `&'static str`; leaking 65 short strings once is fine in a test.
        let counter_names: Vec<&'static str> = (0..65)
            .map(|index| &*Box::leak(format!("c{index:02}").into_boxed_str()))
            .collect();
        // 63 ASCII bytes, then a 2-byte character spanning bytes 63 and 64: 65 bytes in total.
        let long_name = format!("{}é", "x".repeat(63));
        assert_eq!(long_name.len(), 65);

        let mut profile = FrameProfile::default();
        profile.begin(0);
        profile.record(ScopeId(0), &long_name, MS);
        for id in 1..65u16 {
            profile.record(ScopeId(id), &format!("scope{id}"), MS);
        }
        for name in counter_names {
            profile.add_counter(name, 1);
        }
        assert_eq!(profile.scopes().len(), 65);
        assert_eq!(profile.counters().len(), 65);

        let stats = profile.to_stats(&StatsFrame::default());
        assert_eq!(stats.scopes.len(), 64);
        assert_eq!(stats.counters.len(), 64);
        assert_eq!(stats.scopes[0].name, "x".repeat(63));
        assert_eq!(stats.scopes[63].name, "scope63");
        assert_eq!(stats.counters[63].name, "c63");

        let frame = Message::Stats(stats.clone())
            .to_frame(1)
            .expect("to_stats never exceeds the catalogue limits");
        assert_eq!(Message::from_frame(&frame), Ok(Message::Stats(stats)));
    }

    #[test]
    fn registry_assigns_ids_in_first_registration_order() {
        let mut registry = ScopeRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.register("frame"), Some(ScopeId(0)));
        assert_eq!(registry.register("sim"), Some(ScopeId(1)));
        assert_eq!(registry.register("frame"), Some(ScopeId(0)));
        assert_eq!(registry.get("sim"), Some(ScopeId(1)));
        assert_eq!(registry.get("sigil"), None);
        assert_eq!(registry.name(ScopeId(1)), Some("sim"));
        assert_eq!(registry.name(ScopeId(2)), None);
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn registry_refuses_names_beyond_the_id_space_but_keeps_known_ones() {
        let mut registry = ScopeRegistry::new();
        for index in 0..=u32::from(u16::MAX) {
            assert!(registry.register(&index.to_string()).is_some());
        }
        assert_eq!(registry.register("one too many"), None);
        assert_eq!(registry.register("0"), Some(ScopeId(0)));
        assert_eq!(registry.register("65535"), Some(ScopeId(u16::MAX)));
    }

    #[test]
    fn scope_timer_records_the_elapsed_time_and_never_goes_negative() {
        let mut profile = FrameProfile::default();
        profile.begin(0);
        let timer = ScopeTimer::start(ScopeId(5), 10 * MS);
        assert_eq!((timer.scope(), timer.started_at()), (ScopeId(5), 10 * MS));
        assert_eq!(timer.elapsed(9 * MS), Duration::ZERO);
        assert_eq!(timer.stop(&mut profile, "ai", 13 * MS), 3 * MS);
        let early = ScopeTimer::start(ScopeId(5), 10 * MS);
        assert_eq!(early.stop(&mut profile, "ai", MS), Duration::ZERO);
        let total = profile.scope(ScopeId(5)).copied().expect("recorded");
        assert_eq!((total.total, total.calls), (3 * MS, 2));
    }
}
