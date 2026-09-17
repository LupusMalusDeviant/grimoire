//! [`EngineLink`]: the engine side of one debug link, driven without blocking from the engine's
//! frame loop (contract §13 "Handshake", "Nach dem Handshake", §9.7; Plan 0002 WP8.4).
//!
//! [`crate::accept_handshake`] blocks its thread until a `Hello` arrives, which suits a dedicated
//! connection thread but not a frame loop. `EngineLink` applies the same handshake steps
//! (`handshake::check_first_frame`) and the same post-handshake dispatch ([`crate::Session`]) to
//! whatever frames one [`DebugTransport::poll`] returns, and reports the outcome as
//! [`LinkEvent`]s. Socket IO, the pre-handshake one-frame gate and the handshake timeout stay in
//! the transport (the TCP transport's IO thread); the link only ever touches decoded frames, so the
//! caller decides at which frame or tick boundary a message takes effect.

use crate::ProtocolError;
use crate::frame::Frame;
use crate::generated::debug_protocol::{ErrorCode, ErrorMsg};
use crate::handshake::{
    AcceptedHandshake, EngineIdentity, HandshakeError, PostHandshakeOutcome, Session,
    check_first_frame, engine_hello, engine_session,
};
use crate::message::Message;
use crate::transport::{DebugTransport, TransportError};

/// What one [`EngineLink::poll`] observed, in the order it happened.
///
/// `#[non_exhaustive]`: new kinds of event are additive (contract §2 rule 13).
#[non_exhaustive]
#[derive(Clone, PartialEq, Debug)]
pub enum LinkEvent {
    /// A tool completed the handshake; the engine's `Hello` is already sent.
    /// [`AcceptedHandshake::build_hash_unknown_warning`] asks the caller to log a warning.
    Connected(AcceptedHandshake),
    /// A connection attempt was rejected (contract §13 "Handshake" steps 1-7): the matching
    /// `Error` was sent and the connection closed.
    Rejected(HandshakeError),
    /// A message for the engine to act on, with the `seq` of its frame for the reply's
    /// `in_reply_to`. In protocol v1 that is `SwapSigilUnit`; the link answers `SigilPreview`
    /// itself with `Error(NotSupported)` and every misaddressed or undecodable frame with the
    /// `Error` the contract names, so neither appears here.
    Message {
        /// `seq` of the frame the message arrived in.
        seq: u32,
        /// The decoded message.
        message: Message,
    },
    /// The tool sent an `Error`. Logged by the caller, never answered (contract §13: no error
    /// ping-pong).
    PeerError(ErrorMsg),
    /// The connection of a completed handshake ended: the tool left (`error: None`) or a transport
    /// error closed it.
    Disconnected {
        /// The transport error that ended the connection, if any.
        error: Option<TransportError>,
    },
}

/// Why [`EngineLink::send`] sent nothing.
///
/// `#[non_exhaustive]`: new failure modes are additive (contract §2 rule 13).
#[non_exhaustive]
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
pub enum LinkError {
    /// No handshake is completed, so there is no tool to send to.
    #[error("no tool is connected to the debug link")]
    NotConnected,
    /// The message does not fit its documented limits; the link stays connected (contract §9.7:
    /// the engine skips this one message).
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    /// The transport failed (for example [`TransportError::QueueFull`]); the link disconnected
    /// (contract §9.7: transport errors only end the link).
    #[error(transparent)]
    Transport(#[from] TransportError),
}

/// Where the connection of an [`EngineLink`] stands.
#[derive(Debug)]
enum State {
    /// No handshake yet: the next frame is the connection's first.
    AwaitingHello,
    /// Handshake completed.
    Connected {
        session: Session,
        stats_interval_frames: u16,
    },
    /// The connection is over (rejected, failed or left). Frames still buffered from it are
    /// discarded until the transport reports its end through `poll` returning
    /// [`TransportError::Disconnected`]; a transport that never reconnects (an in-process pair)
    /// stays here.
    Closed,
}

/// The engine side of one debug link (contract §13, Plan 0002 WP8.4): handshake, dispatch and
/// replies over any [`DebugTransport`], without ever blocking.
///
/// Call [`EngineLink::poll`] once at a frame boundary and act on the returned events at the
/// boundary the contract names (a `SwapSigilUnit` before the next `Simulation::step`); send
/// `SwapAck`, `Stats` and `Log` with [`EngineLink::send`]. Every reply the protocol prescribes
/// without engine state (errors for misaddressed frames, `NotSupported` for `SigilPreview`) the
/// link sends itself. No input makes it panic.
pub struct EngineLink {
    transport: Box<dyn DebugTransport>,
    identity: EngineIdentity,
    state: State,
    /// `seq` of the next frame this side sends; restarts at 1 with every connection.
    next_seq: u32,
    inbox: Vec<Frame>,
}

impl std::fmt::Debug for EngineLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineLink")
            .field("engine_version", &self.identity.engine_version)
            .field("build_hash", &self.identity.build_hash)
            .field("state", &self.state)
            .field("next_seq", &self.next_seq)
            .finish_non_exhaustive()
    }
}

