//! Behaviour tests for the generated pack v1 manifest codec (contract §12, project ADR-0011,
//! Plan-0002 WP8.1): round trips, every-truncation-fails, an oversized declared count/length
//! that must fail before it would allocate, `decode` never panicking on arbitrary bytes, and a
//! cross-check against the existing, hand-written `PackReader`/`PackWriter`'s own golden fixture
//! (`tests/fixtures/pack_v1_minimal.grimpack`, already trusted by `tests/pack_format.rs`) proving
//! the generated codec parses and reproduces that exact manifest byte-for-byte, without this
//! generated type being wired into `PackReader`/`PackWriter` itself (see
//! `schema/pack_manifest_v1.gschema`'s header comment and `grimoire_assets::generated`'s module
//! docs for why that wiring is deferred to Plan-0002 WP8.3).

use std::sync::Arc;

use grimoire_assets::{AssetSource, PackManifestBody, PackManifestV1Error, PackReader};
use proptest::prelude::*;

// --- A sample manifest body, and the existing golden pack fixture's own manifest --------------

fn sample_manifest() -> PackManifestBody {
    PackManifestBody {
        manifest_version: 1,
        compiler: "sigilc".to_owned(),
        compiler_version: "0.3.1".to_owned(),
        entry_count: 2,
        paths: vec![
            "sigils/basic_bolt.sigil".to_owned(),
            "textures/enemy/core.bin".to_owned(),
        ],
        application: b"fixture-application-block".to_vec(),
    }
}

const GOLDEN_PACK_FIXTURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/pack_v1_minimal.grimpack"
);

/// Slices the manifest section out of a whole pack v1 file, reading only the two header fields
/// needed to find it (contract §12 header layout: `manifest_offset` is the `u64` at byte offset
/// 40, `manifest_len` the `u64` at byte offset 48 — both little-endian). Deliberately
/// independent of [`PackReader`] so the two parsers (hand-written header/TOC parsing here, the
/// generated codec on the slice it produces) do not share a bug.
fn manifest_slice(pack_bytes: &[u8]) -> &[u8] {
    let manifest_offset = u64::from_le_bytes(pack_bytes[40..48].try_into().unwrap()) as usize;
    let manifest_len = u64::from_le_bytes(pack_bytes[48..56].try_into().unwrap()) as usize;
    &pack_bytes[manifest_offset..manifest_offset + manifest_len]
}

/// Cross-checks the generated codec against `PackReader`, an independent, already-tested
/// hand-written parser of the very same bytes (`tests/pack_format.rs`'s golden fixture).
#[test]
fn generated_codec_matches_the_existing_pack_v1_minimal_fixture_manifest() {
    let pack_bytes = std::fs::read(GOLDEN_PACK_FIXTURE_PATH)
        .expect("tests/pack_format.rs's golden fixture must exist");
    let manifest_bytes = manifest_slice(&pack_bytes);

    let body = PackManifestBody::decode(manifest_bytes)
        .expect("the fixture's manifest section must decode with the generated codec");

    let reader = PackReader::from_bytes(Arc::from(pack_bytes.clone()))
        .expect("the fixture must still parse with the hand-written PackReader");

    assert_eq!(body.manifest_version, 1);
    assert_eq!(body.compiler, reader.manifest().compiler());
    assert_eq!(body.compiler_version, reader.manifest().compiler_version());
    assert_eq!(body.entry_count as usize, reader.entries().len());
    assert_eq!(body.paths.len(), reader.entries().len());
    for (path, entry) in body.paths.iter().zip(reader.entries()) {
        let expected = reader
            .manifest()
            .path_of(entry.id)
            .expect("every TOC entry has a manifest path");
        assert_eq!(path, expected.as_str());
    }
    assert_eq!(body.application, reader.manifest().application());
}

