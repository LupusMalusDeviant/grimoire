//! Random-stream number allocation (contract §8.3).
//!
//! A stream number passed to [`crate::derive_rng`] or [`crate::derive_block_rng`] is not a bare
//! integer: bit 63 marks the range (engine or application), bits 62..48 name the owning crate and
//! the low 48 bits are a number local to that owner. [`engine_stream`] and [`app_stream`] build a
//! stream number from its parts; [`is_engine_stream`] and [`owner_of`] read one back. Engine
//! library code draws random numbers only from [`engine_stream`]s of its own crate's
//! [`owner`] constant; applications use [`app_stream`], which never collides with an engine
//! stream because bit 63 differs.

/// Bit 63: set for an engine stream, clear for an application stream.
pub const ENGINE_BIT: u64 = 1 << 63;

/// Bit position where the 15-bit owner field starts.
pub const OWNER_SHIFT: u32 = 48;

/// Mask of the 48-bit local-number field.
pub const LOCAL_MASK: u64 = (1 << 48) - 1;

/// Largest value an owner may hold: 15 bits.
const MAX_OWNER: u16 = 0x7fff;

/// Builds an engine stream number: [`ENGINE_BIT`] set, `owner` in bits 62..48, `local` in bits
/// 47..0.
///
/// # Panics
///
/// If `owner` is 0 (unassigned), greater than `0x7fff` (does not fit in the 15-bit owner field),
/// or `local` is greater than [`LOCAL_MASK`]. Every stream number in this crate's own code is a
/// `const`, so a bad value there is a compile-time error; the panic only fires at runtime if a
/// caller passes a non-constant, invalid `owner` or `local`.
#[must_use]
pub const fn engine_stream(owner: u16, local: u64) -> u64 {
    assert!(owner != 0, "stream owner must not be 0");
    assert!(owner <= MAX_OWNER, "stream owner must fit in 15 bits");
    assert!(
        local <= LOCAL_MASK,
        "stream local number must fit in 48 bits"
    );
    ENGINE_BIT | ((owner as u64) << OWNER_SHIFT) | local
}

/// Builds an application stream number: like [`engine_stream`] but with [`ENGINE_BIT`] clear, so
/// it never collides with an engine stream.
///
/// # Panics
///
/// Same conditions as [`engine_stream`].
#[must_use]
pub const fn app_stream(owner: u16, local: u64) -> u64 {
    assert!(owner != 0, "stream owner must not be 0");
    assert!(owner <= MAX_OWNER, "stream owner must fit in 15 bits");
    assert!(
        local <= LOCAL_MASK,
        "stream local number must fit in 48 bits"
    );
    ((owner as u64) << OWNER_SHIFT) | local
}

/// Whether `stream` has [`ENGINE_BIT`] set.
#[must_use]
pub const fn is_engine_stream(stream: u64) -> bool {
    stream & ENGINE_BIT != 0
}

/// Owner field of `stream` (bits 62..48), regardless of [`ENGINE_BIT`].
#[must_use]
pub const fn owner_of(stream: u64) -> u16 {
    ((stream >> OWNER_SHIFT) & MAX_OWNER as u64) as u16
}

/// Fixed owner numbers of engine crates (contract §8.3).
///
/// Reserved for further core crates: `0x0003..=0x000f`; for further engine subsystems (audio,
/// UI, assets): `0x0012..=0x00ff`. New constants are added only by a contract PR.
pub mod owner {
    /// Owner number of `grimoire_sim`.
    pub const SIM: u16 = 0x0001;
    /// Owner number of the `grimoire` facade.
    pub const FACADE: u16 = 0x0002;
    /// Owner number of `grimoire_sigil`.
    pub const SIGIL: u16 = 0x0010;
    /// Owner number of `grimoire_collide`.
    pub const COLLIDE: u16 = 0x0011;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frozen reference vectors of the bit split; a change here changes every stream number.
    #[test]
    fn engine_stream_reference_vectors() {
        assert_eq!(engine_stream(0x0001, 0), 0x8001_0000_0000_0000);
        assert_eq!(engine_stream(0x0001, 5), 0x8001_0000_0000_0005);
        assert_eq!(engine_stream(owner::SIGIL, 2), 0x8010_0000_0000_0002);
        // Owner and local both all-ones fill every bit: engine bit, 15 owner bits, 48 local bits.
        assert_eq!(engine_stream(MAX_OWNER, LOCAL_MASK), u64::MAX);
    }

    #[test]
    fn app_stream_clears_the_engine_bit() {
        assert_eq!(app_stream(1, 0), 0x0001_0000_0000_0000);
        assert_eq!(app_stream(MAX_OWNER, LOCAL_MASK), !ENGINE_BIT);
        assert_eq!(app_stream(1, 0) & ENGINE_BIT, 0);
    }

    #[test]
    fn is_engine_stream_and_owner_of_round_trip() {
        let engine = engine_stream(owner::COLLIDE, 42);
        assert!(is_engine_stream(engine));
        assert_eq!(owner_of(engine), owner::COLLIDE);

        let app = app_stream(7, 42);
        assert!(!is_engine_stream(app));
        assert_eq!(owner_of(app), 7);

        assert!(!is_engine_stream(0));
        assert_eq!(owner_of(0), 0);
    }

    #[test]
    #[should_panic(expected = "stream owner must not be 0")]
    fn engine_stream_panics_on_zero_owner() {
        let _ = engine_stream(0, 0);
    }

    #[test]
    #[should_panic(expected = "stream owner must fit in 15 bits")]
    fn engine_stream_panics_when_owner_overflows_15_bits() {
        let _ = engine_stream(0x8000, 0);
    }

    #[test]
    #[should_panic(expected = "stream local number must fit in 48 bits")]
    fn engine_stream_panics_when_local_overflows_48_bits() {
        let _ = engine_stream(1, LOCAL_MASK + 1);
    }

    #[test]
    #[should_panic(expected = "stream owner must not be 0")]
    fn app_stream_panics_on_zero_owner() {
        let _ = app_stream(0, 0);
    }

    #[test]
    #[should_panic(expected = "stream owner must fit in 15 bits")]
    fn app_stream_panics_when_owner_overflows_15_bits() {
        let _ = app_stream(0x8000, 0);
    }

    #[test]
    #[should_panic(expected = "stream local number must fit in 48 bits")]
    fn app_stream_panics_when_local_overflows_48_bits() {
        let _ = app_stream(1, LOCAL_MASK + 1);
    }

    #[test]
    fn stream_constants_are_unique_and_owned() {
        let all = [owner::SIM, owner::FACADE, owner::SIGIL, owner::COLLIDE];
        for &value in &all {
            assert!(value <= 0x7fff, "{value:#06x} must fit in 15 bits");
            assert_ne!(value, 0, "an owner constant must not be 0");
        }
        for i in 0..all.len() {
            for &other in &all[i + 1..] {
                assert_ne!(all[i], other, "owner constants must be distinct");
            }
        }
    }
}
