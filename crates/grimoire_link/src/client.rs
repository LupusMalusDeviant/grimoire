//! [`LinkClient`]: the tool side of one debug-link connection (contract §13 "Handshake", "Nach dem
//! Handshake").
//!
//! Written against `grimoire_debug::DebugTransport`, so the same client drives a real socket
//! ([`crate::tcp::TcpClientTransport`]) and an in-process pair in tests. Unlike the engine's
//! `EngineLink`, a tool may block: [`LinkClient::wait_for_ack`] and
//! [`LinkClient::next_message`] poll in a short sleep loop until their deadline.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use grimoire_debug::{
    DebugTransport, Frame, HandshakeError, Hello, Message, PostHandshakeOutcome, ProtocolError,
    Session, SwapAck, SwapSigilUnit, ToolIdentity, TransportError, connect_handshake,
};
use grimoire_sim::{ENGINE_BUILD, ENGINE_VERSION};

/// How long a waiting call sleeps between polls. Short against any sensible round trip (a swap is
/// answered within a frame or two) and far from busy-spinning a core.
const POLL_SLEEP: Duration = Duration::from_millis(1);

/// Time allowed for the engine's `Hello` (contract §13 [`grimoire_debug::HANDSHAKE_TIMEOUT`]).
pub const CONNECT_TIMEOUT: Duration = grimoire_debug::HANDSHAKE_TIMEOUT;

/// What this tool build presents in the handshake: the engine version and build hash it was built
/// against (contract §13 "Handshake" step 7 — a tool is rebuilt per engine tag), the engine's
/// token, and the `Stats` cadence it asks for (`0` = none).
#[must_use]
pub fn tool_identity(token: [u8; 32], stats_interval_frames: u16) -> ToolIdentity {
    let build_hash = if ENGINE_BUILD.is_known() {
        ENGINE_BUILD.to_hex()
    } else {
        "unknown".to_owned()
    };
    ToolIdentity::new(
        ENGINE_VERSION.to_owned(),
        build_hash,
        token,
        stats_interval_frames,
    )
}

/// Every way talking to an engine can fail.
///
/// `#[non_exhaustive]`: new failure modes are additive (contract §2 rule 13).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LinkClientError {
    /// The handshake failed (rejected, timed out, or the engine answered something else).
    #[error("the engine refused the connection: {0}")]
    Handshake(#[from] HandshakeError),
    /// The transport failed; the connection is gone.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// A message could not be encoded or a frame not decoded.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    /// Nothing expected arrived before the deadline.
    #[error("the engine did not answer within {0:?}")]
    Timeout(Duration),
    /// The engine closed the connection.
    #[error("the engine closed the connection")]
    Closed,
}

/// The tool side of one connection: handshake, sending swaps, receiving `SwapAck`, `Stats` and
/// `Log`.
pub struct LinkClient {
    transport: Box<dyn DebugTransport>,
    session: Session,
    engine: Hello,
    /// `seq` of the next frame this tool sends; the `Hello` used 1.
    next_seq: u32,
    /// Messages received while waiting for something else, in arrival order.
    received: VecDeque<Message>,
    inbox: Vec<Frame>,
}

impl std::fmt::Debug for LinkClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkClient")
            .field("engine_version", &self.engine.engine_version)
            .field("build_hash", &self.engine.build_hash)
            .field("connected", &self.transport.is_connected())
            .field("next_seq", &self.next_seq)
            .field("received", &self.received.len())
            .finish_non_exhaustive()
    }
}

impl LinkClient {
    /// Connects over `transport`: sends this tool's `Hello` ([`tool_identity`]) and waits up to
    /// [`CONNECT_TIMEOUT`] for the engine's. Returns the client and the engine's `Hello`.
    ///
    /// # Errors
    /// [`LinkClientError::Handshake`] if the engine rejects the connection (wrong token, other
    /// engine version, another tool already connected) or does not answer in time,
    /// [`LinkClientError::Transport`] if the transport fails.
    pub fn connect(
        transport: Box<dyn DebugTransport>,
        token: [u8; 32],
        stats_interval_frames: u16,
    ) -> Result<(Self, Hello), LinkClientError> {
        let identity = tool_identity(token, stats_interval_frames);
        let mut transport = transport;
        let (engine, mut session) =
            connect_handshake(transport.as_mut(), &identity, CONNECT_TIMEOUT)?;
        let pending = session.take_pending_frames();
        let mut client = Self {
            transport,
            session,
            engine: engine.clone(),
            next_seq: 2,
            received: VecDeque::new(),
            inbox: Vec::new(),
        };
        // Frames the engine pipelined behind its `Hello` are not lost.
        for frame in &pending {
            client.dispatch(frame)?;
        }
        Ok((client, engine))
    }

    /// The engine's `Hello`: its engine version, build hash and protocol version.
    #[must_use]
    pub fn engine(&self) -> &Hello {
        &self.engine
    }

