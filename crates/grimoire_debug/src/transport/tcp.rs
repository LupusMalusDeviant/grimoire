//! [`TcpServerTransport`]: the TCP debug link (contract §13 "TCP (Feature `tcp`)"), gated
//! behind the `tcp` feature.
//!
//! Loopback-only binding and exactly one IO thread (`grimoire-debug-io`) that owns the
//! non-blocking listener and at most one client socket. Frames reach the owner over a bounded
//! channel and are taken from it by [`DebugTransport::poll`], which never blocks; the owner
//! (the engine's [`crate::EngineLink`]) runs the handshake and the dispatch on them at its own
//! frame boundaries (Plan 0002 WP8.4).
//!
//! The IO thread enforces what contract §13 assigns to the transport rather than to the facade:
//!
//! - **One frame before the handshake.** Of a new connection it reads exactly one frame whose
//!   `len` is at most [`crate::MAX_HELLO_FRAME_LEN`] (a larger `len` is answered with
//!   `Error(TooLarge)` and closed before any payload byte is read) and then nothing more, until
//!   the owner has sent a `Hello` frame. A peer without a valid token can make it buffer one small
//!   frame at most.
//! - **Handshake timeout.** Without a complete first frame within [`crate::HANDSHAKE_TIMEOUT`] of
//!   the connection, it sends `Error(HandshakeRequired)` and closes.
//! - **One client.** A second simultaneous client receives `Error(Busy)` and is closed; the first
//!   stays connected.
//! - **Backpressure.** Frames waiting for `poll` hold at most
//!   [`crate::MAX_INBOUND_QUEUED_BYTES`] payload bytes (and 256 frames); at the limit the thread
//!   stops reading the socket instead of dropping frames.
//! - **Connection boundaries.** When a connection ends, `poll` returns every frame that arrived
//!   before the end and then [`TransportError::Disconnected`] once, before any frame of a later
//!   connection. [`DebugTransport::disconnect`] closes only the connection whose frames the owner
//!   has seen, after writing the frames the owner already sent to it (a rejecting `Error`), and a
//!   frame sent for an earlier connection is never written to a later one.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use super::{DEBUG_ADDR_ENV, DEBUG_TOKEN_ENV, DEFAULT_DEBUG_PORT, DebugTransport, TransportError};
use crate::frame::{Frame, FrameDecoder, encode_frame};
use crate::generated::debug_protocol::{ErrorCode, ErrorMsg, catalogue};
use crate::message::Message;

/// Frames per direction the channels between the IO thread and the owner hold (contract §13).
const CHANNEL_FRAMES: usize = 256;

/// Largest number of socket reads per loop iteration, so a peer that writes faster than the
/// thread reads cannot keep it from its other duties (stop flag, writes, accepting).
const READS_PER_ITERATION: usize = 64;

/// Length of the `len` prefix of a frame on the wire.
const LEN_PREFIX: usize = 4;

/// What the IO thread hands the owner, in order. Every entry names the connection it belongs to.
#[derive(Debug)]
enum Inbound {
    Frame { connection: u64, frame: Frame },
    Closed { connection: u64 },
}

/// State shared by the owner's handle and the IO thread.
#[derive(Debug, Default)]
struct Shared {
    stop: AtomicBool,
    /// Number of the connected client's connection, `0` while none is connected.
    current: AtomicU64,
    /// Connection the owner asked to close (`0` = none); connections count from 1.
    disconnect: AtomicU64,
    /// Payload bytes of the frames in the inbound channel (contract §13: at most
    /// `MAX_INBOUND_QUEUED_BYTES`).
    queued_bytes: AtomicUsize,
    /// Highest value `queued_bytes` ever reached, for the behaviour test of the limit.
    queued_bytes_peak: AtomicUsize,
}

/// TCP transport for the debug link, bound exclusively to `127.0.0.1` (contract §13).
///
/// See the module documentation for what the IO thread enforces. Garbage bytes close the
/// connection they arrive on without panicking; the listener keeps accepting new clients.
pub struct TcpServerTransport {
    shared: Arc<Shared>,
    inbound: mpsc::Receiver<Inbound>,
    outbound: mpsc::SyncSender<(u64, Frame)>,
    /// The connection of the newest frame or end `poll` returned (`0` before the first).
    connection: u64,
    /// The connection this handle last asked to close (`0` = none).
    disconnected: u64,
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

