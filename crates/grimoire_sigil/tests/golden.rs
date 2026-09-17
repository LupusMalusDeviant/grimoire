//! Byte-level golden fixtures for `SigilUnit` v1 (contract §2 rule 10: "byteweise Golden-Fixtures
//! unter Versionskontrolle").
//!
//! `tests/golden/*.bin` are hand-derived from `docs/formats/sigil.md`'s binary-format tables and
//! this crate's own `content_hash` formula (contract §11.1: a `StableHasher` over header bytes
//! `0..24` then `32..end`), **not** produced by calling [`SigilUnit::to_bytes`] and saving the
//! result — the same principle `crate::test_support`'s own module docs already state for this
//! crate's other hand-built fixtures ("exercise the decoder against externally constructed bytes,
//! not only the encoder's own round trip"), applied here as real committed files instead of bytes
//! assembled inline in a test. Every field was assembled offset-by-offset against the documented
//! layout with a standalone script that reimplements the `StableHasher` v1 algorithm from
//! `grimoire_core::hash`'s own module documentation (word mixer with the FxHash constant,
//! finalised with SplitMix64), never by calling this crate's `StableHasher` type. Only afterwards
//! are these files checked *against* [`SigilUnit::from_bytes`]/[`SigilUnit::to_bytes`] below —
//! fixtures independent of the encoder, checked against it, not the other way around.
//!
//! - `minimal_unit_v1.bin`: the smallest legal unit — one bullet type, an empty (but present,
//!   contract-required) `Emitters` section, no optional sections at all.
//! - `ring_burst_unit_v1.bin`: exercises every WP4.2 binary addition in one file — a `Programs`
//!   section (one `ring` block plus an `accelerate` modifier, mirroring
//!   `tests/corpus/valid/01-ring-burst.sigil`'s `emitter burst`), a real `Emitters` record
//!   referencing that program, and a `BehaviorRefs` section.
//! - `transforms_unit_v1.bin` (Plan 0002 WP5.2): a `Transforms` section (`docs/formats/sigil.md`
//!   §10.9) with a behavior binding carrying two parameters and all four transform kinds under
//!   all three trigger kinds, next to `Programs`, a sub emitter and `BehaviorRefs`. The script
//!   that derived it also computes the `event` trigger's id from the name `phase_end` with its
//!   own `StableHasher` re-implementation, so `EventId::from_name` is checked against it too.

use grimoire_sigil::{BehaviorId, BulletFlags, EventId, SigilUnit, UnitId};

const MINIMAL_UNIT: &[u8] = include_bytes!("golden/minimal_unit_v1.bin");
const RING_BURST_UNIT: &[u8] = include_bytes!("golden/ring_burst_unit_v1.bin");
const TRANSFORMS_UNIT: &[u8] = include_bytes!("golden/transforms_unit_v1.bin");

#[test]
fn minimal_unit_decodes_as_hand_derived() {
    let unit = SigilUnit::from_bytes(MINIMAL_UNIT).expect("hand-derived fixture must decode");
    assert_eq!(unit.id().0, 42);
    assert_eq!(unit.bullet_types().len(), 1);
    assert_eq!(unit.bullet_types()[0].radius, 0.25);
    assert_eq!(unit.bullet_types()[0].collision_radius, 0.25);
    assert_eq!(unit.emitter_count(), 0);
    assert_eq!(unit.program_count(), 0);
    assert!(unit.behavior_refs().is_empty());
}

#[test]
fn minimal_unit_round_trips_byte_identically() {
    // The real check this module's docs ask for: the hand-derived bytes, once decoded, re-encode
    // to exactly the same bytes -- the encoder is verified against an independently built
    // fixture, not the other way around.
    let unit = SigilUnit::from_bytes(MINIMAL_UNIT).expect("must decode");
    assert_eq!(unit.to_bytes(), MINIMAL_UNIT);
}

