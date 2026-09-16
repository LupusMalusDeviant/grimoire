//! The message catalogue's dispatch layer (contract §13 "Nachrichtenkatalog"): the [`Message`]
//! enum and its [`Message::id`]/[`Message::to_frame`]/[`Message::from_frame`], built on top of
//! the generated payload types (project ADR-0011, WP8.1) and the frame envelope (`frame.rs`,
//! WP1.3). Hand-written per the crate's own scope note (control flow with a handful of message
//! kinds, not the wire-format vocabulary the schema compiler exists to describe): Plan-0002
//! WP8.2.
//!
//! This module knows only how to turn a [`Message`] into a [`Frame`] and back by message id. It
//! does *not* know about handshake state or which direction is valid for the peer currently
//! receiving a frame — that is session-level logic in `handshake.rs`, which uses this module's
//! `classify` and `Direction::valid_incoming_for` (both kept `pub(crate)` since they are
//! meaningless without that session context).

use crate::frame::{Frame, MessageId, ProtocolError};
use crate::generated::debug_protocol::{
    ErrorMsg, Hello, LogMsg, SigilPreview, Stats, SwapAck, SwapSigilUnit, catalogue,
};

/// One message of the debug protocol v1 catalogue (contract §13), carrying its decoded payload.
///
/// `#[non_exhaustive]`: the catalogue's reserved ranges (entity inspection, replay control,
/// non-Sigil asset hot-swap) and the application-defined range have no payload type yet and so no
/// variant here; a later protocol version adds them additively (§2b) once something produces
/// them. A v1 peer's own behaviour for those ids today is `Error(UnknownMessage)` /
/// `Error(NotSupported)`, handled by the session dispatch in `handshake.rs` without ever needing
/// a `Message` variant for them.
#[non_exhaustive]
#[derive(Clone, PartialEq, Debug)]
pub enum Message {
    /// Handshake request/response (id `0x0001`, `T↔E`).
    Hello(Hello),
    /// Protocol- or application-level error (id `0x0002`, `T↔E`).
    Error(ErrorMsg),
    /// One forwarded engine log line (id `0x0003`, `E→T`).
    Log(LogMsg),
    /// Per-frame profiler snapshot (id `0x0010`, `E→T`). Built by the facade from `FrameProfile`
    /// (Plan-0002 WP6.3, not this crate's job); this crate only encodes/decodes the payload.
    Stats(Stats),
    /// Hot-swap request (id `0x0020`, `T→E`). Decoding one here does not apply it: queueing at
    /// the next tick boundary and calling `replace_unit` is the facade's job (Plan-0002 WP8.4).
    SwapSigilUnit(SwapSigilUnit),
    /// Hot-swap acknowledgement (id `0x0021`, `E→T`).
    SwapAck(SwapAck),
    /// Offline preview request (id `0x0022`, `T→E`). In protocol v1 the engine always answers
    /// `Error(NotSupported)` (contract §13); this crate only implements the type and its codec,
    /// per the plan's explicit note that P1 does not build the preview behaviour itself.
    SigilPreview(SigilPreview),
}

/// Which side(s) of a debug-link connection may send a catalogue message
/// (`schema/debug_protocol_v1.gschema`'s `dir` field on each `message` declaration). The
/// generator emits this only as a doc comment on [`catalogue`]'s constants (project ADR-0011
/// point 1: direction *checking* is session dispatch logic, not a wire-format vocabulary), so it
/// is hand-written here, matching the schema file exactly.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Direction {
    /// May be sent by either peer (`Hello`, `Error`).
    Both,
    /// Sent by the tool, received by the engine.
    ToolToEngine,
    /// Sent by the engine, received by the tool.
    EngineToTool,
}

impl Direction {
    /// Whether a message with this direction may be *received* by a peer acting as `local_role`
    /// (contract §13 "Nach dem Handshake": "Katalognachricht in falscher Richtung").
    pub(crate) fn valid_incoming_for(self, local_role: crate::PeerRole) -> bool {
        use crate::PeerRole;
        match (self, local_role) {
            (Direction::Both, _) => true,
            (Direction::ToolToEngine, PeerRole::Engine) => true,
            (Direction::EngineToTool, PeerRole::Tool) => true,
            (Direction::ToolToEngine, PeerRole::Tool)
            | (Direction::EngineToTool, PeerRole::Engine) => false,
        }
    }
}

