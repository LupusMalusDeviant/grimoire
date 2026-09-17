//! Plan 0002 WP8.3: Sigil units from a pack v1 file into the running simulation through
//! `grimoire::adapters::assets` (contract §9.1, §9.11, §11.2, §12).
//!
//! The pack is written with `PackWriter` around the facade's checked-in, `sigilc`-compiled unit
//! fixture, read back with `PackReader` and turned into a `SigilLibrary`; the library must be the
//! same content (same epoch) as one built from the unit bytes directly, and the installed pattern
//! must fire. Every failure of `sigil_units` is triggered once; `SigilAssetsError::Library` only
//! forwards `SigilLibrary::new`, whose own errors `grimoire_sigil` tests.

use std::sync::Arc;

use grimoire::adapters::assets::{SigilAssetsError, sigil_library, sigil_units};
use grimoire::assets::{
    AssetError, AssetId, AssetKind, AssetPath, AssetSource, EmptyAssetSource, MemorySource,
    PackReader, PackWriter,
};
use grimoire::core::Vec2;
use grimoire::sigil::{
    BehaviorRegistry, BehaviorRegistryBuilder, BulletPool, Emitter, SigilConfig, SigilLibrary,
    SigilUnit, install,
};
use grimoire::sim::{Simulation, TickInput};

const SHOWCASE_UNIT: &[u8] = include_bytes!("fixtures/bullet_showcase_unit_v1.bin");
/// The canonical content path `sigilc` compiled the fixture under (its `UnitId` derives from it).
const SHOWCASE_PATH: &str = "fixtures/bullet_showcase.sigil";

fn registry() -> Arc<BehaviorRegistry> {
    BehaviorRegistryBuilder::new(1).build()
}

fn path(text: &str) -> AssetPath {
    AssetPath::new(text).expect("valid asset path")
}

/// A pack with the showcase unit, one application-defined entry and an application block.
fn showcase_pack() -> Vec<u8> {
    let mut writer = PackWriter::new("sigil-assets-test", "1");
    writer
        .add(&path(SHOWCASE_PATH), AssetKind::SIGIL, 1, SHOWCASE_UNIT)
        .unwrap();
    writer
        .add(
            &path("notes/readme.txt"),
            AssetKind(0x8000),
            1,
            b"not a unit",
        )
        .unwrap();
    writer.application(b"game 0.1".to_vec());
    writer.finish().unwrap()
}

#[test]
fn a_packed_unit_becomes_the_same_library_and_its_pattern_fires() {
    let reader = PackReader::from_bytes(Arc::from(showcase_pack())).expect("pack parses");
    let registry = registry();
    let library = sigil_library(&reader, Arc::clone(&registry)).expect("library builds");

    let direct = SigilLibrary::new(
        vec![SigilUnit::from_bytes(SHOWCASE_UNIT).unwrap()],
        Arc::clone(&registry),
    )
    .unwrap();
    assert_eq!(library.units().len(), 1, "only the SIGIL entry is a unit");
    assert_eq!(library.epoch(), direct.epoch(), "same content, same epoch");

    let unit = &library.units()[0];
    assert_eq!(unit.id().0, AssetId::from_path(&path(SHOWCASE_PATH)).0);
    let (unit_id, emitters) = (unit.id(), unit.emitter_count());

    let mut sim = Simulation::new(7);
    install(
        &mut sim,
        library,
        registry,
        SigilConfig::new(4096, Vec2::new(-40.0, -40.0), Vec2::new(40.0, 40.0)),
    )
    .expect("install succeeds");
    for emitter in 0..emitters {
        sim.world_mut().spawn((Emitter {
            unit: unit_id,
            emitter,
            origin: Vec2::ZERO,
            rotation: 0.0,
            started_at: 0,
        },));
    }
    for _ in 0..30 {
        sim.step(TickInput::default());
    }
    let live = sim.world().resource::<BulletPool>().unwrap().len();
    assert!(live > 0, "the pattern loaded from the pack fires");
}

#[test]
fn a_source_without_sigil_entries_is_an_empty_library() {
    assert!(sigil_units(&EmptyAssetSource).unwrap().is_empty());
    let mut memory = MemorySource::new("memory");
    memory.insert(&path("a.bin"), AssetKind(0x8001), 1, vec![1, 2, 3]);
    let library = sigil_library(&memory, registry()).unwrap();
    assert!(library.units().is_empty());
}

#[test]
fn another_kind_version_is_rejected_before_decoding() {
    let mut memory = MemorySource::new("memory");
    let id = memory.insert(
        &path(SHOWCASE_PATH),
        AssetKind::SIGIL,
        2,
        SHOWCASE_UNIT.to_vec(),
    );
    assert!(matches!(
        sigil_units(&memory),
        Err(SigilAssetsError::UnsupportedUnitVersion { id: found, kind_version: 2, expected: 1 }) if found == id
    ));
}

#[test]
fn bytes_that_are_not_a_unit_are_a_unit_error() {
    let mut memory = MemorySource::new("memory");
    let id = memory.insert(
        &path("broken.sigil"),
        AssetKind::SIGIL,
        1,
        b"GRIMSIGL".to_vec(),
    );
    assert!(matches!(
        sigil_units(&memory),
        Err(SigilAssetsError::Unit { id: found, .. }) if found == id
    ));
}

#[test]
fn a_unit_packed_under_another_path_is_a_unit_id_mismatch() {
    let mut memory = MemorySource::new("memory");
    let id = memory.insert(
        &path("elsewhere/showcase.sigil"),
        AssetKind::SIGIL,
        1,
        SHOWCASE_UNIT.to_vec(),
    );
    assert!(matches!(
        sigil_units(&memory),
        Err(SigilAssetsError::UnitIdMismatch { id: found, unit }) if found == id && unit.0 == AssetId::from_path(&path(SHOWCASE_PATH)).0
    ));
}

#[test]
fn a_corrupted_payload_is_a_read_error_with_the_hash_mismatch() {
    let mut bytes = showcase_pack();
    // Flip the last byte of the unit payload: the pack structure stays valid, the SHA-256 does not.
    let reader = PackReader::from_bytes(Arc::from(bytes.clone())).unwrap();
    let id = AssetId::from_path(&path(SHOWCASE_PATH));
    let payload = reader.read(id).unwrap();
    let start = bytes
        .windows(payload.len())
        .position(|window| window == payload.as_ref())
        .unwrap();
    let last = start + payload.len() - 1;
    bytes[last] ^= 0xFF;

    let corrupted = PackReader::from_bytes(Arc::from(bytes)).expect("structure still valid");
    assert!(matches!(
        sigil_units(&corrupted),
        Err(SigilAssetsError::Read { id: found, source: AssetError::HashMismatch(_) }) if found == id
    ));
}
