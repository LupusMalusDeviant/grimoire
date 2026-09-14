//! Seeded, platform-independent random numbers: [`SimRng`] and [`derive_rng`].

use grimoire_core::{StableHash, StableHasher};

const PCG_MULTIPLIER: u64 = 6_364_136_223_846_793_005;
const SPLITMIX_GAMMA: u64 = 0x9e37_79b9_7f4a_7c15;

/// SplitMix64 output function applied to `value + γ` (Steele, Lea, Flood 2014).
const fn splitmix64(value: u64) -> u64 {
    let mut z = value.wrapping_add(SPLITMIX_GAMMA);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Deterministic random number generator for simulation code.
///
/// # Algorithm (version 1, [`SimRng::ALGORITHM_VERSION`])
///
/// PCG32, variant XSH-RR 64/32 (M. E. O'Neill, `pcg32_random_r` of the PCG reference
/// implementation): a 64-bit LCG with multiplier `6364136223846793005` and an odd increment
/// selecting the stream, output by xorshift-high and a random rotation.
///
/// [`SimRng::new`] expands the 64-bit seed with SplitMix64 into the reference seeding pair:
/// `initstate = splitmix64(seed)`, `initseq = splitmix64(seed + γ)` (the first two SplitMix64
/// outputs), then seeds exactly like `pcg32_srandom_r(initstate, initseq)`.
///
/// Derived values:
/// - [`SimRng::next_u64`]: `(next_u32 << 32) | next_u32`, high half drawn first.
/// - [`SimRng::next_f32`]: the upper 24 bits of one `next_u32` times 2⁻²⁴, so every value is an
///   exact multiple of 2⁻²⁴ in `[0, 1)`.
/// - [`SimRng::range_u32`] / [`SimRng::range_i32`]: Lemire's multiply-and-reject method, unbiased;
///   usually one draw, occasionally more.
///
/// Any change to these definitions changes every seeded run and golden hash and must bump
/// `ALGORITHM_VERSION`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimRng {
    state: u64,
    increment: u64,
}

impl SimRng {
    /// Version of the generator and derivation rules. Seeded runs are only comparable within one
    /// version.
    pub const ALGORITHM_VERSION: u32 = 1;

    /// Creates a generator from a 64-bit seed.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self::from_pcg_seed(
            splitmix64(seed),
            splitmix64(seed.wrapping_add(SPLITMIX_GAMMA)),
        )
    }

    /// Reference seeding `pcg32_srandom_r(init_state, init_sequence)`.
    const fn from_pcg_seed(init_state: u64, init_sequence: u64) -> Self {
        let mut rng = Self {
            state: 0,
            increment: (init_sequence << 1) | 1,
        };
        rng.step();
        rng.state = rng.state.wrapping_add(init_state);
        rng.step();
        rng
    }

    #[inline]
    const fn step(&mut self) {
        self.state = self
            .state
            .wrapping_mul(PCG_MULTIPLIER)
            .wrapping_add(self.increment);
    }

    /// Next uniformly distributed `u32`.
    #[inline]
    pub const fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.step();
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rotation = (old >> 59) as u32;
        xorshifted.rotate_right(rotation)
    }

    /// Next uniformly distributed `u64`, built from two `next_u32` calls (high half first).
    #[inline]
    pub const fn next_u64(&mut self) -> u64 {
        let high = self.next_u32() as u64;
        let low = self.next_u32() as u64;
        (high << 32) | low
    }

    /// Uniform `f32` in `[0, 1)` with 24 bits of resolution; consumes one `next_u32`.
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        const SCALE: f32 = 1.0 / 16_777_216.0;
        (self.next_u32() >> 8) as f32 * SCALE
    }

    /// Uniform integer in `[0, range)`; `range` must not be 0.
    fn bounded(&mut self, range: u32) -> u32 {
        let mut product = u64::from(self.next_u32()) * u64::from(range);
        let mut low = product as u32;
        if low < range {
            let threshold = range.wrapping_neg() % range;
            while low < threshold {
                product = u64::from(self.next_u32()) * u64::from(range);
                low = product as u32;
            }
        }
        (product >> 32) as u32
    }

    /// Unbiased uniform `u32` in `[low, high)`.
    ///
    /// # Panics
    ///
    /// If `low >= high` (empty range).
    pub fn range_u32(&mut self, low: u32, high: u32) -> u32 {
        assert!(low < high, "SimRng::range_u32: empty range {low}..{high}");
        low + self.bounded(high - low)
    }

    /// Unbiased uniform `i32` in `[low, high)`; the full span `i32::MIN..i32::MAX` is allowed.
    ///
    /// # Panics
    ///
    /// If `low >= high` (empty range).
    pub fn range_i32(&mut self, low: i32, high: i32) -> i32 {
        assert!(low < high, "SimRng::range_i32: empty range {low}..{high}");
        // The span is in 1..=u32::MAX, and low + offset < high, so both casts are lossless.
        let span = (i64::from(high) - i64::from(low)) as u32;
        (i64::from(low) + i64::from(self.bounded(span))) as i32
    }

    /// Uniform `f32` in `[low, high)`; consumes one `next_u32`.
    ///
    /// When rounding would produce `high`, the largest `f32` below `high` is returned instead.
    ///
    /// # Panics
    ///
    /// Unless `low < high` and `high - low` is finite (NaN and infinite bounds panic).
    pub fn range_f32(&mut self, low: f32, high: f32) -> f32 {
        assert!(
            low < high && (high - low).is_finite(),
            "SimRng::range_f32: invalid range {low}..{high}"
        );
        let value = low + (high - low) * self.next_f32();
        if value < high {
            value
        } else {
            high.next_down()
        }
    }

    /// `true` with probability `p`; consumes exactly one `next_u32` regardless of `p`.
    ///
    /// `p <= 0` or NaN is always `false`, `p >= 1` always `true`.
    pub fn chance(&mut self, p: f32) -> bool {
        self.next_f32() < p
    }
}

impl StableHash for SimRng {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_u64(self.state);
        hasher.write_u64(self.increment);
    }
}

/// Generator for one `stream` (usually one per system) at one `tick` of a run with `seed`.
///
/// The result is a pure function of the three arguments:
/// `SimRng::new(splitmix64(splitmix64(splitmix64(seed) ^ tick) ^ stream))`. It therefore does not
/// depend on which other streams were created or drawn from before, so systems can be reordered
/// or parallelised without changing each other's random numbers.
#[must_use]
pub const fn derive_rng(seed: u64, tick: u64, stream: u64) -> SimRng {
    SimRng::new(splitmix64(splitmix64(splitmix64(seed) ^ tick) ^ stream))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// First outputs of the PCG reference demo (`pcg32-demo`, initstate 42, initseq 54).
    #[test]
    fn matches_pcg32_reference_vector() {
        let mut rng = SimRng::from_pcg_seed(42, 54);
        let expected = [
            0xa15c_02b7,
            0x7b47_f409,
            0xba1d_3330,
            0x83d2_f293,
            0xbfa4_784b,
            0xcbed_606e,
        ];
        for value in expected {
            assert_eq!(rng.next_u32(), value);
        }
    }

    /// Reference outputs of SplitMix64 seeded with 0: `next()` returns `splitmix64(state)` and
    /// advances the state by γ.
    #[test]
    fn matches_splitmix64_reference_vector() {
        assert_eq!(splitmix64(0), 0xe220_a839_7b1d_cdaf);
        assert_eq!(splitmix64(SPLITMIX_GAMMA), 0x6e78_9e6a_a1b9_65f4);
    }
}
