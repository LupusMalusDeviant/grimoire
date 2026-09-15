//! Behaviour tests for the pack v1 format (contract §12): the golden fixture, one test per
//! `PackError` variant triggered by corrupting a valid pack at a specific byte offset,
//! `PackReader::open`'s size limit, `AssetPath` validation, and the `AssetId::from_path` golden
//! hashes.

use std::path::Path;
use std::sync::Arc;

use grimoire_assets::{
    AssetError, AssetId, AssetKind, AssetPath, AssetSource, MAX_ENTRIES, MAX_ENTRY_LEN,
    MAX_PACK_LEN, PackError, PackReader, PackWriter,
};
use grimoire_platform::{FileSystem, MemoryFileSystem};

/// The exact logical content of the golden fixture, `tests/fixtures/pack_v1_minimal.grimpack`:
/// three small entries, fixed compiler metadata and a fixed application block, so
/// [`build_minimal_pack`] is reproducible byte-for-byte.
fn minimal_pack_entries() -> Vec<(AssetPath, AssetKind, u32, Vec<u8>)> {
    vec![
        (
            AssetPath::new("sigils/basic_bolt.sigil").unwrap(),
            AssetKind::SIGIL,
            3,
            b"bolt-payload-bytes".to_vec(),
        ),
        (
            AssetPath::new("textures/enemy/core.bin").unwrap(),
            AssetKind(0x8001),
            1,
            vec![0xAAu8; 40],
        ),
        (
            AssetPath::new("audio/hit.wav").unwrap(),
            AssetKind::SIGIL,
            1,
            vec![0x00, 0x01, 0x02, 0x03],
        ),
    ]
}

fn build_minimal_pack() -> Vec<u8> {
    let mut writer = PackWriter::new("grimoire_assets-tests", "1.0.0");
    for (path, kind, kind_version, bytes) in minimal_pack_entries() {
        writer.add(&path, kind, kind_version, &bytes).unwrap();
    }
    writer.application(b"fixture-application-block".to_vec());
    writer
        .finish()
        .expect("the fixture's own content must be valid")
}

const GOLDEN_FIXTURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/pack_v1_minimal.grimpack"
);

/// Not a normal test: regenerates the golden fixture from [`build_minimal_pack`]. Run explicitly
/// (`cargo test -p grimoire_assets --all-features -- --ignored regenerate_golden_fixture`) after a
/// deliberate change to the fixture's logical content; every other test treats the checked-in
/// file as fixed.
#[test]
#[ignore = "regenerates the golden fixture on disk; run explicitly, not as part of the normal suite"]
fn regenerate_golden_fixture() {
    std::fs::write(GOLDEN_FIXTURE_PATH, build_minimal_pack()).expect("write golden fixture");
}

fn golden_fixture_bytes() -> Vec<u8> {
    std::fs::read(GOLDEN_FIXTURE_PATH)
        .expect("golden fixture must exist; run `regenerate_golden_fixture` if missing")
}

#[test]
fn golden_fixture_parses_and_matches_its_source_content() {
    let bytes = golden_fixture_bytes();
    let reader = PackReader::from_bytes(Arc::from(bytes))
        .expect("golden fixture must parse as a valid pack");

    // `reader.entries()` is sorted ascending by `AssetId`, not in the insertion order
    // `minimal_pack_entries()` lists them in; look each expected entry up by its id instead of
    // zipping the two lists positionally.
    let expected = minimal_pack_entries();
    assert_eq!(reader.entries().len(), expected.len());
    for (path, kind, kind_version, bytes) in &expected {
        let id = AssetId::from_path(path);
        let entry = reader
            .entries()
            .iter()
            .find(|entry| entry.id == id)
            .unwrap_or_else(|| panic!("no entry for {path:?} ({id})"));
        assert_eq!(entry.kind, *kind);
        assert_eq!(entry.kind_version, *kind_version);
        assert_eq!(entry.len, bytes.len() as u64);
        assert_eq!(reader.read(id).unwrap().as_ref(), bytes.as_slice());
        assert_eq!(reader.manifest().path_of(id), Some(path));
    }
    assert_eq!(reader.manifest().compiler(), "grimoire_assets-tests");
    assert_eq!(reader.manifest().compiler_version(), "1.0.0");
    assert_eq!(
        reader.manifest().application(),
        b"fixture-application-block"
    );
}

