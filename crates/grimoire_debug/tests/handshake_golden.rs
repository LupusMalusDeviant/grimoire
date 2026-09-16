//! Hand-authored, full-*frame* golden byte fixtures (contract §13 "Framing" + "Nachrichtenkatalog",
//! §2 rule 10) for the message layer and handshake added in Plan-0002 WP8.2.
//!
//! `tests/generated_debug_protocol.rs` (WP8.1) already carries byte-for-byte fixtures for four
//! *payloads* (`Hello`, `ErrorMsg`, `Stats`, `SwapSigilUnit`), hand-derived from the contract's
//! field tables. This file adds the layer WP8.1 does not cover: the full wire *frame* — the
//! 8-byte length-prefixed header (`len`, `id`, `flags`, `seq`) wrapped around a payload — for the
//! `Hello` request/response exchange plus two further catalogue messages (`Error`, `SwapAck`),
//! satisfying the "at least three messages and the Hello exchange" requirement at the frame
//! level rather than the payload level.
//!
//! Every byte literal below is computed by hand from the contract's field tables and the
//! "Framing"/"Kodierung der Nutzlast" encoding rules (little-endian, `u32` length prefixes,
//! fixed-width fields) — **never** by calling this crate's own `encode_frame`/`Message::to_frame`.
//! Each is checked three ways: `Message::to_frame` on the equivalent value must produce it
//! exactly, `FrameDecoder`/`Message::from_frame` must decode it back to that same value, and (for
//! the Hello exchange specifically) feeding the request bytes through the real
//! [`grimoire_debug::accept_handshake`] over an [`grimoire_debug::InProcessTransport`] must
//! produce exactly the response bytes on the wire — tying the fixture to the actual handshake
//! code path, not just to `Message`'s codec.

use grimoire_debug::{
    DebugTransport, EngineIdentity, ErrorCode, ErrorMsg, Frame, FrameDecoder, Hello,
    InProcessTransport, Message, MessageId, PeerRole, SwapAck, accept_handshake, encode_frame,
};
use std::time::Duration;

/// The `Hello` request a tool sends to open a connection (contract §13 `Hello`, `T→E` half of
/// `T↔E`).
///
/// Field-by-field (all little-endian; see contract §13 "Kodierung der Nutzlast"):
/// - Frame header: `len: u32 = 65` (8-byte header + 57-byte payload, see below, `0x41`),
///   `id: u16 = 0x0001` (`Hello`), `flags: u16 = 0`, `seq: u32 = 1` (this tool's first frame).
/// - Payload (57 bytes): `protocol_version: u16 = 1`; `role: PeerRole = Tool (0)`, one `u8`;
///   `engine_version: Str≤64 = "0.1.1"` (5 ASCII bytes, `u32` length `5` + bytes);
///   `build_hash: Str≤64 = "unknown"` (7 ASCII bytes, `u32` length `7` + bytes);
///   `token: [u8; 32] = [0x11; 32]` (fixed array, no length prefix);
///   `stats_interval_frames: u16 = 30`.
#[rustfmt::skip]
const HELLO_REQUEST_FRAME_BYTES: &[u8] = &[
    // --- frame header ---
    0x41, 0x00, 0x00, 0x00, // len = 65 (8 header + 57 payload)
    0x01, 0x00,             // id = 0x0001 (Hello)
    0x00, 0x00,             // flags = 0
    0x01, 0x00, 0x00, 0x00, // seq = 1
    // --- payload ---
    0x01, 0x00, // protocol_version = 1
    0x00,       // role = Tool
    0x05, 0x00, 0x00, 0x00, // engine_version: length = 5
    b'0', b'.', b'1', b'.', b'1', // engine_version = "0.1.1"
    0x07, 0x00, 0x00, 0x00, // build_hash: length = 7
    b'u', b'n', b'k', b'n', b'o', b'w', b'n', // build_hash = "unknown"
    0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, // token = [0x11; 32]...
    0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
    0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
    0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
    0x1E, 0x00, // stats_interval_frames = 30
];

/// The engine's `Hello` reply to [`HELLO_REQUEST_FRAME_BYTES`] (contract §13 "Handshake" step 8):
/// same `engine_version`/`build_hash` as the request (a matching handshake), `role = Engine`,
/// `token` all zero, and `stats_interval_frames = 0` (meaningless coming from the engine).
///
/// Same field layout as [`HELLO_REQUEST_FRAME_BYTES`] except `role`, `token`, and
/// `stats_interval_frames`; `len` is identical (`65`) since `"0.1.1"`/`"unknown"` are the same
/// lengths. `seq = 1`: the engine's own first frame on this connection (contract §13 "Framing":
/// `seq` counts per sender, not per connection).
#[rustfmt::skip]
const HELLO_RESPONSE_FRAME_BYTES: &[u8] = &[
    // --- frame header ---
    0x41, 0x00, 0x00, 0x00, // len = 65
    0x01, 0x00,             // id = 0x0001 (Hello)
    0x00, 0x00,             // flags = 0
    0x01, 0x00, 0x00, 0x00, // seq = 1
    // --- payload ---
    0x01, 0x00, // protocol_version = 1
    0x01,       // role = Engine
    0x05, 0x00, 0x00, 0x00, // engine_version: length = 5
    b'0', b'.', b'1', b'.', b'1',
    0x07, 0x00, 0x00, 0x00, // build_hash: length = 7
    b'u', b'n', b'k', b'n', b'o', b'w', b'n',
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // token = [0; 32]...
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, // stats_interval_frames = 0
];

