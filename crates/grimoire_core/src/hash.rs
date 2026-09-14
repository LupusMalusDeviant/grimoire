//! Deterministic, platform-independent hashing of simulation state.
//!
//! `std::hash::Hash` together with `DefaultHasher` is unsuitable for golden masters and
//! cross-platform replay comparison: its output is not guaranteed to be stable across Rust
//! releases. [`StableHasher`] implements a fixed, documented algorithm and [`StableHash`]
//! defines *what* a type feeds into it.
//!
//! Algorithm v1: a 64-bit word mixer `state = (state.rotate_left(5) ^ word) * K` over
//! little-endian words (FxHash constant), finalised with SplitMix64 over `state ^ length`.
//! Floats are fed as their IEEE-754 bits, with every NaN replaced by the positive quiet NaN
//! without payload. The hasher remembers whether it was fed a NaN ([`StableHasher::saw_nan`]);
//! that flag is not part of the hash.
//! Any change to the algorithm invalidates every stored golden hash and must bump
//! [`StableHasher::ALGORITHM_VERSION`]; `tests/stable_hash_golden.rs` freezes version 1.

const MIX_CONSTANT: u64 = 0x517c_c1b7_2722_0a95;
const CANONICAL_NAN_F32: u32 = 0x7fc0_0000;
const CANONICAL_NAN_F64: u64 = 0x7ff8_0000_0000_0000;

/// Streaming 64-bit hasher whose output is bit-identical on every platform and Rust release.
///
/// Equality compares only what determines [`StableHasher::finish`], not [`StableHasher::saw_nan`].
#[derive(Debug, Clone)]
pub struct StableHasher {
    state: u64,
    length: u64,
    nan_seen: bool,
}

impl PartialEq for StableHasher {
    fn eq(&self, other: &Self) -> bool {
        self.state == other.state && self.length == other.length
    }
}

impl Eq for StableHasher {}

impl Default for StableHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl StableHasher {
    /// Version of the hashing algorithm. Golden hashes are only comparable within one version.
    pub const ALGORITHM_VERSION: u32 = 1;

    /// Creates a hasher with seed zero.
    #[must_use]
    pub const fn new() -> Self {
        Self::with_seed(0)
    }

    /// Creates a hasher whose output is additionally keyed by `seed`.
    #[must_use]
    pub const fn with_seed(seed: u64) -> Self {
        Self {
            state: seed,
            length: 0,
            nan_seen: false,
        }
    }

    #[inline]
    const fn mix(&mut self, word: u64) {
        self.state = (self.state.rotate_left(5) ^ word).wrapping_mul(MIX_CONSTANT);
    }

    #[inline]
    const fn add_length(&mut self, bytes: u64) {
        self.length = self.length.wrapping_add(bytes);
    }

    /// Feeds an unsigned 8-bit integer.
    #[inline]
    pub const fn write_u8(&mut self, value: u8) {
        self.mix(value as u64);
        self.add_length(1);
    }

    /// Feeds an unsigned 16-bit integer.
    #[inline]
    pub const fn write_u16(&mut self, value: u16) {
        self.mix(value as u64);
        self.add_length(2);
    }

    /// Feeds an unsigned 32-bit integer.
    #[inline]
    pub const fn write_u32(&mut self, value: u32) {
        self.mix(value as u64);
        self.add_length(4);
    }

    /// Feeds an unsigned 64-bit integer.
    #[inline]
    pub const fn write_u64(&mut self, value: u64) {
        self.mix(value);
        self.add_length(8);
    }

