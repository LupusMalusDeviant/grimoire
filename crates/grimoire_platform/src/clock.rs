//! Monotonic clocks.
//!
//! Clocks feed the fixed-timestep accumulator of the simulation. Simulation systems never read a
//! clock: simulation time is the tick counter.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Source of monotonic time.
pub trait Clock: Send + Sync {
    /// Time elapsed since the clock was created. Never decreases.
    fn elapsed(&self) -> Duration;
}

/// Monotonic clock backed by the operating system.
#[derive(Debug, Clone, Copy)]
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    /// Starts a clock at zero.
    #[must_use]
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
}

/// Clock that only moves when told to — deterministic time for tests and headless runs.
#[derive(Debug, Default)]
pub struct ManualClock {
    nanos: AtomicU64,
}

impl ManualClock {
    /// Creates a clock at zero.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Advances the clock by `delta`, saturating at `u64::MAX` nanoseconds.
    pub fn advance(&self, delta: Duration) {
        let delta = u64::try_from(delta.as_nanos()).unwrap_or(u64::MAX);
        let mut current = self.nanos.load(Ordering::Relaxed);
        loop {
            let next = current.saturating_add(delta);
            match self.nanos.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }
}

impl Clock for ManualClock {
    fn elapsed(&self) -> Duration {
        Duration::from_nanos(self.nanos.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_starts_at_zero_and_advances() {
        let clock = ManualClock::new();
        assert_eq!(clock.elapsed(), Duration::ZERO);
        clock.advance(Duration::from_millis(16));
        clock.advance(Duration::from_millis(4));
        assert_eq!(clock.elapsed(), Duration::from_millis(20));
    }

    #[test]
    fn manual_clock_saturates() {
        let clock = ManualClock::new();
        clock.advance(Duration::MAX);
        clock.advance(Duration::from_secs(1));
        assert_eq!(clock.elapsed(), Duration::from_nanos(u64::MAX));
    }

    #[test]
    fn system_clock_is_monotonic() {
        let clock = SystemClock::new();
        let first = clock.elapsed();
        let second = clock.elapsed();
        assert!(second >= first);
    }
}