#[test]
fn golden_fixture_is_reproduced_byte_for_byte_by_the_writer() {
    let rebuilt = build_minimal_pack();
    let on_disk = golden_fixture_bytes();
    assert_eq!(
        rebuilt, on_disk,
        "PackWriter::finish() of the fixture's logical content must reproduce the checked-in \
         bytes exactly (contract §12: identical inputs, byte-identical pack, no timestamp)"
    );
}

#[test]
fn finish_is_deterministic_regardless_of_add_order() {
    let forward = build_minimal_pack();

    let mut writer = PackWriter::new("grimoire_assets-tests", "1.0.0");
    for (path, kind, kind_version, bytes) in minimal_pack_entries().into_iter().rev() {
        writer.add(&path, kind, kind_version, &bytes).unwrap();
    }
    writer.application(b"fixture-application-block".to_vec());
    let reversed = writer.finish().unwrap();

    assert_eq!(
        forward, reversed,
        "output must not depend on the order assets were added in"
    );
}

// ---------------------------------------------------------------------------------------------
// `AssetId::from_path` golden hashes: frozen so an accidental change to the hashing rule is
// caught (contract §12).
// ---------------------------------------------------------------------------------------------

#[test]
#[ignore = "prints the current AssetId::from_path hashes; used only to (re)freeze the golden consts below"]
fn print_asset_id_from_path_hashes() {
    for path in ["sigils/basic_bolt.sigil", "a", "a/b/c.txt"] {
        let id = AssetId::from_path(&AssetPath::new(path).unwrap());
        println!("{path:?} => 0x{:016x}", id.0);
    }
}

