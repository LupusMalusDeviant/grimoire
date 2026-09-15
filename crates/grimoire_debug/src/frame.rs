//! Byte-level framing for the debug protocol wire format (contract §13 "Framing").
//!
//! This module implements only the frame envelope: a length-prefixed header plus an opaque
//! payload. It knows nothing about the message catalog (`Hello`, `Stats`, ...) — decoding those
//! payloads is scope for WP8.2 (see the crate-level docs).

/// Length of the fixed frame header in bytes: `id` (2) + `flags` (2) + `seq` (4).
///
/// This is exactly the minimum value `len` may carry on the wire (contract §13 "Framing").
const HEADER_LEN: usize = 8;

/// Length of the `len: u32` prefix that precedes every frame on the wire.
const LEN_PREFIX_LEN: usize = 4;

/// Numeric identifier of a debug protocol message (contract §13).
///
/// The message catalog itself (`Hello`, `Stats`, and friends) is out of scope for this crate
/// until WP8.2; `MessageId` only carries the raw wire value used by frame decoding and by
/// [`peek_hello_version`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct MessageId(pub u16);

/// One decoded frame of the debug protocol wire format (contract §13 "Framing").
///
/// `payload` is opaque to this crate: interpreting it as a specific message is scope for
/// WP8.2's message catalog.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Frame {
    /// Message identifier carried in the frame header.
    pub id: MessageId,
    /// Per-sender sequence number; `0` means "no reply expected" (§13). Not validated here.
    pub seq: u32,
    /// Raw payload bytes, not interpreted by this crate.
    pub payload: Vec<u8>,
}

/// Errors from decoding or encoding the byte-level frame envelope (contract §13).
///
/// `#[non_exhaustive]`: message-layer errors (`InvalidUtf8`, `FieldTooLong`, `InvalidEnum`,
/// `UnknownMessage`) belong to the WP8.2 message catalog and are deliberately not declared
/// here; a later PR adds them additively (§2b).
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProtocolError {
    /// The wire `len` field was below `HEADER_LEN` (8), so it cannot even cover the fixed
    /// header. Unrecoverable: the caller must close the connection (§13).
    #[error("frame length {0} is shorter than the 8-byte header")]
    FrameTooShort(u32),
    /// The wire `len` field exceeded [`crate::MAX_FRAME_LEN`]. Unrecoverable: the caller must
    /// close the connection (§13).
    #[error("frame length {0} exceeds the 16 MiB frame limit (MAX_FRAME_LEN)")]
    FrameTooLarge(u32),
    /// The wire `flags` field was non-zero; all flags are reserved and must be `0` in protocol
    /// v1. Recoverable: the frame's bytes are still fully consumed, so the stream stays in
    /// sync (§13).
    #[error("frame flags {0:#06x} are non-zero, which is invalid in protocol v1")]
    NonZeroFlags(u16),
}

/// Streaming decoder for the frame envelope (contract §13 "Framing").
///
/// Buffers bytes across any number of [`FrameDecoder::push`] calls and yields frames as soon as
/// they are complete. Validates the length field *before* allocating anything for the payload
/// (contract §2 rule 9, "foreign bytes"), and never panics on any input.
#[derive(Default, Debug)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
}

impl FrameDecoder {
    /// Creates an empty decoder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends `bytes` to the internal buffer. Never blocks, never panics.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Decodes and removes the next complete frame from the internal buffer, if any.
    ///
    /// Returns `Ok(None)` when the buffered bytes do not yet contain a complete frame.
    /// Returns `Err` only for a malformed length field ([`ProtocolError::FrameTooShort`] or
    /// [`ProtocolError::FrameTooLarge`]) — the caller must close the connection in that case,
    /// since the true frame boundary can no longer be trusted — or for
    /// [`ProtocolError::NonZeroFlags`], which *does* consume the offending frame's bytes so the
    /// stream stays in sync (contract §13). Never panics on any input.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, ProtocolError> {
        if self.buffer.len() < LEN_PREFIX_LEN {
            return Ok(None);
        }
        // Length check before any allocation (§2 rule 9): decide validity from the 4-byte
        // prefix alone, before touching anything that scales with `len`.
        let len = u32::from_le_bytes([
            self.buffer[0],
            self.buffer[1],
            self.buffer[2],
            self.buffer[3],
        ]);
        if len < HEADER_LEN as u32 {
            return Err(ProtocolError::FrameTooShort(len));
        }
        if len > crate::MAX_FRAME_LEN {
            return Err(ProtocolError::FrameTooLarge(len));
        }

