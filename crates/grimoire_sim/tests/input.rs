//! Input frames and the binary `InputLog` format, including hostile input.

use grimoire_core::hash_of;
use grimoire_sim::{InputFrame, InputLog, MAX_INPUT_SLOTS, SimError, TickInput};
use proptest::prelude::*;

fn sample_log() -> InputLog {
    let mut frames = Vec::new();
    for tick in 0..5i16 {
        let mut input = TickInput::default();
        input.slots[0] = InputFrame {
            axes: [32_767, -32_768, tick, -tick],
            buttons: 0x8000_0001 | u32::from(tick.unsigned_abs()),
        };
        input.slots[MAX_INPUT_SLOTS - 1].buttons = 0xFFFF_FFFF;
        frames.push(input);
    }
    InputLog {
        seed: 0x0123_4567_89AB_CDEF,
        tick_rate_hz: 60,
        frames,
    }
}

#[test]
fn axes_are_normalised_and_out_of_range_indices_read_zero() {
    let frame = InputFrame {
        axes: [32_767, -32_767, -32_768, 0],
        buttons: 0,
    };
    assert_eq!(frame.axis(0), 1.0);
    assert_eq!(frame.axis(1), -1.0);
    assert_eq!(frame.axis(2), -1.0);
    assert_eq!(frame.axis(3), 0.0);
    assert_eq!(frame.axis(4), 0.0);
    assert_eq!(frame.axis(usize::MAX), 0.0);
    let half = InputFrame {
        axes: [16_384, 0, 0, 0],
        buttons: 0,
    };
    assert!((half.axis(0) - 0.5).abs() < 1e-4);
}

#[test]
fn buttons_read_bits_and_out_of_range_bits_are_released() {
    let frame = InputFrame {
        axes: [0; 4],
        buttons: 0x8000_0005,
    };
    assert!(frame.is_pressed(0));
    assert!(!frame.is_pressed(1));
    assert!(frame.is_pressed(2));
    assert!(frame.is_pressed(31));
    let all = InputFrame {
        axes: [0; 4],
        buttons: u32::MAX,
    };
    for bit in [32, 33, 64, 255] {
        assert!(!all.is_pressed(bit));
    }
}

#[test]
fn input_hashes_distinguish_slots_and_fields() {
    let mut a = TickInput::default();
    let mut b = TickInput::default();
    a.slots[0].buttons = 1;
    b.slots[1].buttons = 1;
    assert_ne!(hash_of(&a), hash_of(&b));
    assert_ne!(hash_of(&a), hash_of(&TickInput::default()));
    assert_eq!(hash_of(&a), hash_of(&a.clone()));
}

#[test]
fn log_round_trips() {
    let log = sample_log();
    let bytes = log.to_bytes();
    assert_eq!(bytes.len(), 32 + 48 * log.frames.len());
    assert_eq!(InputLog::from_bytes(&bytes), Ok(log));

    let empty = InputLog {
        seed: 0,
        tick_rate_hz: 1,
        frames: Vec::new(),
    };
    assert_eq!(InputLog::from_bytes(&empty.to_bytes()), Ok(empty));
}

#[test]
#[should_panic(expected = "tick_rate_hz must not be 0")]
fn encoding_a_zero_tick_rate_panics_instead_of_writing_an_unloadable_log() {
    let log = InputLog {
        tick_rate_hz: 0,
        ..sample_log()
    };
    let _ = log.to_bytes();
}

#[test]
fn header_layout_is_little_endian() {
    let bytes = sample_log().to_bytes();
    assert_eq!(&bytes[0..8], b"GRIMREPL");
    assert_eq!(&bytes[8..12], &[1, 0, 0, 0]);
    assert_eq!(
        &bytes[12..20],
        &[0xEF, 0xCD, 0xAB, 0x89, 0x67, 0x45, 0x23, 0x01]
    );
    assert_eq!(&bytes[20..24], &[60, 0, 0, 0]);
    assert_eq!(&bytes[24..32], &[5, 0, 0, 0, 0, 0, 0, 0]);
    // First slot of the first frame: axes 32767, -32768, 0, 0; buttons 0x80000001.
    assert_eq!(
        &bytes[32..44],
        &[0xFF, 0x7F, 0x00, 0x80, 0, 0, 0, 0, 0x01, 0, 0, 0x80]
    );
}

#[test]
fn malformed_logs_are_rejected() {
    let bytes = sample_log().to_bytes();

    assert_eq!(
        InputLog::from_bytes(&[]),
        Err(SimError::UnexpectedEnd {
            offset: 0,
            needed: 8,
            available: 0
        })
    );
    assert!(matches!(
        InputLog::from_bytes(&bytes[..30]),
        Err(SimError::UnexpectedEnd { offset: 24, .. })
    ));

    let mut bad_magic = bytes.clone();
    bad_magic[0] = b'X';
    assert_eq!(InputLog::from_bytes(&bad_magic), Err(SimError::BadMagic));

    let mut version = bytes.clone();
    version[8] = 2;
    assert_eq!(
        InputLog::from_bytes(&version),
        Err(SimError::UnsupportedVersion(2))
    );

    let mut zero_rate = bytes.clone();
    zero_rate[20] = 0;
    assert_eq!(
        InputLog::from_bytes(&zero_rate),
        Err(SimError::InvalidTickRate)
    );

    assert_eq!(
        InputLog::from_bytes(&bytes[..bytes.len() - 1]),
        Err(SimError::FrameDataLength {
            frames: 5,
            remaining: 239
        })
    );

    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(matches!(
        InputLog::from_bytes(&trailing),
        Err(SimError::FrameDataLength { remaining: 241, .. })
    ));
}

#[test]
fn huge_declared_frame_count_is_rejected_without_allocating() {
    let mut bytes = sample_log().to_bytes();
    for count in [u64::MAX, u64::MAX / 48 + 1, 1 << 40] {
        bytes[24..32].copy_from_slice(&count.to_le_bytes());
        assert_eq!(
            InputLog::from_bytes(&bytes),
            Err(SimError::FrameDataLength {
                frames: count,
                remaining: 240
            })
        );
    }
}

#[test]
fn errors_have_readable_messages() {
    assert_eq!(
        SimError::UnsupportedVersion(9).to_string(),
        "unsupported replay format version 9"
    );
}

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

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let _ = InputLog::from_bytes(&bytes);
    }

    #[test]
    fn valid_header_with_arbitrary_tail_never_panics(
        count in any::<u64>(),
        rate in any::<u32>(),
        tail in prop::collection::vec(any::<u8>(), 0..400),
    ) {
        let mut bytes = b"GRIMREPL".to_vec();
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&tail);
        let decoded = InputLog::from_bytes(&bytes);
        let consistent = rate != 0 && u64::try_from(tail.len()).ok() == count.checked_mul(48);
        prop_assert_eq!(decoded.is_ok(), consistent);
    }

    #[test]
    fn arbitrary_logs_round_trip_and_every_truncation_fails(
        seed in any::<u64>(),
        tick_rate_hz in 1u32..,
        frames in prop::collection::vec(arbitrary_frame(), 0..20),
    ) {
        let log = InputLog { seed, tick_rate_hz, frames };
        let bytes = log.to_bytes();
        prop_assert_eq!(InputLog::from_bytes(&bytes), Ok(log));
        for length in 0..bytes.len() {
            prop_assert!(InputLog::from_bytes(&bytes[..length]).is_err());
        }
    }
}