    /// Whether the transport still has a connection.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.transport.is_connected()
    }

    /// Closes the connection. Idempotent.
    pub fn disconnect(&mut self) {
        self.transport.disconnect();
    }

    /// Sends one message and returns the `seq` of its frame.
    ///
    /// # Errors
    /// [`LinkClientError::Protocol`] if the message exceeds a documented limit (nothing is sent),
    /// [`LinkClientError::Transport`] if the transport fails.
    pub fn send(&mut self, message: &Message) -> Result<u32, LinkClientError> {
        let seq = self.next_seq;
        let frame = message.to_frame(seq)?;
        self.transport.send(&frame)?;
        self.next_seq = self.next_seq.saturating_add(1);
        Ok(seq)
    }

    /// Sends a `SwapSigilUnit` for `unit_path` (contract §13: the canonical content path the
    /// unit's id derives from) and returns the `seq` to match its `SwapAck` against.
    ///
    /// # Errors
    /// As [`LinkClient::send`]; a unit over `grimoire_debug::MAX_UNIT_BYTES` or a path over 255
    /// bytes is [`LinkClientError::Protocol`].
    pub fn swap(&mut self, unit_path: &str, unit_bytes: Vec<u8>) -> Result<u32, LinkClientError> {
        self.send(&Message::SwapSigilUnit(SwapSigilUnit {
            unit_path: unit_path.to_owned(),
            unit_bytes,
        }))
    }

    /// Reads everything the transport has, applies the post-handshake rules and queues the
    /// messages for the engine's side of the conversation. Never blocks.
    ///
    /// # Errors
    /// [`LinkClientError::Transport`] if the transport failed or the engine closed.
    pub fn poll(&mut self) -> Result<(), LinkClientError> {
        let result = self.transport.poll(&mut self.inbox);
        let frames = std::mem::take(&mut self.inbox);
        for frame in &frames {
            self.dispatch(frame)?;
        }
        self.inbox = frames;
        self.inbox.clear();
        result.map_err(LinkClientError::from)
    }

    /// The messages received so far, in arrival order, and clears the queue.
    pub fn take_messages(&mut self) -> Vec<Message> {
        self.received.drain(..).collect()
    }

    /// Waits up to `timeout` for the next message from the engine.
    ///
    /// # Errors
    /// [`LinkClientError::Timeout`] if nothing arrives, [`LinkClientError::Closed`] if the engine
    /// closes the connection, [`LinkClientError::Transport`] on a transport failure.
    pub fn next_message(&mut self, timeout: Duration) -> Result<Message, LinkClientError> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(message) = self.received.pop_front() {
                return Ok(message);
            }
            match self.poll() {
                Ok(()) => {}
                Err(LinkClientError::Transport(TransportError::Disconnected)) => {
                    // Messages decoded before the close are still handed out first.
                    if let Some(message) = self.received.pop_front() {
                        return Ok(message);
                    }
                    return Err(LinkClientError::Closed);
                }
                Err(error) => return Err(error),
            }
            if self.received.is_empty() {
                if Instant::now() >= deadline {
                    return Err(LinkClientError::Timeout(timeout));
                }
                std::thread::sleep(POLL_SLEEP);
            }
        }
    }

    /// Waits up to `timeout` for the `SwapAck` of the swap sent with `seq`. Other messages that
    /// arrive meanwhile stay queued for [`LinkClient::take_messages`].
    ///
    /// # Errors
    /// As [`LinkClient::next_message`].
    pub fn wait_for_ack(
        &mut self,
        seq: u32,
        timeout: Duration,
    ) -> Result<SwapAck, LinkClientError> {
        let deadline = Instant::now() + timeout;
        let mut others = Vec::new();
        let result = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.next_message(left) {
                Ok(Message::SwapAck(ack)) if ack.in_reply_to == seq => break Ok(ack),
                Ok(message) => others.push(message),
                Err(LinkClientError::Timeout(_)) => break Err(LinkClientError::Timeout(timeout)),
                Err(error) => break Err(error),
            }
        };
        // Keep what arrived on the way in arrival order, ahead of anything still queued.
        for message in others.into_iter().rev() {
            self.received.push_front(message);
        }
        result
    }

    /// Applies the "Nach dem Handshake" rules to one frame: a message is queued, a prescribed
    /// `Error` reply is sent, an incoming `Error` is queued for the caller to log.
    fn dispatch(&mut self, frame: &Frame) -> Result<(), LinkClientError> {
        match self.session.dispatch_frame(frame) {
            PostHandshakeOutcome::Message(message) => self.received.push_back(message),
            PostHandshakeOutcome::LoggedError(error) => {
                self.received.push_back(Message::Error(error));
            }
            PostHandshakeOutcome::Reply(error) => {
                self.send(&Message::Error(error))?;
            }
            // `PostHandshakeOutcome` is `#[non_exhaustive]`: a later protocol version may add an
            // outcome this tool does not know yet, which is not a reason to end the connection.
            other => {
                self.received
                    .push_back(Message::Log(grimoire_debug::LogMsg {
                        level: 4,
                        tick: 0,
                        target: "grimoire-link".to_owned(),
                        text: format!("ignored an unknown dispatch outcome: {other:?}"),
                    }));
            }
        }
        Ok(())
    }
}