impl EngineLink {
    /// A link over `transport` that accepts tools presenting `identity`'s token, engine version
    /// and build hash (contract §13 "Handshake").
    pub fn new(transport: Box<dyn DebugTransport>, identity: EngineIdentity) -> Self {
        Self {
            transport,
            identity,
            state: State::AwaitingHello,
            next_seq: 1,
            inbox: Vec::new(),
        }
    }

    /// Whether a tool has completed the handshake and is still connected.
    pub fn is_connected(&self) -> bool {
        matches!(self.state, State::Connected { .. })
    }

    /// The `Stats` cadence the connected tool asked for in its `Hello` (`0` = no `Stats`, also
    /// while no tool is connected).
    pub fn stats_interval_frames(&self) -> u16 {
        match self.state {
            State::Connected {
                stats_interval_frames,
                ..
            } => stats_interval_frames,
            _ => 0,
        }
    }

    /// Reads every frame the transport has, runs the handshake or the post-handshake dispatch on
    /// each in arrival order, sends the replies the protocol prescribes and appends what the
    /// engine has to act on to `events`. Never blocks, never panics.
    pub fn poll(&mut self, events: &mut Vec<LinkEvent>) {
        let result = self.transport.poll(&mut self.inbox);
        let frames = std::mem::take(&mut self.inbox);
        for frame in &frames {
            self.handle_frame(frame, events);
        }
        // Keep the buffer's capacity for the next poll.
        self.inbox = frames;
        self.inbox.clear();

        match result {
            // The transport reports the end of a connection once, after that connection's frames:
            // the next frame belongs to a new connection (contract §13: a server accepts again).
            Err(TransportError::Disconnected) => {
                if self.is_connected() {
                    events.push(LinkEvent::Disconnected { error: None });
                }
                self.state = State::AwaitingHello;
                self.next_seq = 1;
            }
            Err(error) => self.close(Some(error), events),
            Ok(()) => {
                if self.is_connected() && !self.transport.is_connected() {
                    self.close(None, events);
                }
            }
        }
    }

    /// Sends `message` to the connected tool and returns the `seq` of its frame.
    ///
    /// # Errors
    /// - [`LinkError::NotConnected`] without a completed handshake;
    /// - [`LinkError::Protocol`] if the message exceeds a documented limit (nothing is sent, the
    ///   link stays connected);
    /// - [`LinkError::Transport`] if the transport fails; the link is disconnected.
    pub fn send(&mut self, message: &Message) -> Result<u32, LinkError> {
        if !self.is_connected() {
            return Err(LinkError::NotConnected);
        }
        let seq = self.next_seq;
        let frame = message.to_frame(seq)?;
        if let Err(error) = self.transport.send(&frame) {
            self.transport.disconnect();
            self.state = State::Closed;
            return Err(LinkError::Transport(error));
        }
        self.next_seq = self.next_seq.wrapping_add(1).max(1);
        Ok(seq)
    }

    /// Closes the current connection, if any. Idempotent.
    pub fn disconnect(&mut self) {
        self.transport.disconnect();
        if !matches!(self.state, State::AwaitingHello) {
            self.state = State::Closed;
        }
    }

    fn handle_frame(&mut self, frame: &Frame, events: &mut Vec<LinkEvent>) {
        let outcome = match &self.state {
            State::Closed => return,
            State::AwaitingHello => {
                self.accept(frame, events);
                return;
            }
            State::Connected { session, .. } => session.dispatch_frame(frame),
        };
        match outcome {
            PostHandshakeOutcome::Message(Message::SigilPreview(_)) => {
                // Contract §13 "SigilPreview": answered with NotSupported in P1.
                self.reply(ErrorMsg {
                    code: ErrorCode::NotSupported,
                    in_reply_to: frame.seq,
                    message: "SigilPreview is not supported by this engine".to_owned(),
                });
            }
            PostHandshakeOutcome::Message(message) => events.push(LinkEvent::Message {
                seq: frame.seq,
                message,
            }),
            PostHandshakeOutcome::Reply(error) => self.reply(error),
            PostHandshakeOutcome::LoggedError(error) => events.push(LinkEvent::PeerError(error)),
        }
    }

