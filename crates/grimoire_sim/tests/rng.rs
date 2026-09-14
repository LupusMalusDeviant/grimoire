//! `SimRng` and `derive_rng`: golden sequence, value ranges, bias checks and stream independence.

use grimoire_core::hash_of;
use grimoire_sim::{SimRng, derive_rng};

/// First eight `next_u32` values of `SimRng::new(0x5EED)` (algorithm version 1).
///
/// Measured once on Windows x86_64 and hardcoded: the test fails on any platform whose integer
/// arithmetic or seeding differs, so a CI run on each target doubles as a cross-platform check
/// (not yet verified on other platforms). Renew only together with a deliberate change of
/// `SimRng::ALGORITHM_VERSION`.
const GOLDEN_5EED: [u32; 8] = [
    2_851_957_292,
    406_565_052,
    3_973_736_297,
    2_852_121_472,
    1_195_749_783,
    3_550_453_828,
    82_058_311,
    1_980_337_350,
];

#[test]
fn golden_sequence_for_seed_5eed() {
    assert_eq!(SimRng::ALGORITHM_VERSION, 1);
    let mut rng = SimRng::new(0x5EED);
    let measured: [u32; 8] = std::array::from_fn(|_| rng.next_u32());
    assert_eq!(measured, GOLDEN_5EED, "measured: {measured:#010x?}");
}

#[test]
fn equal_seeds_give_equal_sequences_and_hashes() {
    let mut a = SimRng::new(99);
    let mut b = SimRng::new(99);
    assert_eq!(a, b);
    assert_eq!(hash_of(&a), hash_of(&b));
    for _ in 0..1_000 {
        assert_eq!(a.next_u64(), b.next_u64());
    }
    let before = hash_of(&a);
    a.next_u32();
    assert_ne!(hash_of(&a), before);
    assert_ne!(SimRng::new(1), SimRng::new(2));
}

#[test]
fn next_u64_is_high_then_low_u32() {
    let mut a = SimRng::new(3);
    let mut b = a.clone();
    let high = u64::from(b.next_u32());
    let low = u64::from(b.next_u32());
    assert_eq!(a.next_u64(), (high << 32) | low);
    assert_eq!(a, b);
}

#[test]
fn next_f32_is_in_unit_interval_with_24_bit_steps() {
    let mut rng = SimRng::new(5);
    let mut sum = 0.0f64;
    for _ in 0..100_000 {
        let value = rng.next_f32();
        assert!((0.0..1.0).contains(&value));
        assert_eq!((value * 16_777_216.0).fract(), 0.0);
        sum += f64::from(value);
    }
    let mean = sum / 100_000.0;
    assert!((mean - 0.5).abs() < 0.01, "mean {mean}");
}

#[test]
fn range_u32_is_evenly_distributed() {
    let mut rng = SimRng::new(0xD1CE);
    let mut buckets = [0u32; 10];
    for _ in 0..100_000 {
        buckets[rng.range_u32(0, 10) as usize] += 1;
    }
    for (value, &count) in buckets.iter().enumerate() {
        assert!((9_500..=10_500).contains(&count), "value {value}: {count}");
    }
}

#[test]
fn range_u32_has_no_modulo_bias() {
    // With `next_u32 % span`, values below 2^30 would appear half the time instead of a third.
    let span = 0xC000_0000u32;
    let mut rng = SimRng::new(17);
    let low = (0..90_000)
        .filter(|_| rng.range_u32(0, span) < 0x4000_0000)
        .count();
    assert!(
        (29_000..=31_000).contains(&low),
        "{low} of 90000 below 2^30"
    );
}

