//! Replay binary format version 2 (contract §8.1): round trips, hostile input and the checked-in
//! golden fixtures under `tests/fixtures/`.

use grimoire_sim::{
    BuildHash, ContentManifestHash, InputFrame, InputLog, MAX_APP_KEY_BYTES, MAX_APP_METADATA,
    MAX_APP_VALUE_BYTES, MAX_ENGINE_VERSION_BYTES, MAX_INPUT_SLOTS, MAX_SWAP_RECORDS, Replay,
    ReplayHeader, SimError, SwapRecord, TickInput,
};
use proptest::prelude::*;

/// `engine_version` of the golden v2 fixtures and of the sample replays built like them. Fixed on
/// purpose: the fixtures pin the byte format, not the version of the day, so a release that raises
/// `ENGINE_VERSION` leaves them untouched (PO decision 2026-09-17).
const FIXTURE_ENGINE_VERSION: &str = "0.4.0";

/// A v2 header with the fixed fixture identity: [`FIXTURE_ENGINE_VERSION`], an unknown build,
/// `content_manifest`, no swaps and no metadata. Never `ENGINE_VERSION` or `ENGINE_BUILD`, which
/// change with every release and with `GRIMOIRE_BUILD_HASH`.
fn fixture_header(content_manifest: ContentManifestHash) -> ReplayHeader {
    let mut header = ReplayHeader::for_this_build(content_manifest);
    header.engine_version = FIXTURE_ENGINE_VERSION.to_string();
    header.engine_build = BuildHash::UNKNOWN;
    header
}

fn sample_log(frame_count: usize) -> InputLog {
    let frames = (0..frame_count)
        .map(|tick| {
            let mut input = TickInput::default();
            input.slots[0] = InputFrame {
                axes: [tick as i16, -(tick as i16), 1, -1],
                buttons: tick as u32,
            };
            input
        })
        .collect();
    InputLog {
        seed: 0x1234_5678_9abc_def0,
        tick_rate_hz: 60,
        frames,
    }
}

fn v1_replay() -> Replay {
    Replay {
        header: None,
        log: sample_log(4),
    }
}

fn minimal_replay() -> Replay {
    Replay {
        header: Some(fixture_header(ContentManifestHash::EMPTY)),
        log: sample_log(3),
    }
}

fn full_replay() -> Replay {
    let mut header = fixture_header(ContentManifestHash(0x0011_2233_4455_6677));
    header.engine_build = BuildHash([0xab; 20]);
    header.swaps = vec![
        SwapRecord::new(2, ContentManifestHash(0xaa)),
        SwapRecord::new(4, ContentManifestHash(0xbb)),
    ];
    header
        .app_metadata
        .insert("app.name".to_string(), "grimoire-harness".to_string());
    header
        .app_metadata
        .insert("app.version".to_string(), "0.1.1".to_string());
    Replay {
        header: Some(header),
        log: sample_log(5),
    }
}

// --- golden fixtures ------------------------------------------------------------------------
//
// Real, checked-in binary fixtures. `include_bytes!` runs at compile time, so the files must
// already exist before this integration test binary can even compile; the helper that generates
// them therefore lives in `src/replay.rs`'s own `#[cfg(test)]` module (an ignored unit test,
// `cargo test -p grimoire_sim --lib -- --ignored regenerate_fixtures`), not here.

const V1_FIXTURE: &[u8] = include_bytes!("fixtures/replay_v1.bin");
const V2_MINIMAL_FIXTURE: &[u8] = include_bytes!("fixtures/replay_v2_minimal.bin");
const V2_FULL_FIXTURE: &[u8] = include_bytes!("fixtures/replay_v2_full.bin");

#[test]
fn v1_fixture_decodes_to_header_none_and_matches_input_log_from_bytes() {
    let replay = Replay::from_bytes(V1_FIXTURE).expect("v1 fixture decodes");
    assert_eq!(replay.header, None);
    assert_eq!(
        replay.log,
        InputLog::from_bytes(V1_FIXTURE).expect("v1 fixture is also a valid plain InputLog")
    );
    assert_eq!(replay, v1_replay());
}