/// How a message id classifies for the "Nach dem Handshake" dispatch rules (contract §13): a
/// catalogue member with its declared direction, or one of the four ways an id can be
/// unrecognised, each of which gets its own [`crate::ErrorCode`] reply (session-level concern,
/// `handshake.rs`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum IdClass {
    /// `0x0000`, explicitly invalid.
    Zero,
    /// A catalogue message, with its direction.
    Known(Direction),
    /// Not in the v1 catalogue and not reserved: a future protocol version could assign it.
    Free,
    /// In a reserved range (entity inspection, replay control, non-Sigil asset hot-swap).
    Reserved,
    /// In the application-defined range (`0x8000..=0xFFFF`).
    Application,
}

/// Classifies `id` per the message catalogue (contract §13 "Nachrichtenkatalog",
/// "Versionierung"). `pub(crate)`: meaningful only to the session dispatch in `handshake.rs`.
pub(crate) fn classify(id: u16) -> IdClass {
    match id {
        0x0000 => IdClass::Zero,
        catalogue::HELLO | catalogue::ERROR => IdClass::Known(Direction::Both),
        catalogue::LOG | catalogue::STATS | catalogue::SWAP_ACK => {
            IdClass::Known(Direction::EngineToTool)
        }
        catalogue::SWAP_SIGIL_UNIT | catalogue::SIGIL_PREVIEW => {
            IdClass::Known(Direction::ToolToEngine)
        }
        // Contract §13 names these as three separate reserved bands — entity inspection
        // (0x0100-0x01FF), replay control (0x0200-0x02FF), non-Sigil asset hot-swap
        // (0x0300-0x03FF) — but they are contiguous with no unassigned gap between them, so one
        // range covers exactly the same ids as the three separate patterns would.
        0x0100..=0x03FF => IdClass::Reserved,
        0x8000..=0xFFFF => IdClass::Application,
        _ => IdClass::Free,
    }
}

impl Message {
    /// The catalogue id this message is framed under (contract §13).
    pub fn id(&self) -> MessageId {
        MessageId(match self {
            Message::Hello(_) => catalogue::HELLO,
            Message::Error(_) => catalogue::ERROR,
            Message::Log(_) => catalogue::LOG,
            Message::Stats(_) => catalogue::STATS,
            Message::SwapSigilUnit(_) => catalogue::SWAP_SIGIL_UNIT,
            Message::SwapAck(_) => catalogue::SWAP_ACK,
            Message::SigilPreview(_) => catalogue::SIGIL_PREVIEW,
        })
    }

    /// Encodes this message's payload and wraps it in a [`Frame`] with the given `seq` (contract
    /// §13; `seq` is the *sender's own* increasing per-connection counter, unrelated to any
    /// `in_reply_to` field a payload may carry — see [`Frame::seq`]'s own docs).
    ///
    /// Fails only if a field of the payload exceeds its documented maximum length or count
    /// (contract §2 rule 9); never panics.
    pub fn to_frame(&self, seq: u32) -> Result<Frame, ProtocolError> {
        let mut payload = Vec::new();
        match self {
            Message::Hello(message) => message.encode(&mut payload)?,
            Message::Error(message) => message.encode(&mut payload)?,
            Message::Log(message) => message.encode(&mut payload)?,
            Message::Stats(message) => message.encode(&mut payload)?,
            Message::SwapSigilUnit(message) => message.encode(&mut payload)?,
            Message::SwapAck(message) => message.encode(&mut payload)?,
            Message::SigilPreview(message) => message.encode(&mut payload)?,
        }
        Ok(Frame {
            id: self.id(),
            seq,
            payload,
        })
    }