#[test]
fn range_bounds_hold_at_the_edges() {
    let mut rng = SimRng::new(8);
    for _ in 0..1_000 {
        assert_eq!(rng.range_u32(7, 8), 7);
        assert!(rng.range_u32(0, u32::MAX) < u32::MAX);
        assert!(rng.range_u32(u32::MAX - 3, u32::MAX) >= u32::MAX - 3);
        let wide = rng.range_i32(i32::MIN, i32::MAX);
        assert!(wide < i32::MAX);
        assert!((-2.0..2.0).contains(&rng.range_f32(-2.0, 2.0)));
        assert_eq!(rng.range_f32(1.0, 1.0f32.next_up()), 1.0);
    }
    let mut seen = [false; 6];
    for _ in 0..1_000 {
        let value = rng.range_i32(-3, 3);
        assert!((-3..3).contains(&value));
        seen[(value + 3) as usize] = true;
    }
    assert!(seen.iter().all(|&hit| hit));
}

#[test]
fn range_f32_never_returns_high() {
    let mut rng = SimRng::new(12);
    for _ in 0..100_000 {
        let value = rng.range_f32(100_000.0, 100_000.01);
        assert!((100_000.0..100_000.01).contains(&value));
    }
}

#[test]
#[should_panic(expected = "empty range")]
fn empty_u32_range_panics() {
    SimRng::new(0).range_u32(5, 5);
}

#[test]
#[should_panic(expected = "empty range")]
fn empty_i32_range_panics() {
    SimRng::new(0).range_i32(1, -1);
}

#[test]
#[should_panic(expected = "invalid range")]
fn infinite_f32_range_panics() {
    SimRng::new(0).range_f32(0.0, f32::INFINITY);
}

#[test]
fn chance_respects_probability_and_consumes_one_draw() {
    let mut rng = SimRng::new(21);
    let hits = (0..100_000).filter(|_| rng.chance(0.25)).count();
    assert!((24_000..=26_000).contains(&hits), "{hits}");

    for p in [f32::NAN, -1.0, 0.0, 0.5, 1.0, 2.0] {
        let mut drawn = rng.clone();
        drawn.next_u32();
        let result = rng.chance(p);
        assert_eq!(rng, drawn, "chance({p}) must consume exactly one draw");
        if p.is_nan() || p <= 0.0 {
            assert!(!result);
        }
        if p >= 1.0 {
            assert!(result);
        }
    }
}

#[test]
fn derived_streams_are_pure_functions_of_their_arguments() {
    let mut first = derive_rng(42, 7, 1);
    for _ in 0..100 {
        first.next_u32();
    }
    let mut other = derive_rng(42, 7, 0);
    other.next_u64();
    // Creating and drawing from other streams does not influence stream 1.
    let mut again = derive_rng(42, 7, 1);
    let mut reference = derive_rng(42, 7, 1);
    for _ in 0..100 {
        assert_eq!(again.next_u32(), reference.next_u32());
    }
    assert_eq!(again, first);
}

#[test]
fn every_argument_selects_a_different_stream() {
    let base = derive_rng(42, 7, 1);
    for other in [
        derive_rng(43, 7, 1),
        derive_rng(42, 8, 1),
        derive_rng(42, 7, 2),
        derive_rng(42, 1, 7),
        derive_rng(7, 42, 1),
        SimRng::new(42),
    ] {
        assert_ne!(base, other);
    }
}

#[test]
fn neighbouring_streams_are_uncorrelated() {
    let firsts = |make: &dyn Fn(u64) -> SimRng| -> Vec<u64> {
        (0..2_000).map(|i| make(i).next_u64()).collect()
    };
    for values in [
        firsts(&|i| derive_rng(0x5EED, 100, i)),
        firsts(&|i| derive_rng(0x5EED, i, 3)),
        firsts(&|i| derive_rng(i, 0, 0)),
    ] {
        let mut sorted = values.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), values.len(), "duplicate first outputs");

        let differing_bits: u32 = values
            .windows(2)
            .map(|pair| (pair[0] ^ pair[1]).count_ones())
            .sum();
        let mean = f64::from(differing_bits) / (values.len() - 1) as f64;
        assert!((31.5..=32.5).contains(&mean), "mean differing bits {mean}");

        let ones: u32 = values.iter().map(|value| value.count_ones()).sum();
        let mean_ones = f64::from(ones) / values.len() as f64;
        assert!(
            (31.5..=32.5).contains(&mean_ones),
            "mean set bits {mean_ones}"
        );
    }
}