#[test]
fn v2_minimal_fixture_round_trips() {
    let replay = Replay::from_bytes(V2_MINIMAL_FIXTURE).expect("v2 minimal fixture decodes");
    assert_eq!(replay, minimal_replay());
    let header = replay.header.as_ref().expect("v2 replay has a header");
    assert!(header.swaps.is_empty());
    assert!(header.app_metadata.is_empty());
    assert!(header.is_golden_eligible());
    assert_eq!(replay.to_bytes().expect("re-encodes"), V2_MINIMAL_FIXTURE);
}

#[test]
fn v2_full_fixture_round_trips_with_swaps_and_metadata() {
    let replay = Replay::from_bytes(V2_FULL_FIXTURE).expect("v2 full fixture decodes");
    assert_eq!(replay, full_replay());
    let header = replay.header.as_ref().expect("v2 replay has a header");
    assert!(!header.swaps.is_empty());
    assert!(!header.app_metadata.is_empty());
    assert!(!header.is_golden_eligible());
    assert_eq!(replay.to_bytes().expect("re-encodes"), V2_FULL_FIXTURE);
}

/// Appends `frames` sample frames (the values of `sample_log`) in the frame encoding, written out
/// by hand: slot 0 carries the axes `tick, -tick, 1, -1` and the buttons `tick`, slots 1 to 3 are
/// zero, each slot four `i16` axes and one `u32` button mask.
fn hand_frames(bytes: &mut Vec<u8>, frames: u32) {
    for tick in 0..frames {
        let axis = tick as i16;
        for value in [axis, -axis, 1, -1] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&tick.to_le_bytes());
        bytes.extend_from_slice(&[0; 12 * (MAX_INPUT_SLOTS - 1)]);
    }
}

/// The start of a fixture's v2 header written out by hand from the layout table: seed, tick rate,
/// engine version, build hash and content manifest; the caller appends swaps and metadata.
fn hand_header_start(build: [u8; 20], content_manifest: u64) -> Vec<u8> {
    let mut head = Vec::new();
    head.extend_from_slice(&0x1234_5678_9abc_def0_u64.to_le_bytes());
    head.extend_from_slice(&60u32.to_le_bytes());
    head.push(5);
    head.extend_from_slice(b"0.4.0");
    head.extend_from_slice(&build);
    head.extend_from_slice(&content_manifest.to_le_bytes());
    head
}

/// Magic, version 2, `header_len`, the header, `frame_count` and the frames.
fn hand_v2(head: &[u8], frames: u32) -> Vec<u8> {
    let mut bytes = b"GRIMREPL".to_vec();
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&(head.len() as u32).to_le_bytes());
    bytes.extend_from_slice(head);
    bytes.extend_from_slice(&u64::from(frames).to_le_bytes());
    hand_frames(&mut bytes, frames);
    bytes
}

/// The checked-in fixtures, written out byte by byte from the layout in `docs/formats/replay.md`
/// without `Replay::to_bytes` (contract §2 rule 10, project ADR-0011), with the lengths the format
/// document derives: 224, 220 and 394 bytes, `header_len` 52 and 130.
#[test]
fn fixtures_match_their_hand_derived_bytes() {
    let mut v1 = b"GRIMREPL".to_vec();
    v1.extend_from_slice(&1u32.to_le_bytes());
    v1.extend_from_slice(&0x1234_5678_9abc_def0_u64.to_le_bytes());
    v1.extend_from_slice(&60u32.to_le_bytes());
    v1.extend_from_slice(&4u64.to_le_bytes());
    hand_frames(&mut v1, 4);
    assert_eq!(v1.len(), 224);
    assert_eq!(V1_FIXTURE, v1.as_slice(), "replay_v1.bin");

    let mut minimal_head = hand_header_start([0; 20], 0);
    minimal_head.extend_from_slice(&0u32.to_le_bytes()); // swap_count
    minimal_head.extend_from_slice(&0u16.to_le_bytes()); // meta_count
    assert_eq!(minimal_head.len(), 52);
    let minimal = hand_v2(&minimal_head, 3);
    assert_eq!(minimal.len(), 220);
    assert_eq!(
        V2_MINIMAL_FIXTURE,
        minimal.as_slice(),
        "replay_v2_minimal.bin"
    );

    let mut full_head = hand_header_start([0xab; 20], 0x0011_2233_4455_6677);
    full_head.extend_from_slice(&2u32.to_le_bytes()); // swap_count
    for (tick, manifest) in [(2u64, 0xaau64), (4, 0xbb)] {
        full_head.extend_from_slice(&tick.to_le_bytes());
        full_head.extend_from_slice(&manifest.to_le_bytes());
    }
    full_head.extend_from_slice(&2u16.to_le_bytes()); // meta_count, keys ascending
    for (key, value) in [("app.name", "grimoire-harness"), ("app.version", "0.1.1")] {
        full_head.push(key.len() as u8);
        full_head.extend_from_slice(key.as_bytes());
        full_head.extend_from_slice(&(value.len() as u16).to_le_bytes());
        full_head.extend_from_slice(value.as_bytes());
    }
    assert_eq!(full_head.len(), 130);
    let full = hand_v2(&full_head, 5);
    assert_eq!(full.len(), 394);
    assert_eq!(V2_FULL_FIXTURE, full.as_slice(), "replay_v2_full.bin");
}

