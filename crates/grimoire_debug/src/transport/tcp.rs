//! [`TcpServerTransport`]: the TCP debug link (contract §13 "TCP (Feature `tcp`)"), gated
//! behind the `tcp` feature.
//!
//! This module implements only the transport described in the WP1.3 scope note: loopback-only
//! binding, a non-blocking IO thread, at most one connected client at a time, and panic-free
//! handling of arbitrary bytes. The `Hello` handshake, token verification, and version
//! negotiation described in contract §13 belong to WP8.2 and are intentionally not implemented
//! here — `TcpConfig::token` is carried but not yet checked by this transport.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use super::{DEBUG_ADDR_ENV, DEBUG_TOKEN_ENV, DEFAULT_DEBUG_PORT, DebugTransport, TransportError};
use crate::frame::{Frame, FrameDecoder, encode_frame};

/// TCP transport for the debug link, bound exclusively to `127.0.0.1` (contract §13).
///
/// `bind` starts exactly one IO thread named `grimoire-debug-io` that owns the non-blocking
/// listener and, once one arrives, the single non-blocking client socket. The thread accepts
/// connections, decodes inbound bytes through a [`FrameDecoder`], and writes outbound frames,
/// all bounded by [`crate::IO_POLL_INTERVAL`] and [`crate::IO_WRITE_TIMEOUT`] so `Drop` never
/// hangs. A second simultaneous client is rejected by closing it; the first stays connected.
/// Garbage bytes on the socket close that connection without panicking; the listener keeps
/// accepting new clients afterwards.
pub struct TcpServerTransport {
    stop: Arc<AtomicBool>,
    disconnect_requested: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    inbound: mpsc::Receiver<Frame>,
    outbound: mpsc::SyncSender<Frame>,
    thread: Option<thread::JoinHandle<()>>,
    local_addr: SocketAddrV4,
}

impl TcpServerTransport {
    /// Binds a new debug TCP server and starts its IO thread.
    ///
    /// Rejects any address other than exactly `127.0.0.1` with
    /// [`TransportError::NonLoopbackAddress`] *before* creating a socket; port `0` is allowed
    /// (useful for tests that want an OS-assigned ephemeral port, see [`Self::local_addr`]).
    pub fn bind(config: TcpConfig) -> Result<Self, TransportError> {
        if *config.addr.ip() != Ipv4Addr::LOCALHOST {
            return Err(TransportError::NonLoopbackAddress(
                config.addr.ip().to_string(),
            ));
        }

        let listener = TcpListener::bind(SocketAddr::V4(config.addr)).map_err(io_err)?;
        listener.set_nonblocking(true).map_err(io_err)?;
        let local_addr = match listener.local_addr().map_err(io_err)? {
            SocketAddr::V4(addr) => addr,
            // Proven invariant: we bound `SocketAddr::V4(config.addr)` above, so a bound IPv4
            // listener's local address is V4 too; this arm only avoids an unreachable panic.
            SocketAddr::V6(addr) => SocketAddrV4::new(Ipv4Addr::LOCALHOST, addr.port()),
        };

        let (inbound_tx, inbound_rx) = mpsc::sync_channel::<Frame>(256);
        let (outbound_tx, outbound_rx) = mpsc::sync_channel::<Frame>(256);
        let stop = Arc::new(AtomicBool::new(false));
        let connected = Arc::new(AtomicBool::new(false));
        let disconnect_requested = Arc::new(AtomicBool::new(false));

        let thread_stop = Arc::clone(&stop);
        let thread_connected = Arc::clone(&connected);
        let thread_disconnect = Arc::clone(&disconnect_requested);
        let thread = thread::Builder::new()
            .name("grimoire-debug-io".to_owned())
            .spawn(move || {
                run_io_loop(
                    listener,
                    &thread_stop,
                    &thread_connected,
                    &thread_disconnect,
                    &inbound_tx,
                    &outbound_rx,
                );
            })
            .map_err(io_err)?;

        Ok(Self {
            stop,
            disconnect_requested,
            connected,
            inbound: inbound_rx,
            outbound: outbound_tx,
            thread: Some(thread),
            local_addr,
        })
    }

    /// The address this server is actually bound to (useful when [`TcpConfig::addr`] used the
    /// ephemeral port `0`).
    pub fn local_addr(&self) -> SocketAddrV4 {
        self.local_addr
    }
}

impl std::fmt::Debug for TcpServerTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpServerTransport")
            .field("local_addr", &self.local_addr)
            .field("connected", &self.is_connected())
            .finish_non_exhaustive()
    }
}

impl Drop for TcpServerTransport {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            // Bounded by one `IO_POLL_INTERVAL` loop iteration plus, at most, one
            // `IO_WRITE_TIMEOUT` write-retry window (see `write_all_bounded`): well under a
            // second even in the worst case.
            let _ = thread.join();
        }
    }
}

