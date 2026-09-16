//! # grimoire_debug
//!
//! Debug IPC protocol of the Grimoire engine (contract §13, §2a "Debug-Transport").
//!
//! This crate implements the byte-level frame envelope ([`Frame`], [`FrameDecoder`],
//! [`encode_frame`], [`peek_hello_version`]), the [`DebugTransport`] trait with its three
//! implementations ([`NullTransport`], [`InProcessTransport`], and [`TcpServerTransport`] behind
//! the `tcp` feature, plus their conformance suite behind `conformance`), the message catalogue's
//! payload types and dispatch ([`Message`]), and the handshake state machine
//! ([`accept_handshake`]/[`connect_handshake`], yielding a [`Session`] whose
//! [`Session::dispatch_frame`] applies the "Nach dem Handshake" dispatch rules — the only way
//! anything outside `handshake.rs` can reach them).
//!
//! ## Scope (WP1.3 vs. WP8.1 vs. WP8.2 vs. later)
//!
//! The full engine contract for `grimoire_debug` (contract §13) specifies an entire versioned
//! wire *protocol*: a message catalog, the handshake that negotiates it, and a profiler data
//! model. Per the engine's work-package plan, that splits across several steps:
//!
//! - **WP1.3**: the frame envelope and [`DebugTransport`] with its three implementations.
//! - **WP8.1** (project ADR-0011): the message catalog's *payload types* — [`Hello`],
//!   [`ErrorMsg`], [`LogMsg`], [`Stats`], [`StatsScope`], [`StatsCounter`], [`SwapSigilUnit`],
//!   [`SwapAck`], [`SigilPreview`], [`PeerRole`] and [`ErrorCode`] — generated from
//!   `schema/debug_protocol_v1.gschema` by `grimoire_schemagen` into
//!   `src/generated/debug_protocol.rs` and re-exported here. Each carries its own
//!   `encode`/`decode` pair following contract §2 rule 9 (length/count checked against both a
//!   documented maximum and the bytes actually remaining, before any allocation; never panics).
//! - **WP8.2** (this crate's current state): the [`Message`] enum and its
//!   [`Message::id`]/[`Message::to_frame`]/[`Message::from_frame`] dispatch, plus the handshake
//!   state machine (contract §13 "Handshake", PO decision V-13) and the "Nach dem Handshake"
//!   dispatch rules (reachable only via [`Session::dispatch_frame`]) — all hand-written on top of
//!   the payload types above, since they are control flow with only a few fields each, not a
//!   wire-format vocabulary a schema compiler earns its keep describing (project ADR-0011
//!   "Vorschlag" point 1). [`accept_handshake`] and [`connect_handshake`] are written purely against
//!   [`DebugTransport`], so they are exercised in this crate's own tests only through
//!   [`InProcessTransport`]; wiring them into [`TcpServerTransport`]'s connection-handling thread
//!   (with the tighter, pre-allocation byte-level defenses contract §13 "TCP" describes for a
//!   not-yet-authenticated socket peer) is Plan-0002 WP8.4's "Engine-Server ... IO-Thread", not
//!   this crate's job. `TcpConfig::token` is still carried by [`TcpConfig`] but not checked by
//!   [`TcpServerTransport`] itself for the same reason.
//! - **Later** (Plan-0002 WP6.3, not this crate's job at all): the profiler data model
//!   (`FrameProfile`, `ScopeId`, `StatsFrame`) that builds a [`Stats`] value in the first place.
//!   [`Message::SwapSigilUnit`] and [`Message::SigilPreview`] decode and dispatch correctly here,
//!   but *acting* on one (queueing a swap at a tick boundary, or ever answering `SigilPreview`
//!   with anything but `Error(NotSupported)`) is Plan-0002 WP8.3/WP8.4's job.

mod frame;
mod generated;
mod handshake;
mod message;
mod transport;

#[cfg(feature = "conformance")]
pub mod conformance;

pub use frame::{Frame, FrameDecoder, MessageId, ProtocolError, encode_frame, peek_hello_version};
pub use generated::debug_protocol::{
    ErrorCode, ErrorMsg, Hello, LogMsg, PeerRole, SigilPreview, Stats, StatsCounter, StatsScope,
    SwapAck, SwapSigilUnit, catalogue,
};
pub use handshake::{
    AcceptedHandshake, EngineIdentity, HandshakeError, PostHandshakeOutcome, Session, ToolIdentity,
    accept_handshake, connect_handshake,
};
pub use message::Message;
pub use transport::{
    DEBUG_ADDR_ENV, DEBUG_TOKEN_ENV, DEFAULT_DEBUG_PORT, DebugTransport, InProcessOptions,
    InProcessTransport, NullTransport, SOCKET_TESTS_ENV, TransportError, socket_tests_enabled,
};
#[cfg(feature = "tcp")]
pub use transport::{TcpConfig, TcpServerTransport};

use std::time::Duration;

/// Wire protocol version negotiated by `Hello` (frozen field, contract §13). Compared by
/// [`accept_handshake`] and [`connect_handshake`].
pub const PROTOCOL_VERSION: u16 = 1;

/// Largest value the wire `len` field may carry, including the 8-byte header (contract §13).
pub const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

/// Largest byte size of a `SwapSigilUnit`/`SigilPreview` unit payload (contract §13). Must
/// equal `grimoire_sigil::SigilUnit::MAX_UNIT_BYTES`, added by a sibling PR; there is no crate
/// edge to `grimoire_sigil` from here, so the equality is a documentation contract, not a type
/// dependency.
pub const MAX_UNIT_BYTES: u32 = 8 * 1024 * 1024;

/// Largest `len` value accepted for the *first* frame of a connection, before the handshake
/// completes. Frozen across all protocol versions (contract §13). Enforced semantically by
/// [`accept_handshake`] against the wire length its transport reconstructs from a decoded
/// [`Frame`]; the tighter, pre-allocation byte-level enforcement contract §13 "TCP" describes for
/// [`TcpServerTransport`] specifically is Plan-0002 WP8.4's job (see the crate docs).
pub const MAX_HELLO_FRAME_LEN: u32 = 1024;

/// Byte budget for a transport's inbound queue (contract §13, TCP). Not yet enforced by
/// [`TcpServerTransport`] in this crate; that backpressure wiring is Plan-0002 WP8.4's job.
pub const MAX_INBOUND_QUEUED_BYTES: usize = 2 * MAX_FRAME_LEN as usize;

/// Time allowed for the first complete frame of a connection to arrive (contract §13). Enforced
/// by [`accept_handshake`], which blocks its calling thread for up to this long while polling its
/// transport (see that function's own docs for why that thread must not be a simulation thread).
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Longest a transport's IO loop waits between iterations (contract §13, TCP).
pub const IO_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Longest a transport's IO loop retries a blocked socket write before giving up on the
/// connection (contract §13, TCP).
pub const IO_WRITE_TIMEOUT: Duration = Duration::from_millis(100);
