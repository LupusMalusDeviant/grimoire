//! Fixed-timestep accumulator: exact tick counts, zero drift, clamping and alpha bounds.

use std::time::Duration;

use grimoire_sim::FixedTimestep;
use proptest::prelude::*;

const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// Frame deltas of a display running at `fps`, derived from absolute integer timestamps
/// (`⌊k · 10⁹ / fps⌋` ns) like a real monotonic clock, so they sum to the exact elapsed time.
fn display_deltas(fps: u64, frames: u64) -> impl Iterator<Item = (u64, Duration)> {
    (1..=frames).map(move |k| {
        let now = k * NANOS_PER_SECOND / fps;
        let previous = (k - 1) * NANOS_PER_SECOND / fps;
        (now, Duration::from_nanos(now - previous))
    })
}

#[test]
fn one_second_of_144_hz_frames_gives_exactly_60_ticks() {
    let mut timestep = FixedTimestep::new(60);
    let mut ticks = 0u64;
    for (_, delta) in display_deltas(144, 144) {
        let plan = timestep.advance(delta);
        assert!(
            plan.ticks <= 1,
            "a 144 Hz frame never needs two 60 Hz ticks"
        );
        ticks += u64::from(plan.ticks);
    }
    assert_eq!(ticks, 60);
    assert_eq!(timestep.dropped_time(), Duration::ZERO);
    assert_eq!(timestep.advance(Duration::ZERO).alpha, 0.0);
}

#[test]
fn ten_minutes_of_144_hz_frames_do_not_drift() {
    let mut timestep = FixedTimestep::new(60);
    let mut ticks = 0u64;
    for (now, delta) in display_deltas(144, 144 * 600) {
        ticks += u64::from(timestep.advance(delta).ticks);
        assert_eq!(ticks, now * 60 / NANOS_PER_SECOND, "drift at {now} ns");
    }
    assert_eq!(ticks, 36_000);
    assert_eq!(timestep.dropped_time(), Duration::ZERO);
}

#[test]
fn rounded_constant_frame_delta_accumulates_exactly() {
    // 60 Hz vsync reported as a rounded 16_666_667 ns per frame.
    let delta_nanos = 16_666_667u64;
    let mut timestep = FixedTimestep::new(60);
    let mut ticks = 0u64;
    for frame in 1..=36_000u64 {
        ticks += u64::from(timestep.advance(Duration::from_nanos(delta_nanos)).ticks);
        assert_eq!(ticks, frame * delta_nanos * 60 / NANOS_PER_SECOND);
    }
    assert_eq!(ticks, 36_000);
}

#[test]
fn huge_frame_is_clamped_and_counted_as_dropped_time() {
    let mut timestep = FixedTimestep::new(60);
    assert_eq!(timestep.max_ticks_per_frame(), 8);

    let plan = timestep.advance(Duration::from_secs(10));
    assert_eq!(plan.ticks, 8);
    assert_eq!(plan.alpha, 0.0);
    // 600 ticks due, 592 dropped: 592 / 60 s, truncated to whole nanoseconds.
    assert_eq!(timestep.dropped_time(), Duration::from_nanos(9_866_666_666));

    let plan = timestep.advance(Duration::from_nanos(25_000_000));
    assert_eq!(plan.ticks, 1);
    assert_eq!(timestep.dropped_time(), Duration::from_nanos(9_866_666_666));

    // 60.5 ticks due, 8 run, 52 more dropped. Truncation applies once to the exact total of
    // 644 dropped ticks, so no rounding error accumulates across frames.
    let plan = timestep.advance(Duration::from_secs(1));
    assert_eq!(plan.ticks, 8);
    assert_eq!(
        timestep.dropped_time(),
        Duration::from_nanos(10_733_333_333)
    );

    timestep.reset();
    assert_eq!(timestep.dropped_time(), Duration::ZERO);
    assert_eq!(timestep.advance(Duration::ZERO).ticks, 0);
}

#[test]
fn custom_tick_limit_is_respected() {
    let mut timestep = FixedTimestep::new(100).with_max_ticks_per_frame(2);
    let plan = timestep.advance(Duration::from_millis(55));
    assert_eq!(plan.ticks, 2);
    assert_eq!(timestep.dropped_time(), Duration::from_millis(30));
    assert!((plan.alpha - 0.5).abs() < 1e-6);
}

#[test]
fn extreme_deltas_never_panic() {
    let mut timestep = FixedTimestep::new(u32::MAX).with_max_ticks_per_frame(u32::MAX);
    for _ in 0..4 {
        let plan = timestep.advance(Duration::MAX);
        assert_eq!(plan.ticks, u32::MAX);
        assert!((0.0..1.0).contains(&plan.alpha));
    }
    assert!(timestep.dropped_time() > Duration::ZERO);
}

#[test]
fn dropped_time_saturates_at_duration_max() {
    let mut timestep = FixedTimestep::new(1);
    timestep.advance(Duration::MAX);
    timestep.advance(Duration::MAX);
    assert_eq!(timestep.dropped_time(), Duration::MAX);
    timestep.advance(Duration::MAX);
    assert_eq!(timestep.dropped_time(), Duration::MAX);
}

#[test]
fn alpha_stays_below_one_when_rounding_would_reach_it() {
    let mut timestep = FixedTimestep::new(1);
    let plan = timestep.advance(Duration::from_nanos(999_999_999));
    assert_eq!(plan.ticks, 0);
    assert!(plan.alpha < 1.0);
    assert!(plan.alpha > 0.999);

    let plan = timestep.advance(Duration::from_nanos(1));
    assert_eq!(plan.ticks, 1);
    assert_eq!(plan.alpha, 0.0);
}

#[test]
fn alpha_is_the_elapsed_fraction_of_the_next_tick() {
    let mut timestep = FixedTimestep::new(60);
    let plan = timestep.advance(Duration::from_nanos(8_333_333));
    assert_eq!(plan.ticks, 0);
    assert!((plan.alpha - 0.5).abs() < 1e-6);
}

#[test]
fn tick_duration_is_truncated_to_nanoseconds() {
    let timestep = FixedTimestep::new(60);
    assert_eq!(timestep.tick_rate_hz(), 60);
    assert_eq!(timestep.tick_duration(), Duration::from_nanos(16_666_666));
    assert_eq!(
        FixedTimestep::new(1).tick_duration(),
        Duration::from_secs(1)
    );
}

#[test]
#[should_panic(expected = "at least 1 Hz")]
fn zero_tick_rate_panics() {
    let _ = FixedTimestep::new(0);
}

#[test]
#[should_panic(expected = "at least 1 tick per frame")]
fn zero_tick_limit_panics() {
    let _ = FixedTimestep::new(60).with_max_ticks_per_frame(0);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Without a tick limit the planned ticks equal ⌊total time × rate⌋ for any split of time.
    #[test]
    fn unlimited_ticks_match_total_time(
        tick_rate_hz in 1u32..10_000,
        deltas in prop::collection::vec(0u64..200_000_000, 0..200),
    ) {
        let mut timestep = FixedTimestep::new(tick_rate_hz).with_max_ticks_per_frame(u32::MAX);
        let mut ticks = 0u128;
        let mut total = 0u128;
        for delta in deltas {
            let plan = timestep.advance(Duration::from_nanos(delta));
            prop_assert!((0.0..1.0).contains(&plan.alpha));
            ticks += u128::from(plan.ticks);
            total += u128::from(delta);
            prop_assert_eq!(ticks, total * u128::from(tick_rate_hz) / 1_000_000_000);
        }
        prop_assert_eq!(timestep.dropped_time(), Duration::ZERO);
    }
}
