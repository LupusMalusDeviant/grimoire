//! # grimoire_debug
//!
//! Debug IPC transport layer of the Grimoire engine (contract §13, §2a "Debug-Transport").
//!
//! This crate implements the **transport layer** of the debug protocol: the byte-level frame
//! envelope ([`Frame`], [`FrameDecoder`], [`encode_frame`], [`peek_hello_version`]), the
//! protocol constants, and the [`DebugTransport`] trait with its three implementations
//! ([`NullTransport`], [`InProcessTransport`], and [`TcpServerTransport`] behind the `tcp`
//! feature) plus their conformance suite (`conformance`, behind the `conformance` feature; not
//! an intra-doc link, since that module does not exist for a `cargo doc` run without it).
//!
//! ## Scope (WP1.3 vs. WP8.2)
//!
//! The full engine contract for `grimoire_debug` (contract §13) also specifies an entire
//! versioned wire *protocol* on top of this transport: a message catalog (`Hello`, `Error`,
//! `Log`, `Stats`, `SwapSigilUnit`, `SwapAck`, `SigilPreview`), the handshake state machine that
//! negotiates it, and a profiler data model (`FrameProfile`, `ScopeId`, `StatsFrame`). Per the
//! engine's work-package plan, that entire layer is scope for **WP8.2**, not this crate's
//! current state. Accordingly, this crate does **not** define `Message`, the seven payload
//! structs, `PeerRole`, `ErrorCode`, any handshake logic, or the profiler types — not even as
//! stubs, since a half-finished version would only create rework and contract drift once WP8.2
//! lands. [`PROTOCOL_VERSION`], [`MAX_HELLO_FRAME_LEN`], [`HANDSHAKE_TIMEOUT`] and
//! [`MAX_INBOUND_QUEUED_BYTES`] are kept as constants now (other crates and tests reference
//! them), but nothing in this crate enforces the handshake or queue-size semantics they describe
//! yet — that wiring is also WP8.2's job. Similarly, `TcpConfig::token` is carried through
//! `TcpConfig::from_env` but not validated by [`TcpServerTransport`]: there is no handshake here
//! to check it against yet.

mod frame;
mod transport;

#[cfg(feature = "conformance")]
pub mod conformance;

pub use frame::{Frame, FrameDecoder, MessageId, ProtocolError, encode_frame, peek_hello_version};
pub use transport::{
    DEBUG_ADDR_ENV, DEBUG_TOKEN_ENV, DEFAULT_DEBUG_PORT, DebugTransport, InProcessOptions,
    InProcessTransport, NullTransport, SOCKET_TESTS_ENV, TransportError, socket_tests_enabled,
};
#[cfg(feature = "tcp")]
pub use transport::{TcpConfig, TcpServerTransport};

use std::time::Duration;

/// Wire protocol version negotiated by `Hello` (frozen field, contract §13). Not yet checked by
/// anything in this crate; the handshake that would compare it is WP8.2.
pub const PROTOCOL_VERSION: u16 = 1;

/// Largest value the wire `len` field may carry, including the 8-byte header (contract §13).
pub const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

/// Largest byte size of a `SwapSigilUnit`/`SigilPreview` unit payload (contract §13). Must
/// equal `grimoire_sigil::SigilUnit::MAX_UNIT_BYTES`, added by a sibling PR; there is no crate
/// edge to `grimoire_sigil` from here, so the equality is a documentation contract, not a type
/// dependency.
pub const MAX_UNIT_BYTES: u32 = 8 * 1024 * 1024;

/// Largest `len` value accepted for the *first* frame of a connection, before the (WP8.2)
/// handshake completes. Frozen across all protocol versions (contract §13).
pub const MAX_HELLO_FRAME_LEN: u32 = 1024;

/// Byte budget for a transport's inbound queue (contract §13, TCP). Not yet enforced by
/// [`TcpServerTransport`] in this crate; that backpressure wiring is WP8.2's job.
pub const MAX_INBOUND_QUEUED_BYTES: usize = 2 * MAX_FRAME_LEN as usize;

/// Time allowed for the first complete frame of a connection to arrive (contract §13). Not yet
/// enforced by [`TcpServerTransport`] in this crate; the handshake that would watch it is
/// WP8.2's job.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Longest a transport's IO loop waits between iterations (contract §13, TCP).
pub const IO_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Longest a transport's IO loop retries a blocked socket write before giving up on the
/// connection (contract §13, TCP).
pub const IO_WRITE_TIMEOUT: Duration = Duration::from_millis(100);
