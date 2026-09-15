//! Behaviour tests for the generated debug protocol v1 payload types (contract §13, project
//! ADR-0011, Plan-0002 WP8.1): round trips, every-truncation-fails, an oversized declared count
//! that must fail before it would allocate, `decode` panicking on no input, and golden byte
//! fixtures for a few messages checked by hand against the contract layout.

use grimoire_debug::{
    ErrorCode, ErrorMsg, Hello, LogMsg, PeerRole, ProtocolError, SigilPreview, Stats, StatsCounter,
    StatsScope, SwapAck, SwapSigilUnit,
};
use proptest::prelude::*;

// --- Sample values, one per payload type -----------------------------------------------------

fn sample_hello() -> Hello {
    Hello {
        protocol_version: 1,
        role: PeerRole::Tool,
        engine_version: "0.1.1".to_owned(),
        build_hash: "deadbeefdeadbeefdeadbeefdeadbeef".to_owned(),
        token: [0xAA; 32],
        stats_interval_frames: 30,
    }
}

fn sample_error_msg() -> ErrorMsg {
    ErrorMsg {
        code: ErrorCode::VersionMismatch,
        in_reply_to: 7,
        message: "protocol version mismatch".to_owned(),
    }
}

fn sample_log_msg() -> LogMsg {
    LogMsg {
        level: 3,
        tick: 12_345,
        target: "grimoire_sim".to_owned(),
        text: "tick advanced".to_owned(),
    }
}

fn sample_stats_scope() -> StatsScope {
    // `StatsScope` is `#[non_exhaustive]` (contract §2 rule 13): a struct-literal expression is
    // rejected outright for a non-exhaustive type from another crate, even with `..Default::
    // default()` (E0639), so it has to be `Default` plus field assignment on separate
    // statements, exactly as the facade builds it outside `grimoire_debug`.
    let mut value = StatsScope::default();
    value.scope = 4;
    value.name = "physics".to_owned();
    value.total_ns = 123_456;
    value.calls = 10;
    value.budget_ns = 200_000;
    value.estimate = false;
    value
}

fn sample_stats_counter() -> StatsCounter {
    StatsCounter {
        name: "draw_calls".to_owned(),
        value: 42,
    }
}

fn sample_stats() -> Stats {
    // `Stats` is `#[non_exhaustive]`; see `sample_stats_scope`'s comment for why this cannot be a
    // single struct-literal expression.
    let mut value = Stats::default();
    value.frame = 100;
    value.sim_tick = 100;
    value.ticks_this_frame = 1;
    value.alpha = 0.5;
    value.frame_time_ns = 16_000_000;
    value.fps = 60.0;
    value.dropped_time_ns = 0;
    value.content_swaps = 0;
    value.content_manifest = 0;
    value.scopes = vec![sample_stats_scope()];
    value.counters = vec![sample_stats_counter()];
    value
}

fn sample_swap_sigil_unit() -> SwapSigilUnit {
    SwapSigilUnit {
        unit_path: "sigils/basic_bolt.sigil".to_owned(),
        unit_bytes: vec![1, 2, 3, 4],
    }
}

fn sample_swap_ack() -> SwapAck {
    // `SwapAck` is `#[non_exhaustive]`; see `sample_stats_scope`'s comment for why this cannot be
    // a single struct-literal expression.
    let mut value = SwapAck::default();
    value.in_reply_to = 5;
    value.status = 0;
    value.applied_tick = 101;
    value.content_swaps = 1;
    value.content_manifest = 999;
    value.unit_hash = 0xDEAD_BEEF_0000_0001;
    value.reason = String::new();
    value
}

fn sample_sigil_preview_with_target() -> SigilPreview {
    SigilPreview {
        unit_path: "sigils/basic_bolt.sigil".to_owned(),
        unit_bytes: vec![9, 9, 9],
        ticks: 30,
        target: Some([1.5, -2.5]),
    }
}

fn sample_sigil_preview_without_target() -> SigilPreview {
    SigilPreview {
        target: None,
        ..sample_sigil_preview_with_target()
    }
}

// --- Round trips -------------------------------------------------------------------------------

macro_rules! round_trip_test {
    ($name:ident, $sample:expr) => {
        #[test]
        fn $name() {
            let value = $sample;
            let mut bytes = Vec::new();
            value.encode(&mut bytes).expect("encode must succeed");
            let decoded = decode_of(&value, &bytes);
            assert_eq!(decoded, value);
        }
    };
}

