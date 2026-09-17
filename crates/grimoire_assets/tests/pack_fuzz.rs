//! Property tests for `PackReader::from_bytes` (contract §2 rule 9, §12): completely random
//! bytes and single-byte/truncation mutations of a valid pack must never panic, only ever return
//! `Ok` or a [`PackError`].
//!
//! These are the same entry points the future fuzz targets (WP11.4) will use.

use std::sync::Arc;

use grimoire_assets::{
    AssetId, AssetKind, AssetPath, AssetSource, MemorySource, PackReader, PackWriter,
};
use proptest::prelude::*;

/// A small, valid pack to mutate. Not the golden fixture on disk: this one is rebuilt in-process
/// so the property test has no file I/O dependency.
fn valid_pack_bytes() -> Vec<u8> {
    let mut writer = PackWriter::new("fuzz", "1");
    writer
        .add(
            &AssetPath::new("a/one").unwrap(),
            AssetKind::SIGIL,
            1,
            b"hello",
        )
        .unwrap();
    writer
        .add(
            &AssetPath::new("b/two").unwrap(),
            AssetKind(0x8000),
            2,
            b"world!!",
        )
        .unwrap();
    writer.application(b"fuzz-app-block".to_vec());
    writer.finish().unwrap()
}

proptest! {
    /// Arbitrary byte sequences of varying length, entirely unrelated to the pack format, must
    /// never panic `from_bytes` — only `Ok` or `Err` is acceptable.
    #[test]
    fn from_bytes_never_panics_on_arbitrary_bytes(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        let _ = PackReader::from_bytes(Arc::from(bytes));
    }

    /// Flipping any single byte of a valid pack must never panic `from_bytes`.
    #[test]
    fn from_bytes_never_panics_on_single_byte_mutation(index in 0usize..512, new_byte in any::<u8>()) {
        let mut bytes = valid_pack_bytes();
        if index < bytes.len() {
            bytes[index] = new_byte;
        }
        let _ = PackReader::from_bytes(Arc::from(bytes));
    }

    /// Truncating a valid pack to any shorter length must never panic `from_bytes`.
    #[test]
    fn from_bytes_never_panics_on_truncation(len in 0usize..512) {
        let bytes = valid_pack_bytes();
        let truncated = bytes[..len.min(bytes.len())].to_vec();
        let _ = PackReader::from_bytes(Arc::from(truncated));
    }
}

/// The hand-derived Sigil fixture (Plan 0002 WP8.3), a second, larger shape to mutate.
fn sigil_fixture_bytes() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/pack_v1_sigil.grimpack"
    ))
    .expect("pack_v1_sigil.grimpack")
}

/// A valid `AssetPath` of one to three segments (each starts with a letter, digit or `_`, so no
/// segment is `.` or `..`).
fn asset_path() -> impl Strategy<Value = String> {
    "[a-z0-9_]{1,8}(/[a-z0-9_][a-z0-9_.-]{0,7}){0,2}"
}

proptest! {
    /// Any single-byte change or truncation of the hand-derived Sigil fixture never panics, and
    /// an accepted mutation still reads back every entry without panicking.
    #[test]
    fn the_sigil_fixture_never_panics_under_mutation(index in 0usize..744, new_byte in any::<u8>(), cut in 0usize..=744) {
        let mut bytes = sigil_fixture_bytes();
        bytes[index] = new_byte;
        bytes.truncate(cut.max(index + 1).min(bytes.len()));
        if let Ok(reader) = PackReader::from_bytes(Arc::from(bytes)) {
            for entry in reader.entries() {
                let _ = reader.read(entry.id);
            }
        }
    }

    /// Whatever valid content the writer is given, its output parses, reads back every payload
    /// unchanged, keeps the manifest, and has the content hash of a `MemorySource` with the same
    /// content (contract §12).
    #[test]
    fn writer_output_always_reads_back(
        assets in prop::collection::vec(
            (asset_path(), prop_oneof![Just(1u16), 0x8000u16..=0xFFFF], any::<u32>(), prop::collection::vec(any::<u8>(), 0..48)),
            0..8,
        ),
        compiler in "[a-z_-]{0,64}",
        application in prop::collection::vec(any::<u8>(), 0..64),
    ) {
        let mut writer = PackWriter::new(&compiler, "1");
        let mut memory = MemorySource::new("memory");
        let mut expected = std::collections::BTreeMap::new();
        for (path, kind, kind_version, bytes) in &assets {
            let path = AssetPath::new(path).unwrap();
            let id = AssetId::from_path(&path);
            if expected.contains_key(&id) {
                continue;
            }
            writer.add(&path, AssetKind(*kind), *kind_version, bytes).unwrap();
            memory.insert(&path, AssetKind(*kind), *kind_version, bytes.clone());
            expected.insert(id, (path, bytes.clone()));
        }
        writer.application(application.clone());
        let bytes = writer.finish().unwrap();

        let reader = PackReader::from_bytes(Arc::from(bytes)).unwrap();
        prop_assert_eq!(reader.entries().len(), expected.len());
        for (id, (path, payload)) in &expected {
            let read = reader.read(*id).unwrap();
            prop_assert_eq!(read.as_ref(), payload.as_slice());
            prop_assert_eq!(reader.manifest().path_of(*id), Some(path));
        }
        prop_assert_eq!(reader.manifest().compiler(), compiler.as_str());
        prop_assert_eq!(reader.manifest().application(), application.as_slice());
        prop_assert_eq!(reader.content_hash(), memory.content_hash());
    }
}