    /// Handshake steps 1-8 for the first frame of a connection (contract §13).
    fn accept(&mut self, frame: &Frame, events: &mut Vec<LinkEvent>) {
        match check_first_frame(frame, &self.identity) {
            Ok(accepted) => {
                let stats_interval_frames = accepted.peer_hello.stats_interval_frames;
                let sent = engine_hello(&self.identity)
                    .to_frame(1)
                    .map_err(TransportError::from)
                    .and_then(|reply| self.transport.send(&reply));
                match sent {
                    Ok(()) => {
                        self.next_seq = 2;
                        self.state = State::Connected {
                            session: engine_session(),
                            stats_interval_frames,
                        };
                        events.push(LinkEvent::Connected(accepted));
                    }
                    Err(error) => {
                        self.transport.disconnect();
                        self.state = State::Closed;
                        events.push(LinkEvent::Rejected(HandshakeError::Transport(error)));
                    }
                }
            }
            Err(rejection) => {
                let _ = self.transport.send(&rejection.reply);
                self.transport.disconnect();
                self.state = State::Closed;
                events.push(LinkEvent::Rejected(rejection.error));
            }
        }
    }

    /// Sends a protocol-prescribed `Error` reply; a failure disconnects like any send.
    fn reply(&mut self, error: ErrorMsg) {
        let _ = self.send(&Message::Error(error));
    }