// --- round trips and byte-identity with InputLog --------------------------------------------

#[test]
fn header_none_to_bytes_is_byte_identical_to_input_log_to_bytes() {
    let log = sample_log(6);
    let replay = Replay {
        header: None,
        log: log.clone(),
    };
    assert_eq!(replay.to_bytes().unwrap(), log.to_bytes());
}

#[test]
fn zero_tick_rate_is_an_error_not_a_panic_for_both_header_variants() {
    let mut log = sample_log(2);
    log.tick_rate_hz = 0;
    assert_eq!(
        Replay {
            header: None,
            log: log.clone(),
        }
        .to_bytes(),
        Err(SimError::InvalidTickRate)
    );
    let header = ReplayHeader::for_this_build(ContentManifestHash::EMPTY);
    assert_eq!(
        Replay {
            header: Some(header),
            log,
        }
        .to_bytes(),
        Err(SimError::InvalidTickRate)
    );
}

#[test]
fn every_truncation_of_a_valid_v2_replay_fails() {
    let replay = full_replay();
    let bytes = replay.to_bytes().unwrap();
    for length in 0..bytes.len() {
        assert!(
            Replay::from_bytes(&bytes[..length]).is_err(),
            "length {length} unexpectedly decoded"
        );
    }
    assert_eq!(Replay::from_bytes(&bytes), Ok(replay));

    let mut trailing = bytes;
    trailing.push(0);
    assert!(Replay::from_bytes(&trailing).is_err());
}

#[test]
fn unsupported_version_is_rejected() {
    let mut bytes = minimal_replay().to_bytes().unwrap();
    bytes[8..12].copy_from_slice(&3u32.to_le_bytes());
    assert_eq!(
        Replay::from_bytes(&bytes),
        Err(SimError::UnsupportedVersion(3))
    );
}

#[test]
fn bad_magic_is_rejected() {
    let mut bytes = minimal_replay().to_bytes().unwrap();
    bytes[0] = b'X';
    assert_eq!(Replay::from_bytes(&bytes), Err(SimError::BadMagic));
}

// --- header_len ------------------------------------------------------------------------------

#[test]
fn header_length_mismatch_is_rejected() {
    let bytes = full_replay().to_bytes().unwrap();
    let declared = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
    let mut corrupted = bytes;
    corrupted[12..16].copy_from_slice(&(declared - 1).to_le_bytes());
    assert!(matches!(
        Replay::from_bytes(&corrupted),
        Err(SimError::HeaderLength { .. })
    ));
}

#[test]
fn header_length_exceeding_the_remaining_input_is_rejected() {
    let bytes = minimal_replay().to_bytes().unwrap();
    let expected_consumed = bytes.len() - 16;
    let mut corrupted = bytes;
    corrupted[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        Replay::from_bytes(&corrupted),
        Err(SimError::HeaderLength {
            declared: u32::MAX,
            consumed: expected_consumed,
        })
    );
}

// --- oversized counts, checked before allocation ---------------------------------------------