        let (inbound_tx, inbound_rx) = mpsc::sync_channel::<Inbound>(CHANNEL_FRAMES);
        let (outbound_tx, outbound_rx) = mpsc::sync_channel::<(u64, Frame)>(CHANNEL_FRAMES);
        let shared = Arc::new(Shared::default());

        let thread_shared = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("grimoire-debug-io".to_owned())
            .spawn(move || {
                IoLoop::new(listener, thread_shared, inbound_tx, outbound_rx).run();
            })
            .map_err(io_err)?;

        Ok(Self {
            shared,
            inbound: inbound_rx,
            outbound: outbound_tx,
            connection: 0,
            disconnected: 0,
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
            .field("connection", &self.connection)
            .finish_non_exhaustive()
    }
}

impl Drop for TcpServerTransport {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            // Bounded by one `IO_POLL_INTERVAL` loop iteration plus the bounded writes of that
            // iteration (each at most `IO_WRITE_TIMEOUT`, see `write_all_bounded`).
            let _ = thread.join();
        }
    }
}

impl DebugTransport for TcpServerTransport {
    fn poll(&mut self, inbox: &mut Vec<Frame>) -> Result<(), TransportError> {
        loop {
            match self.inbound.try_recv() {
                Ok(Inbound::Frame { connection, frame }) => {
                    self.shared
                        .queued_bytes
                        .fetch_sub(frame.payload.len(), Ordering::SeqCst);
                    self.connection = connection;
                    inbox.push(frame);
                }
                Ok(Inbound::Closed { connection }) => {
                    self.connection = connection;
                    return Err(TransportError::Disconnected);
                }
                Err(_) => return Ok(()),
            }
        }
    }

    fn send(&mut self, frame: &Frame) -> Result<(), TransportError> {
        if self.connection == 0 || !self.is_connected() {
            return Err(TransportError::NotConnected);
        }
        self.outbound
            .try_send((self.connection, frame.clone()))
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => TransportError::QueueFull,
                mpsc::TrySendError::Disconnected(_) => TransportError::Disconnected,
            })
    }

    fn is_connected(&self) -> bool {
        let current = self.shared.current.load(Ordering::SeqCst);
        // A connection this handle asked to close counts as closed at once, before the IO thread
        // has written its last frames and shut the socket.
        current != 0 && current != self.disconnected
    }

    fn disconnect(&mut self) {
        if self.connection != 0 {
            self.disconnected = self.connection;
            self.shared
                .disconnect
                .store(self.connection, Ordering::SeqCst);
        }
    }
}

/// Where the current connection stands (contract §13 "TCP").
#[derive(Debug)]
enum Phase {
    /// Reading the first frame, at most `MAX_HELLO_FRAME_LEN`, into a small buffer.
    First { bytes: Vec<u8>, deadline: Instant },
    /// First frame delivered; nothing more is read until the owner sends a `Hello`.
    Gated,
    /// Handshake reply sent: frames are decoded as they come.
    Open,
}

struct Client {
    stream: TcpStream,
    connection: u64,
    phase: Phase,
    decoder: FrameDecoder,
}

/// The IO thread: accept, read, deliver, write, repeat, at most [`crate::IO_POLL_INTERVAL`]
/// apart. Never panics on any bytes a peer sends.
struct IoLoop {
    listener: TcpListener,
    shared: Arc<Shared>,
    inbound: mpsc::SyncSender<Inbound>,
    outbound: mpsc::Receiver<(u64, Frame)>,
    client: Option<Client>,
    /// Entries of earlier or current connections the inbound channel had no room for yet, in
    /// order. Holds at most one frame plus one end marker: nothing is read while it is non-empty.
    undelivered: VecDeque<Inbound>,
    connections: u64,
    read_buf: Box<[u8; 8192]>,
}

impl IoLoop {
    fn new(
        listener: TcpListener,
        shared: Arc<Shared>,
        inbound: mpsc::SyncSender<Inbound>,
        outbound: mpsc::Receiver<(u64, Frame)>,
    ) -> Self {
        Self {
            listener,
            shared,
            inbound,
            outbound,
            client: None,
            undelivered: VecDeque::new(),
            connections: 0,
            read_buf: Box::new([0; 8192]),
        }
    }

    fn run(mut self) {
        while !self.shared.stop.load(Ordering::Relaxed) {
            self.deliver_undelivered();
            self.handle_disconnect_request();
            self.accept();
            self.read();
            self.write();
            self.check_handshake_deadline();
            thread::sleep(crate::IO_POLL_INTERVAL);
        }
    }