    fn close(&mut self, error: Option<TransportError>, events: &mut Vec<LinkEvent>) {
        let was_connected = self.is_connected();
        self.transport.disconnect();
        if !matches!(self.state, State::Closed) {
            self.state = State::Closed;
        }
        if was_connected {
            events.push(LinkEvent::Disconnected { error });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::generated::debug_protocol::{Hello, PeerRole, SigilPreview, SwapSigilUnit};
    use crate::handshake::{ToolIdentity, connect_handshake};
    use crate::{InProcessOptions, InProcessTransport, MessageId, encode_frame};

    fn engine_identity() -> EngineIdentity {
        EngineIdentity::new("0.4.0".to_owned(), "b".repeat(40), [0x42; 32])
    }

    fn tool_hello(token: [u8; 32], stats_interval_frames: u16) -> Message {
        Message::Hello(Hello {
            protocol_version: crate::PROTOCOL_VERSION,
            role: PeerRole::Tool,
            engine_version: "0.4.0".to_owned(),
            build_hash: "b".repeat(40),
            token,
            stats_interval_frames,
        })
    }

    fn link_pair() -> (EngineLink, InProcessTransport) {
        let (engine, tool) = InProcessTransport::pair();
        (EngineLink::new(Box::new(engine), engine_identity()), tool)
    }

    fn receive(tool: &mut InProcessTransport) -> Vec<Message> {
        let mut frames = Vec::new();
        let _ = tool.poll(&mut frames);
        frames
            .iter()
            .map(|frame| Message::from_frame(frame).expect("the engine sends valid messages"))
            .collect()
    }

    fn log() -> Message {
        Message::Log(crate::LogMsg {
            level: 3,
            tick: 0,
            target: "test".to_owned(),
            text: String::new(),
        })
    }

    fn swap(seq: u32) -> Frame {
        Message::SwapSigilUnit(SwapSigilUnit {
            unit_path: "fixtures/a.sigil".to_owned(),
            unit_bytes: vec![1, 2, 3],
        })
        .to_frame(seq)
        .unwrap()
    }

    #[test]
    fn a_matching_hello_connects_and_later_messages_become_events() {
        let (mut link, mut tool) = link_pair();
        let mut events = Vec::new();
        link.poll(&mut events);
        assert!(events.is_empty(), "nothing arrived yet");

        tool.send(&tool_hello([0x42; 32], 30).to_frame(1).unwrap())
            .unwrap();
        tool.send(&swap(2)).unwrap(); // pipelined behind the Hello
        link.poll(&mut events);

        assert!(
            matches!(&events[0], LinkEvent::Connected(accepted) if !accepted.build_hash_unknown_warning)
        );
        assert!(matches!(
            &events[1],
            LinkEvent::Message {
                seq: 2,
                message: Message::SwapSigilUnit(_)
            }
        ));
        assert!(link.is_connected());
        assert_eq!(link.stats_interval_frames(), 30);

        let replies = receive(&mut tool);
        assert!(
            matches!(&replies[..], [Message::Hello(hello)] if hello.role == PeerRole::Engine && hello.token == [0; 32])
        );
    }

    #[test]
    fn the_link_and_the_blocking_tool_handshake_agree() {
        let (engine, mut tool) = InProcessTransport::pair();
        let mut link = EngineLink::new(Box::new(engine), engine_identity());
        let identity = ToolIdentity::new("0.4.0".to_owned(), "unknown".to_owned(), [0x42; 32], 0);
        let tool_thread = std::thread::spawn(move || {
            connect_handshake(&mut tool, &identity, Duration::from_secs(5)).map(|(hello, _)| hello)
        });
        let mut events = Vec::new();
        while events.is_empty() {
            link.poll(&mut events);
            std::thread::sleep(Duration::from_millis(1));
        }
        let hello = tool_thread
            .join()
            .unwrap()
            .expect("tool handshake succeeds");
        assert_eq!(hello.build_hash, "b".repeat(40));
        assert!(
            matches!(&events[..], [LinkEvent::Connected(accepted)] if accepted.build_hash_unknown_warning)
        );
    }

    #[test]
    fn a_wrong_token_is_rejected_with_unauthorized_and_closes() {
        let (mut link, mut tool) = link_pair();
        tool.send(&tool_hello([0; 32], 0).to_frame(1).unwrap())
            .unwrap();
        tool.send(&swap(2)).unwrap();
        let mut events = Vec::new();
        link.poll(&mut events);

        assert_eq!(
            events,
            [LinkEvent::Rejected(HandshakeError::Unauthorized)],
            "the frame behind a rejected Hello is discarded"
        );
        assert!(!link.is_connected());
        assert!(!tool.is_connected());
        assert!(matches!(
            &receive(&mut tool)[..],
            [Message::Error(error)] if error.code == ErrorCode::Unauthorized && error.in_reply_to == 1
        ));

        // A closed in-process link stays closed.
        link.poll(&mut events);
        assert_eq!(events.len(), 1);
        assert_eq!(link.send(&log()), Err(LinkError::NotConnected));
    }

    #[test]
    fn a_first_frame_that_is_not_hello_is_rejected() {
        let (mut link, mut tool) = link_pair();
        tool.send(&swap(1)).unwrap();
        let mut events = Vec::new();
        link.poll(&mut events);
        assert_eq!(
            events,
            [LinkEvent::Rejected(HandshakeError::HandshakeRequired)]
        );
    }

    #[test]
    fn after_the_handshake_misaddressed_frames_are_answered_and_the_link_stays() {
        let (mut link, mut tool) = link_pair();
        tool.send(&tool_hello([0x42; 32], 0).to_frame(1).unwrap())
            .unwrap();
        let mut events = Vec::new();
        link.poll(&mut events);
        receive(&mut tool);

        let preview = Message::SigilPreview(SigilPreview {
            unit_path: "fixtures/a.sigil".to_owned(),
            unit_bytes: vec![],
            ticks: 10,
            target: None,
        });
        tool.send(&preview.to_frame(2).unwrap()).unwrap();
        tool.send(&Frame {
            id: MessageId(0x0300),
            seq: 3,
            payload: vec![],
        })
        .unwrap();
        tool.send(&Frame {
            id: MessageId(0x0020),
            seq: 4,
            payload: vec![0xFF],
        })
        .unwrap();
        tool.send(
            &Message::Error(ErrorMsg {
                code: ErrorCode::Internal,
                in_reply_to: 0,
                message: "tool trouble".to_owned(),
            })
            .to_frame(5)
            .unwrap(),
        )
        .unwrap();
        events.clear();
        link.poll(&mut events);

        assert!(
            matches!(&events[..], [LinkEvent::PeerError(error)] if error.message == "tool trouble")
        );
        let codes: Vec<(ErrorCode, u32)> = receive(&mut tool)
            .into_iter()
            .map(|message| match message {
                Message::Error(error) => (error.code, error.in_reply_to),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(
            codes,
            [
                (ErrorCode::NotSupported, 2),
                (ErrorCode::NotSupported, 3),
                (ErrorCode::Malformed, 4)
            ]
        );
        assert!(link.is_connected());
    }

    #[test]
    fn replies_count_their_own_seq_from_one_per_connection() {
        let (mut link, mut tool) = link_pair();
        tool.send(&tool_hello([0x42; 32], 0).to_frame(1).unwrap())
            .unwrap();
        let mut events = Vec::new();
        link.poll(&mut events);
        assert_eq!(link.send(&log()), Ok(2));
        assert_eq!(link.send(&log()), Ok(3));
        let mut frames = Vec::new();
        tool.poll(&mut frames).unwrap();
        let seqs: Vec<u32> = frames.iter().map(|frame| frame.seq).collect();
        assert_eq!(seqs, [1, 2, 3]);
    }

    #[test]
    fn a_message_over_its_limits_is_skipped_and_the_link_stays() {
        let (mut link, mut tool) = link_pair();
        tool.send(&tool_hello([0x42; 32], 0).to_frame(1).unwrap())
            .unwrap();
        let mut events = Vec::new();
        link.poll(&mut events);
        let mut long = log();
        if let Message::Log(message) = &mut long {
            message.text = "x".repeat(4097);
        }
        assert!(matches!(
            link.send(&long),
            Err(LinkError::Protocol(ProtocolError::FieldTooLong { .. }))
        ));
        assert!(link.is_connected());
    }

    #[test]
    fn the_tool_leaving_is_a_disconnect_event() {
        let (mut link, mut tool) = link_pair();
        tool.send(&tool_hello([0x42; 32], 0).to_frame(1).unwrap())
            .unwrap();
        let mut events = Vec::new();
        link.poll(&mut events);
        tool.disconnect();
        events.clear();
        link.poll(&mut events);
        assert_eq!(events, [LinkEvent::Disconnected { error: None }]);
        assert!(!link.is_connected());
    }

    #[test]
    fn a_connection_ending_mid_frame_never_delivers_the_partial_frame() {
        let options = InProcessOptions::default();
        let (engine, mut tool) = InProcessTransport::pair_with(options);
        let mut link = EngineLink::new(Box::new(engine), engine_identity());
        tool.send(&tool_hello([0x42; 32], 0).to_frame(1).unwrap())
            .unwrap();
        let mut events = Vec::new();
        link.poll(&mut events);

        let mut bytes = Vec::new();
        encode_frame(&swap(2), &mut bytes).unwrap();
        tool.send_bytes(&bytes[..bytes.len() / 2]).unwrap();
        tool.disconnect();
        events.clear();
        link.poll(&mut events);
        assert_eq!(events, [LinkEvent::Disconnected { error: None }]);
    }

    #[test]
    fn garbage_and_over_length_bytes_close_the_link_without_panicking() {
        for garbage in [
            3u32.to_le_bytes().to_vec(), // shorter than the header
            (crate::MAX_FRAME_LEN + 1).to_le_bytes().to_vec(), // longer than any frame
        ] {
            let (mut link, mut tool) = link_pair();
            tool.send(&tool_hello([0x42; 32], 0).to_frame(1).unwrap())
                .unwrap();
            let mut events = Vec::new();
            link.poll(&mut events);
            tool.send_bytes(&garbage).unwrap();
            events.clear();
            link.poll(&mut events);
            assert!(matches!(
                &events[..],
                [LinkEvent::Disconnected {
                    error: Some(TransportError::Protocol(_))
                }]
            ));
            assert!(!tool.is_connected());
            let mut frames = Vec::new();
            let _ = tool.poll(&mut frames); // the tool side survives too
        }
    }

    #[test]
    fn an_over_length_first_frame_is_too_large() {
        let (mut link, mut tool) = link_pair();
        let mut hello = tool_hello([0x42; 32], 0).to_frame(1).unwrap();
        hello.payload.resize(crate::MAX_HELLO_FRAME_LEN as usize, 0);
        tool.send(&hello).unwrap();
        let mut events = Vec::new();
        link.poll(&mut events);
        assert_eq!(events, [LinkEvent::Rejected(HandshakeError::TooLarge)]);
        assert!(matches!(
            &receive(&mut tool)[..],
            [Message::Error(error)] if error.code == ErrorCode::TooLarge
        ));
    }

    mod properties {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn arbitrary_bytes_after_a_handshake_never_panic(
                chunks in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..64), 0..8),
            ) {
                let (mut link, mut tool) = link_pair();
                tool.send(&tool_hello([0x42; 32], 0).to_frame(1).unwrap()).unwrap();
                let mut events = Vec::new();
                for chunk in &chunks {
                    let _ = tool.send_bytes(chunk);
                    link.poll(&mut events);
                }
                let _ = link.send(&log());
            }

            #[test]
            fn arbitrary_first_frames_never_panic(
                id in any::<u16>(),
                payload in proptest::collection::vec(any::<u8>(), 0..128),
            ) {
                let (mut link, mut tool) = link_pair();
                tool.send(&Frame { id: MessageId(id), seq: 1, payload }).unwrap();
                let mut events = Vec::new();
                link.poll(&mut events);
                prop_assert!(events.len() <= 1);
            }
        }
    }
}