impl DebugTransport for TcpServerTransport {
    fn poll(&mut self, inbox: &mut Vec<Frame>) -> Result<(), TransportError> {
        while let Ok(frame) = self.inbound.try_recv() {
            inbox.push(frame);
        }
        Ok(())
    }

    fn send(&mut self, frame: &Frame) -> Result<(), TransportError> {
        self.outbound
            .try_send(frame.clone())
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => TransportError::QueueFull,
                mpsc::TrySendError::Disconnected(_) => TransportError::Disconnected,
            })
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    fn disconnect(&mut self) {
        self.connected.store(false, Ordering::SeqCst);
        self.disconnect_requested.store(true, Ordering::SeqCst);
    }
}

/// The IO thread's main loop: accept, read+decode, dispatch, write, repeat, bounded by
/// [`crate::IO_POLL_INTERVAL`] per iteration. Never panics on any bytes a peer sends.
fn run_io_loop(
    listener: TcpListener,
    stop: &AtomicBool,
    connected: &AtomicBool,
    disconnect_requested: &AtomicBool,
    inbound_tx: &mpsc::SyncSender<Frame>,
    outbound_rx: &mpsc::Receiver<Frame>,
) {
    let mut client: Option<TcpStream> = None;
    let mut decoder = FrameDecoder::new();
    let mut read_buf = [0u8; 8192];

    while !stop.load(Ordering::Relaxed) {
        // A caller-requested disconnect is a one-shot signal: consume it, drop the current
        // client if any, but do not affect connections accepted afterwards.
        if disconnect_requested.swap(false, Ordering::SeqCst) && client.is_some() {
            client = None;
            decoder = FrameDecoder::new();
            connected.store(false, Ordering::SeqCst);
        }

        match listener.accept() {
            Ok((stream, _addr)) => {
                // A second simultaneous client is rejected by dropping `stream` here, which
                // closes it; the first client stays connected (contract §13). If configuring
                // the socket fails, `stream` is likewise dropped silently and serving continues.
                if client.is_none() && stream.set_nonblocking(true).is_ok() {
                    client = Some(stream);
                    decoder = FrameDecoder::new();
                    connected.store(true, Ordering::SeqCst);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(_) => {} // Transient accept errors are ignored; the loop just retries.
        }

        let mut drop_client = false;

        if let Some(stream) = client.as_mut() {
            loop {
                match stream.read(&mut read_buf) {
                    Ok(0) => {
                        drop_client = true; // Peer closed the connection.
                        break;
                    }
                    Ok(n) => {
                        decoder.push(&read_buf[..n]);
                        if n < read_buf.len() {
                            break; // Drained the socket for this iteration.
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(_) => {
                        drop_client = true;
                        break;
                    }
                }
            }
        }

        if !drop_client && client.is_some() {
            loop {
                match decoder.next_frame() {
                    Ok(Some(frame)) => {
                        // Best-effort: a full inbound channel drops the frame rather than
                        // blocking this thread (`poll` never blocks either).
                        let _ = inbound_tx.try_send(frame);
                    }
                    Ok(None) => break,
                    Err(_) => {
                        // Garbage bytes or a malformed length field: close this connection
                        // without panicking; the listener keeps accepting new clients.
                        drop_client = true;
                        break;
                    }
                }
            }
        }

        if !drop_client && let Some(stream) = client.as_mut() {
            while let Ok(frame) = outbound_rx.try_recv() {
                let mut bytes = Vec::new();
                if encode_frame(&frame, &mut bytes).is_err() {
                    continue; // Too large to encode; drop this one frame, keep connected.
                }
                if !write_all_bounded(stream, &bytes, crate::IO_WRITE_TIMEOUT) {
                    drop_client = true;
                    break;
                }
            }
        }

        if drop_client {
            client = None;
            decoder = FrameDecoder::new();
            connected.store(false, Ordering::SeqCst);
        }

        thread::sleep(crate::IO_POLL_INTERVAL);
    }
}

/// Writes `bytes` to `stream` (non-blocking), retrying on `WouldBlock` until `timeout` has
/// elapsed. Returns `false` on any error or on hitting the deadline with bytes still
/// unwritten, in which case the caller drops the connection; this is what keeps `Drop` bounded.
fn write_all_bounded(stream: &mut TcpStream, bytes: &[u8], timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    let mut offset = 0;
    while offset < bytes.len() {
        match stream.write(&bytes[offset..]) {
            Ok(0) => return false,
            Ok(n) => offset += n,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return false;
                }
                thread::sleep(Duration::from_millis(1));
            }
            Err(_) => return false,
        }
    }
    true
}

fn io_err(error: io::Error) -> TransportError {
    TransportError::Io {
        kind: error.kind(),
        message: error.to_string(),
    }
}

/// Configuration for [`TcpServerTransport::bind`] (contract §13).
///
/// `#[non_exhaustive]`: built via [`TcpConfig::new`] or [`TcpConfig::from_env`] (§2 rule 13).
#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TcpConfig {
    /// The address to bind. Must be exactly `127.0.0.1` (port `0` is allowed for tests).
    pub addr: SocketAddrV4,
    /// Shared-secret token for the (not yet implemented, WP8.2) handshake. Carried through by
    /// this transport but not validated by it.
    pub token: [u8; 32],
}

impl TcpConfig {
    /// Builds a configuration directly from an address and token.
    pub fn new(addr: SocketAddrV4, token: [u8; 32]) -> Self {
        Self { addr, token }
    }

    /// Reads [`DEBUG_ADDR_ENV`] (`127.0.0.1:<port>`) and [`DEBUG_TOKEN_ENV`] (64 hex digits).
    ///
    /// Returns `Ok(None)` if [`DEBUG_ADDR_ENV`] is unset. Returns
    /// [`TransportError::InvalidConfig`] if it is set but not exactly `127.0.0.1:<port>`, or if
    /// the token is missing or not exactly 64 hex digits.
    pub fn from_env() -> Result<Option<TcpConfig>, TransportError> {
        if std::env::var_os(DEBUG_ADDR_ENV).is_none() {
            return Ok(None);
        }
        let addr_var = std::env::var(DEBUG_ADDR_ENV).map_err(|_| {
            TransportError::InvalidConfig(format!("{DEBUG_ADDR_ENV} is not valid unicode"))
        })?;
        let token_var = std::env::var(DEBUG_TOKEN_ENV).map_err(|_| {
            TransportError::InvalidConfig(format!(
                "{DEBUG_TOKEN_ENV} is missing or not valid unicode"
            ))
        })?;
        Self::from_values(&addr_var, &token_var).map(Some)
    }

    /// The parsing and validation logic behind [`Self::from_env`], factored out so it can be
    /// unit-tested with plain strings instead of mutating the (unsafe-to-mutate, process-wide)
    /// environment.
    fn from_values(addr_var: &str, token_var: &str) -> Result<TcpConfig, TransportError> {
        let addr: SocketAddrV4 = addr_var.parse().map_err(|_| {
            TransportError::InvalidConfig(format!(
                "{DEBUG_ADDR_ENV} must be an IPv4 socket address like 127.0.0.1:{DEFAULT_DEBUG_PORT}, got {addr_var:?}"
            ))
        })?;
        if *addr.ip() != Ipv4Addr::LOCALHOST {
            return Err(TransportError::InvalidConfig(format!(
                "{DEBUG_ADDR_ENV} must use 127.0.0.1, got {}",
                addr.ip()
            )));
        }
        let token = parse_hex_token(token_var).ok_or_else(|| {
            TransportError::InvalidConfig(format!(
                "{DEBUG_TOKEN_ENV} must be exactly 64 hex digits"
            ))
        })?;
        Ok(TcpConfig { addr, token })
    }
}

/// Parses exactly 64 ASCII hex digits into a 32-byte token, or `None` if `value` does not match.
fn parse_hex_token(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 || !value.is_ascii() {
        return None;
    }
    let mut token = [0u8; 32];
    let (pairs, _remainder) = value.as_bytes().as_chunks::<2>();
    for (index, pair) in pairs.iter().enumerate() {
        // `pair` is exactly 2 ASCII bytes (checked above), so this is always valid UTF-8.
        let hex_pair = std::str::from_utf8(pair).ok()?;
        token[index] = u8::from_str_radix(hex_pair, 16).ok()?;
    }
    Some(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::socket_tests_enabled;

    fn skip_without_sockets() -> bool {
        if !socket_tests_enabled() {
            eprintln!("skipping: set GRIMOIRE_SOCKET_TESTS=1");
            true
        } else {
            false
        }
    }

    fn any_port_config() -> TcpConfig {
        TcpConfig::new(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0), [0x11; 32])
    }

    #[test]
    fn bind_rejects_non_loopback_without_touching_a_socket() {
        let config = TcpConfig::new(SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 0), [0u8; 32]);
        assert!(matches!(
            TcpServerTransport::bind(config),
            Err(TransportError::NonLoopbackAddress(_))
        ));
    }

    #[test]
    fn from_values_parses_and_validates() {
        assert!(matches!(
            TcpConfig::from_values("not-an-address", &"ab".repeat(32)),
            Err(TransportError::InvalidConfig(_))
        ));
        assert!(matches!(
            TcpConfig::from_values("0.0.0.0:1234", &"ab".repeat(32)),
            Err(TransportError::InvalidConfig(_))
        ));
        assert!(matches!(
            TcpConfig::from_values("127.0.0.1:1234", "not-hex-and-wrong-length"),
            Err(TransportError::InvalidConfig(_))
        ));
        assert!(matches!(
            TcpConfig::from_values("127.0.0.1:1234", &"zz".repeat(32)),
            Err(TransportError::InvalidConfig(_))
        ));

        let token_hex = "ab".repeat(32);
        let config = TcpConfig::from_values("127.0.0.1:1234", &token_hex).expect("valid config");
        assert_eq!(config.addr, SocketAddrV4::new(Ipv4Addr::LOCALHOST, 1234));
        assert_eq!(config.token, [0xabu8; 32]);
    }

    #[test]
    fn from_env_returns_none_when_unset_in_the_ambient_environment() {
        // Mutating process-wide environment variables from a test is unsound under concurrent
        // access (and `unsafe` besides, which this crate denies), so this only checks the
        // common case rather than driving every branch through real env vars; `from_values`
        // above covers the parsing/validation logic itself without touching global state.
        if std::env::var_os(DEBUG_ADDR_ENV).is_none() {
            assert_eq!(TcpConfig::from_env(), Ok(None));
        }
    }

    #[test]
    fn drop_without_a_connected_client_returns_quickly() {
        if skip_without_sockets() {
            return;
        }
        let transport = TcpServerTransport::bind(any_port_config()).unwrap();
        let started = Instant::now();
        drop(transport);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_client_can_round_trip_frames() {
        if skip_without_sockets() {
            return;
        }
        let mut server = TcpServerTransport::bind(any_port_config()).unwrap();
        let mut client = TcpStream::connect(server.local_addr()).unwrap();
        client.set_nonblocking(true).unwrap();

        let frame = Frame {
            id: crate::MessageId(1),
            seq: 1,
            payload: vec![1, 2, 3, 4],
        };
        let mut bytes = Vec::new();
        encode_frame(&frame, &mut bytes).unwrap();
        write_all_bounded(&mut client, &bytes, Duration::from_secs(1));

        let mut inbox = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(1);
        while inbox.is_empty() && Instant::now() < deadline {
            server.poll(&mut inbox).unwrap();
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(inbox, vec![frame]);
        assert!(server.is_connected());
    }

    #[test]
    fn a_second_simultaneous_client_is_rejected() {
        if skip_without_sockets() {
            return;
        }
        let server = TcpServerTransport::bind(any_port_config()).unwrap();
        let first = TcpStream::connect(server.local_addr()).unwrap();
        // Give the IO thread a moment to accept the first client before the second connects.
        thread::sleep(Duration::from_millis(50));
        let mut second = TcpStream::connect(server.local_addr()).unwrap();
        second.set_nonblocking(true).unwrap();

        // The second connection should be closed by the server rather than serviced: reading
        // from it eventually observes EOF instead of ever seeing application data.
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut buf = [0u8; 16];
        let mut saw_eof = false;
        while Instant::now() < deadline {
            match second.read(&mut buf) {
                Ok(0) => {
                    saw_eof = true;
                    break;
                }
                Ok(_) => break, // Received data would also mean it was NOT rejected; fail below.
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(_) => {
                    saw_eof = true;
                    break;
                }
            }
        }
        assert!(saw_eof, "a second simultaneous client must be closed");
        drop(first);
    }

    #[test]
    fn garbage_bytes_close_the_connection_without_panicking() {
        if skip_without_sockets() {
            return;
        }
        let mut server = TcpServerTransport::bind(any_port_config()).unwrap();
        let mut client = TcpStream::connect(server.local_addr()).unwrap();
        client.set_nonblocking(true).unwrap();

        // A declared length far beyond `MAX_FRAME_LEN` is pure garbage from the framing layer's
        // point of view.
        let garbage = (crate::MAX_FRAME_LEN + 1).to_le_bytes();
        write_all_bounded(&mut client, &garbage, Duration::from_secs(1));

        // The connection should close; a fresh client can then connect and talk normally.
        thread::sleep(Duration::from_millis(100));
        let mut inbox = Vec::new();
        server.poll(&mut inbox).unwrap();
        assert!(inbox.is_empty());

        let mut reconnect = TcpStream::connect(server.local_addr()).unwrap();
        reconnect.set_nonblocking(true).unwrap();
        let frame = Frame {
            id: crate::MessageId(2),
            seq: 1,
            payload: vec![9],
        };
        let mut bytes = Vec::new();
        encode_frame(&frame, &mut bytes).unwrap();
        write_all_bounded(&mut reconnect, &bytes, Duration::from_secs(1));

        let mut inbox = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(1);
        while inbox.is_empty() && Instant::now() < deadline {
            server.poll(&mut inbox).unwrap();
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(inbox, vec![frame]);
    }
}