    /// Hands queued entries to the owner in order; stops at the first that does not fit.
    fn deliver_undelivered(&mut self) {
        while let Some(entry) = self.undelivered.pop_front() {
            if let Err(entry) = self.try_deliver(entry) {
                self.undelivered.push_front(entry);
                return;
            }
        }
    }

    /// Puts one entry into the inbound channel if the frame and byte limits allow it.
    fn try_deliver(&mut self, entry: Inbound) -> Result<(), Inbound> {
        let bytes = match &entry {
            Inbound::Frame { frame, .. } => frame.payload.len(),
            Inbound::Closed { .. } => 0,
        };
        let queued = self.shared.queued_bytes.load(Ordering::SeqCst);
        // A single frame always fits an empty queue, whatever its size (at most MAX_FRAME_LEN).
        if bytes > 0 && queued > 0 && queued + bytes > crate::MAX_INBOUND_QUEUED_BYTES {
            return Err(entry);
        }
        let now_queued = self.shared.queued_bytes.fetch_add(bytes, Ordering::SeqCst) + bytes;
        match self.inbound.try_send(entry) {
            Ok(()) => {
                self.shared
                    .queued_bytes_peak
                    .fetch_max(now_queued, Ordering::SeqCst);
                Ok(())
            }
            Err(mpsc::TrySendError::Full(entry) | mpsc::TrySendError::Disconnected(entry)) => {
                self.shared.queued_bytes.fetch_sub(bytes, Ordering::SeqCst);
                Err(entry)
            }
        }
    }

    /// Queues or delivers one frame of the current connection.
    fn deliver_frame(&mut self, connection: u64, frame: Frame) {
        let entry = Inbound::Frame { connection, frame };
        if !self.undelivered.is_empty() {
            self.undelivered.push_back(entry);
        } else if let Err(entry) = self.try_deliver(entry) {
            self.undelivered.push_back(entry);
        }
    }

    fn handle_disconnect_request(&mut self) {
        let requested = self.shared.disconnect.load(Ordering::SeqCst);
        let current = self.client.as_ref().map(|client| client.connection);
        if current == Some(requested) {
            // Write what the owner already sent to this connection (a rejecting `Error`) first.
            self.write();
            self.close();
        }
    }

    fn accept(&mut self) {
        loop {
            // A new connection waits in the backlog until the previous one's entries are handed
            // over, so its frames can never overtake the previous connection's end.
            if self.client.is_none() && !self.undelivered.is_empty() {
                return;
            }
            match self.listener.accept() {
                Ok((stream, _addr)) => {
                    if stream.set_nonblocking(true).is_err() {
                        continue;
                    }
                    if self.client.is_some() {
                        // Contract §13: a second simultaneous client gets Busy and is closed.
                        let mut stream = stream;
                        let _ =
                            write_error(&mut stream, ErrorCode::Busy, "another tool is connected");
                        continue;
                    }
                    // Frames the owner sent for an earlier connection are never written to this one.
                    while self.outbound.try_recv().is_ok() {}
                    self.connections += 1;
                    self.client = Some(Client {
                        stream,
                        connection: self.connections,
                        phase: Phase::First {
                            bytes: Vec::with_capacity(
                                LEN_PREFIX + crate::MAX_HELLO_FRAME_LEN as usize,
                            ),
                            deadline: Instant::now() + crate::HANDSHAKE_TIMEOUT,
                        },
                        decoder: FrameDecoder::new(),
                    });
                    self.shared
                        .current
                        .store(self.connections, Ordering::SeqCst);
                }
                Err(_) => return, // WouldBlock, or a transient error: retried next iteration.
            }
        }
    }

    fn read(&mut self) {
        let Some(client) = self.client.as_ref() else {
            return;
        };
        match client.phase {
            Phase::First { .. } => self.read_first_frame(),
            Phase::Gated => {}
            Phase::Open => self.read_open(),
        }
    }

    /// Reads the first frame of a connection byte-exactly, checking `len` before any payload byte.
    fn read_first_frame(&mut self) {
        let step = match self.client.as_mut() {
            Some(client) => first_frame_step(client, &mut self.read_buf[..]),
            None => return,
        };
        match step {
            ReadStep::Idle | ReadStep::Continue => {}
            ReadStep::Frame { connection, frame } => self.deliver_frame(connection, frame),
            ReadStep::TooLarge => {
                if let Some(client) = self.client.as_mut() {
                    let _ = write_error(
                        &mut client.stream,
                        ErrorCode::TooLarge,
                        "first frame exceeds MAX_HELLO_FRAME_LEN",
                    );
                }
                self.close();
            }
            ReadStep::Close => self.close(),
        }
    }