/// Helper purely so the macro above can call the right type's `decode` without repeating the
/// type name; relies on inference from `value`'s type.
fn decode_of<T>(_value: &T, bytes: &[u8]) -> T
where
    T: Decodable,
{
    T::decode(bytes).expect("decode of a freshly encoded value must succeed")
}

/// Local helper trait so [`decode_of`] can be generic: every generated payload type has an
/// inherent `decode`, but they are not required to implement a common trait for it (they are
/// plain generated structs, not trait objects), so this test-only trait bridges the two.
trait Decodable: Sized {
    fn decode(bytes: &[u8]) -> Result<Self, ProtocolError>;
}
macro_rules! impl_decodable {
    ($($ty:ty),* $(,)?) => {
        $(impl Decodable for $ty {
            fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
                <$ty>::decode(bytes)
            }
        })*
    };
}
impl_decodable!(
    Hello,
    ErrorMsg,
    LogMsg,
    StatsScope,
    StatsCounter,
    Stats,
    SwapSigilUnit,
    SwapAck,
    SigilPreview
);

round_trip_test!(round_trip_hello, sample_hello());
round_trip_test!(round_trip_error_msg, sample_error_msg());
round_trip_test!(round_trip_log_msg, sample_log_msg());
round_trip_test!(round_trip_stats_scope, sample_stats_scope());
round_trip_test!(round_trip_stats_counter, sample_stats_counter());
round_trip_test!(round_trip_stats, sample_stats());
round_trip_test!(round_trip_swap_sigil_unit, sample_swap_sigil_unit());
round_trip_test!(round_trip_swap_ack, sample_swap_ack());
round_trip_test!(
    round_trip_sigil_preview_with_target,
    sample_sigil_preview_with_target()
);
round_trip_test!(
    round_trip_sigil_preview_without_target,
    sample_sigil_preview_without_target()
);

#[test]
fn unknown_error_code_decodes_as_malformed_fallback() {
    // Contract §13: "unbekannter Code -> Malformed". `Error`'s id and layout are frozen across
    // every protocol version, so a code from a newer version must still decode, not fail.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&999u16.to_le_bytes()); // an ErrorCode value no variant declares
    bytes.extend_from_slice(&0u32.to_le_bytes()); // in_reply_to
    bytes.extend_from_slice(&0u32.to_le_bytes()); // message: empty Str
    let decoded = ErrorMsg::decode(&bytes).expect("an unknown code must not fail decoding");
    assert_eq!(decoded.code, ErrorCode::Malformed);
}

#[test]
fn unknown_peer_role_is_invalid_enum() {
    // PeerRole has no `fallback`, so an unknown value must be a hard decode error.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&1u16.to_le_bytes()); // protocol_version
    bytes.push(7); // role: not 0 (Tool) or 1 (Engine)
    bytes.extend_from_slice(&0u32.to_le_bytes()); // engine_version: empty
    bytes.extend_from_slice(&0u32.to_le_bytes()); // build_hash: empty
    bytes.extend_from_slice(&[0u8; 32]); // token
    bytes.extend_from_slice(&0u16.to_le_bytes()); // stats_interval_frames
    let error = Hello::decode(&bytes).unwrap_err();
    assert_eq!(
        error,
        ProtocolError::InvalidEnum {
            field: "role",
            value: 7
        }
    );
}

// --- Every truncation fails ---------------------------------------------------------------

/// For every payload type exercised by `truncation_test!` below, every strict prefix of its
/// canonical encoding must fail to decode: `decode_from` always reads a fixed field sequence, so
/// any missing byte is caught as `UnexpectedEnd` somewhere in that sequence (a prefix can never
/// look like a complete, different value of the same type, since it is a prefix of the *only*
/// encoding of `value`, contract §2 rule 10 "kanonische Bytes").
macro_rules! truncation_test {
    ($name:ident, $ty:ty, $sample:expr) => {
        #[test]
        fn $name() {
            let value: $ty = $sample;
            let mut full = Vec::new();
            value.encode(&mut full).expect("encode must succeed");
            for len in 0..full.len() {
                let prefix = &full[..len];
                assert!(
                    <$ty>::decode(prefix).is_err(),
                    "a {len}-byte prefix of {value:?}'s encoding must not decode successfully"
                );
            }
        }
    };
}

