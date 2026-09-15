//! Runs the `grimoire_assets::conformance` suite (contract §2 rule 12, §12) against every
//! `AssetSource` this crate ships, plus the cross-implementation `content_hash` equality check
//! the single-source suite cannot perform on its own.

#![cfg(feature = "conformance")]

use std::sync::Arc;

use grimoire_assets::{
    AssetKind, AssetPath, AssetSource, EmptyAssetSource, MemorySource, PackReader, PackWriter,
    conformance,
};

#[test]
fn empty_source() {
    conformance::asset_source(&EmptyAssetSource);
}

#[test]
fn memory_source() {
    let mut source = MemorySource::new("conformance-memory");
    source.insert(
        &AssetPath::new("a/one.bin").unwrap(),
        AssetKind::SIGIL,
        1,
        vec![1, 2, 3],
    );
    source.insert(
        &AssetPath::new("b/two.bin").unwrap(),
        AssetKind(0x8000),
        7,
        vec![4, 5, 6, 7],
    );
    source.insert(
        &AssetPath::new("c/three.bin").unwrap(),
        AssetKind::SIGIL,
        1,
        vec![],
    );
    conformance::asset_source(&source);
}

/// Builds a `PackReader` and an equivalent `MemorySource` (same paths, kinds and bytes), so a
/// single fixture can be checked both individually and against each other.
fn sample_pack_and_memory() -> (PackReader, MemorySource) {
    let path_a = AssetPath::new("a/one.bin").unwrap();
    let path_b = AssetPath::new("b/two.bin").unwrap();

    let mut writer = PackWriter::new("conformance-writer", "0.0.0");
    writer
        .add(&path_a, AssetKind::SIGIL, 1, &[1, 2, 3])
        .unwrap();
    writer
        .add(&path_b, AssetKind(0x8000), 7, &[4, 5, 6, 7])
        .unwrap();
    let bytes = writer.finish().unwrap();
    let reader = PackReader::from_bytes(Arc::from(bytes)).unwrap();

    let mut memory = MemorySource::new("conformance-memory-equivalent");
    memory.insert(&path_a, AssetKind::SIGIL, 1, vec![1, 2, 3]);
    memory.insert(&path_b, AssetKind(0x8000), 7, vec![4, 5, 6, 7]);

    (reader, memory)
}

#[test]
fn pack_reader() {
    let (reader, _memory) = sample_pack_and_memory();
    conformance::asset_source(&reader);
}

#[test]
fn pack_reader_and_equivalent_memory_source_share_content_hash() {
    let (reader, memory) = sample_pack_and_memory();
    assert_eq!(
        reader.content_hash(),
        memory.content_hash(),
        "a PackReader and a MemorySource with the same entries must agree on content_hash"
    );
}