#[test]
fn minimal_unit_truncations_and_bit_flips_never_panic_and_never_wrongly_decode() {
    for cut in 0..MINIMAL_UNIT.len() {
        let truncated = &MINIMAL_UNIT[..cut];
        assert!(
            SigilUnit::from_bytes(truncated).is_err(),
            "a truncation to {cut} bytes must not decode as valid"
        );
    }
    for index in 0..MINIMAL_UNIT.len() {
        let mut mutated = MINIMAL_UNIT.to_vec();
        mutated[index] ^= 0xFF;
        // Never panics (contract §2 rule 9); most single-byte flips also invalidate the unit
        // (wrong magic, wrong hash, an out-of-range field, ...), but this loop's only universal
        // claim is "no panic", so the result is deliberately not asserted either way.
        let _ = SigilUnit::from_bytes(&mutated);
    }
}

#[test]
fn ring_burst_unit_decodes_with_a_program_and_a_behavior_ref() {
    let unit = SigilUnit::from_bytes(RING_BURST_UNIT).expect("hand-derived fixture must decode");
    assert_eq!(unit.id().0, 7);
    assert_eq!(unit.bullet_types().len(), 1);
    assert_eq!(unit.emitter_count(), 1);
    assert_eq!(unit.program_count(), 1);
    assert_eq!(unit.behavior_refs(), &[BehaviorId(7)]);
}

#[test]
fn ring_burst_unit_round_trips_byte_identically() {
    let unit = SigilUnit::from_bytes(RING_BURST_UNIT).expect("must decode");
    assert_eq!(unit.to_bytes(), RING_BURST_UNIT);
}

#[test]
fn transforms_unit_decodes_as_hand_derived() {
    let unit = SigilUnit::from_bytes(TRANSFORMS_UNIT).expect("hand-derived fixture must decode");
    assert_eq!(unit.id(), UnitId(0x5157_5049_4744_2a01));
    assert_eq!(unit.content_hash(), 0x79e1_18d6_7779_7b79);
    assert_eq!(unit.bullet_types().len(), 3);
    assert_eq!(
        unit.bullet_types()[2].flags,
        BulletFlags(BulletFlags::REFLECTABLE.0 | BulletFlags::ENV_ACTIVE.0)
    );
    assert_eq!(unit.program_count(), 2);
    assert_eq!(unit.emitter_count(), 2);
    assert_eq!(unit.behavior_refs(), &[BehaviorId(7)]);
}

#[test]
fn transforms_unit_round_trips_byte_identically() {
    let unit = SigilUnit::from_bytes(TRANSFORMS_UNIT).expect("must decode");
    assert_eq!(unit.to_bytes(), TRANSFORMS_UNIT);
}

#[test]
fn event_ids_match_the_hand_derived_fixture() {
    // Offset of the `reverse` record's `event` field: header 40, table 4 + 5 * 24, BulletTypes
    // 2 + 3 * 20, Programs 2 + 2 * 34, Emitters 2 + 2 * 30, Transforms count 2, first script head
    // 12 plus two parameters 8, the `become_emitter` record 24, then 12 bytes into `reverse`.
    let offset = 40 + 124 + 62 + 70 + 62 + 2 + 12 + 8 + 24 + 12;
    let stored = u32::from_le_bytes(TRANSFORMS_UNIT[offset..offset + 4].try_into().unwrap());
    assert_eq!(stored, 0x04c5_2141);
    assert_eq!(EventId::from_name("phase_end"), EventId(stored));
}

#[test]
fn transforms_unit_truncations_and_bit_flips_never_panic_and_never_wrongly_decode() {
    for cut in 0..TRANSFORMS_UNIT.len() {
        assert!(
            SigilUnit::from_bytes(&TRANSFORMS_UNIT[..cut]).is_err(),
            "a truncation to {cut} bytes must not decode as valid"
        );
    }
    for index in 0..TRANSFORMS_UNIT.len() {
        let mut mutated = TRANSFORMS_UNIT.to_vec();
        mutated[index] ^= 0xFF;
        let _ = SigilUnit::from_bytes(&mutated);
    }
}