#[test]
fn asset_id_from_path_matches_frozen_golden_hashes() {
    const GOLDEN: [(&str, u64); 3] = [
        ("sigils/basic_bolt.sigil", 0xd95f_25b6_0afc_d01e),
        ("a", 0x7a9e_604f_93d3_09f6),
        ("a/b/c.txt", 0xe593_762b_ce3e_ef99),
    ];
    for (path, expected) in GOLDEN {
        let id = AssetId::from_path(&AssetPath::new(path).unwrap());
        assert_eq!(
            id,
            AssetId(expected),
            "AssetId::from_path({path:?}) changed; this is a breaking change to the hashing rule (contract §12)"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// `AssetPath::new` validation.
// ---------------------------------------------------------------------------------------------

#[test]
fn asset_path_rejects_leading_slash() {
    assert!(matches!(
        AssetPath::new("/a"),
        Err(AssetError::InvalidPath { .. })
    ));
}

#[test]
fn asset_path_rejects_trailing_slash() {
    assert!(matches!(
        AssetPath::new("a/"),
        Err(AssetError::InvalidPath { .. })
    ));
}

#[test]
fn asset_path_rejects_dot_dot_segment() {
    assert!(matches!(
        AssetPath::new("a/../b"),
        Err(AssetError::InvalidPath { .. })
    ));
}

#[test]
fn asset_path_rejects_dot_segment() {
    assert!(matches!(
        AssetPath::new("a/./b"),
        Err(AssetError::InvalidPath { .. })
    ));
}

#[test]
fn asset_path_rejects_uppercase() {
    assert!(matches!(
        AssetPath::new("Sigils/a"),
        Err(AssetError::InvalidPath { .. })
    ));
}

#[test]
fn asset_path_rejects_non_ascii() {
    assert!(matches!(
        AssetPath::new("café/a"),
        Err(AssetError::InvalidPath { .. })
    ));
}

#[test]
fn asset_path_rejects_backslash() {
    assert!(matches!(
        AssetPath::new("a\\b"),
        Err(AssetError::InvalidPath { .. })
    ));
}

#[test]
fn asset_path_rejects_empty() {
    assert!(matches!(
        AssetPath::new(""),
        Err(AssetError::InvalidPath { .. })
    ));
}

#[test]
fn asset_path_rejects_empty_segment() {
    assert!(matches!(
        AssetPath::new("a//b"),
        Err(AssetError::InvalidPath { .. })
    ));
}

// ---------------------------------------------------------------------------------------------
// `PackReader::open` size limit.
// ---------------------------------------------------------------------------------------------

#[test]
fn open_rejects_a_file_larger_than_max_pack_len() {
    let fs = MemoryFileSystem::new();
    let path = Path::new("oversized.grimpack");
    // One byte over the limit; content does not matter, the size check runs first.
    let oversized = vec![0u8; (MAX_PACK_LEN + 1) as usize];
    fs.write_atomic(path, &oversized).unwrap();

    let error = PackReader::open(&fs, path).expect_err("a file over MAX_PACK_LEN must be rejected");
    assert!(matches!(
        error,
        AssetError::TooLarge { max, .. } if max == MAX_PACK_LEN
    ));
}

#[test]
fn open_reads_a_valid_pack_at_or_under_the_limit() {
    let fs = MemoryFileSystem::new();
    let path = Path::new("ok.grimpack");
    fs.write_atomic(path, &build_minimal_pack()).unwrap();

    let reader = PackReader::open(&fs, path).expect("a valid, small pack must open");
    assert_eq!(reader.entries().len(), minimal_pack_entries().len());
}

#[test]
fn open_maps_missing_file_to_io_error() {
    let fs = MemoryFileSystem::new();
    let error =
        PackReader::open(&fs, Path::new("missing.grimpack")).expect_err("missing file must error");
    assert!(matches!(error, AssetError::Io { .. }));
}

// ---------------------------------------------------------------------------------------------
// `PackWriter` write-time validation.
// ---------------------------------------------------------------------------------------------

#[test]
fn writer_rejects_duplicate_id() {
    let mut writer = PackWriter::new("c", "1");
    let path = AssetPath::new("a").unwrap();
    writer.add(&path, AssetKind::SIGIL, 1, b"x").unwrap();
    let error = writer
        .add(&path, AssetKind::SIGIL, 1, b"y")
        .expect_err("a duplicate id must be a write error");
    assert!(matches!(error, PackError::Manifest(_)));
}

#[test]
fn writer_rejects_reserved_kind() {
    let mut writer = PackWriter::new("c", "1");
    let error = writer
        .add(&AssetPath::new("a").unwrap(), AssetKind::MESH, 1, b"x")
        .expect_err("a v1 writer must never emit a reserved kind");
    assert!(matches!(error, PackError::ReservedKind { kind: 2, .. }));
}

#[test]
fn writer_rejects_invalid_kind() {
    let mut writer = PackWriter::new("c", "1");
    let error = writer
        .add(&AssetPath::new("a").unwrap(), AssetKind(0), 1, b"x")
        .expect_err("kind 0 is always invalid");
    assert!(matches!(error, PackError::InvalidKind { kind: 0, .. }));
}

#[test]
fn writer_rejects_entry_over_max_entry_len() {
    let mut writer = PackWriter::new("c", "1");
    let oversized = vec![0u8; (MAX_ENTRY_LEN + 1) as usize];
    let error = writer
        .add(
            &AssetPath::new("a").unwrap(),
            AssetKind::SIGIL,
            1,
            &oversized,
        )
        .expect_err("a payload over MAX_ENTRY_LEN must be rejected");
    assert!(
        matches!(error, PackError::EntryTooLarge { index: 0, len } if len == MAX_ENTRY_LEN + 1)
    );
}

// ---------------------------------------------------------------------------------------------
// One test per `PackError` variant, triggered by corrupting a byte-exact offset of a valid pack.
// ---------------------------------------------------------------------------------------------

const HEADER_MAGIC: usize = 0;
const HEADER_VERSION: usize = 8;
const HEADER_LEN_FIELD: usize = 12;
const HEADER_FILE_LEN: usize = 16;
const HEADER_TOC_OFFSET: usize = 24;
const HEADER_ENTRY_COUNT: usize = 32;
const HEADER_RESERVED: usize = 56;
const TOC_START: usize = 64;
const TOC_ENTRY_LEN: usize = 64;

fn set_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn set_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn set_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
fn toc_entry(index: usize) -> usize {
    TOC_START + index * TOC_ENTRY_LEN
}

#[test]
fn bad_magic() {
    let mut bytes = build_minimal_pack();
    bytes[HEADER_MAGIC] ^= 0xFF;
    assert!(matches!(
        PackReader::from_bytes(Arc::from(bytes)),
        Err(PackError::BadMagic)
    ));
}

#[test]
fn unsupported_version() {
    let mut bytes = build_minimal_pack();
    set_u32(&mut bytes, HEADER_VERSION, 2);
    assert!(matches!(
        PackReader::from_bytes(Arc::from(bytes)),
        Err(PackError::UnsupportedVersion(2))
    ));
}

#[test]
fn wrong_header_length() {
    let mut bytes = build_minimal_pack();
    set_u32(&mut bytes, HEADER_LEN_FIELD, 63);
    assert!(matches!(
        PackReader::from_bytes(Arc::from(bytes)),
        Err(PackError::HeaderLength(63))
    ));
}

#[test]
fn wrong_file_length() {
    let mut bytes = build_minimal_pack();
    let actual = bytes.len() as u64;
    set_u64(&mut bytes, HEADER_FILE_LEN, actual + 1);
    assert!(matches!(
        PackReader::from_bytes(Arc::from(bytes)),
        Err(PackError::FileLength { declared, actual: a }) if declared == actual + 1 && a == actual
    ));
}

#[test]
fn truncated_header_is_unexpected_end() {
    let bytes = build_minimal_pack();
    let truncated = bytes[..10].to_vec();
    assert!(matches!(
        PackReader::from_bytes(Arc::from(truncated)),
        Err(PackError::UnexpectedEnd { .. })
    ));
}

#[test]
fn non_zero_header_reserved() {
    let mut bytes = build_minimal_pack();
    set_u64(&mut bytes, HEADER_RESERVED, 1);
    assert!(matches!(
        PackReader::from_bytes(Arc::from(bytes)),
        Err(PackError::NonZeroReserved { offset: 56 })
    ));
}

#[test]
fn too_many_entries() {
    let mut bytes = build_minimal_pack();
    set_u32(&mut bytes, HEADER_ENTRY_COUNT, MAX_ENTRIES + 1);
    assert!(matches!(
        PackReader::from_bytes(Arc::from(bytes)),
        Err(PackError::TooManyEntries(count)) if count == MAX_ENTRIES + 1
    ));
}

#[test]
fn bad_toc_offset_is_out_of_bounds() {
    let mut bytes = build_minimal_pack();
    set_u64(&mut bytes, HEADER_TOC_OFFSET, 128);
    assert!(matches!(
        PackReader::from_bytes(Arc::from(bytes)),
        Err(PackError::OutOfBounds {
            what: "toc_offset",
            ..
        })
    ));
}

#[test]
fn misaligned_payload_offset() {
    let mut bytes = build_minimal_pack();
    let offset_field = toc_entry(0) + 16;
    let original = u64::from_le_bytes(bytes[offset_field..offset_field + 8].try_into().unwrap());
    set_u64(&mut bytes, offset_field, original + 1);
    assert!(matches!(
        PackReader::from_bytes(Arc::from(bytes)),
        Err(PackError::Misaligned { index: 0 })
    ));
}

#[test]
fn overlapping_entries() {
    let mut bytes = build_minimal_pack();
    let entry0_offset_field = toc_entry(0) + 16;
    let entry0_offset = u64::from_le_bytes(
        bytes[entry0_offset_field..entry0_offset_field + 8]
            .try_into()
            .unwrap(),
    );
    let entry1_offset_field = toc_entry(1) + 16;
    set_u64(&mut bytes, entry1_offset_field, entry0_offset);
    assert!(matches!(
        PackReader::from_bytes(Arc::from(bytes)),
        Err(PackError::Overlap { index: 1 })
    ));
}

#[test]
fn unsorted_ids() {
    let mut bytes = build_minimal_pack();
    let entry0_id_field = toc_entry(0);
    let entry0_id = u64::from_le_bytes(
        bytes[entry0_id_field..entry0_id_field + 8]
            .try_into()
            .unwrap(),
    );
    let entry1_id_field = toc_entry(1);
    // Any id not greater than entry 0's id makes entry 1 out of order.
    set_u64(&mut bytes, entry1_id_field, entry0_id);
    assert!(matches!(
        PackReader::from_bytes(Arc::from(bytes)),
        Err(PackError::UnsortedIds { index: 1 })
    ));
}

#[test]
fn invalid_kind() {
    let mut bytes = build_minimal_pack();
    set_u16(&mut bytes, toc_entry(0) + 8, 0);
    assert!(matches!(
        PackReader::from_bytes(Arc::from(bytes)),
        Err(PackError::InvalidKind { index: 0, kind: 0 })
    ));
}

#[test]
fn reserved_kind() {
    let mut bytes = build_minimal_pack();
    set_u16(&mut bytes, toc_entry(1) + 8, 3);
    assert!(matches!(
        PackReader::from_bytes(Arc::from(bytes)),
        Err(PackError::ReservedKind { index: 1, kind: 3 })
    ));
}

#[test]
fn entry_too_large() {
    let mut bytes = build_minimal_pack();
    set_u64(&mut bytes, toc_entry(0) + 24, MAX_ENTRY_LEN + 1);
    assert!(matches!(
        PackReader::from_bytes(Arc::from(bytes)),
        Err(PackError::EntryTooLarge { index: 0, len }) if len == MAX_ENTRY_LEN + 1
    ));
}

#[test]
fn non_zero_toc_reserved() {
    let mut bytes = build_minimal_pack();
    set_u16(&mut bytes, toc_entry(0) + 10, 1);
    assert!(matches!(
        PackReader::from_bytes(Arc::from(bytes)),
        Err(PackError::NonZeroReserved { offset }) if offset == (toc_entry(0) + 10) as u64
    ));
}

#[test]
fn manifest_version_mismatch_is_a_manifest_error() {
    let mut bytes = build_minimal_pack();
    // The manifest starts right where the last entry's payload ends; `manifest_version` is its
    // first four bytes.
    let manifest_offset_field = 40;
    let manifest_offset = u64::from_le_bytes(
        bytes[manifest_offset_field..manifest_offset_field + 8]
            .try_into()
            .unwrap(),
    );
    set_u32(&mut bytes, manifest_offset as usize, 2);
    assert!(matches!(
        PackReader::from_bytes(Arc::from(bytes)),
        Err(PackError::Manifest(_))
    ));
}

#[test]
fn manifest_path_mismatch() {
    let mut bytes = build_minimal_pack();
    // Flip one ASCII byte inside the manifest's copy of the first path: it stays a syntactically
    // valid `AssetPath` (still ASCII `[a-z0-9_.-]` and `/`) but no longer hashes to entry 0's id.
    let needle = b"sigils/basic_bolt.sigil";
    let position = bytes
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("the fixture's own path bytes must appear in its manifest");
    bytes[position] = b'z';
    assert!(matches!(
        PackReader::from_bytes(Arc::from(bytes)),
        Err(PackError::ManifestMismatch { index: 0 })
    ));
}