        // `len` is now known to be in `HEADER_LEN..=MAX_FRAME_LEN`, so this addition cannot
        // overflow `usize` on any platform this engine targets.
        let total = LEN_PREFIX_LEN + len as usize;
        if self.buffer.len() < total {
            return Ok(None);
        }

        let id = u16::from_le_bytes([self.buffer[4], self.buffer[5]]);
        let flags = u16::from_le_bytes([self.buffer[6], self.buffer[7]]);
        let seq = u32::from_le_bytes([
            self.buffer[8],
            self.buffer[9],
            self.buffer[10],
            self.buffer[11],
        ]);
        let payload = self.buffer[LEN_PREFIX_LEN + HEADER_LEN..total].to_vec();

        // The length was valid, so the frame boundary is known regardless of what is found
        // inside it: consume exactly these bytes so the stream stays in sync (§13) even when a
        // header-level error is about to be returned below.
        self.buffer.drain(0..total);

        if flags != 0 {
            return Err(ProtocolError::NonZeroFlags(flags));
        }

        Ok(Some(Frame {
            id: MessageId(id),
            seq,
            payload,
        }))
    }
}

/// Encodes `frame` in the wire format (contract §13 "Framing"), appending to `out`.
///
/// Fails with [`ProtocolError::FrameTooLarge`] only if `8 + frame.payload.len()` would not fit
/// in the `len: u32` wire field, i.e. exceeds [`crate::MAX_FRAME_LEN`]; `out` is left unchanged
/// in that case.
pub fn encode_frame(frame: &Frame, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
    let len = HEADER_LEN
        .checked_add(frame.payload.len())
        .filter(|&len| len <= crate::MAX_FRAME_LEN as usize)
        .ok_or_else(|| {
            let reported = HEADER_LEN
                .saturating_add(frame.payload.len())
                .try_into()
                .unwrap_or(u32::MAX);
            ProtocolError::FrameTooLarge(reported)
        })?;
    // `len` fits `crate::MAX_FRAME_LEN` (a `u32` value), so this cast is lossless.
    let len = len as u32;

    out.reserve(LEN_PREFIX_LEN + len as usize);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&frame.id.0.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // flags: always 0 in protocol v1
    out.extend_from_slice(&frame.seq.to_le_bytes());
    out.extend_from_slice(&frame.payload);
    Ok(())
}