    /// Feeds a `usize`, always widened to 64 bits so 32- and 64-bit targets agree.
    #[inline]
    pub const fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }

    /// Feeds a signed 8-bit integer (two's complement bit pattern).
    #[inline]
    pub const fn write_i8(&mut self, value: i8) {
        self.write_u8(value as u8);
    }

    /// Feeds a signed 16-bit integer (two's complement bit pattern).
    #[inline]
    pub const fn write_i16(&mut self, value: i16) {
        self.write_u16(value as u16);
    }

    /// Feeds a signed 32-bit integer (two's complement bit pattern).
    #[inline]
    pub const fn write_i32(&mut self, value: i32) {
        self.write_u32(value as u32);
    }

    /// Feeds a signed 64-bit integer (two's complement bit pattern).
    #[inline]
    pub const fn write_i64(&mut self, value: i64) {
        self.write_u64(value as u64);
    }

    /// Feeds an `isize`, always widened to 64 bits.
    #[inline]
    pub const fn write_isize(&mut self, value: isize) {
        self.write_i64(value as i64);
    }

    /// Feeds a boolean as one byte.
    #[inline]
    pub const fn write_bool(&mut self, value: bool) {
        self.write_u8(value as u8);
    }

    /// Feeds an `f32` bit-exactly, except that every NaN is fed as one canonical quiet NaN.
    ///
    /// `-0.0` and `0.0` still hash differently. NaN sign and payload are not portable: they
    /// differ between x86_64 and aarch64 and between constant folding and run time (engine
    /// ADR 0004), while every other bit of an IEEE-754 result is. A NaN also sets
    /// [`Self::saw_nan`].
    #[inline]
    pub const fn write_f32(&mut self, value: f32) {
        let bits = if value.is_nan() {
            self.nan_seen = true;
            CANONICAL_NAN_F32
        } else {
            value.to_bits()
        };
        self.write_u32(bits);
    }

    /// Feeds an `f64` bit-exactly, with the same NaN canonicalisation as [`Self::write_f32`].
    #[inline]
    pub const fn write_f64(&mut self, value: f64) {
        let bits = if value.is_nan() {
            self.nan_seen = true;
            CANONICAL_NAN_F64
        } else {
            value.to_bits()
        };
        self.write_u64(bits);
    }

    /// Feeds raw bytes, prefixed with their length so that concatenations stay unambiguous.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.write_usize(bytes.len());
        let (words, rest) = bytes.as_chunks::<8>();
        for word in words {
            self.mix(u64::from_le_bytes(*word));
        }
        if !rest.is_empty() {
            let mut word = [0u8; 8];
            word[..rest.len()].copy_from_slice(rest);
            self.mix(u64::from_le_bytes(word));
        }
        self.add_length(bytes.len() as u64);
    }

    /// Feeds a string as its UTF-8 bytes (length-prefixed).
    pub fn write_str(&mut self, value: &str) {
        self.write_bytes(value.as_bytes());
    }

    /// Returns the hash of everything written so far without resetting the hasher.
    #[must_use]
    pub const fn finish(&self) -> u64 {
        splitmix64(self.state ^ self.length)
    }

    /// Whether any `f32` or `f64` written so far was NaN.
    ///
    /// NaN is forbidden in simulation state (engine ADR 0004, rule 4), but the canonicalisation
    /// makes it invisible in the hash; this flag is how hash-based gates still detect it.
    #[must_use]
    pub const fn saw_nan(&self) -> bool {
        self.nan_seen
    }
}

const fn splitmix64(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Hashes a single value with a fresh [`StableHasher`].
#[must_use]
pub fn hash_of<T: StableHash + ?Sized>(value: &T) -> u64 {
    let mut hasher = StableHasher::new();
    value.stable_hash(&mut hasher);
    hasher.finish()
}

/// Types whose simulation-relevant state can be fed into a [`StableHasher`].
///
/// Contract:
/// - The fed byte stream depends only on the value — never on memory addresses, capacities,
///   `TypeId`s, hash-map iteration order or the platform's word size.
/// - Floats are fed bit-exactly: the purpose is detecting *any* divergence, not semantic equality.
///   The only exception is NaN, whose sign and payload are platform-dependent; all NaNs hash
///   alike. NaN in simulation state is a bug regardless (engine ADR 0004).
/// - Adding, removing or reordering fed fields changes hashes; golden masters are then renewed
///   deliberately.
pub trait StableHash {
    /// Feeds `self` into `hasher`.
    fn stable_hash(&self, hasher: &mut StableHasher);
}

macro_rules! impl_primitive {
    ($($ty:ty => $method:ident),* $(,)?) => {
        $(
            impl StableHash for $ty {
                #[inline]
                fn stable_hash(&self, hasher: &mut StableHasher) {
                    hasher.$method(*self);
                }
            }
        )*
    };
}

impl_primitive!(
    u8 => write_u8,
    u16 => write_u16,
    u32 => write_u32,
    u64 => write_u64,
    usize => write_usize,
    i8 => write_i8,
    i16 => write_i16,
    i32 => write_i32,
    i64 => write_i64,
    isize => write_isize,
    bool => write_bool,
    f32 => write_f32,
    f64 => write_f64,
);

impl StableHash for char {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_u32(u32::from(*self));
    }
}

impl StableHash for str {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_str(self);
    }
}

impl StableHash for String {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_str(self);
    }
}

impl StableHash for () {
    fn stable_hash(&self, _hasher: &mut StableHasher) {}
}

impl<T: StableHash> StableHash for [T] {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_usize(self.len());
        for item in self {
            item.stable_hash(hasher);
        }
    }
}

impl<T: StableHash, const N: usize> StableHash for [T; N] {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        for item in self {
            item.stable_hash(hasher);
        }
    }
}

impl<T: StableHash> StableHash for Vec<T> {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        self.as_slice().stable_hash(hasher);
    }
}

impl<T: StableHash> StableHash for Option<T> {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        match self {
            None => hasher.write_u8(0),
            Some(value) => {
                hasher.write_u8(1);
                value.stable_hash(hasher);
            }
        }
    }
}

impl<T: StableHash + ?Sized> StableHash for &T {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        (**self).stable_hash(hasher);
    }
}

impl<T: StableHash + ?Sized> StableHash for Box<T> {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        (**self).stable_hash(hasher);
    }
}

macro_rules! impl_tuple {
    ($($name:ident),+) => {
        impl<$($name: StableHash),+> StableHash for ($($name,)+) {
            #[allow(non_snake_case)]
            fn stable_hash(&self, hasher: &mut StableHasher) {
                let ($($name,)+) = self;
                $($name.stable_hash(hasher);)+
            }
        }
    };
}

