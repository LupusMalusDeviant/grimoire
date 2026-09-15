//! `DebugTransport` trait and its implementations (contract §13 "Transporte", §2a).
//!
//! A transport carries encoded [`crate::Frame`] bytes between two peers. It never blocks and
//! never panics, regardless of what bytes a peer sends. `grimoire_debug` ships three
//! implementations: [`NullTransport`] (never connected), [`InProcessTransport`] (an in-process
//! pair that still exercises the full byte codec) and, behind the `tcp` feature,
//! [`TcpServerTransport`]. All three are used only outside simulation stages, at tick
//! boundaries (§9.3, §9.7) — never from inside a system.

mod in_process;
mod null;
#[cfg(feature = "tcp")]
mod tcp;

pub use in_process::{InProcessOptions, InProcessTransport};
pub use null::NullTransport;
#[cfg(feature = "tcp")]
pub use tcp::{TcpConfig, TcpServerTransport};

use std::io;

use crate::frame::{Frame, ProtocolError};

/// Non-blocking transport for debug protocol frames (contract §13, §2a).
///
/// Implementations are object-safe and [`Send`] so a game or tool can hold one behind
/// `Box<dyn DebugTransport>` and swap it at startup (Null in tests, TCP in a real build).
/// Neither `poll` nor `send` may block the calling thread.
pub trait DebugTransport: Send {
    /// Appends every fully decoded inbound frame to `inbox`, in the order it arrived. Never
    /// blocks. A decode error observed while polling disconnects the transport rather than
    /// panicking or corrupting later frames.
    fn poll(&mut self, inbox: &mut Vec<Frame>) -> Result<(), TransportError>;
    /// Attempts to send `frame`. Never blocks: a full outbound buffer fails fast with
    /// [`TransportError::QueueFull`] instead of waiting for room.
    fn send(&mut self, frame: &Frame) -> Result<(), TransportError>;
    /// Whether a peer is currently connected.
    fn is_connected(&self) -> bool;
    /// Disconnects the current peer, if any. Idempotent: calling it again, or calling it when
    /// already disconnected, is a no-op.
    fn disconnect(&mut self);
}

/// Name of the environment variable carrying the debug TCP bind address (`127.0.0.1:<port>`).
pub const DEBUG_ADDR_ENV: &str = "GRIMOIRE_DEBUG_ADDR";
/// Name of the environment variable carrying the 64 hex digit debug link auth token.
pub const DEBUG_TOKEN_ENV: &str = "GRIMOIRE_DEBUG_TOKEN";
/// Name of the environment variable that enables tests which bind real TCP sockets.
pub const SOCKET_TESTS_ENV: &str = "GRIMOIRE_SOCKET_TESTS";
/// Default TCP port for the debug link.
pub const DEFAULT_DEBUG_PORT: u16 = 47_474;

/// Returns `true` exactly when [`SOCKET_TESTS_ENV`] is set to the literal value `"1"`.
///
/// Tests that bind a real socket must check this first and self-skip otherwise (contract §13
/// "Socket-Tests"); only CI sets it.
pub fn socket_tests_enabled() -> bool {
    std::env::var(SOCKET_TESTS_ENV).is_ok_and(|value| value == "1")
}

/// Errors produced by a [`DebugTransport`] implementation (contract §13).
///
/// `#[non_exhaustive]`: transports may need to report new failure modes later without an
/// incompatible change (§2b).
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TransportError {
    /// The transport has never connected to a peer (e.g. [`NullTransport`], or a real
    /// transport before its first connection).
    #[error("transport is not connected")]
    NotConnected,
    /// `send` could not enqueue the frame because the outbound buffer is full.
    #[error("outbound queue is full")]
    QueueFull,
    /// The transport was connected but the peer has since disconnected.
    #[error("peer disconnected")]
    Disconnected,
    /// A TCP bind address was not exactly `127.0.0.1` (contract §13 "TCP").
    #[error("address {0} is not loopback (127.0.0.1)")]
    NonLoopbackAddress(String),
    /// A transport configuration (e.g. from environment variables) was missing or malformed.
    #[error("invalid transport configuration: {0}")]
    InvalidConfig(String),
    /// A framing error surfaced while decoding inbound bytes.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    /// An underlying I/O operation failed.
    #[error("io error ({kind:?}): {message}")]
    Io {
        /// The underlying [`std::io::ErrorKind`].
        kind: io::ErrorKind,
        /// A human-readable description of the I/O failure.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `DebugTransport` must stay object-safe and `Send` for every implementation shipped here
    /// (contract §2a).
    #[test]
    fn debug_transport_is_object_safe_and_send() {
        fn assert_send<T: Send>() {}
        assert_send::<NullTransport>();
        assert_send::<InProcessTransport>();
        #[cfg(feature = "tcp")]
        assert_send::<TcpServerTransport>();

        let _: Option<Box<dyn DebugTransport>> = None;
    }
}
