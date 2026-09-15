//! Layer masks: which broadphase objects can find each other (contract §14).

use std::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, Not};

use grimoire_core::{StableHash, StableHasher};

/// Bitmask of up to 32 layers.
///
/// [`Self::NONE`] finds nothing and is found by nothing; [`Self::ALL`] finds and is found by
/// everything. The engine assigns no meaning to individual bits — the layout of the 32 bits
/// belongs to the game (contract §14).
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct LayerMask(pub u32);

impl StableHash for LayerMask {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_u32(self.0);
    }
}

impl LayerMask {
    /// No layers set. Equal to [`Self::default`].
    pub const NONE: Self = Self(0);
    /// Every layer set.
    pub const ALL: Self = Self(u32::MAX);

    /// Mask with only `bit` set.
    ///
    /// # Panics
    ///
    /// Panics if `bit >= 32`.
    #[must_use]
    pub const fn layer(bit: u8) -> Self {
        assert!(bit < 32, "layer bit must be below 32");
        Self(1 << bit)
    }

    /// Whether `self` and `other` share at least one layer.
    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// Whether `self` contains every layer set in `other`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether no layer is set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for LayerMask {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitAnd for LayerMask {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl Not for LayerMask {
    type Output = Self;
    fn not(self) -> Self {
        Self(!self.0)
    }
}

impl BitOrAssign for LayerMask {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAndAssign for LayerMask {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_none() {
        assert_eq!(LayerMask::default(), LayerMask::NONE);
    }

    #[test]
    fn layer_sets_a_single_bit() {
        assert_eq!(LayerMask::layer(0), LayerMask(1));
        assert_eq!(LayerMask::layer(3), LayerMask(8));
        assert_eq!(LayerMask::layer(31), LayerMask(1 << 31));
    }

    #[test]
    #[should_panic(expected = "layer bit must be below 32")]
    fn layer_panics_on_out_of_range_bit() {
        let _ = LayerMask::layer(32);
    }

    #[test]
    fn intersects_and_contains_and_is_empty() {
        let a = LayerMask::layer(0) | LayerMask::layer(1);
        let b = LayerMask::layer(1) | LayerMask::layer(2);
        assert!(a.intersects(b));
        assert!(!LayerMask::NONE.intersects(LayerMask::ALL));
        assert!(LayerMask::ALL.contains(a));
        assert!(!a.contains(b));
        assert!(LayerMask::NONE.is_empty());
        assert!(!a.is_empty());
    }

    #[test]
    fn operators_match_bitwise_semantics() {
        let mut a = LayerMask::layer(0);
        a |= LayerMask::layer(1);
        assert_eq!(a, LayerMask::layer(0) | LayerMask::layer(1));
        a &= LayerMask::layer(1);
        assert_eq!(a, LayerMask::layer(1));
        assert_eq!(!LayerMask::NONE, LayerMask::ALL);
    }

    #[test]
    fn ordering_is_by_raw_bits() {
        assert!(LayerMask::layer(0) < LayerMask::layer(1));
        assert!(LayerMask::NONE < LayerMask::ALL);
    }
}
