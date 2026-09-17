//! The handshake state machine and the post-handshake dispatch rules (contract §13 "Handshake",
//! "Nach dem Handshake", PO decision V-13). Hand-written on top of the message layer
//! (`message.rs`) and the frame envelope (`frame.rs`): Plan-0002 WP8.2.
//!
//! [`accept_handshake`] and [`connect_handshake`] are the two halves of the exchange (contract
//! §13 "Handshake", numbered steps 1-8): the engine accepts a connecting tool's `Hello` and
//! either rejects it or answers with its own; the tool sends its `Hello` first and waits for the
//! engine's reply or an `Error`. Both are written purely against [`DebugTransport`], so they run
//! identically over [`crate::InProcessTransport`] (this crate's own tests) and over any other
//! transport. Each returns a [`Session`] on success — the only way to
//! obtain one, and the only way to reach [`Session::dispatch_frame`] — so "cannot be skipped"
//! falls out of the type system rather than a documented calling convention.
//!
//! Both functions block the calling thread (a short sleep-poll loop) for up to their `timeout`
//! argument, so they suit a dedicated connection thread or a tool, never a frame loop. The engine's
//! frame loop uses [`crate::EngineLink`] instead, which applies the same steps
//! (`check_first_frame`) and the same dispatch without blocking (Plan 0002 WP8.4); the
//! byte-level defences against a not-yet-authenticated socket peer (one frame of at most
//! [`crate::MAX_HELLO_FRAME_LEN`] before the handshake, the [`crate::HANDSHAKE_TIMEOUT`]) live in
//! [`crate::TcpServerTransport`]'s IO thread. [`accept_handshake`] still enforces both
//! semantically for whatever [`Frame`] a transport hands it.

use std::collections::VecDeque;
use std::thread;
use std::time::{Duration, Instant};

use crate::frame::{Frame, MessageId, ProtocolError, peek_hello_version};
use crate::generated::debug_protocol::{ErrorCode, ErrorMsg, Hello, PeerRole, catalogue};
use crate::message::{Direction, IdClass, Message, classify};
use crate::transport::{DebugTransport, TransportError};

/// How long [`accept_handshake`] and [`connect_handshake`] sleep between polls while waiting for
/// more bytes. Short relative to any realistic [`crate::HANDSHAKE_TIMEOUT`], so it does not
/// meaningfully delay a fast local peer (e.g. [`crate::InProcessTransport`] in tests) while still
/// not busy-spinning a whole CPU core.
const POLL_SLEEP: Duration = Duration::from_millis(1);

/// What this build considers its own identity when accepting a connection as the engine
/// (contract §13 "Handshake" step 7-8).
///
/// `#[non_exhaustive]`: built via [`EngineIdentity::new`] plus field assignment (§2 rule 13), so
/// adding a field later is not a breaking change, following [`crate::TcpConfig`]'s pattern.
#[non_exhaustive]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EngineIdentity {
    /// This build's `ENGINE_VERSION` (contract §8.1). Compared byte-for-byte against the
    /// connecting tool's `Hello.engine_version`.
    pub engine_version: String,
    /// This build's `BuildHash::to_hex()`, or the literal `"unknown"` (contract §13, §8.1). Note
    /// that `grimoire_debug` has no dependency edge to `grimoire_sim` (contract §1), which is
    /// where `BuildHash`/`ENGINE_BUILD` actually live: the caller (the facade) computes this
    /// string and passes it in, the same pattern `FrameProfile::to_stats` uses for `StatsFrame`.
    pub build_hash: String,
    /// The token an incoming `Hello` must present, compared in constant time (contract §13
    /// "Handshake" step 6). Typically [`crate::TcpConfig::token`].
    pub token: [u8; 32],
}

impl EngineIdentity {
    /// Builds an engine identity from its three fields (contract §13 "Handshake" step 7-8).
    pub fn new(engine_version: String, build_hash: String, token: [u8; 32]) -> Self {
        Self {
            engine_version,
            build_hash,
            token,
        }
    }
}

/// What this build presents when connecting as a tool (contract §13 "Handshake").
///
/// `#[non_exhaustive]`: built via [`ToolIdentity::new`] plus field assignment (§2 rule 13), so
/// adding a field later is not a breaking change, following [`crate::TcpConfig`]'s pattern.
#[non_exhaustive]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ToolIdentity {
    /// The engine version this tool build targets (project ADR-0011/contract §13: "Werkzeuge
    /// müssen je Tag neu gebaut werden" — a tool is rebuilt per engine patch tag and so has a
    /// definite `engine_version` to declare, not `"unknown"`).
    pub engine_version: String,
    /// This tool build's own build hash, or `"unknown"`.
    pub build_hash: String,
    /// The token to present to the engine.
    pub token: [u8; 32],
    /// `Stats` cadence this tool requests; `0` = no `Stats`.
    pub stats_interval_frames: u16,
}

impl ToolIdentity {
    /// Builds a tool identity from its four fields (contract §13 "Handshake").
    pub fn new(
        engine_version: String,
        build_hash: String,
        token: [u8; 32],
        stats_interval_frames: u16,
    ) -> Self {
        Self {
            engine_version,
            build_hash,
            token,
            stats_interval_frames,
        }
    }
}

/// Successful outcome of [`accept_handshake`]: the tool's `Hello`, plus whether either side's
/// build hash was `"unknown"` (contract §13 "Handshake" step 7: accepted, but logged as a
/// warning by the caller — this crate has no logging dependency of its own, so it only reports
/// the fact).
///
/// `#[non_exhaustive]`: built via [`AcceptedHandshake::new`] plus field assignment (§2 rule 13),
/// following [`crate::TcpConfig`]'s pattern, though in practice only [`accept_handshake`] itself
/// ever needs to construct one.
#[non_exhaustive]
#[derive(Clone, PartialEq, Debug)]
pub struct AcceptedHandshake {
    /// The connecting tool's full `Hello` payload.
    pub peer_hello: Hello,
    /// `true` if this engine's or the tool's `build_hash` was `"unknown"` (contract §13: "wird
    /// die Verbindung angenommen und eine Warnung protokolliert").
    pub build_hash_unknown_warning: bool,
}

impl AcceptedHandshake {
    /// Builds an accepted-handshake outcome from its two fields.
    pub fn new(peer_hello: Hello, build_hash_unknown_warning: bool) -> Self {
        Self {
            peer_hello,
            build_hash_unknown_warning,
        }
    }
}