/// The other direction: encoding the decoded body must reproduce the exact manifest bytes the
/// hand-written `PackWriter` already produced — the byte-for-byte parity project ADR-0011
/// requires before this codec could ever be swapped in (Plan-0002 WP8.3).
#[test]
fn generated_codec_reproduces_the_fixtures_manifest_bytes_exactly() {
    let pack_bytes = std::fs::read(GOLDEN_PACK_FIXTURE_PATH)
        .expect("tests/pack_format.rs's golden fixture must exist");
    let manifest_bytes = manifest_slice(&pack_bytes).to_vec();

    let body = PackManifestBody::decode(&manifest_bytes).expect("must decode");
    let mut re_encoded = Vec::new();
    body.encode(&mut re_encoded).expect("must encode");

    assert_eq!(re_encoded, manifest_bytes);
}

// --- Round trip, truncation, oversized count/length, proptest ---------------------------------

#[test]
fn round_trip() {
    let value = sample_manifest();
    let mut bytes = Vec::new();
    value.encode(&mut bytes).expect("encode must succeed");
    let decoded = PackManifestBody::decode(&bytes).expect("decode must succeed");
    assert_eq!(decoded, value);
}

#[test]
fn every_truncation_fails() {
    let value = sample_manifest();
    let mut full = Vec::new();
    value.encode(&mut full).expect("encode must succeed");
    for len in 0..full.len() {
        let prefix = &full[..len];
        assert!(
            PackManifestBody::decode(prefix).is_err(),
            "a {len}-byte prefix must not decode successfully"
        );
    }
}

#[test]
fn oversized_paths_count_fails_before_allocating() {
    // `paths` has no wire count of its own (`vec_using`): its count is `entry_count`, so an
    // oversized `entry_count` (contract §12 `MAX_ENTRIES` is 65_536) must fail via
    // `FieldTooLong` before attempting to reserve room for billions of `String`s (contract §2
    // rule 9) — no further bytes are needed for the failure to occur.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&1u32.to_le_bytes()); // manifest_version
    bytes.extend_from_slice(&3u16.to_le_bytes()); // compiler: Str16 length
    bytes.extend_from_slice(b"abc");
    bytes.extend_from_slice(&3u16.to_le_bytes()); // compiler_version: Str16 length
    bytes.extend_from_slice(b"1.0");
    bytes.extend_from_slice(&u32::MAX.to_le_bytes()); // entry_count: also paths' count

    let error = PackManifestBody::decode(&bytes).unwrap_err();
    assert_eq!(
        error,
        PackManifestV1Error::FieldTooLong {
            field: "paths",
            len: u32::MAX as usize,
            max: 65_536,
        }
    );
}

#[test]
fn oversized_application_length_fails_before_allocating() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&1u32.to_le_bytes()); // manifest_version
    bytes.extend_from_slice(&0u16.to_le_bytes()); // compiler: empty
    bytes.extend_from_slice(&0u16.to_le_bytes()); // compiler_version: empty
    bytes.extend_from_slice(&0u32.to_le_bytes()); // entry_count = 0 (also paths' count)
    bytes.extend_from_slice(&u32::MAX.to_le_bytes()); // application: oversized length

    let error = PackManifestBody::decode(&bytes).unwrap_err();
    assert_eq!(
        error,
        PackManifestV1Error::FieldTooLong {
            field: "application",
            len: u32::MAX as usize,
            max: 65_536,
        }
    );
}

proptest! {
    #[test]
    fn decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = PackManifestBody::decode(&bytes);
    }

    /// Mutating a single byte of a valid encoding must never panic either (contract §2 rule 9).
    #[test]
    fn decode_never_panics_on_single_byte_mutations(
        index in 0usize..200,
        replacement in any::<u8>(),
    ) {
        let mut bytes = Vec::new();
        sample_manifest().encode(&mut bytes).unwrap();
        if index < bytes.len() {
            bytes[index] = replacement;
        }
        let _ = PackManifestBody::decode(&bytes);
    }
}