/// A full `Error` frame (contract §13 `ErrorMsg`, id `0x0002`).
///
/// - Header: `len = 43` (8 + 35 payload), `id = 0x0002`, `flags = 0`, `seq = 5`.
/// - Payload (35 bytes): `code: ErrorCode = VersionMismatch (2)`, `u16` per the catalogue's
///   `ErrorCode` exception -> `2`; `in_reply_to: u32 = 7`; `message: Str≤1024 =
///   "protocol version mismatch"` (25 ASCII bytes, length `25`). Total: `2 + 4 + (4 + 25) = 35`.
#[rustfmt::skip]
const ERROR_FRAME_BYTES: &[u8] = &[
    // --- frame header ---
    0x2B, 0x00, 0x00, 0x00, // len = 43 (8 header + 35 payload)
    0x02, 0x00,             // id = 0x0002 (Error)
    0x00, 0x00,             // flags = 0
    0x05, 0x00, 0x00, 0x00, // seq = 5
    // --- payload ---
    0x02, 0x00,             // code = VersionMismatch (2), u16
    0x07, 0x00, 0x00, 0x00, // in_reply_to = 7
    0x19, 0x00, 0x00, 0x00, // message: length = 25
    b'p', b'r', b'o', b't', b'o', b'c', b'o', b'l', b' ',
    b'v', b'e', b'r', b's', b'i', b'o', b'n', b' ',
    b'm', b'i', b's', b'm', b'a', b't', b'c', b'h',
];

/// A full `SwapAck` frame (contract §13 `SwapAck`, id `0x0021`, `#[non_exhaustive]` + `Default`).
///
/// - Header: `len = 61` (8 + 53 payload), `id = 0x0021`, `flags = 0`, `seq = 9`.
/// - Payload (53 bytes, field order per `SwapAck::encode`): `in_reply_to: u32 = 42`;
///   `status: u8 = 1` (rejected); `applied_tick: u64 = 0`; `content_swaps: u32 = 3`;
///   `content_manifest: u64 = 0xAABBCCDD`, little-endian `DD CC BB AA 00 00 00 00`;
///   `unit_hash: u64 = 0x1122334455667788`, little-endian `88 77 66 55 44 33 22 11`;
///   `reason: Str≤1024 = "unit id mismatch"` (16 ASCII bytes).
#[rustfmt::skip]
const SWAP_ACK_FRAME_BYTES: &[u8] = &[
    // --- frame header ---
    0x3D, 0x00, 0x00, 0x00, // len = 61 (8 header + 53 payload)
    0x21, 0x00,             // id = 0x0021 (SwapAck)
    0x00, 0x00,             // flags = 0
    0x09, 0x00, 0x00, 0x00, // seq = 9
    // --- payload ---
    0x2A, 0x00, 0x00, 0x00, // in_reply_to = 42
    0x01,                   // status = 1 (rejected)
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // applied_tick = 0
    0x03, 0x00, 0x00, 0x00, // content_swaps = 3
    0xDD, 0xCC, 0xBB, 0xAA, 0x00, 0x00, 0x00, 0x00, // content_manifest = 0xAABBCCDD
    0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, // unit_hash = 0x1122334455667788
    0x10, 0x00, 0x00, 0x00, // reason: length = 16
    b'u', b'n', b'i', b't', b' ', b'i', b'd', b' ', b'm', b'i', b's', b'm', b'a', b't', b'c', b'h',
];

fn hello_request_value() -> Hello {
    Hello {
        protocol_version: 1,
        role: PeerRole::Tool,
        engine_version: "0.1.1".to_owned(),
        build_hash: "unknown".to_owned(),
        token: [0x11; 32],
        stats_interval_frames: 30,
    }
}

fn hello_response_value() -> Hello {
    Hello {
        protocol_version: 1,
        role: PeerRole::Engine,
        engine_version: "0.1.1".to_owned(),
        build_hash: "unknown".to_owned(),
        token: [0u8; 32],
        stats_interval_frames: 0,
    }
}

fn error_value() -> ErrorMsg {
    ErrorMsg {
        code: ErrorCode::VersionMismatch,
        in_reply_to: 7,
        message: "protocol version mismatch".to_owned(),
    }
}

fn swap_ack_value() -> SwapAck {
    let mut value = SwapAck::default();
    value.in_reply_to = 42;
    value.status = 1;
    value.applied_tick = 0;
    value.content_swaps = 3;
    value.content_manifest = 0xAABB_CCDD;
    value.unit_hash = 0x1122_3344_5566_7788;
    value.reason = "unit id mismatch".to_owned();
    value
}