    /// Reads and decodes after the handshake, pausing at the inbound limits.
    fn read_open(&mut self) {
        for _ in 0..READS_PER_ITERATION {
            if !self.undelivered.is_empty() {
                return; // Backpressure: nothing more is read until the owner catches up.
            }
            let queued = self.shared.queued_bytes.load(Ordering::SeqCst);
            let step = match self.client.as_mut() {
                Some(client) => open_step(client, &mut self.read_buf[..], queued),
                None => return,
            };
            match step {
                ReadStep::Continue => {}
                ReadStep::Frame { connection, frame } => self.deliver_frame(connection, frame),
                ReadStep::Idle => return,
                ReadStep::TooLarge | ReadStep::Close => {
                    self.close();
                    return;
                }
            }
        }
    }

    /// Writes the owner's frames for the current connection; frames for any other are dropped.
    fn write(&mut self) {
        while let Ok((connection, frame)) = self.outbound.try_recv() {
            let Some(client) = self.client.as_mut() else {
                continue;
            };
            if connection != client.connection {
                continue;
            }
            let mut bytes = Vec::new();
            if encode_frame(&frame, &mut bytes).is_err() {
                continue; // Too large to encode; drop this one frame, keep connected.
            }
            let written = write_all_bounded(&mut client.stream, &bytes, crate::IO_WRITE_TIMEOUT);
            if written && frame.id.0 == catalogue::HELLO && matches!(client.phase, Phase::Gated) {
                client.phase = Phase::Open;
            }
            if !written {
                self.close();
                return;
            }
        }
    }

    fn check_handshake_deadline(&mut self) {
        let expired = match self.client.as_mut() {
            Some(client) => match client.phase {
                Phase::First { deadline, .. } if Instant::now() >= deadline => {
                    let _ = write_error(
                        &mut client.stream,
                        ErrorCode::HandshakeRequired,
                        "no Hello arrived within the handshake timeout",
                    );
                    true
                }
                _ => false,
            },
            None => false,
        };
        if expired {
            self.close();
        }
    }

    /// Ends the current connection: frames it delivered stay, followed by its end marker.
    fn close(&mut self) {
        let Some(client) = self.client.take() else {
            return;
        };
        let _ = client.stream.shutdown(std::net::Shutdown::Both);
        self.shared.current.store(0, Ordering::SeqCst);
        let end = Inbound::Closed {
            connection: client.connection,
        };
        if !self.undelivered.is_empty() {
            self.undelivered.push_back(end);
        } else if let Err(end) = self.try_deliver(end) {
            self.undelivered.push_back(end);
        }
    }
}

/// What one read step of the IO thread asks the loop to do.
enum ReadStep {
    /// Nothing more to read now.
    Idle,
    /// Bytes were read; step again.
    Continue,
    /// A complete frame to hand to the owner.
    Frame { connection: u64, frame: Frame },
    /// The first frame is longer than `MAX_HELLO_FRAME_LEN`: answer `TooLarge` and close.
    TooLarge,
    /// End of stream, a socket error or garbage: close.
    Close,
}

/// Reads towards the first frame of `client` without reading past it (contract §13 "TCP"): the
/// `len` prefix first, checked against `MAX_HELLO_FRAME_LEN` before any payload byte, then exactly
/// the bytes it announces.
fn first_frame_step(client: &mut Client, read_buf: &mut [u8]) -> ReadStep {
    let Phase::First { bytes, .. } = &mut client.phase else {
        return ReadStep::Idle;
    };
    loop {
        let wanted = if bytes.len() < LEN_PREFIX {
            LEN_PREFIX - bytes.len()
        } else {
            let len = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            if len > crate::MAX_HELLO_FRAME_LEN {
                return ReadStep::TooLarge;
            }
            if len < crate::frame::HEADER_LEN as u32 {
                return ReadStep::Close;
            }
            LEN_PREFIX + len as usize - bytes.len()
        };
        if wanted == 0 {
            break;
        }
        match client.stream.read(&mut read_buf[..wanted]) {
            Ok(0) => return ReadStep::Close,
            Ok(n) => bytes.extend_from_slice(&read_buf[..n]),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return ReadStep::Idle,
            Err(_) => return ReadStep::Close,
        }
    }

    client.decoder.push(bytes);
    match client.decoder.next_frame() {
        Ok(Some(frame)) => {
            client.phase = Phase::Gated;
            ReadStep::Frame {
                connection: client.connection,
                frame,
            }
        }
        // The buffer holds exactly one frame of a checked length, so `None` cannot occur; a
        // header error (non-zero flags) closes like garbage.
        Ok(None) | Err(_) => ReadStep::Close,
    }
}