/// Byte offset of the `swap_count` field in a header built by `fixture_header` with no swaps or
/// metadata yet: magic(8) + version(4) + header_len(4) + seed(8) + tick_rate_hz(4) +
/// engine_version(1 + len) + engine_build(20) + content_manifest(8).
fn swap_count_offset() -> usize {
    8 + 4 + 4 + 8 + 4 + (1 + FIXTURE_ENGINE_VERSION.len()) + 20 + 8
}

#[test]
fn huge_swap_count_is_rejected_without_reading_any_swap_entry() {
    let bytes = minimal_replay().to_bytes().unwrap();
    let offset = swap_count_offset();
    assert_eq!(
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()),
        0,
        "the minimal replay has no swaps yet"
    );
    let mut corrupted = bytes;
    corrupted[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        Replay::from_bytes(&corrupted),
        Err(SimError::TooManyEntries {
            field: "swaps",
            count: u64::from(u32::MAX),
            max: MAX_SWAP_RECORDS,
        })
    );
}

#[test]
fn huge_frame_count_is_rejected_without_allocating() {
    let bytes = full_replay().to_bytes().unwrap();
    let header_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
    let frame_count_offset = 16 + header_len as usize;
    let mut corrupted = bytes.clone();
    for declared in [u64::MAX, u64::MAX / 48 + 1, 1 << 40] {
        corrupted[frame_count_offset..frame_count_offset + 8]
            .copy_from_slice(&declared.to_le_bytes());
        let remaining = bytes.len() - frame_count_offset - 8;
        assert_eq!(
            Replay::from_bytes(&corrupted),
            Err(SimError::FrameDataLength {
                frames: declared,
                remaining,
            })
        );
    }
}

// --- every maximum + 1 ------------------------------------------------------------------------

#[test]
fn empty_engine_version_is_rejected() {
    let mut header = ReplayHeader::for_this_build(ContentManifestHash::EMPTY);
    header.engine_version = String::new();
    let replay = Replay {
        header: Some(header),
        log: sample_log(1),
    };
    assert_eq!(
        replay.to_bytes(),
        Err(SimError::InvalidText {
            field: "engine_version"
        })
    );
}

#[test]
fn engine_version_longer_than_the_maximum_is_rejected() {
    let mut header = ReplayHeader::for_this_build(ContentManifestHash::EMPTY);
    header.engine_version = "0".repeat(MAX_ENGINE_VERSION_BYTES + 1);
    let replay = Replay {
        header: Some(header),
        log: sample_log(1),
    };
    assert_eq!(
        replay.to_bytes(),
        Err(SimError::FieldTooLong {
            field: "engine_version",
            len: MAX_ENGINE_VERSION_BYTES + 1,
            max: MAX_ENGINE_VERSION_BYTES,
        })
    );
}

#[test]
fn engine_version_outside_its_charset_is_rejected() {
    let mut header = ReplayHeader::for_this_build(ContentManifestHash::EMPTY);
    header.engine_version = "1.0 beta".to_string();
    let replay = Replay {
        header: Some(header),
        log: sample_log(1),
    };
    assert_eq!(
        replay.to_bytes(),
        Err(SimError::InvalidText {
            field: "engine_version"
        })
    );
}

#[test]
fn too_many_swap_records_is_rejected() {
    let mut header = ReplayHeader::for_this_build(ContentManifestHash::EMPTY);
    header.swaps = (0..=MAX_SWAP_RECORDS as u64)
        .map(|tick| SwapRecord::new(tick + 1, ContentManifestHash::EMPTY))
        .collect();
    let count = header.swaps.len() as u64;
    let replay = Replay {
        header: Some(header),
        log: sample_log(0),
    };
    assert_eq!(
        replay.to_bytes(),
        Err(SimError::TooManyEntries {
            field: "swaps",
            count,
            max: MAX_SWAP_RECORDS,
        })
    );
}

