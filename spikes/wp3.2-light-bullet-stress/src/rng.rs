//! Deterministic pseudo-random source shared by [`crate::bullets`] and [`crate::lights`], so
//! every CI run and every local `cargo test` sees byte-identical synthetic workloads (WP3.2 needs
//! comparable stress scenes, not fresh random ones each run).

/// `xorshift64*`: small, dependency-free, and good enough for generating synthetic stress data
/// (not used for anything security- or gameplay-sensitive).
pub struct Rng(u64);

impl Rng {
    /// `seed` is forced odd (xorshift64* requires a non-zero state, and this keeps it simple).
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    pub fn next_u32(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 32) as u32
    }

    /// Uniform in `[0, 1)`.
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Uniform in `[low, high)`.
    pub fn next_range(&mut self, low: f32, high: f32) -> f32 {
        low + self.next_f32() * (high - low)
    }
}

#[cfg(test)]
mod tests {
    use super::Rng;

    #[test]
    fn same_seed_is_deterministic() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        let sequence_a: Vec<u32> = (0..16).map(|_| a.next_u32()).collect();
        let sequence_b: Vec<u32> = (0..16).map(|_| b.next_u32()).collect();
        assert_eq!(sequence_a, sequence_b);
    }

    #[test]
    fn next_f32_stays_in_unit_range() {
        let mut rng = Rng::new(7);
        for _ in 0..1000 {
            let value = rng.next_f32();
            assert!((0.0..1.0).contains(&value), "{value} out of range");
        }
    }
}