truncation_test!(every_truncation_of_hello_fails, Hello, sample_hello());
truncation_test!(
    every_truncation_of_error_msg_fails,
    ErrorMsg,
    sample_error_msg()
);
truncation_test!(every_truncation_of_log_msg_fails, LogMsg, sample_log_msg());
truncation_test!(every_truncation_of_stats_fails, Stats, sample_stats());
truncation_test!(
    every_truncation_of_swap_sigil_unit_fails,
    SwapSigilUnit,
    sample_swap_sigil_unit()
);
truncation_test!(
    every_truncation_of_swap_ack_fails,
    SwapAck,
    sample_swap_ack()
);
truncation_test!(
    every_truncation_of_sigil_preview_fails,
    SigilPreview,
    sample_sigil_preview_with_target()
);

// --- Oversized declared count never allocates before it is checked -----------------------------

#[test]
fn oversized_scopes_count_fails_before_allocating() {
    // Hand-built bytes for `Stats` up to (but not including) `scopes`, followed by a `u32::MAX`
    // element count. If the emitted code allocated `Vec::with_capacity(count)` before checking
    // `count <= 64`, this would try to reserve room for billions of `StatsScope`s; instead
    // `count_u32` must reject it first (contract §2 rule 9).
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0u64.to_le_bytes()); // frame
    bytes.extend_from_slice(&0u64.to_le_bytes()); // sim_tick
    bytes.extend_from_slice(&0u32.to_le_bytes()); // ticks_this_frame
    bytes.extend_from_slice(&0f32.to_le_bytes()); // alpha
    bytes.extend_from_slice(&0u64.to_le_bytes()); // frame_time_ns
    bytes.extend_from_slice(&0f32.to_le_bytes()); // fps
    bytes.extend_from_slice(&0u64.to_le_bytes()); // dropped_time_ns
    bytes.extend_from_slice(&0u32.to_le_bytes()); // content_swaps
    bytes.extend_from_slice(&0u64.to_le_bytes()); // content_manifest
    bytes.extend_from_slice(&u32::MAX.to_le_bytes()); // scopes: oversized count, no elements follow

    let error = Stats::decode(&bytes).unwrap_err();
    assert_eq!(
        error,
        ProtocolError::FieldTooLong {
            field: "scopes",
            len: u32::MAX as usize,
            max: 64,
        }
    );
}

#[test]
fn oversized_str_length_fails_before_allocating() {
    // `Hello.engine_version` (`Str≤64`): a declared length far larger than both the field's
    // maximum and the bytes actually available must fail on the maximum check, not the
    // remaining-bytes check — both guard against allocation, but `FieldTooLong` is the more
    // specific diagnosis when both would apply.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&1u16.to_le_bytes()); // protocol_version
    bytes.push(0); // role: Tool
    bytes.extend_from_slice(&u32::MAX.to_le_bytes()); // engine_version: oversized length
    let error = Hello::decode(&bytes).unwrap_err();
    assert_eq!(
        error,
        ProtocolError::FieldTooLong {
            field: "engine_version",
            len: u32::MAX as usize,
            max: 64,
        }
    );
}

// --- Proptest: decode never panics on arbitrary bytes -----------------------------------------

proptest! {
    #[test]
    fn hello_decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        let _ = Hello::decode(&bytes);
    }

    #[test]
    fn error_msg_decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        let _ = ErrorMsg::decode(&bytes);
    }

    #[test]
    fn log_msg_decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = LogMsg::decode(&bytes);
    }

    #[test]
    fn stats_decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = Stats::decode(&bytes);
    }

    #[test]
    fn swap_sigil_unit_decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        let _ = SwapSigilUnit::decode(&bytes);
    }

    #[test]
    fn swap_ack_decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        let _ = SwapAck::decode(&bytes);
    }

    #[test]
    fn sigil_preview_decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        let _ = SigilPreview::decode(&bytes);
    }

    /// Mutating a single byte of a valid encoding must never panic either (contract §2 rule 9:
    /// "einzeln veränderten gültigen Eingaben").
    #[test]
    fn stats_decode_never_panics_on_single_byte_mutations(
        index in 0usize..200,
        replacement in any::<u8>(),
    ) {
        let mut bytes = Vec::new();
        sample_stats().encode(&mut bytes).unwrap();
        if index < bytes.len() {
            bytes[index] = replacement;
        }
        let _ = Stats::decode(&bytes);
    }
}