#[test]
fn too_many_metadata_entries_is_rejected() {
    let mut header = ReplayHeader::for_this_build(ContentManifestHash::EMPTY);
    for index in 0..=MAX_APP_METADATA {
        header
            .app_metadata
            .insert(format!("k{index:04}"), "v".to_string());
    }
    let count = header.app_metadata.len() as u64;
    let replay = Replay {
        header: Some(header),
        log: sample_log(1),
    };
    assert_eq!(
        replay.to_bytes(),
        Err(SimError::TooManyEntries {
            field: "app_metadata",
            count,
            max: MAX_APP_METADATA,
        })
    );
}

#[test]
fn metadata_key_longer_than_the_maximum_is_rejected() {
    let mut header = ReplayHeader::for_this_build(ContentManifestHash::EMPTY);
    header
        .app_metadata
        .insert("a".repeat(MAX_APP_KEY_BYTES + 1), "v".to_string());
    let replay = Replay {
        header: Some(header),
        log: sample_log(1),
    };
    assert_eq!(
        replay.to_bytes(),
        Err(SimError::FieldTooLong {
            field: "app_metadata.key",
            len: MAX_APP_KEY_BYTES + 1,
            max: MAX_APP_KEY_BYTES,
        })
    );
}

#[test]
fn metadata_value_longer_than_the_maximum_is_rejected() {
    let mut header = ReplayHeader::for_this_build(ContentManifestHash::EMPTY);
    header
        .app_metadata
        .insert("key".to_string(), "v".repeat(MAX_APP_VALUE_BYTES + 1));
    let replay = Replay {
        header: Some(header),
        log: sample_log(1),
    };
    assert_eq!(
        replay.to_bytes(),
        Err(SimError::FieldTooLong {
            field: "app_metadata.value",
            len: MAX_APP_VALUE_BYTES + 1,
            max: MAX_APP_VALUE_BYTES,
        })
    );
}

#[test]
fn metadata_key_outside_its_charset_is_rejected() {
    let mut header = ReplayHeader::for_this_build(ContentManifestHash::EMPTY);
    header
        .app_metadata
        .insert("Bad-Key".to_string(), "v".to_string());
    let replay = Replay {
        header: Some(header),
        log: sample_log(1),
    };
    assert_eq!(
        replay.to_bytes(),
        Err(SimError::InvalidText {
            field: "app_metadata.key"
        })
    );
}

// --- swap ordering and range -------------------------------------------------------------------

#[test]
fn swap_ticks_out_of_order_are_rejected() {
    let mut header = ReplayHeader::for_this_build(ContentManifestHash::EMPTY);
    header.swaps = vec![
        SwapRecord::new(3, ContentManifestHash::EMPTY),
        SwapRecord::new(2, ContentManifestHash::EMPTY),
    ];
    let replay = Replay {
        header: Some(header),
        log: sample_log(5),
    };
    assert_eq!(replay.to_bytes(), Err(SimError::SwapOrder { index: 1 }));
}

#[test]
fn duplicate_swap_ticks_are_rejected() {
    let mut header = ReplayHeader::for_this_build(ContentManifestHash::EMPTY);
    header.swaps = vec![
        SwapRecord::new(2, ContentManifestHash::EMPTY),
        SwapRecord::new(2, ContentManifestHash::EMPTY),
    ];
    let replay = Replay {
        header: Some(header),
        log: sample_log(5),
    };
    assert_eq!(replay.to_bytes(), Err(SimError::SwapOrder { index: 1 }));
}

#[test]
fn swap_beyond_the_recorded_frames_is_rejected() {
    let mut header = ReplayHeader::for_this_build(ContentManifestHash::EMPTY);
    header.swaps = vec![SwapRecord::new(10, ContentManifestHash::EMPTY)];
    let replay = Replay {
        header: Some(header),
        log: sample_log(5),
    };
    assert_eq!(
        replay.to_bytes(),
        Err(SimError::SwapOutOfRange {
            tick: 10,
            frames: 5
        })
    );
}

#[test]
fn swap_tick_equal_to_the_frame_count_is_allowed() {
    let mut header = ReplayHeader::for_this_build(ContentManifestHash::EMPTY);
    header.swaps = vec![SwapRecord::new(5, ContentManifestHash::EMPTY)];
    let replay = Replay {
        header: Some(header),
        log: sample_log(5),
    };
    assert!(replay.to_bytes().is_ok());
}