    /// Decodes `frame` as a catalogue message purely by id (contract §13 "Versionierung").
    ///
    /// Returns [`ProtocolError::UnknownMessage`] uniformly for id `0x0000`, a free id, a reserved
    /// range, and the application-defined range — the contract text names all four as producing
    /// this single case ("liefert ... `ProtocolError::UnknownMessage(id)`"); picking the specific
    /// [`crate::ErrorCode`] to reply with (`Malformed`/`UnknownMessage`/`NotSupported`) is
    /// session-level dispatch, done by the crate-private dispatch in `handshake.rs` via this
    /// module's own (private) `classify`, not here. A *decode failure* of a known id's payload is a
    /// different case entirely: it surfaces as whatever [`ProtocolError`] the payload's own
    /// `decode` returned, which the session layer maps to `Malformed`. Never panics on any input.
    pub fn from_frame(frame: &Frame) -> Result<Message, ProtocolError> {
        match frame.id.0 {
            catalogue::HELLO => Ok(Message::Hello(Hello::decode(&frame.payload)?)),
            catalogue::ERROR => Ok(Message::Error(ErrorMsg::decode(&frame.payload)?)),
            catalogue::LOG => Ok(Message::Log(LogMsg::decode(&frame.payload)?)),
            catalogue::STATS => Ok(Message::Stats(Stats::decode(&frame.payload)?)),
            catalogue::SWAP_SIGIL_UNIT => Ok(Message::SwapSigilUnit(SwapSigilUnit::decode(
                &frame.payload,
            )?)),
            catalogue::SWAP_ACK => Ok(Message::SwapAck(SwapAck::decode(&frame.payload)?)),
            catalogue::SIGIL_PREVIEW => {
                Ok(Message::SigilPreview(SigilPreview::decode(&frame.payload)?))
            }
            _ => Err(ProtocolError::UnknownMessage(frame.id)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PeerRole;
    use proptest::prelude::*;

    fn sample_hello() -> Hello {
        Hello {
            protocol_version: crate::PROTOCOL_VERSION,
            role: PeerRole::Tool,
            engine_version: "0.1.1".to_owned(),
            build_hash: "unknown".to_owned(),
            token: [0u8; 32],
            stats_interval_frames: 0,
        }
    }

    fn sample_log() -> LogMsg {
        LogMsg {
            level: 3,
            tick: 7,
            target: "grimoire_sim".to_owned(),
            text: "tick advanced".to_owned(),
        }
    }

    // --- id() matches the catalogue -------------------------------------------------------

    #[test]
    fn id_matches_the_catalogue_constant_for_every_variant() {
        assert_eq!(
            Message::Hello(sample_hello()).id(),
            MessageId(catalogue::HELLO)
        );
        assert_eq!(
            Message::Error(ErrorMsg {
                code: crate::ErrorCode::Internal,
                in_reply_to: 0,
                message: String::new(),
            })
            .id(),
            MessageId(catalogue::ERROR)
        );
        assert_eq!(Message::Log(sample_log()).id(), MessageId(catalogue::LOG));
        assert_eq!(
            Message::Stats(Stats::default()).id(),
            MessageId(catalogue::STATS)
        );
        assert_eq!(
            Message::SwapSigilUnit(SwapSigilUnit {
                unit_path: String::new(),
                unit_bytes: Vec::new(),
            })
            .id(),
            MessageId(catalogue::SWAP_SIGIL_UNIT)
        );
        assert_eq!(
            Message::SwapAck(SwapAck::default()).id(),
            MessageId(catalogue::SWAP_ACK)
        );
        assert_eq!(
            Message::SigilPreview(SigilPreview {
                unit_path: String::new(),
                unit_bytes: Vec::new(),
                ticks: 0,
                target: None,
            })
            .id(),
            MessageId(catalogue::SIGIL_PREVIEW)
        );
    }

    // --- Round trips through Frame -----------------------------------------------------------

    macro_rules! round_trip_test {
        ($name:ident, $sample:expr) => {
            #[test]
            fn $name() {
                let message = $sample;
                let frame = message.to_frame(1).expect("encode must succeed");
                assert_eq!(frame.id, message.id());
                assert_eq!(frame.seq, 1);
                let decoded = Message::from_frame(&frame).expect("decode must succeed");
                assert_eq!(decoded, message);
            }
        };
    }

    round_trip_test!(round_trip_hello, Message::Hello(sample_hello()));
    round_trip_test!(
        round_trip_error,
        Message::Error(ErrorMsg {
            code: crate::ErrorCode::VersionMismatch,
            in_reply_to: 3,
            message: "mismatch".to_owned(),
        })
    );
    round_trip_test!(round_trip_log, Message::Log(sample_log()));
    round_trip_test!(round_trip_stats, Message::Stats(Stats::default()));
    round_trip_test!(
        round_trip_swap_sigil_unit,
        Message::SwapSigilUnit(SwapSigilUnit {
            unit_path: "sigils/basic_bolt.sigil".to_owned(),
            unit_bytes: vec![1, 2, 3],
        })
    );
    round_trip_test!(round_trip_swap_ack, Message::SwapAck(SwapAck::default()));
    round_trip_test!(
        round_trip_sigil_preview,
        Message::SigilPreview(SigilPreview {
            unit_path: "sigils/basic_bolt.sigil".to_owned(),
            unit_bytes: vec![9],
            ticks: 10,
            target: Some([1.0, 2.0]),
        })
    );

    // --- from_frame on an unrecognised id ---------------------------------------------------

    #[test]
    fn from_frame_rejects_id_zero_as_unknown_message() {
        let frame = Frame {
            id: MessageId(0x0000),
            seq: 1,
            payload: Vec::new(),
        };
        assert_eq!(
            Message::from_frame(&frame),
            Err(ProtocolError::UnknownMessage(MessageId(0x0000)))
        );
    }

    #[test]
    fn from_frame_rejects_a_free_id() {
        let frame = Frame {
            id: MessageId(0x00FF),
            seq: 1,
            payload: Vec::new(),
        };
        assert_eq!(
            Message::from_frame(&frame),
            Err(ProtocolError::UnknownMessage(MessageId(0x00FF)))
        );
    }

    #[test]
    fn from_frame_rejects_a_reserved_id() {
        for id in [0x0100u16, 0x0200, 0x0300] {
            let frame = Frame {
                id: MessageId(id),
                seq: 1,
                payload: Vec::new(),
            };
            assert_eq!(
                Message::from_frame(&frame),
                Err(ProtocolError::UnknownMessage(MessageId(id)))
            );
        }
    }

    #[test]
    fn from_frame_rejects_an_application_id() {
        let frame = Frame {
            id: MessageId(0x8000),
            seq: 1,
            payload: Vec::new(),
        };
        assert_eq!(
            Message::from_frame(&frame),
            Err(ProtocolError::UnknownMessage(MessageId(0x8000)))
        );
    }

    // --- classify() agrees with the catalogue -------------------------------------------------

    #[test]
    fn classify_matches_the_catalogue_direction_for_every_known_message() {
        assert_eq!(classify(catalogue::HELLO), IdClass::Known(Direction::Both));
        assert_eq!(classify(catalogue::ERROR), IdClass::Known(Direction::Both));
        assert_eq!(
            classify(catalogue::LOG),
            IdClass::Known(Direction::EngineToTool)
        );
        assert_eq!(
            classify(catalogue::STATS),
            IdClass::Known(Direction::EngineToTool)
        );
        assert_eq!(
            classify(catalogue::SWAP_SIGIL_UNIT),
            IdClass::Known(Direction::ToolToEngine)
        );
        assert_eq!(
            classify(catalogue::SWAP_ACK),
            IdClass::Known(Direction::EngineToTool)
        );
        assert_eq!(
            classify(catalogue::SIGIL_PREVIEW),
            IdClass::Known(Direction::ToolToEngine)
        );
    }

    #[test]
    fn classify_matches_zero_free_reserved_and_application_ranges() {
        assert_eq!(classify(0x0000), IdClass::Zero);
        assert_eq!(classify(0x00FF), IdClass::Free);
        assert_eq!(classify(0x0004), IdClass::Free);
        assert_eq!(classify(0x0100), IdClass::Reserved);
        assert_eq!(classify(0x01FF), IdClass::Reserved);
        assert_eq!(classify(0x0200), IdClass::Reserved);
        assert_eq!(classify(0x02FF), IdClass::Reserved);
        assert_eq!(classify(0x0300), IdClass::Reserved);
        assert_eq!(classify(0x03FF), IdClass::Reserved);
        assert_eq!(classify(0x0400), IdClass::Free);
        assert_eq!(classify(0x7FFF), IdClass::Free);
        assert_eq!(classify(0x8000), IdClass::Application);
        assert_eq!(classify(0xFFFF), IdClass::Application);
    }

    #[test]
    fn direction_valid_incoming_for_matches_the_catalogue() {
        assert!(Direction::Both.valid_incoming_for(PeerRole::Tool));
        assert!(Direction::Both.valid_incoming_for(PeerRole::Engine));
        assert!(Direction::ToolToEngine.valid_incoming_for(PeerRole::Engine));
        assert!(!Direction::ToolToEngine.valid_incoming_for(PeerRole::Tool));
        assert!(Direction::EngineToTool.valid_incoming_for(PeerRole::Tool));
        assert!(!Direction::EngineToTool.valid_incoming_for(PeerRole::Engine));
    }

    // --- Proptest: from_frame never panics on arbitrary bytes --------------------------------

    proptest! {
        #[test]
        fn from_frame_never_panics_on_arbitrary_id_and_payload(
            id in any::<u16>(),
            seq in any::<u32>(),
            payload in proptest::collection::vec(any::<u8>(), 0..1024),
        ) {
            let frame = Frame { id: MessageId(id), seq, payload };
            let _ = Message::from_frame(&frame);
        }

        /// Mutating a single byte of a validly encoded catalogue message must never panic either
        /// (contract §2 rule 9: "einzeln veränderten gültigen Eingaben").
        #[test]
        fn from_frame_never_panics_on_single_byte_mutations_of_a_valid_hello_frame(
            index in 0usize..128,
            replacement in any::<u8>(),
        ) {
            let mut frame = Message::Hello(sample_hello()).to_frame(1).unwrap();
            if index < frame.payload.len() {
                frame.payload[index] = replacement;
            }
            let _ = Message::from_frame(&frame);
        }
    }
}