// --- Golden byte fixtures ------------------------------------------------------------------

mod golden {
    //! Byte-for-byte golden fixtures for a handful of messages (contract §13, §2 rule 10), hand
    //! checked against the field layout in `schema/debug_protocol_v1.gschema` /
    //! `docs/formats/debug-protocol.md`. Follows the same convention as
    //! `grimoire_assets`'s `tests/pack_format.rs`: a `regenerate_*` test (run explicitly, never
    //! part of the normal suite) writes the fixture from its logical content, and every other
    //! test treats the checked-in file as fixed.

    use super::*;

    fn fixture_path(name: &str) -> String {
        format!(
            "{}/tests/fixtures/debug_v1/{name}.bin",
            env!("CARGO_MANIFEST_DIR")
        )
    }

    fn fixture_bytes(name: &str) -> Vec<u8> {
        std::fs::read(fixture_path(name)).unwrap_or_else(|error| {
            panic!("golden fixture {name:?} must exist ({error}); run `regenerate_golden_fixtures`")
        })
    }

    #[test]
    #[ignore = "regenerates the golden fixtures on disk; run explicitly, not as part of the normal suite"]
    fn regenerate_golden_fixtures() {
        for (name, bytes) in [
            ("hello", encode(&sample_hello())),
            ("error", encode(&sample_error_msg())),
            ("stats", encode(&sample_stats())),
            ("swap_sigil_unit", encode(&sample_swap_sigil_unit())),
        ] {
            std::fs::write(fixture_path(name), bytes).expect("write golden fixture");
        }
    }

    fn encode<T: Encodable>(value: &T) -> Vec<u8> {
        let mut out = Vec::new();
        value.encode_into(&mut out);
        out
    }

    /// Test-only bridge trait, the encode-side counterpart of [`super::Decodable`].
    trait Encodable {
        fn encode_into(&self, out: &mut Vec<u8>);
    }
    macro_rules! impl_encodable {
        ($($ty:ty),* $(,)?) => {
            $(impl Encodable for $ty {
                fn encode_into(&self, out: &mut Vec<u8>) {
                    self.encode(out).expect("encode of a fixture value must succeed");
                }
            })*
        };
    }
    impl_encodable!(Hello, ErrorMsg, Stats, SwapSigilUnit);

    #[test]
    fn hello_fixture_decodes_to_the_expected_value() {
        let bytes = fixture_bytes("hello");
        assert_eq!(Hello::decode(&bytes).unwrap(), sample_hello());
    }

    #[test]
    fn hello_fixture_is_reproduced_byte_for_byte_by_encode() {
        assert_eq!(encode(&sample_hello()), fixture_bytes("hello"));
    }

    #[test]
    fn error_fixture_decodes_to_the_expected_value() {
        let bytes = fixture_bytes("error");
        assert_eq!(ErrorMsg::decode(&bytes).unwrap(), sample_error_msg());
    }

    #[test]
    fn error_fixture_is_reproduced_byte_for_byte_by_encode() {
        assert_eq!(encode(&sample_error_msg()), fixture_bytes("error"));
    }

    #[test]
    fn stats_fixture_decodes_to_the_expected_value() {
        let bytes = fixture_bytes("stats");
        assert_eq!(Stats::decode(&bytes).unwrap(), sample_stats());
    }

    #[test]
    fn stats_fixture_is_reproduced_byte_for_byte_by_encode() {
        assert_eq!(encode(&sample_stats()), fixture_bytes("stats"));
    }

    #[test]
    fn swap_sigil_unit_fixture_decodes_to_the_expected_value() {
        let bytes = fixture_bytes("swap_sigil_unit");
        assert_eq!(
            SwapSigilUnit::decode(&bytes).unwrap(),
            sample_swap_sigil_unit()
        );
    }

    #[test]
    fn swap_sigil_unit_fixture_is_reproduced_byte_for_byte_by_encode() {
        assert_eq!(
            encode(&sample_swap_sigil_unit()),
            fixture_bytes("swap_sigil_unit")
        );
    }
}