// --- BuildHash / ContentManifestHash ------------------------------------------------------------

#[test]
fn build_hash_round_trips_through_the_v2_header_known_and_unknown() {
    for engine_build in [BuildHash::UNKNOWN, BuildHash([0x42; 20])] {
        let mut header = ReplayHeader::for_this_build(ContentManifestHash::EMPTY);
        header.engine_build = engine_build;
        let replay = Replay {
            header: Some(header),
            log: sample_log(1),
        };
        let bytes = replay.to_bytes().unwrap();
        let decoded = Replay::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.header.unwrap().engine_build, engine_build);
    }
}

// --- proptests -----------------------------------------------------------------------------

fn arbitrary_frame() -> impl Strategy<Value = TickInput> {
    prop::collection::vec(any::<(i16, i16, i16, i16, u32)>(), MAX_INPUT_SLOTS).prop_map(|slots| {
        let mut input = TickInput::default();
        for (slot, (a, b, c, d, buttons)) in input.slots.iter_mut().zip(slots) {
            *slot = InputFrame {
                axes: [a, b, c, d],
                buttons,
            };
        }
        input
    })
}

fn arbitrary_engine_version() -> impl Strategy<Value = String> {
    "[0-9A-Za-z.+-]{1,20}"
}

fn arbitrary_metadata_key() -> impl Strategy<Value = String> {
    "[a-z0-9._-]{1,16}"
}

fn arbitrary_metadata_value() -> impl Strategy<Value = String> {
    ".{0,24}"
}

/// A `Replay` with `header: Some(..)` that satisfies every encoding constraint by construction.
fn arbitrary_replay() -> impl Strategy<Value = Replay> {
    prop::collection::vec(arbitrary_frame(), 0..8).prop_flat_map(|frames| {
        let frame_count = frames.len() as u64;
        (
            Just(frames),
            arbitrary_engine_version(),
            any::<[u8; 20]>(),
            any::<u64>(),
            prop::collection::vec(0..=frame_count, 0..4),
            prop::collection::btree_map(arbitrary_metadata_key(), arbitrary_metadata_value(), 0..4),
        )
            .prop_map(
                move |(frames, engine_version, build_bytes, manifest, mut ticks, app_metadata)| {
                    ticks.sort_unstable();
                    ticks.dedup();
                    let swaps = ticks
                        .into_iter()
                        .map(|tick| SwapRecord::new(tick, ContentManifestHash(manifest)))
                        .collect();
                    // `ReplayHeader` is `#[non_exhaustive]`: build via `for_this_build`, then
                    // assign fields, as its own documentation prescribes.
                    let mut header = ReplayHeader::for_this_build(ContentManifestHash(manifest));
                    header.engine_version = engine_version;
                    header.engine_build = BuildHash(build_bytes);
                    header.swaps = swaps;
                    header.app_metadata = app_metadata;
                    Replay {
                        header: Some(header),
                        log: InputLog {
                            seed: manifest,
                            tick_rate_hz: 60,
                            frames,
                        },
                    }
                },
            )
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..600)) {
        let _ = Replay::from_bytes(&bytes);
    }

    #[test]
    fn single_byte_mutations_of_a_valid_replay_never_panic(
        position in any::<usize>(),
        value in any::<u8>(),
    ) {
        let bytes = full_replay().to_bytes().unwrap();
        let mut mutated = bytes;
        let index = position % mutated.len();
        mutated[index] = value;
        let _ = Replay::from_bytes(&mutated);
    }

    #[test]
    fn arbitrary_valid_replays_round_trip_from_bytes_of_to_bytes(replay in arbitrary_replay()) {
        let bytes = replay.to_bytes().expect("constructed to satisfy every constraint");
        prop_assert_eq!(Replay::from_bytes(&bytes), Ok(replay));
    }

    #[test]
    fn decoding_then_reencoding_reproduces_the_same_bytes(replay in arbitrary_replay()) {
        let bytes = replay.to_bytes().expect("constructed to satisfy every constraint");
        let decoded = Replay::from_bytes(&bytes).expect("just encoded above");
        prop_assert_eq!(decoded.to_bytes().expect("round trip"), bytes);
    }
}