/// Decodes exactly one frame from `bytes` and asserts nothing is left over, so a fixture that is
/// even one byte too long or short is caught here rather than by a downstream field mismatch.
fn decode_one_frame(bytes: &[u8]) -> Frame {
    let mut decoder = FrameDecoder::new();
    decoder.push(bytes);
    let frame = decoder
        .next_frame()
        .expect("fixture must decode as a well-formed frame")
        .expect("fixture must contain a complete frame");
    assert_eq!(
        decoder.next_frame().unwrap(),
        None,
        "fixture must contain exactly one frame, no trailing bytes"
    );
    frame
}

#[test]
fn hello_request_frame_round_trips_through_message() {
    let frame = decode_one_frame(HELLO_REQUEST_FRAME_BYTES);
    assert_eq!(frame.id, MessageId(0x0001));
    assert_eq!(frame.seq, 1);
    assert_eq!(
        Message::from_frame(&frame).unwrap(),
        Message::Hello(hello_request_value())
    );

    let mut encoded = Vec::new();
    encode_frame(
        &Message::Hello(hello_request_value()).to_frame(1).unwrap(),
        &mut encoded,
    )
    .unwrap();
    assert_eq!(encoded, HELLO_REQUEST_FRAME_BYTES);
}

#[test]
fn hello_response_frame_round_trips_through_message() {
    let frame = decode_one_frame(HELLO_RESPONSE_FRAME_BYTES);
    assert_eq!(frame.id, MessageId(0x0001));
    assert_eq!(frame.seq, 1);
    assert_eq!(
        Message::from_frame(&frame).unwrap(),
        Message::Hello(hello_response_value())
    );

    let mut encoded = Vec::new();
    encode_frame(
        &Message::Hello(hello_response_value()).to_frame(1).unwrap(),
        &mut encoded,
    )
    .unwrap();
    assert_eq!(encoded, HELLO_RESPONSE_FRAME_BYTES);
}

#[test]
fn error_frame_round_trips_through_message() {
    let frame = decode_one_frame(ERROR_FRAME_BYTES);
    assert_eq!(frame.id, MessageId(0x0002));
    assert_eq!(frame.seq, 5);
    assert_eq!(
        Message::from_frame(&frame).unwrap(),
        Message::Error(error_value())
    );

    let mut encoded = Vec::new();
    encode_frame(
        &Message::Error(error_value()).to_frame(5).unwrap(),
        &mut encoded,
    )
    .unwrap();
    assert_eq!(encoded, ERROR_FRAME_BYTES);
}

#[test]
fn swap_ack_frame_round_trips_through_message() {
    let frame = decode_one_frame(SWAP_ACK_FRAME_BYTES);
    assert_eq!(frame.id, MessageId(0x0021));
    assert_eq!(frame.seq, 9);
    assert_eq!(
        Message::from_frame(&frame).unwrap(),
        Message::SwapAck(swap_ack_value())
    );

    let mut encoded = Vec::new();
    encode_frame(
        &Message::SwapAck(swap_ack_value()).to_frame(9).unwrap(),
        &mut encoded,
    )
    .unwrap();
    assert_eq!(encoded, SWAP_ACK_FRAME_BYTES);
}

/// Ties the Hello exchange fixture to the real handshake code path: feeding exactly
/// [`HELLO_REQUEST_FRAME_BYTES`] into [`accept_handshake`] over a raw byte-oriented
/// [`InProcessTransport`] must produce exactly [`HELLO_RESPONSE_FRAME_BYTES`] on the wire. This
/// is a stronger check than decoding the fixtures separately: it would catch a bug where
/// `accept_handshake` builds a subtly different reply than the one this file hand-derived.
#[test]
fn accept_handshake_reproduces_the_hello_exchange_fixture_byte_for_byte() {
    let (mut engine_side, mut wire_side) = InProcessTransport::pair();

    // Push the raw request bytes directly onto the wire, bypassing `Message`/`Hello` entirely,
    // so this test does not depend on the encoder to get the request onto the connection.
    let mut decoder = FrameDecoder::new();
    decoder.push(HELLO_REQUEST_FRAME_BYTES);
    let request_frame = decoder.next_frame().unwrap().unwrap();
    wire_side.send(&request_frame).unwrap();

    let identity = EngineIdentity::new("0.1.1".to_owned(), "unknown".to_owned(), [0x11; 32]);
    accept_handshake(&mut engine_side, &identity, Duration::from_secs(2))
        .expect("a Hello matching the engine's own identity must be accepted");

    let mut inbox = Vec::new();
    wire_side.poll(&mut inbox).unwrap();
    assert_eq!(inbox.len(), 1, "the engine must send exactly one reply");

    let mut wire_bytes = Vec::new();
    encode_frame(&inbox[0], &mut wire_bytes).unwrap();
    assert_eq!(wire_bytes, HELLO_RESPONSE_FRAME_BYTES);
}
