//! Simulation time: the [`Tick`] and [`SimSeed`] resources and the [`FixedTimestep`] accumulator.

use std::time::Duration;

use grimoire_core::{StableHash, StableHasher};

/// Resource: index of the tick currently being simulated.
///
/// During the first [`crate::Simulation::step`] it is `Tick(0)`; after `n` completed steps it is
/// `Tick(n)`. The simulation owns this value and overwrites it on every step, so systems read it
/// but never write it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tick(pub u64);

impl StableHash for Tick {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_u64(self.0);
    }
}

/// Resource: seed of the running simulation, the root of every [`crate::derive_rng`] stream.
///
/// Owned by the simulation and overwritten on every step, like [`Tick`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SimSeed(pub u64);

impl StableHash for SimSeed {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_u64(self.0);
    }
}

/// Result of [`FixedTimestep::advance`]: how many ticks to simulate this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StepPlan {
    /// Number of simulation ticks to run now, at most the configured maximum per frame.
    pub ticks: u32,
    /// Fraction of the next tick already elapsed, in `[0, 1)`.
    ///
    /// Rendering interpolates between the last two simulation states with it. It must never
    /// influence simulation state.
    pub alpha: f32,
}

/// One tick expressed in accumulator units (nanoseconds times tick rate).
const UNITS_PER_TICK: u128 = 1_000_000_000;
const NANOS_PER_SECOND: u128 = 1_000_000_000;
const DEFAULT_MAX_TICKS_PER_FRAME: u32 = 8;

/// Fixed-timestep accumulator that converts variable frame times into whole simulation ticks.
///
/// Time is accumulated exactly in integer units of `nanoseconds × tick_rate_hz`, where one tick
/// equals 10⁹ units. Nothing is rounded, so frame times that sum to `t` seconds yield exactly
/// `⌊t × tick_rate_hz⌋` ticks run plus dropped, no matter how they are split: there is no drift.
/// As long as no single frame exceeds `max_ticks_per_frame`, all of them are run.
///
/// When a frame is so long that more than `max_ticks_per_frame` ticks are due, the excess whole
/// ticks are discarded (the simulation slows down instead of spiralling) and counted in
/// [`FixedTimestep::dropped_time`]. The fractional remainder is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedTimestep {
    tick_rate_hz: u32,
    max_ticks_per_frame: u32,
    accumulator: u128,
    dropped_units: u128,
}

impl FixedTimestep {
    /// Creates an accumulator for `tick_rate_hz` ticks per second, at most 8 ticks per frame.
    ///
    /// # Panics
    ///
    /// If `tick_rate_hz` is 0.
    #[must_use]
    pub fn new(tick_rate_hz: u32) -> Self {
        assert!(
            tick_rate_hz > 0,
            "FixedTimestep tick rate must be at least 1 Hz"
        );
        Self {
            tick_rate_hz,
            max_ticks_per_frame: DEFAULT_MAX_TICKS_PER_FRAME,
            accumulator: 0,
            dropped_units: 0,
        }
    }

    /// Sets the maximum number of ticks a single [`FixedTimestep::advance`] may plan.
    ///
    /// # Panics
    ///
    /// If `max_ticks_per_frame` is 0.
    #[must_use]
    pub fn with_max_ticks_per_frame(mut self, max_ticks_per_frame: u32) -> Self {
        assert!(
            max_ticks_per_frame > 0,
            "FixedTimestep must allow at least 1 tick per frame"
        );
        self.max_ticks_per_frame = max_ticks_per_frame;
        self
    }

    /// Ticks per simulated second.
    #[must_use]
    pub fn tick_rate_hz(&self) -> u32 {
        self.tick_rate_hz
    }

    /// Maximum number of ticks planned per frame (default 8).
    #[must_use]
    pub fn max_ticks_per_frame(&self) -> u32 {
        self.max_ticks_per_frame
    }

    /// Length of one tick, truncated to whole nanoseconds (60 Hz: 16 666 666 ns).
    ///
    /// Only for display and budgeting; the accumulator itself never uses this rounded value.
    #[must_use]
    pub fn tick_duration(&self) -> Duration {
        let nanos = NANOS_PER_SECOND / u128::from(self.tick_rate_hz);
        nanos_to_duration(nanos)
    }

    /// Adds the real time `delta` of one frame and returns how many ticks to run now.
    ///
    /// Never panics or overflows, even for `Duration::MAX`.
    pub fn advance(&mut self, delta: Duration) -> StepPlan {
        let added = delta
            .as_nanos()
            .saturating_mul(u128::from(self.tick_rate_hz));
        self.accumulator = self.accumulator.saturating_add(added);
        let due = self.accumulator / UNITS_PER_TICK;
        self.accumulator %= UNITS_PER_TICK;

        let ticks = due.min(u128::from(self.max_ticks_per_frame));
        let dropped = due - ticks;
        self.dropped_units = self
            .dropped_units
            .saturating_add(dropped.saturating_mul(UNITS_PER_TICK));

        StepPlan {
            ticks: u32::try_from(ticks).unwrap_or(self.max_ticks_per_frame),
            alpha: self.alpha(),
        }
    }

    /// Total real time discarded so far because frames exceeded the tick limit.
    ///
    /// Truncated to whole nanoseconds; saturates at the largest representable `Duration`.
    #[must_use]
    pub fn dropped_time(&self) -> Duration {
        nanos_to_duration(self.dropped_units / u128::from(self.tick_rate_hz))
    }

    /// Clears the accumulated fraction and the dropped-time counter; the configuration stays.
    pub fn reset(&mut self) {
        self.accumulator = 0;
        self.dropped_units = 0;
    }

    fn alpha(&self) -> f32 {
        // The remainder is below 10⁹, so both conversions are exact; only the final narrowing
        // can round up to 1.0, which the clamp excludes.
        let fraction = self.accumulator as f64 / UNITS_PER_TICK as f64;
        grimoire_core::math::dmath::min(fraction as f32, 1.0f32.next_down())
    }
}

fn nanos_to_duration(nanos: u128) -> Duration {
    let Ok(seconds) = u64::try_from(nanos / NANOS_PER_SECOND) else {
        return Duration::MAX;
    };
    let subsec = u32::try_from(nanos % NANOS_PER_SECOND).unwrap_or(0);
    Duration::new(seconds, subsec)
}