impl_tuple!(A);
impl_tuple!(A, B);
impl_tuple!(A, B, C);
impl_tuple!(A, B, C, D);
impl_tuple!(A, B, C, D, E);
impl_tuple!(A, B, C, D, E, F);
impl_tuple!(A, B, C, D, E, F, G);
impl_tuple!(A, B, C, D, E, F, G, H);

/// Implements [`StableHash`] for a struct by feeding the listed fields in the given order.
///
/// ```
/// use grimoire_core::{hash_of, impl_stable_hash};
///
/// #[derive(Clone)]
/// struct Position {
///     x: f32,
///     y: f32,
/// }
/// impl_stable_hash!(Position { x, y });
///
/// assert_eq!(hash_of(&Position { x: 1.0, y: 2.0 }), hash_of(&Position { x: 1.0, y: 2.0 }));
/// ```
#[macro_export]
macro_rules! impl_stable_hash {
    ($ty:ty { $($field:ident),* $(,)? }) => {
        impl $crate::StableHash for $ty {
            #[allow(unused_variables)]
            fn stable_hash(&self, hasher: &mut $crate::StableHasher) {
                $( $crate::StableHash::stable_hash(&self.$field, hasher); )*
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_input_gives_equal_hash() {
        assert_eq!(hash_of(&(1u32, 2.5f32, "x")), hash_of(&(1u32, 2.5f32, "x")));
    }

    #[test]
    fn different_input_gives_different_hash() {
        assert_ne!(hash_of(&1u64), hash_of(&2u64));
    }

    #[test]
    fn field_order_matters() {
        assert_ne!(hash_of(&(1u32, 2u32)), hash_of(&(2u32, 1u32)));
    }

    #[test]
    fn nested_sequences_are_length_prefixed() {
        let a = [vec![1u8], vec![2u8, 3]];
        let b = [vec![1u8, 2], vec![3u8]];
        assert_ne!(hash_of(&a), hash_of(&b));
    }

    #[test]
    fn floats_hash_bit_exactly() {
        assert_ne!(hash_of(&0.0f32), hash_of(&-0.0f32));
        assert_ne!(hash_of(&0.0f64), hash_of(&-0.0f64));
        assert_ne!(
            hash_of(&1.0f32),
            hash_of(&f32::from_bits(1.0f32.to_bits() + 1))
        );
    }

    #[test]
    fn nan_sign_and_payload_are_canonicalised() {
        let canonical = hash_of(&f32::NAN);
        for bits in [0x7fc0_0000u32, 0xffc0_0000, 0x7f80_0001, 0xffff_ffff] {
            assert_eq!(hash_of(&f32::from_bits(bits)), canonical, "{bits:#010x}");
        }
        let canonical = hash_of(&f64::NAN);
        for bits in [
            0x7ff8_0000_0000_0000u64,
            0xfff8_0000_0000_0000,
            0x7ff0_0000_0000_0001,
        ] {
            assert_eq!(hash_of(&f64::from_bits(bits)), canonical, "{bits:#018x}");
        }
    }

    #[test]
    fn nan_is_distinct_from_infinities() {
        let nan = hash_of(&f32::NAN);
        assert_ne!(nan, hash_of(&f32::INFINITY));
        assert_ne!(nan, hash_of(&f32::NEG_INFINITY));
    }

    #[test]
    fn nan_is_flagged_without_affecting_hash_or_equality() {
        let mut clean = StableHasher::new();
        clean.write_f32(1.0);
        clean.write_f32(f32::INFINITY);
        clean.write_f64(-0.0);
        clean.write_f64(f64::NEG_INFINITY);
        assert!(!clean.saw_nan());

        let mut from_f32 = StableHasher::new();
        from_f32.write_f32(f32::from_bits(0xffc0_0001));
        from_f32.write_u64(7);
        assert!(from_f32.saw_nan(), "the flag stays set after later writes");
        assert!(from_f32.clone().saw_nan());

        let mut from_f64 = StableHasher::new();
        from_f64.write_f64(f64::NAN);
        assert!(from_f64.saw_nan());

        let mut canonical_bits = StableHasher::new();
        canonical_bits.write_u32(CANONICAL_NAN_F32);
        canonical_bits.write_u64(7);
        assert!(!canonical_bits.saw_nan());
        assert_eq!(canonical_bits.finish(), from_f32.finish());
        assert_eq!(canonical_bits, from_f32);
    }

    #[test]
    fn seed_changes_output() {
        let mut seeded = StableHasher::with_seed(7);
        42u64.stable_hash(&mut seeded);
        assert_ne!(seeded.finish(), hash_of(&42u64));
    }

    #[test]
    fn finish_does_not_reset() {
        let mut hasher = StableHasher::new();
        hasher.write_u32(5);
        assert_eq!(hasher.finish(), hasher.finish());
    }

    #[test]
    fn byte_tail_is_hashed() {
        assert_ne!(hash_of("abcdefgh1"), hash_of("abcdefgh2"));
    }
}