/// Every way [`accept_handshake`] or [`connect_handshake`] can fail (contract §13 "Handshake"),
/// each with the [`ErrorCode`] the engine sends the tool for it (`Timeout` has none: nothing was
/// sent yet to reply to).
///
/// `#[non_exhaustive]`: a new failure mode is additive (§2b).
#[non_exhaustive]
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
pub enum HandshakeError {
    /// No complete first frame arrived within the timeout (contract §13: the engine sends
    /// `Error(HandshakeRequired)` itself before returning this).
    #[error("no complete Hello frame arrived within the handshake timeout")]
    Timeout,
    /// The first frame's wire length exceeded [`crate::MAX_HELLO_FRAME_LEN`] (step 1).
    #[error("the first frame exceeds MAX_HELLO_FRAME_LEN")]
    TooLarge,
    /// The first frame's id was not `Hello` (step 2).
    #[error("the first frame was not a Hello message")]
    HandshakeRequired,
    /// The `Hello` payload was shorter than 2 bytes (step 3), failed strict decoding, had
    /// trailing bytes, or declared `role != Tool` (step 5); or, for [`connect_handshake`], the
    /// engine's reply itself failed to decode as `Hello`.
    #[error("the Hello payload was malformed")]
    Malformed,
    /// `protocol_version` (step 4), `engine_version`, or a mutually known `build_hash` (step 7)
    /// did not match.
    #[error("protocol version, engine version, or build hash did not match")]
    VersionMismatch,
    /// The token did not match (step 6).
    #[error("the Hello token did not match")]
    Unauthorized,
    /// The peer replied with a second client already connected.
    #[error("the engine reported it is already busy with another client")]
    Busy,
    /// The peer's `Error` reply carried a code this function does not otherwise map (e.g.
    /// `Internal`, `NotSupported`), or [`connect_handshake`] received neither `Hello` nor `Error`.
    #[error("the peer rejected the handshake ({0:?})")]
    Rejected(ErrorCode),
    /// A payload exceeded a documented maximum while encoding this side's own `Hello`.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    /// The transport itself failed.
    #[error(transparent)]
    Transport(#[from] TransportError),
}

/// Compares two 32-byte tokens without branching on the position of the first differing byte
/// (contract §13 "Handshake" step 6: "konstantzeitiger Vergleich"), unlike `==`, whose exact
/// running time can depend on where two byte slices first differ.
fn tokens_match(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// Builds the `Error` frame a peer sends for handshake step `code`/`message`, with `seq = 0`
/// (contract §13 "Framing": "`0` bedeutet 'keine Antwort erwartet'" — fitting, since the
/// connection closes right after every handshake-time `Error`) and `in_reply_to` set to the
/// triggering frame's own `seq` (or `0` for a failure detected before any frame arrived, i.e.
/// [`HandshakeError::Timeout`]).
fn error_frame(code: ErrorCode, in_reply_to: u32, message: impl Into<String>) -> Frame {
    // `ErrorMsg::encode` only fails on an overlong `message` field (max 1024 bytes). Every literal
    // message this module writes is short and far under that limit, but several of them (steps
    // 7's version/build-hash mismatches) interpolate `identity.engine_version`/`build_hash` and
    // the peer's own `Hello` fields, none of which this module bounds itself — an operator or a
    // future caller could configure a pathologically long one. Rather than `.expect`-ing that away
    // as a proven invariant (§2 rule 9: never panic on a value this module does not fully
    // control), fall back to a short, fixed message on the rare encode failure.
    Message::Error(ErrorMsg {
        code,
        in_reply_to,
        message: message.into(),
    })
    .to_frame(0)
    .unwrap_or_else(|_| {
        Message::Error(ErrorMsg {
            code,
            in_reply_to,
            message: "handshake rejected (error detail omitted: too long to encode)".to_owned(),
        })
        .to_frame(0)
        .expect("this fixed fallback message is far under ErrorMsg's length limit")
    })
}

/// Polls `transport` in a bounded loop until it yields at least one inbound frame or `deadline`
/// passes. Returns `Ok(None)` on timeout, never blocks past `deadline` by more than one
/// [`POLL_SLEEP`] tick.
///
/// Both [`crate::InProcessTransport`] and [`crate::TcpServerTransport`] drain every frame they
/// have buffered into a single `poll` call, so the peer's first frame can arrive alongside
/// others it pipelined right behind it (a tool that sends `Hello` and, without waiting for a
/// reply, immediately sends another message). Returning only the first and discarding the rest
/// would lose that second frame permanently, so any surplus frames from the same `poll` call come
/// back too, in the order the transport produced them, for the caller to queue on the resulting
/// [`Session`].
fn poll_for_first_frame(
    transport: &mut dyn DebugTransport,
    deadline: Instant,
) -> Result<Option<(Frame, Vec<Frame>)>, TransportError> {
    loop {
        let mut inbox = Vec::new();
        transport.poll(&mut inbox)?;
        if !inbox.is_empty() {
            let mut frames = inbox.into_iter();
            // `inbox` was just checked non-empty, so this `Vec`-drop-checked first element
            // always exists.
            let first = frames.next().expect("inbox is non-empty");
            let rest = frames.collect();
            return Ok(Some((first, rest)));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(POLL_SLEEP);
    }
}

/// Proof that a handshake completed successfully, for one side of one connection (contract §13
/// "Handshake"): the only way to obtain one is a successful [`accept_handshake`] or
/// [`connect_handshake`], and [`Self::dispatch_frame`] is the only way to reach the
/// `handle_post_handshake_frame` dispatch rules — that free function is `pub(crate)`, not
/// exported from this crate at all. "State machine that cannot be skipped" is therefore a
/// type-level guarantee, not just a documented calling convention: nothing in this crate's public
/// API lets a caller apply the "Nach dem Handshake" rules without first holding a `Session`, and
/// the only way to hold one is to have completed the handshake steps above.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Session {
    local_role: PeerRole,
    /// Frames that arrived pipelined with the frame that completed this handshake (the accepted
    /// `Hello`, or, on the tool side, the engine's reply): both transports in this crate drain
    /// every buffered frame per `poll`, so a peer that writes `Hello` immediately followed by
    /// another message can hand `poll_for_first_frame` more than one frame in the same call.
    /// Dropping the surplus silently would lose it forever, so it is queued here instead and
    /// handed back in order by [`Self::take_pending_frames`].
    pending: VecDeque<Frame>,
}

impl Session {
    /// Applies the "Nach dem Handshake" dispatch rules (contract §13) to one inbound `frame`, as
    /// this session's own role. Delegates to the crate-private `handle_post_handshake_frame`,
    /// which stays a free function too (with no session state of its own) so its per-case
    /// behaviour can be unit tested directly, without going through a full handshake first.
    pub fn dispatch_frame(&self, frame: &Frame) -> PostHandshakeOutcome {
        handle_post_handshake_frame(self.local_role, frame)
    }

    /// Which side of the connection this session is (contract §13 `PeerRole`).
    pub fn local_role(&self) -> PeerRole {
        self.local_role
    }

    /// Returns any frames that were already buffered by the transport alongside the frame that
    /// completed this handshake, in the order they arrived, and clears the queue.
    ///
    /// A tool is free to pipeline a message right after its `Hello` instead of waiting for the
    /// engine's reply first, and a transport's `poll` can likewise hand the engine both in one
    /// call; without this, that trailing message would be silently discarded before a `Session`
    /// even existed to dispatch it through [`Self::dispatch_frame`]. Call this once right after a
    /// successful [`accept_handshake`]/[`connect_handshake`] and dispatch each returned frame, in
    /// order, before polling the transport again. Returns an empty `Vec` in the common case where
    /// nothing was pipelined.
    pub fn take_pending_frames(&mut self) -> Vec<Frame> {
        self.pending.drain(..).collect()
    }
}

/// Runs the engine side of the handshake (contract §13 "Handshake" steps 1-8, PO decision V-13)
/// over `transport`, blocking the calling thread for up to `timeout`.
///
/// On *every* rejection, including a failure to encode or send this engine's own `Hello` at step
/// 8, calls [`DebugTransport::disconnect`] before returning `Err` — there is no path that returns
/// `Err` while leaving the transport connected. Every case but [`HandshakeError::Timeout`] also
/// sends the matching `Error` first (contract §13); `Timeout` has nothing to reply to, so it
/// instead attempts `Error(HandshakeRequired)` best-effort (a transport that never connected,
/// like [`crate::NullTransport`], simply fails that send, which this function ignores).
///
/// On success, has already sent this engine's own `Hello` (`role = Engine`, `token` all zero,
/// frame `seq = 1`); the connection is left open and ready for the returned [`Session`]. If the
/// transport had already buffered further frames behind the accepted `Hello` in the same `poll`
/// (a tool pipelining a message right after its `Hello`), those frames are not lost: they are
/// queued on the returned `Session` and retrievable via [`Session::take_pending_frames`].
pub fn accept_handshake(
    transport: &mut dyn DebugTransport,
    identity: &EngineIdentity,
    timeout: Duration,
) -> Result<(AcceptedHandshake, Session), HandshakeError> {
    let deadline = Instant::now() + timeout;

    let (first, pending) = match poll_for_first_frame(transport, deadline)? {
        Some(pair) => pair,
        None => {
            let _ = transport.send(&error_frame(
                ErrorCode::HandshakeRequired,
                0,
                "handshake timed out waiting for Hello",
            ));
            transport.disconnect();
            return Err(HandshakeError::Timeout);
        }
    };

    let accepted = match check_first_frame(&first, identity) {
        Ok(accepted) => accepted,
        Err(rejection) => {
            let _ = transport.send(&rejection.reply);
            transport.disconnect();
            return Err(rejection.error);
        }
    };

    // Step 8: the engine replies with its own Hello. Both the encode and the send below are
    // disconnected explicitly on failure rather than via a bare `?`, so this step keeps the same
    // "always disconnect before returning Err" guarantee as every step before it.
    let reply_frame = match engine_hello(identity).to_frame(1) {
        Ok(frame) => frame,
        Err(error) => {
            transport.disconnect();
            return Err(HandshakeError::from(error));
        }
    };
    if let Err(error) = transport.send(&reply_frame) {
        transport.disconnect();
        return Err(HandshakeError::from(error));
    }

    Ok((
        accepted,
        Session {
            local_role: PeerRole::Engine,
            pending: pending.into(),
        },
    ))
}

/// A first frame the engine rejects (contract §13 "Handshake" steps 1-7): the error to report and
/// the `Error` frame (`seq = 0`) to send before closing the connection.
pub(crate) struct Rejection {
    pub(crate) error: HandshakeError,
    pub(crate) reply: Frame,
}

impl Rejection {
    fn new(
        error: HandshakeError,
        code: ErrorCode,
        in_reply_to: u32,
        message: impl Into<String>,
    ) -> Self {
        Self {
            error,
            reply: error_frame(code, in_reply_to, message),
        }
    }
}

/// Applies handshake steps 1-7 (contract §13, PO decision V-13) to the first frame of a
/// connection, without touching a transport: [`accept_handshake`] and the non-blocking
/// [`crate::EngineLink`] share it, so both reject exactly the same frames with exactly the same
/// replies. Never panics on any frame.
pub(crate) fn check_first_frame(
    first: &Frame,
    identity: &EngineIdentity,
) -> Result<AcceptedHandshake, Rejection> {
    // Step 1: the wire `len` value (payload length plus the 8-byte header) must not exceed
    // MAX_HELLO_FRAME_LEN. `Frame` no longer carries the raw `len`, but it is exactly
    // `HEADER_LEN + payload.len()` by construction (`FrameDecoder::next_frame`), so this
    // reconstructs it rather than needing the transport to hand over a raw length separately.
    let wire_len = crate::frame::HEADER_LEN as u64 + first.payload.len() as u64;
    if wire_len > u64::from(crate::MAX_HELLO_FRAME_LEN) {
        return Err(Rejection::new(
            HandshakeError::TooLarge,
            ErrorCode::TooLarge,
            first.seq,
            "first frame exceeds MAX_HELLO_FRAME_LEN",
        ));
    }

    // Step 2: the first frame must be a Hello.
    if first.id != MessageId(catalogue::HELLO) {
        return Err(Rejection::new(
            HandshakeError::HandshakeRequired,
            ErrorCode::HandshakeRequired,
            first.seq,
            "the first frame of a connection must be Hello",
        ));
    }

    // Steps 3-4: peek the frozen first field before a full strict decode.
    match peek_hello_version(first) {
        None => {
            return Err(Rejection::new(
                HandshakeError::Malformed,
                ErrorCode::Malformed,
                first.seq,
                "Hello payload is shorter than 2 bytes",
            ));
        }
        Some(version) if version != crate::PROTOCOL_VERSION => {
            return Err(Rejection::new(
                HandshakeError::VersionMismatch,
                ErrorCode::VersionMismatch,
                first.seq,
                format!(
                    "protocol version {version} does not match {}",
                    crate::PROTOCOL_VERSION
                ),
            ));
        }
        Some(_) => {}
    }

    // Step 5: strict decode (no trailing bytes) plus role check.
    let hello = match Hello::decode(&first.payload) {
        Ok(hello) if hello.role == PeerRole::Tool => hello,
        _ => {
            return Err(Rejection::new(
                HandshakeError::Malformed,
                ErrorCode::Malformed,
                first.seq,
                "Hello payload failed to decode, or role was not Tool",
            ));
        }
    };

    // Step 6: constant-time token comparison.
    if !tokens_match(&hello.token, &identity.token) {
        return Err(Rejection::new(
            HandshakeError::Unauthorized,
            ErrorCode::Unauthorized,
            first.seq,
            "Hello token did not match",
        ));
    }

    // Step 7: engine version and build hash, named in the message text only now that the token
    // check has passed (contract §13: "Beide Versionen nennt ein Meldungstext erst nach
    // bestandener Token-Prüfung").
    if hello.engine_version != identity.engine_version {
        return Err(Rejection::new(
            HandshakeError::VersionMismatch,
            ErrorCode::VersionMismatch,
            first.seq,
            format!(
                "engine version {:?} does not match {:?}",
                hello.engine_version, identity.engine_version
            ),
        ));
    }
    let both_build_hashes_known = hello.build_hash != "unknown" && identity.build_hash != "unknown";
    let build_hash_unknown_warning = !both_build_hashes_known;
    if both_build_hashes_known && hello.build_hash != identity.build_hash {
        return Err(Rejection::new(
            HandshakeError::VersionMismatch,
            ErrorCode::VersionMismatch,
            first.seq,
            format!(
                "build hash {:?} does not match {:?}",
                hello.build_hash, identity.build_hash
            ),
        ));
    }

    Ok(AcceptedHandshake::new(hello, build_hash_unknown_warning))
}

/// The engine's reply `Hello` (contract §13 "Handshake" step 8): `role = Engine`, this build's
/// engine version and build hash, a token of only zeros. `stats_interval_frames` only expresses
/// what a *tool* requests, so the engine sends `0`.
pub(crate) fn engine_hello(identity: &EngineIdentity) -> Message {
    Message::Hello(Hello {
        protocol_version: crate::PROTOCOL_VERSION,
        role: PeerRole::Engine,
        engine_version: identity.engine_version.clone(),
        build_hash: identity.build_hash.clone(),
        token: [0u8; 32],
        stats_interval_frames: 0,
    })
}

/// Creates the [`Session`] of an engine whose handshake [`crate::EngineLink`] completed without
/// blocking; frames pipelined behind the `Hello` reach the link through its own inbox instead of
/// the session's pending queue.
pub(crate) fn engine_session() -> Session {
    Session {
        local_role: PeerRole::Engine,
        pending: VecDeque::new(),
    }
}

/// Runs the tool side of the handshake: sends this tool's `Hello` first (frame `seq = 1`), then
/// blocks up to `timeout` for the engine's reply.
///
/// Returns the engine's `Hello` and a [`Session`] on success; any further frames the transport
/// had already buffered behind that reply in the same `poll` are queued on the returned `Session`
/// and retrievable via [`Session::take_pending_frames`], rather than being lost. On an `Error`
/// reply, maps well-known codes to their matching [`HandshakeError`] variant (`Busy`,
/// `Unauthorized`, `VersionMismatch`, `HandshakeRequired`, `TooLarge` all round-trip to the
/// same-named variant)
/// and anything else to [`HandshakeError::Rejected`]. Never sends anything after the initial
/// `Hello` — a rejected handshake is the engine's job to close, not this function's.
pub fn connect_handshake(
    transport: &mut dyn DebugTransport,
    identity: &ToolIdentity,
    timeout: Duration,
) -> Result<(Hello, Session), HandshakeError> {
    let hello = Message::Hello(Hello {
        protocol_version: crate::PROTOCOL_VERSION,
        role: PeerRole::Tool,
        engine_version: identity.engine_version.clone(),
        build_hash: identity.build_hash.clone(),
        token: identity.token,
        stats_interval_frames: identity.stats_interval_frames,
    });
    transport.send(&hello.to_frame(1)?)?;

    let deadline = Instant::now() + timeout;
    let (reply, pending) = match poll_for_first_frame(transport, deadline)? {
        Some(pair) => pair,
        None => return Err(HandshakeError::Timeout),
    };

    match reply.id {
        MessageId(catalogue::HELLO) => {
            let hello = Hello::decode(&reply.payload).map_err(|_| HandshakeError::Malformed)?;
            Ok((
                hello,
                Session {
                    local_role: PeerRole::Tool,
                    pending: pending.into(),
                },
            ))
        }
        MessageId(catalogue::ERROR) => {
            let error = ErrorMsg::decode(&reply.payload).map_err(|_| HandshakeError::Malformed)?;
            Err(match error.code {
                ErrorCode::Busy => HandshakeError::Busy,
                ErrorCode::Unauthorized => HandshakeError::Unauthorized,
                ErrorCode::VersionMismatch => HandshakeError::VersionMismatch,
                ErrorCode::HandshakeRequired => HandshakeError::HandshakeRequired,
                ErrorCode::TooLarge => HandshakeError::TooLarge,
                other => HandshakeError::Rejected(other),
            })
        }
        _ => Err(HandshakeError::Malformed),
    }
}

/// What a peer does with one frame received after the handshake has completed (contract §13
/// "Nach dem Handshake").
///
/// `#[non_exhaustive]`: a new outcome kind (e.g. once a later protocol version gives the
/// application-defined range a receiver) is additive (§2b), matching [`Message`]'s own
/// `#[non_exhaustive]`.
#[non_exhaustive]
#[derive(Clone, PartialEq, Debug)]
pub enum PostHandshakeOutcome {
    /// A correctly addressed, successfully decoded catalogue message: hand it to whatever
    /// applies its behaviour (the facade, Plan 0002 WP8.4 — this crate stops here).
    Message(Message),
    /// Send this `Error` back (via `to_frame` with the caller's own next `seq`) and keep the
    /// connection open.
    Reply(ErrorMsg),
    /// An incoming `Error`: contract §13 says to log it and never answer it ("kein
    /// Fehler-Pingpong"). Carried here so a caller can still log it; this crate has no logging
    /// dependency of its own.
    LoggedError(ErrorMsg),
}

/// Applies the "Nach dem Handshake" dispatch rules (contract §13) to one inbound `frame`, for a
/// peer acting as `local_role`. Never panics on any input (contract §2 rule 9).
///
/// Must only be called once a handshake has completed successfully
/// ([`accept_handshake`]/[`connect_handshake`] returned `Ok`); nothing before that point should
/// be reaching this function at all ("state machine that cannot be skipped"). `pub(crate)`, not
/// exported: [`Session::dispatch_frame`] is the only way anything outside this module can reach
/// these rules, which is what makes that guarantee hold rather than just documenting it.
pub(crate) fn handle_post_handshake_frame(
    local_role: PeerRole,
    frame: &Frame,
) -> PostHandshakeOutcome {
    match classify(frame.id.0) {
        IdClass::Zero => PostHandshakeOutcome::Reply(ErrorMsg {
            code: ErrorCode::Malformed,
            in_reply_to: frame.seq,
            message: "message id 0x0000 is invalid".to_owned(),
        }),
        IdClass::Reserved => PostHandshakeOutcome::Reply(ErrorMsg {
            code: ErrorCode::NotSupported,
            in_reply_to: frame.seq,
            message: "message id is in a reserved range".to_owned(),
        }),
        IdClass::Application => PostHandshakeOutcome::Reply(ErrorMsg {
            code: ErrorCode::NotSupported,
            in_reply_to: frame.seq,
            message: "application-defined messages have no receiver in protocol v1".to_owned(),
        }),
        IdClass::Free => PostHandshakeOutcome::Reply(ErrorMsg {
            code: ErrorCode::UnknownMessage,
            in_reply_to: frame.seq,
            message: "message id is not in the v1 catalogue".to_owned(),
        }),
        IdClass::Known(direction) => {
            if frame.id == MessageId(catalogue::HELLO) {
                return PostHandshakeOutcome::Reply(ErrorMsg {
                    code: ErrorCode::Malformed,
                    in_reply_to: frame.seq,
                    message: "a second Hello is invalid after the handshake".to_owned(),
                });
            }
            if !direction_valid(direction, local_role) {
                return PostHandshakeOutcome::Reply(ErrorMsg {
                    code: ErrorCode::NotSupported,
                    in_reply_to: frame.seq,
                    message: "message direction is invalid for this peer".to_owned(),
                });
            }
            match Message::from_frame(frame) {
                Ok(Message::Error(error)) => PostHandshakeOutcome::LoggedError(error),
                Ok(message) => PostHandshakeOutcome::Message(message),
                Err(_) => PostHandshakeOutcome::Reply(ErrorMsg {
                    code: ErrorCode::Malformed,
                    in_reply_to: frame.seq,
                    message: "payload failed to decode despite a valid frame length".to_owned(),
                }),
            }
        }
    }
}

/// Small free function rather than a method, since [`Direction::valid_incoming_for`] is
/// `pub(crate)` on a type from `message.rs`; kept as a thin wrapper here so the match arms above
/// read without an extra import alias.
fn direction_valid(direction: Direction, local_role: PeerRole) -> bool {
    direction.valid_incoming_for(local_role)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InProcessTransport;

    fn identities() -> (EngineIdentity, ToolIdentity) {
        let engine = EngineIdentity {
            engine_version: "0.1.1".to_owned(),
            build_hash: "a".repeat(64),
            token: [0x11; 32],
        };
        let tool = ToolIdentity {
            engine_version: engine.engine_version.clone(),
            build_hash: engine.build_hash.clone(),
            token: engine.token,
            stats_interval_frames: 30,
        };
        (engine, tool)
    }

    type EngineHandshakeResult = Result<(AcceptedHandshake, Session), HandshakeError>;
    type ToolHandshakeResult = Result<(Hello, Session), HandshakeError>;

    /// Runs a full handshake over a fresh in-process pair, returning both sides' results so a
    /// test can assert on whichever side it cares about without repeating the plumbing.
    fn run_handshake(
        engine_identity: &EngineIdentity,
        tool_identity: &ToolIdentity,
    ) -> (EngineHandshakeResult, ToolHandshakeResult) {
        let (mut engine_side, mut tool_side) = InProcessTransport::pair();
        let tool_identity = tool_identity.clone();
        let engine_identity = engine_identity.clone();
        let handle = thread::spawn(move || {
            connect_handshake(&mut tool_side, &tool_identity, Duration::from_secs(2))
        });
        let engine_result =
            accept_handshake(&mut engine_side, &engine_identity, Duration::from_secs(2));
        let tool_result = handle.join().expect("tool thread must not panic");
        (engine_result, tool_result)
    }

    #[test]
    fn matching_identities_complete_the_handshake_on_both_sides() {
        let (engine, tool) = identities();
        let (engine_result, tool_result) = run_handshake(&engine, &tool);

        let (accepted, engine_session) =
            engine_result.expect("engine must accept a matching Hello");
        assert_eq!(accepted.peer_hello.role, PeerRole::Tool);
        assert_eq!(accepted.peer_hello.engine_version, tool.engine_version);
        assert!(!accepted.build_hash_unknown_warning);
        assert_eq!(engine_session.local_role(), PeerRole::Engine);

        let (engine_hello, tool_session) =
            tool_result.expect("tool must receive the engine's Hello");
        assert_eq!(engine_hello.role, PeerRole::Engine);
        assert_eq!(engine_hello.engine_version, engine.engine_version);
        assert_eq!(engine_hello.token, [0u8; 32]);
        assert_eq!(tool_session.local_role(), PeerRole::Tool);
    }

    #[test]
    fn a_frame_pipelined_right_behind_hello_survives_and_dispatches_in_order() {
        // A tool need not wait for the engine's reply before sending its next message, and both
        // `InProcessTransport` and `TcpServerTransport` drain every frame they have buffered into
        // a single `poll` call. So the very `poll_for_first_frame` call that accepts the `Hello`
        // can also hand back a second, already-buffered frame right behind it — which must not be
        // silently dropped.
        let (engine, tool) = identities();
        let (mut engine_side, mut client_side) = InProcessTransport::pair();

        let hello = Message::Hello(Hello {
            protocol_version: crate::PROTOCOL_VERSION,
            role: PeerRole::Tool,
            engine_version: tool.engine_version.clone(),
            build_hash: tool.build_hash.clone(),
            token: tool.token,
            stats_interval_frames: tool.stats_interval_frames,
        })
        .to_frame(1)
        .unwrap();
        let pipelined = Message::SwapSigilUnit(crate::SwapSigilUnit {
            unit_path: "sigils/basic_bolt.sigil".to_owned(),
            unit_bytes: vec![9, 9, 9],
        })
        .to_frame(2)
        .unwrap();

        // Both frames are fully written into the shared channel *before* the engine ever polls,
        // so the first (and only, here) call to `InProcessTransport::poll` inside
        // `accept_handshake` is guaranteed to decode and return both at once, reproducing the
        // pipelining scenario deterministically instead of racing two threads against each other.
        client_side.send(&hello).unwrap();
        client_side.send(&pipelined).unwrap();

        let (_, mut session) = accept_handshake(&mut engine_side, &engine, Duration::from_secs(2))
            .expect("engine must accept a matching Hello");

        let pending = session.take_pending_frames();
        assert_eq!(pending.len(), 1, "the pipelined frame must not be dropped");
        assert_eq!(pending[0].seq, 2);

        // Dispatching it through the same `Session` applies the normal post-handshake rules, in
        // order, exactly as if it had arrived on a later `poll`.
        let outcome = session.dispatch_frame(&pending[0]);
        assert!(matches!(
            outcome,
            PostHandshakeOutcome::Message(Message::SwapSigilUnit(_))
        ));

        // The queue is drained, not merely peeked: a second call finds nothing left.
        assert!(session.take_pending_frames().is_empty());
    }

    #[test]
    fn mismatched_engine_version_is_rejected_on_both_sides() {
        let (engine, mut tool) = identities();
        tool.engine_version = "0.1.2".to_owned();
        let (engine_result, tool_result) = run_handshake(&engine, &tool);

        assert_eq!(engine_result.unwrap_err(), HandshakeError::VersionMismatch);
        assert_eq!(tool_result.unwrap_err(), HandshakeError::VersionMismatch);
    }

    #[test]
    fn an_oversized_configured_identity_string_does_not_panic_the_error_frame() {
        // A misconfigured build script or environment variable could hand this module an
        // `engine_version` long enough that the mismatch message `error_frame` builds from it
        // (interpolating both sides' `engine_version`) would, on its own, overflow `ErrorMsg`'s
        // 1024-byte `message` limit. `error_frame` must fall back to a short static message
        // instead of panicking on that encode failure (contract §2 rule 9).
        let (mut engine, tool) = identities();
        engine.engine_version = "9".repeat(2000);
        let (mut engine_side, mut client_side) = InProcessTransport::pair();

        let hello = Message::Hello(Hello {
            protocol_version: crate::PROTOCOL_VERSION,
            role: PeerRole::Tool,
            engine_version: tool.engine_version.clone(),
            build_hash: tool.build_hash.clone(),
            token: tool.token,
            stats_interval_frames: 0,
        })
        .to_frame(1)
        .unwrap();
        client_side.send(&hello).unwrap();

        let result = accept_handshake(&mut engine_side, &engine, Duration::from_secs(2));
        assert_eq!(result.unwrap_err(), HandshakeError::VersionMismatch);

        // The engine must still reply with *some* Error frame and disconnect, rather than
        // panicking mid-handshake and leaving the client hanging.
        let mut inbox = Vec::new();
        for _ in 0..64 {
            client_side.poll(&mut inbox).unwrap();
            if !inbox.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(inbox.len(), 1);
        let error = crate::ErrorMsg::decode(&inbox[0].payload).unwrap();
        assert_eq!(error.code, ErrorCode::VersionMismatch);
    }

    #[test]
    fn mismatched_known_build_hashes_are_rejected() {
        let (engine, mut tool) = identities();
        tool.build_hash = "b".repeat(64);
        let (engine_result, tool_result) = run_handshake(&engine, &tool);

        assert_eq!(engine_result.unwrap_err(), HandshakeError::VersionMismatch);
        assert_eq!(tool_result.unwrap_err(), HandshakeError::VersionMismatch);
    }

    #[test]
    fn tool_build_hash_unknown_is_accepted_with_a_warning() {
        let (engine, mut tool) = identities();
        tool.build_hash = "unknown".to_owned();
        let (engine_result, tool_result) = run_handshake(&engine, &tool);

        let (accepted, _) =
            engine_result.expect("an unknown build hash on one side must be accepted");
        assert!(accepted.build_hash_unknown_warning);
        tool_result.expect("tool side must also complete");
    }

    #[test]
    fn engine_build_hash_unknown_is_accepted_with_a_warning() {
        let (mut engine, tool) = identities();
        engine.build_hash = "unknown".to_owned();
        let (engine_result, tool_result) = run_handshake(&engine, &tool);

        let (accepted, _) =
            engine_result.expect("an unknown build hash on one side must be accepted");
        assert!(accepted.build_hash_unknown_warning);
        tool_result.expect("tool side must also complete");
    }

    #[test]
    fn both_build_hashes_unknown_is_accepted_with_a_warning() {
        let (mut engine, mut tool) = identities();
        engine.build_hash = "unknown".to_owned();
        tool.build_hash = "unknown".to_owned();
        let (engine_result, tool_result) = run_handshake(&engine, &tool);

        let (accepted, _) = engine_result.expect("both build hashes unknown must be accepted");
        assert!(accepted.build_hash_unknown_warning);
        tool_result.expect("tool side must also complete");
    }

    #[test]
    fn wrong_token_is_unauthorized_on_both_sides() {
        let (engine, mut tool) = identities();
        tool.token = [0x99; 32];
        let (engine_result, tool_result) = run_handshake(&engine, &tool);

        assert_eq!(engine_result.unwrap_err(), HandshakeError::Unauthorized);
        assert_eq!(tool_result.unwrap_err(), HandshakeError::Unauthorized);
    }

    #[test]
    fn a_message_sent_before_the_handshake_is_rejected() {
        let (engine, _tool) = identities();
        let (mut engine_side, mut client_side) = InProcessTransport::pair();

        // Skip Hello entirely and send a catalogue message straight away.
        let premature = Message::SwapSigilUnit(crate::SwapSigilUnit {
            unit_path: "sigils/basic_bolt.sigil".to_owned(),
            unit_bytes: vec![1, 2, 3],
        })
        .to_frame(1)
        .unwrap();
        client_side.send(&premature).unwrap();

        let result = accept_handshake(&mut engine_side, &engine, Duration::from_secs(2));
        assert_eq!(result.unwrap_err(), HandshakeError::HandshakeRequired);

        // The engine must have replied with Error(HandshakeRequired) and disconnected.
        let mut inbox = Vec::new();
        for _ in 0..64 {
            client_side.poll(&mut inbox).unwrap();
            if !inbox.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(inbox.len(), 1);
        let error = crate::ErrorMsg::decode(&inbox[0].payload).unwrap();
        assert_eq!(error.code, ErrorCode::HandshakeRequired);
    }

    #[test]
    fn a_hello_with_the_wrong_protocol_version_is_a_version_mismatch() {
        let (engine, _tool) = identities();
        let (mut engine_side, mut client_side) = InProcessTransport::pair();

        let mut bad_hello = Message::Hello(Hello {
            protocol_version: crate::PROTOCOL_VERSION,
            role: PeerRole::Tool,
            engine_version: engine.engine_version.clone(),
            build_hash: engine.build_hash.clone(),
            token: engine.token,
            stats_interval_frames: 0,
        })
        .to_frame(1)
        .unwrap();
        // Corrupt only the frozen first field (contract §13: `peek_hello_version` reads exactly
        // these two bytes), leaving the rest of a structurally valid Hello payload behind it.
        bad_hello.payload[0] = 0xFF;
        bad_hello.payload[1] = 0xFF;

        client_side.send(&bad_hello).unwrap();
        let result = accept_handshake(&mut engine_side, &engine, Duration::from_secs(2));
        assert_eq!(result.unwrap_err(), HandshakeError::VersionMismatch);
    }

    #[test]
    fn a_hello_with_trailing_bytes_is_malformed() {
        let (engine, _tool) = identities();
        let (mut engine_side, mut client_side) = InProcessTransport::pair();

        let mut good_hello = Message::Hello(Hello {
            protocol_version: crate::PROTOCOL_VERSION,
            role: PeerRole::Tool,
            engine_version: engine.engine_version.clone(),
            build_hash: engine.build_hash.clone(),
            token: engine.token,
            stats_interval_frames: 0,
        })
        .to_frame(1)
        .unwrap();
        good_hello.payload.push(0xAA); // one trailing byte

        client_side.send(&good_hello).unwrap();
        let result = accept_handshake(&mut engine_side, &engine, Duration::from_secs(2));
        assert_eq!(result.unwrap_err(), HandshakeError::Malformed);
    }

    #[test]
    fn a_hello_declaring_the_engine_role_is_malformed() {
        let (engine, _tool) = identities();
        let (mut engine_side, mut client_side) = InProcessTransport::pair();

        let wrong_role = Message::Hello(Hello {
            protocol_version: crate::PROTOCOL_VERSION,
            role: PeerRole::Engine, // a connecting peer must claim Tool
            engine_version: engine.engine_version.clone(),
            build_hash: engine.build_hash.clone(),
            token: engine.token,
            stats_interval_frames: 0,
        })
        .to_frame(1)
        .unwrap();

        client_side.send(&wrong_role).unwrap();
        let result = accept_handshake(&mut engine_side, &engine, Duration::from_secs(2));
        assert_eq!(result.unwrap_err(), HandshakeError::Malformed);
    }

    #[test]
    fn an_oversized_first_frame_is_rejected_before_engine_version_is_checked() {
        let (engine, _tool) = identities();
        let (mut engine_side, mut client_side) = InProcessTransport::pair();

        // A structurally valid Hello whose payload alone already exceeds MAX_HELLO_FRAME_LEN
        // (1024) once the 8-byte header is added back.
        let oversized = Message::Hello(Hello {
            protocol_version: crate::PROTOCOL_VERSION,
            role: PeerRole::Tool,
            engine_version: engine.engine_version.clone(),
            build_hash: "a".repeat(64),
            token: engine.token,
            stats_interval_frames: 0,
        })
        .to_frame(1)
        .unwrap();
        let mut frame = oversized;
        frame.payload.extend(std::iter::repeat_n(0u8, 2000));

        client_side.send(&frame).unwrap();
        let result = accept_handshake(&mut engine_side, &engine, Duration::from_secs(2));
        assert_eq!(result.unwrap_err(), HandshakeError::TooLarge);
    }

    #[test]
    fn timeout_without_any_frame_is_reported_as_timeout() {
        let (engine, _tool) = identities();
        let (mut engine_side, _client_side) = InProcessTransport::pair();
        // `_client_side` is kept alive but never sends anything.
        let result = accept_handshake(&mut engine_side, &engine, Duration::from_millis(20));
        assert_eq!(result.unwrap_err(), HandshakeError::Timeout);
    }

    // --- Post-handshake dispatch ---------------------------------------------------------------

    fn frame_for(message: Message, seq: u32) -> Frame {
        message.to_frame(seq).unwrap()
    }

    #[test]
    fn id_zero_after_handshake_is_malformed() {
        let frame = Frame {
            id: MessageId(0x0000),
            seq: 5,
            payload: Vec::new(),
        };
        let outcome = handle_post_handshake_frame(PeerRole::Engine, &frame);
        assert!(matches!(
            outcome,
            PostHandshakeOutcome::Reply(ErrorMsg {
                code: ErrorCode::Malformed,
                in_reply_to: 5,
                ..
            })
        ));
    }

    #[test]
    fn a_free_id_after_handshake_is_unknown_message() {
        let frame = Frame {
            id: MessageId(0x00AB),
            seq: 6,
            payload: Vec::new(),
        };
        let outcome = handle_post_handshake_frame(PeerRole::Engine, &frame);
        assert!(matches!(
            outcome,
            PostHandshakeOutcome::Reply(ErrorMsg {
                code: ErrorCode::UnknownMessage,
                in_reply_to: 6,
                ..
            })
        ));
    }

    #[test]
    fn a_reserved_id_after_handshake_is_not_supported() {
        let frame = Frame {
            id: MessageId(0x0150),
            seq: 7,
            payload: Vec::new(),
        };
        let outcome = handle_post_handshake_frame(PeerRole::Tool, &frame);
        assert!(matches!(
            outcome,
            PostHandshakeOutcome::Reply(ErrorMsg {
                code: ErrorCode::NotSupported,
                in_reply_to: 7,
                ..
            })
        ));
    }

    #[test]
    fn an_application_id_after_handshake_is_not_supported() {
        let frame = Frame {
            id: MessageId(0x9000),
            seq: 8,
            payload: Vec::new(),
        };
        let outcome = handle_post_handshake_frame(PeerRole::Engine, &frame);
        assert!(matches!(
            outcome,
            PostHandshakeOutcome::Reply(ErrorMsg {
                code: ErrorCode::NotSupported,
                in_reply_to: 8,
                ..
            })
        ));
    }

    #[test]
    fn a_message_in_the_wrong_direction_is_not_supported() {
        // Stats/Log/SwapAck are E->T; the *engine* receiving one is the contract's own example.
        let stats_frame = frame_for(Message::Stats(crate::Stats::default()), 9);
        let outcome = handle_post_handshake_frame(PeerRole::Engine, &stats_frame);
        assert!(matches!(
            outcome,
            PostHandshakeOutcome::Reply(ErrorMsg {
                code: ErrorCode::NotSupported,
                in_reply_to: 9,
                ..
            })
        ));

        // SwapSigilUnit/SigilPreview are T->E; a *tool* receiving one is the symmetric case.
        let swap_frame = frame_for(
            Message::SwapSigilUnit(crate::SwapSigilUnit {
                unit_path: "x".to_owned(),
                unit_bytes: vec![],
            }),
            10,
        );
        let outcome = handle_post_handshake_frame(PeerRole::Tool, &swap_frame);
        assert!(matches!(
            outcome,
            PostHandshakeOutcome::Reply(ErrorMsg {
                code: ErrorCode::NotSupported,
                in_reply_to: 10,
                ..
            })
        ));
    }

    #[test]
    fn a_message_in_the_right_direction_dispatches_normally() {
        let frame = frame_for(Message::Stats(crate::Stats::default()), 11);
        let outcome = handle_post_handshake_frame(PeerRole::Tool, &frame);
        assert!(matches!(
            outcome,
            PostHandshakeOutcome::Message(Message::Stats(_))
        ));
    }

    #[test]
    fn a_second_hello_after_handshake_is_malformed() {
        let frame = frame_for(
            Message::Hello(Hello {
                protocol_version: crate::PROTOCOL_VERSION,
                role: PeerRole::Tool,
                engine_version: String::new(),
                build_hash: "unknown".to_owned(),
                token: [0u8; 32],
                stats_interval_frames: 0,
            }),
            12,
        );
        let outcome = handle_post_handshake_frame(PeerRole::Engine, &frame);
        assert!(matches!(
            outcome,
            PostHandshakeOutcome::Reply(ErrorMsg {
                code: ErrorCode::Malformed,
                in_reply_to: 12,
                ..
            })
        ));
    }

    #[test]
    fn an_incoming_error_is_logged_not_replied_to() {
        let frame = frame_for(
            Message::Error(ErrorMsg {
                code: ErrorCode::Internal,
                in_reply_to: 3,
                message: "boom".to_owned(),
            }),
            13,
        );
        let outcome = handle_post_handshake_frame(PeerRole::Engine, &frame);
        assert!(matches!(outcome, PostHandshakeOutcome::LoggedError(_)));
    }

    #[test]
    fn a_payload_decode_error_at_a_valid_length_is_malformed() {
        // A `SwapSigilUnit` frame (T->E, valid direction for the engine) whose payload is
        // truncated mid-field: a valid frame length, but not a valid encoding.
        let mut frame = frame_for(
            Message::SwapSigilUnit(crate::SwapSigilUnit {
                unit_path: "sigils/basic_bolt.sigil".to_owned(),
                unit_bytes: vec![1, 2, 3, 4],
            }),
            14,
        );
        frame.payload.truncate(4); // cuts off mid `unit_path` string
        let outcome = handle_post_handshake_frame(PeerRole::Engine, &frame);
        assert!(matches!(
            outcome,
            PostHandshakeOutcome::Reply(ErrorMsg {
                code: ErrorCode::Malformed,
                in_reply_to: 14,
                ..
            })
        ));
    }

    proptest::proptest! {
        /// `handle_post_handshake_frame` must never panic, whatever a peer sends after the
        /// handshake completes (contract §2 rule 9).
        #[test]
        fn handle_post_handshake_frame_never_panics(
            id in proptest::prelude::any::<u16>(),
            seq in proptest::prelude::any::<u32>(),
            payload in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..256),
            role_is_engine in proptest::prelude::any::<bool>(),
        ) {
            let frame = Frame { id: MessageId(id), seq, payload };
            let role = if role_is_engine { PeerRole::Engine } else { PeerRole::Tool };
            let _ = handle_post_handshake_frame(role, &frame);
        }

        /// `accept_handshake` must never panic on an arbitrary first frame, well-formed or not
        /// (contract §2 rule 9). `id` is biased to land on `catalogue::HELLO` about half the time
        /// (`prop_oneof!`): drawing it uniformly over `u16` instead reaches the Hello path (steps
        /// 3-8: version peek, strict decode, role, token, engine version, build hash) only with
        /// probability ~1/65536, so almost every case would be rejected at step 2 before any of
        /// that logic ever ran. Fixing `id` on every run would lose coverage of steps 1-2 (a
        /// wrong id, or a too-large frame) instead, so the other half keeps `id` fully arbitrary.
        #[test]
        fn accept_handshake_never_panics_on_an_arbitrary_first_frame(
            id in proptest::prop_oneof![
                proptest::prelude::Just(catalogue::HELLO),
                proptest::prelude::any::<u16>(),
            ],
            seq in proptest::prelude::any::<u32>(),
            payload in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..300),
        ) {
            let (engine, _tool) = identities();
            let (mut engine_side, mut client_side) = InProcessTransport::pair();
            let frame = Frame { id: MessageId(id), seq, payload };
            // A `Frame` built from arbitrary `id`/`payload` always encodes fine on its own (only
            // the frame *header* is validated at that layer); it is decoding its payload *as a
            // catalogue message* that can fail, which is exactly what this test wants to fuzz.
            let _ = client_side.send(&frame);
            let _ = accept_handshake(&mut engine_side, &engine, Duration::from_millis(50));
        }
    }
}