/// Reads the frozen first field (`protocol_version: u16`) of a `Hello` frame without decoding
/// the rest of its payload (contract §13).
///
/// Returns `Some` only for message id `0x0001` with a payload of at least 2 bytes; any other
/// frame yields `None`. `Hello`'s full layout beyond this first field is not frozen across
/// protocol versions and decoding it is out of scope for this crate (WP8.2).
pub fn peek_hello_version(frame: &Frame) -> Option<u16> {
    const HELLO_ID: MessageId = MessageId(0x0001);
    if frame.id == HELLO_ID && frame.payload.len() >= 2 {
        Some(u16::from_le_bytes([frame.payload[0], frame.payload[1]]))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn sample_frame(id: u16, seq: u32, payload: Vec<u8>) -> Frame {
        Frame {
            id: MessageId(id),
            seq,
            payload,
        }
    }

    #[test]
    fn round_trip_simple_frame() {
        let frame = sample_frame(7, 42, vec![1, 2, 3, 4, 5]);
        let mut bytes = Vec::new();
        encode_frame(&frame, &mut bytes).unwrap();

        let mut decoder = FrameDecoder::new();
        decoder.push(&bytes);
        assert_eq!(decoder.next_frame().unwrap(), Some(frame));
        assert_eq!(decoder.next_frame().unwrap(), None);
    }

    #[test]
    fn round_trip_empty_payload() {
        let frame = sample_frame(1, 1, Vec::new());
        let mut bytes = Vec::new();
        encode_frame(&frame, &mut bytes).unwrap();
        assert_eq!(bytes.len(), LEN_PREFIX_LEN + HEADER_LEN);

        let mut decoder = FrameDecoder::new();
        decoder.push(&bytes);
        assert_eq!(decoder.next_frame().unwrap(), Some(frame));
    }

    #[test]
    fn incremental_push_waits_for_a_complete_frame() {
        let frame = sample_frame(3, 1, vec![0xAB; 10]);
        let mut bytes = Vec::new();
        encode_frame(&frame, &mut bytes).unwrap();

        let mut decoder = FrameDecoder::new();
        decoder.push(&bytes[..5]);
        assert_eq!(decoder.next_frame().unwrap(), None);
        decoder.push(&bytes[5..]);
        assert_eq!(decoder.next_frame().unwrap(), Some(frame));
    }

    #[test]
    fn two_frames_in_one_push_decode_in_order() {
        let first = sample_frame(1, 1, vec![1]);
        let second = sample_frame(2, 2, vec![2, 2]);
        let mut bytes = Vec::new();
        encode_frame(&first, &mut bytes).unwrap();
        encode_frame(&second, &mut bytes).unwrap();

        let mut decoder = FrameDecoder::new();
        decoder.push(&bytes);
        assert_eq!(decoder.next_frame().unwrap(), Some(first));
        assert_eq!(decoder.next_frame().unwrap(), Some(second));
        assert_eq!(decoder.next_frame().unwrap(), None);
    }

    #[test]
    fn frame_too_short_length_is_rejected() {
        let mut decoder = FrameDecoder::new();
        decoder.push(&7u32.to_le_bytes());
        assert_eq!(decoder.next_frame(), Err(ProtocolError::FrameTooShort(7)));
    }

    #[test]
    fn frame_too_large_length_is_rejected_before_the_payload_arrives() {
        let mut decoder = FrameDecoder::new();
        let huge = crate::MAX_FRAME_LEN + 1;
        decoder.push(&huge.to_le_bytes());
        // Only the 4-byte prefix was ever pushed: the error fires without waiting for (or
        // allocating) the declared payload.
        assert_eq!(
            decoder.next_frame(),
            Err(ProtocolError::FrameTooLarge(huge))
        );
    }

    #[test]
    fn non_zero_flags_error_still_consumes_the_frame() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&8u32.to_le_bytes()); // len: header only, no payload
        bytes.extend_from_slice(&1u16.to_le_bytes()); // id
        bytes.extend_from_slice(&1u16.to_le_bytes()); // flags: non-zero, invalid in v1
        bytes.extend_from_slice(&0u32.to_le_bytes()); // seq

        let good = sample_frame(2, 9, vec![9, 9]);
        encode_frame(&good, &mut bytes).unwrap();

        let mut decoder = FrameDecoder::new();
        decoder.push(&bytes);
        assert_eq!(decoder.next_frame(), Err(ProtocolError::NonZeroFlags(1)));
        // The stream stayed in sync: the following, well-formed frame still decodes.
        assert_eq!(decoder.next_frame().unwrap(), Some(good));
    }

    #[test]
    fn encode_frame_rejects_oversized_payload() {
        let frame = sample_frame(1, 1, vec![0u8; crate::MAX_FRAME_LEN as usize]);
        let mut out = Vec::new();
        assert!(encode_frame(&frame, &mut out).is_err());
        assert!(out.is_empty());
    }

    #[test]
    fn peek_hello_version_reads_only_the_frozen_field() {
        let hello = sample_frame(0x0001, 1, vec![2, 0, 0xFF]);
        assert_eq!(peek_hello_version(&hello), Some(2));

        let too_short = sample_frame(0x0001, 1, vec![2]);
        assert_eq!(peek_hello_version(&too_short), None);

        let wrong_id = sample_frame(0x0002, 1, vec![2, 0]);
        assert_eq!(peek_hello_version(&wrong_id), None);
    }

    proptest! {
        #[test]
        fn round_trip_holds_for_arbitrary_frames(
            id in any::<u16>(),
            seq in any::<u32>(),
            payload in proptest::collection::vec(any::<u8>(), 0..4096),
        ) {
            let frame = sample_frame(id, seq, payload);
            let mut bytes = Vec::new();
            encode_frame(&frame, &mut bytes).unwrap();

            let mut decoder = FrameDecoder::new();
            decoder.push(&bytes);
            prop_assert_eq!(decoder.next_frame().unwrap(), Some(frame));
            prop_assert_eq!(decoder.next_frame().unwrap(), None);
        }

        /// The single most important property (§2 rule 6): `next_frame` must never panic, no
        /// matter what bytes a peer sends or how they are chunked across `push` calls.
        #[test]
        fn decoder_never_panics_on_arbitrary_bytes(
            chunks in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..64), 0..32),
        ) {
            let mut decoder = FrameDecoder::new();
            'chunks: for chunk in chunks {
                decoder.push(&chunk);
                // Drain everything decodable. On an error, a real caller closes the
                // connection instead of calling `next_frame` again, so stop here too.
                loop {
                    match decoder.next_frame() {
                        Ok(Some(_)) => continue,
                        Ok(None) => continue 'chunks,
                        Err(_) => break 'chunks,
                    }
                }
            }
        }
    }
}