/// One decode-or-read step after the handshake. Reads only when no complete frame is buffered
/// and the inbound queue is below its byte limit.
fn open_step(client: &mut Client, read_buf: &mut [u8], queued_bytes: usize) -> ReadStep {
    match client.decoder.next_frame() {
        Ok(Some(frame)) => {
            return ReadStep::Frame {
                connection: client.connection,
                frame,
            };
        }
        Ok(None) => {}
        Err(_) => return ReadStep::Close, // Garbage or a malformed length.
    }
    if queued_bytes >= crate::MAX_INBOUND_QUEUED_BYTES {
        return ReadStep::Idle;
    }
    match client.stream.read(read_buf) {
        Ok(0) => ReadStep::Close,
        Ok(n) => {
            client.decoder.push(&read_buf[..n]);
            ReadStep::Continue
        }
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => ReadStep::Idle,
        Err(_) => ReadStep::Close,
    }
}

/// Writes an `Error` frame with `seq = 0` and `in_reply_to = 0` (the IO thread answers before or
/// instead of reading the frame it refers to).
fn write_error(stream: &mut TcpStream, code: ErrorCode, message: &str) -> bool {
    let Ok(frame) = Message::Error(ErrorMsg {
        code,
        in_reply_to: 0,
        message: message.to_owned(),
    })
    .to_frame(0) else {
        return false;
    };
    let mut bytes = Vec::new();
    encode_frame(&frame, &mut bytes).is_ok()
        && write_all_bounded(stream, &bytes, crate::IO_WRITE_TIMEOUT)
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
    /// Shared-secret token a tool must present in its `Hello` (contract §13 "Handshake" step 6).
    /// The transport only carries it: the engine's [`crate::EngineLink`] checks it, with an
    /// [`crate::EngineIdentity`] built from this value.
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
    use crate::generated::debug_protocol::{Hello, PeerRole};
    use crate::handshake::EngineIdentity;
    use crate::link::{EngineLink, LinkEvent};
    use crate::{MessageId, socket_tests_enabled};

    fn skip_without_sockets() -> bool {
        if !socket_tests_enabled() {
            eprintln!("skipping: set GRIMOIRE_SOCKET_TESTS=1");
            true
        } else {
            false
        }
    }

    const TOKEN: [u8; 32] = [0x11; 32];

    fn any_port_config() -> TcpConfig {
        TcpConfig::new(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0), TOKEN)
    }

    fn frame(id: u16, seq: u32, payload: Vec<u8>) -> Frame {
        Frame {
            id: MessageId(id),
            seq,
            payload,
        }
    }

    /// A raw socket client that speaks the frame envelope and nothing else.
    struct RawClient {
        stream: TcpStream,
        decoder: FrameDecoder,
        closed: bool,
    }

    impl RawClient {
        fn connect(server: &TcpServerTransport) -> Self {
            let stream = TcpStream::connect(server.local_addr()).expect("connect");
            stream.set_nonblocking(true).expect("set_nonblocking");
            Self {
                stream,
                decoder: FrameDecoder::new(),
                closed: false,
            }
        }

        fn send(&mut self, frame: &Frame) {
            let mut bytes = Vec::new();
            encode_frame(frame, &mut bytes).unwrap();
            self.send_bytes(&bytes);
        }

        fn send_bytes(&mut self, bytes: &[u8]) {
            assert!(write_all_bounded(
                &mut self.stream,
                bytes,
                Duration::from_secs(5)
            ));
        }

        fn pump(&mut self) {
            let mut buf = [0u8; 4096];
            loop {
                match self.stream.read(&mut buf) {
                    Ok(0) => {
                        self.closed = true;
                        return;
                    }
                    Ok(n) => self.decoder.push(&buf[..n]),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => return,
                    Err(_) => {
                        self.closed = true;
                        return;
                    }
                }
            }
        }

        /// Frames received until `count` arrived or `timeout` passed.
        fn receive(&mut self, count: usize, timeout: Duration) -> Vec<Frame> {
            let deadline = Instant::now() + timeout;
            let mut frames = Vec::new();
            loop {
                self.pump();
                while let Ok(Some(frame)) = self.decoder.next_frame() {
                    frames.push(frame);
                }
                if frames.len() >= count || self.closed || Instant::now() >= deadline {
                    return frames;
                }
                thread::sleep(Duration::from_millis(2));
            }
        }

        /// Whether the server closed the connection within `timeout`.
        fn closed_within(&mut self, timeout: Duration) -> bool {
            let deadline = Instant::now() + timeout;
            while !self.closed && Instant::now() < deadline {
                self.pump();
                thread::sleep(Duration::from_millis(2));
            }
            self.closed
        }
    }

    /// Polls `server` until `count` frames arrived or `timeout` passed; returns them and whether
    /// the end of a connection was reported.
    fn poll_frames(
        server: &mut TcpServerTransport,
        count: usize,
        timeout: Duration,
    ) -> (Vec<Frame>, bool) {
        let deadline = Instant::now() + timeout;
        let mut frames = Vec::new();
        loop {
            if server.poll(&mut frames) == Err(TransportError::Disconnected) {
                return (frames, true);
            }
            if frames.len() >= count || Instant::now() >= deadline {
                return (frames, false);
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn error_code(frame: &Frame) -> ErrorCode {
        match Message::from_frame(frame) {
            Ok(Message::Error(error)) => error.code,
            other => panic!("expected an Error frame, got {other:?}"),
        }
    }

    fn hello_frame(seq: u32, token: [u8; 32]) -> Frame {
        Message::Hello(Hello {
            protocol_version: crate::PROTOCOL_VERSION,
            role: PeerRole::Tool,
            engine_version: "0.4.0".to_owned(),
            build_hash: "unknown".to_owned(),
            token,
            stats_interval_frames: 0,
        })
        .to_frame(seq)
        .unwrap()
    }

    /// Opens the pre-handshake gate the way the engine does: poll the first frame, send a Hello.
    fn open_gate(server: &mut TcpServerTransport, client: &mut RawClient) {
        client.send(&hello_frame(1, TOKEN));
        let (frames, closed) = poll_frames(server, 1, Duration::from_secs(5));
        assert_eq!((frames.len(), closed), (1, false));
        server.send(&frame(catalogue::HELLO, 1, vec![])).unwrap();
        assert_eq!(client.receive(1, Duration::from_secs(5)).len(), 1);
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
    fn a_send_without_a_connection_is_not_connected() {
        // No socket is needed to observe this: nothing ever connected.
        if skip_without_sockets() {
            return;
        }
        let mut server = TcpServerTransport::bind(any_port_config()).unwrap();
        assert_eq!(
            server.send(&frame(1, 1, vec![])),
            Err(TransportError::NotConnected)
        );
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
    fn the_first_frame_arrives_and_nothing_more_is_read_until_a_hello_is_sent() {
        if skip_without_sockets() {
            return;
        }
        let mut server = TcpServerTransport::bind(any_port_config()).unwrap();
        let mut client = RawClient::connect(&server);
        let first = hello_frame(1, TOKEN);
        let second = frame(catalogue::SWAP_SIGIL_UNIT, 2, vec![7; 16]);
        client.send(&first);
        client.send(&second);

        let (frames, closed) = poll_frames(&mut server, 2, Duration::from_millis(500));
        assert_eq!(
            frames,
            [first],
            "only the first frame is read before the handshake"
        );
        assert!(!closed);
        assert!(server.is_connected());

        server.send(&frame(catalogue::HELLO, 1, vec![])).unwrap();
        let (frames, _) = poll_frames(&mut server, 1, Duration::from_secs(5));
        assert_eq!(frames, [second], "the engine's Hello opens the connection");
    }

    #[test]
    fn an_over_length_first_frame_is_too_large_and_closes() {
        if skip_without_sockets() {
            return;
        }
        let mut server = TcpServerTransport::bind(any_port_config()).unwrap();
        let mut client = RawClient::connect(&server);
        // Only the length prefix: the server answers without waiting for the announced bytes.
        client.send_bytes(&(crate::MAX_HELLO_FRAME_LEN + 1).to_le_bytes());

        let replies = client.receive(1, Duration::from_secs(5));
        assert_eq!(replies.len(), 1);
        assert_eq!(error_code(&replies[0]), ErrorCode::TooLarge);
        assert!(client.closed_within(Duration::from_secs(5)));
        let (frames, closed) = poll_frames(&mut server, 1, Duration::from_secs(5));
        assert!(frames.is_empty());
        assert!(closed, "the end of the connection is reported once");
    }

    #[test]
    fn garbage_bytes_close_the_connection_and_a_new_client_is_served() {
        if skip_without_sockets() {
            return;
        }
        let mut server = TcpServerTransport::bind(any_port_config()).unwrap();
        let mut garbage = RawClient::connect(&server);
        garbage.send_bytes(&3u32.to_le_bytes()); // shorter than the frame header
        assert!(garbage.closed_within(Duration::from_secs(5)));
        let (frames, closed) = poll_frames(&mut server, 1, Duration::from_secs(5));
        assert!(frames.is_empty());
        assert!(closed);

        let mut client = RawClient::connect(&server);
        let hello = hello_frame(1, TOKEN);
        client.send(&hello);
        let (frames, closed) = poll_frames(&mut server, 1, Duration::from_secs(5));
        assert_eq!(frames, [hello]);
        assert!(!closed);
    }

    #[test]
    fn a_second_simultaneous_client_is_busy_and_the_first_stays_connected() {
        if skip_without_sockets() {
            return;
        }
        let mut server = TcpServerTransport::bind(any_port_config()).unwrap();
        let mut first = RawClient::connect(&server);
        open_gate(&mut server, &mut first);

        let mut second = RawClient::connect(&server);
        let replies = second.receive(1, Duration::from_secs(5));
        assert_eq!(replies.len(), 1);
        assert_eq!(error_code(&replies[0]), ErrorCode::Busy);
        assert!(second.closed_within(Duration::from_secs(5)));

        assert!(server.is_connected());
        let message = frame(catalogue::SWAP_SIGIL_UNIT, 2, vec![1, 2, 3]);
        first.send(&message);
        let (frames, closed) = poll_frames(&mut server, 1, Duration::from_secs(5));
        assert_eq!((frames, closed), (vec![message], false));
    }

    #[test]
    fn a_silent_client_is_closed_after_the_handshake_timeout_and_a_new_client_connects() {
        if skip_without_sockets() {
            return;
        }
        let mut server = TcpServerTransport::bind(any_port_config()).unwrap();
        let started = Instant::now();
        let mut silent = RawClient::connect(&server);
        let replies = silent.receive(1, crate::HANDSHAKE_TIMEOUT + Duration::from_secs(5));
        assert!(started.elapsed() >= crate::HANDSHAKE_TIMEOUT);
        assert_eq!(replies.len(), 1);
        assert_eq!(error_code(&replies[0]), ErrorCode::HandshakeRequired);
        assert!(silent.closed_within(Duration::from_secs(5)));
        let (_, closed) = poll_frames(&mut server, 1, Duration::from_secs(5));
        assert!(closed);

        let mut client = RawClient::connect(&server);
        let hello = hello_frame(1, TOKEN);
        client.send(&hello);
        let (frames, _) = poll_frames(&mut server, 1, Duration::from_secs(5));
        assert_eq!(frames, [hello]);
    }

    #[test]
    fn a_requested_disconnect_writes_the_pending_error_first() {
        if skip_without_sockets() {
            return;
        }
        let mut server = TcpServerTransport::bind(any_port_config()).unwrap();
        let mut client = RawClient::connect(&server);
        client.send(&hello_frame(1, [0; 32]));
        let (frames, _) = poll_frames(&mut server, 1, Duration::from_secs(5));
        assert_eq!(frames.len(), 1);

        let rejection = Message::Error(ErrorMsg {
            code: ErrorCode::Unauthorized,
            in_reply_to: 1,
            message: "wrong token".to_owned(),
        })
        .to_frame(0)
        .unwrap();
        server.send(&rejection).unwrap();
        server.disconnect();

        let replies = client.receive(1, Duration::from_secs(5));
        assert_eq!(replies, [rejection]);
        assert!(client.closed_within(Duration::from_secs(5)));
        let (_, closed) = poll_frames(&mut server, 1, Duration::from_secs(5));
        assert!(closed);
    }

    #[test]
    fn many_frames_arrive_in_order_without_loss_and_within_the_byte_limit() {
        if skip_without_sockets() {
            return;
        }
        let mut server = TcpServerTransport::bind(any_port_config()).unwrap();
        let mut client = RawClient::connect(&server);
        open_gate(&mut server, &mut client);

        // More frames than the channel holds, and more payload bytes than the queue may hold:
        // nothing is polled while the client writes.
        let big: Vec<Frame> = (0..40u32)
            .map(|index| {
                frame(
                    catalogue::SWAP_SIGIL_UNIT,
                    index + 2,
                    vec![index as u8; 1 << 20],
                )
            })
            .collect();
        let small: Vec<Frame> = (0..600u32)
            .map(|index| {
                frame(
                    catalogue::SWAP_SIGIL_UNIT,
                    index + 42,
                    index.to_le_bytes().to_vec(),
                )
            })
            .collect();
        let expected: Vec<Frame> = big.iter().chain(&small).cloned().collect();
        let writer_stream = client.stream.try_clone().unwrap();
        let to_write = expected.clone();
        let writer = thread::spawn(move || {
            let mut stream = writer_stream;
            for frame in &to_write {
                let mut bytes = Vec::new();
                encode_frame(frame, &mut bytes).unwrap();
                assert!(write_all_bounded(
                    &mut stream,
                    &bytes,
                    Duration::from_secs(30)
                ));
            }
        });
        thread::sleep(Duration::from_millis(1500));
        assert!(
            server.shared.queued_bytes_peak.load(Ordering::SeqCst)
                <= crate::MAX_INBOUND_QUEUED_BYTES
        );

        let (frames, closed) = poll_frames(&mut server, expected.len(), Duration::from_secs(60));
        writer.join().unwrap();
        assert!(!closed);
        assert_eq!(frames.len(), expected.len());
        assert!(
            frames == expected,
            "order preserved, nothing dropped or duplicated"
        );
        assert!(
            server.shared.queued_bytes_peak.load(Ordering::SeqCst)
                <= crate::MAX_INBOUND_QUEUED_BYTES
        );
    }

    #[test]
    fn a_frame_sent_for_an_earlier_connection_never_reaches_a_later_one() {
        if skip_without_sockets() {
            return;
        }
        let mut server = TcpServerTransport::bind(any_port_config()).unwrap();
        let mut first = RawClient::connect(&server);
        open_gate(&mut server, &mut first);
        drop(first);
        thread::sleep(Duration::from_millis(200));
        let mut second = RawClient::connect(&server);
        thread::sleep(Duration::from_millis(200));
        // The owner has not polled the first connection's end yet: this frame is for it.
        assert!(server.is_connected(), "the second client is connected");
        let stale = frame(catalogue::STATS, 2, vec![0xEE; 8]);
        server.send(&stale).unwrap();

        let hello = hello_frame(1, TOKEN);
        second.send(&hello);
        let (frames, closed) = poll_frames(&mut server, 1, Duration::from_secs(5));
        assert!(
            closed,
            "the first connection's end comes before the second's frames"
        );
        assert!(frames.is_empty());
        let (frames, _) = poll_frames(&mut server, 1, Duration::from_secs(5));
        assert_eq!(frames, [hello]);
        assert!(second.receive(1, Duration::from_millis(300)).is_empty());
    }

    #[test]
    fn the_engine_link_rejects_a_first_frame_that_is_not_hello_over_tcp() {
        if skip_without_sockets() {
            return;
        }
        let server = TcpServerTransport::bind(any_port_config()).unwrap();
        let addr = server.local_addr();
        let identity = EngineIdentity::new("0.4.0".to_owned(), "unknown".to_owned(), TOKEN);
        let mut link = EngineLink::new(Box::new(server), identity);
        let stream = TcpStream::connect(addr).unwrap();
        stream.set_nonblocking(true).unwrap();
        let mut client = RawClient {
            stream,
            decoder: FrameDecoder::new(),
            closed: false,
        };
        client.send(&frame(catalogue::SWAP_SIGIL_UNIT, 1, vec![1, 2, 3]));

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut events = Vec::new();
        while events.is_empty() && Instant::now() < deadline {
            link.poll(&mut events);
            thread::sleep(Duration::from_millis(5));
        }
        assert!(matches!(
            &events[..],
            [LinkEvent::Rejected(
                crate::HandshakeError::HandshakeRequired
            )]
        ));
        let replies = client.receive(1, Duration::from_secs(5));
        assert_eq!(error_code(&replies[0]), ErrorCode::HandshakeRequired);
        assert!(client.closed_within(Duration::from_secs(5)));
    }
}
