//! `grimoire_debug` protocol v1, `DebugTransport`, profiler data (contract §13, §2a).

use std::io;
#[cfg(feature = "tcp")]
use std::net::SocketAddrV4;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;
pub const MAX_UNIT_BYTES: u32 = 8 * 1024 * 1024;
pub const MAX_HELLO_FRAME_LEN: u32 = 1024;
pub const MAX_INBOUND_QUEUED_BYTES: usize = 2 * MAX_FRAME_LEN as usize;
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
pub const IO_POLL_INTERVAL: Duration = Duration::from_millis(10);
pub const IO_WRITE_TIMEOUT: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct MessageId(pub u16);

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Frame {
    pub id: MessageId,
    pub seq: u32,
    pub payload: Vec<u8>,
}

#[derive(Default, Debug)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }
    pub fn next_frame(&mut self) -> Result<Option<Frame>, ProtocolError> {
        unimplemented!()
    }
}

pub fn encode_frame(frame: &Frame, out: &mut Vec<u8>) -> Result<(), ProtocolError> {
    let _ = (frame, out);
    unimplemented!()
}

pub fn peek_hello_version(frame: &Frame) -> Option<u16> {
    (frame.id == MessageId(0x0001) && frame.payload.len() >= 2)
        .then(|| u16::from_le_bytes([frame.payload[0], frame.payload[1]]))
}

/// Payload types in the Rust form of §13 ("Rust-Form der Nutzlasttypen").
pub mod payloads {
    use super::{ErrorCode, PeerRole};

    #[derive(Clone, PartialEq, Debug)]
    pub struct Hello {
        pub protocol_version: u16,
        pub role: PeerRole,
        pub engine_version: String,
        pub build_hash: String,
        pub token: [u8; 32],
        pub stats_interval_frames: u16,
    }
    #[derive(Clone, PartialEq, Debug)]
    pub struct ErrorMsg {
        pub code: ErrorCode,
        pub in_reply_to: u32,
        pub message: String,
    }
    #[derive(Clone, PartialEq, Debug)]
    pub struct LogMsg {
        pub level: u8,
        pub tick: u64,
        pub target: String,
        pub text: String,
    }
    #[non_exhaustive]
    #[derive(Clone, Default, PartialEq, Debug)]
    pub struct StatsScope {
        pub scope: u16,
        pub name: String,
        pub total_ns: u64,
        pub calls: u32,
        pub budget_ns: u64,
        pub estimate: bool,
    }
    #[derive(Clone, PartialEq, Debug)]
    pub struct StatsCounter {
        pub name: String,
        pub value: u64,
    }
    #[non_exhaustive]
    #[derive(Clone, Default, PartialEq, Debug)]
    pub struct Stats {
        pub frame: u64,
        pub sim_tick: u64,
        pub ticks_this_frame: u32,
        pub alpha: f32,
        pub frame_time_ns: u64,
        pub fps: f32,
        pub dropped_time_ns: u64,
        pub content_swaps: u32,
        pub content_manifest: u64,
        pub scopes: Vec<StatsScope>,
        pub counters: Vec<StatsCounter>,
    }
    #[derive(Clone, PartialEq, Debug)]
    pub struct SwapSigilUnit {
        pub unit_path: String,
        pub unit_bytes: Vec<u8>,
    }
    #[non_exhaustive]
    #[derive(Clone, Default, PartialEq, Debug)]
    pub struct SwapAck {
        pub in_reply_to: u32,
        pub status: u8,
        pub applied_tick: u64,
        pub content_swaps: u32,
        pub content_manifest: u64,
        pub unit_hash: u64,
        pub reason: String,
    }
    #[derive(Clone, PartialEq, Debug)]
    pub struct SigilPreview {
        pub unit_path: String,
        pub unit_bytes: Vec<u8>,
        pub ticks: u32,
        pub target: Option<[f32; 2]>,
    }
}
pub use payloads::{ErrorMsg, Hello, LogMsg, SigilPreview, Stats, StatsCounter, StatsScope, SwapAck, SwapSigilUnit};

#[non_exhaustive]
#[derive(Clone, PartialEq, Debug)]
pub enum Message {
    Hello(Hello),
    Error(ErrorMsg),
    Log(LogMsg),
    Stats(Stats),
    SwapSigilUnit(SwapSigilUnit),
    SwapAck(SwapAck),
    SigilPreview(SigilPreview),
}

impl Message {
    pub fn id(&self) -> MessageId {
        unimplemented!()
    }
    pub fn to_frame(&self, seq: u32) -> Result<Frame, ProtocolError> {
        let _ = seq;
        unimplemented!()
    }
    pub fn from_frame(frame: &Frame) -> Result<Message, ProtocolError> {
        let _ = frame;
        unimplemented!()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PeerRole {
    Tool = 0,
    Engine = 1,
}

#[non_exhaustive]
#[repr(u16)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ErrorCode {
    HandshakeRequired = 1,
    VersionMismatch = 2,
    Unauthorized = 3,
    Malformed = 4,
    UnknownMessage = 5,
    TooLarge = 6,
    NotSupported = 7,
    Busy = 8,
    Internal = 9,
}

#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProtocolError {
    #[error("frame too short {0}")]
    FrameTooShort(u32),
    #[error("frame too large {0}")]
    FrameTooLarge(u32),
    #[error("non-zero flags {0}")]
    NonZeroFlags(u16),
    #[error("unexpected end")]
    UnexpectedEnd { offset: usize, needed: usize, available: usize },
    #[error("trailing bytes")]
    TrailingBytes { extra: usize },
    #[error("invalid utf-8")]
    InvalidUtf8 { field: &'static str },
    #[error("field too long")]
    FieldTooLong { field: &'static str, len: usize, max: usize },
    #[error("invalid enum")]
    InvalidEnum { field: &'static str, value: u64 },
    #[error("unknown message {0:?}")]
    UnknownMessage(MessageId),
}

pub trait DebugTransport: Send {
    fn poll(&mut self, inbox: &mut Vec<Frame>) -> Result<(), TransportError>;
    fn send(&mut self, frame: &Frame) -> Result<(), TransportError>;
    fn is_connected(&self) -> bool;
    fn disconnect(&mut self);
}

#[derive(Default, Debug)]
pub struct NullTransport;

impl DebugTransport for NullTransport {
    fn poll(&mut self, _inbox: &mut Vec<Frame>) -> Result<(), TransportError> {
        Ok(())
    }
    fn send(&mut self, _frame: &Frame) -> Result<(), TransportError> {
        Err(TransportError::NotConnected)
    }
    fn is_connected(&self) -> bool {
        false
    }
    fn disconnect(&mut self) {}
}

#[derive(Debug)]
pub struct InProcessTransport {
    shared: Arc<Mutex<Vec<u8>>>,
    options: InProcessOptions,
}

impl InProcessTransport {
    pub fn pair() -> (Self, Self) {
        Self::pair_with(InProcessOptions::default())
    }
    pub fn pair_with(options: InProcessOptions) -> (Self, Self) {
        let shared = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                shared: Arc::clone(&shared),
                options,
            },
            Self { shared, options },
        )
    }
}

impl DebugTransport for InProcessTransport {
    fn poll(&mut self, inbox: &mut Vec<Frame>) -> Result<(), TransportError> {
        let _ = (inbox, &self.shared, self.options);
        unimplemented!()
    }
    fn send(&mut self, frame: &Frame) -> Result<(), TransportError> {
        let _ = frame;
        unimplemented!()
    }
    fn is_connected(&self) -> bool {
        unimplemented!()
    }
    fn disconnect(&mut self) {}
}

#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InProcessOptions {
    pub max_chunk: usize,
    pub capacity_frames: usize,
}

impl Default for InProcessOptions {
    fn default() -> Self {
        Self {
            max_chunk: usize::MAX,
            capacity_frames: 256,
        }
    }
}

#[cfg(feature = "tcp")]
#[derive(Debug)]
pub struct TcpServerTransport {
    stop: Arc<std::sync::atomic::AtomicBool>,
    inbound: std::sync::mpsc::Receiver<Frame>,
    outbound: std::sync::mpsc::SyncSender<Frame>,
    thread: Option<std::thread::JoinHandle<()>>,
    local_addr: std::net::SocketAddr,
}

#[cfg(feature = "tcp")]
impl TcpServerTransport {
    pub fn bind(config: TcpConfig) -> Result<Self, TransportError> {
        let listener = std::net::TcpListener::bind(config.addr).map_err(|error| TransportError::Io {
            kind: error.kind(),
            message: error.to_string(),
        })?;
        let local_addr = listener.local_addr().map_err(|error| TransportError::Io {
            kind: error.kind(),
            message: error.to_string(),
        })?;
        let (in_tx, inbound) = std::sync::mpsc::sync_channel::<Frame>(256);
        let (outbound, out_rx) = std::sync::mpsc::sync_channel::<Frame>(256);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("grimoire-debug-io".to_owned())
            .spawn(move || {
                let _ = (listener, in_tx, out_rx, config.token);
                while !thread_stop.load(std::sync::atomic::Ordering::Relaxed) {
                    std::thread::sleep(IO_POLL_INTERVAL);
                }
            })
            .map_err(|error| TransportError::Io {
                kind: error.kind(),
                message: error.to_string(),
            })?;
        Ok(Self {
            stop,
            inbound,
            outbound,
            thread: Some(thread),
            local_addr,
        })
    }
    pub fn local_addr(&self) -> std::net::SocketAddr {
        self.local_addr
    }
}

#[cfg(feature = "tcp")]
impl Drop for TcpServerTransport {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(feature = "tcp")]
impl DebugTransport for TcpServerTransport {
    fn poll(&mut self, inbox: &mut Vec<Frame>) -> Result<(), TransportError> {
        inbox.extend(self.inbound.try_iter());
        Ok(())
    }
    fn send(&mut self, frame: &Frame) -> Result<(), TransportError> {
        self.outbound
            .try_send(frame.clone())
            .map_err(|_| TransportError::QueueFull)
    }
    fn is_connected(&self) -> bool {
        false
    }
    fn disconnect(&mut self) {}
}

#[cfg(feature = "tcp")]
#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TcpConfig {
    pub addr: SocketAddrV4,
    pub token: [u8; 32],
}

#[cfg(feature = "tcp")]
impl TcpConfig {
    pub fn new(addr: SocketAddrV4, token: [u8; 32]) -> Self {
        Self { addr, token }
    }
    pub fn from_env() -> Result<Option<TcpConfig>, TransportError> {
        unimplemented!()
    }
}

pub const DEBUG_ADDR_ENV: &str = "GRIMOIRE_DEBUG_ADDR";
pub const DEBUG_TOKEN_ENV: &str = "GRIMOIRE_DEBUG_TOKEN";
pub const SOCKET_TESTS_ENV: &str = "GRIMOIRE_SOCKET_TESTS";
pub const DEFAULT_DEBUG_PORT: u16 = 47_474;

pub fn socket_tests_enabled() -> bool {
    std::env::var(SOCKET_TESTS_ENV).is_ok_and(|value| value == "1")
}

#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TransportError {
    #[error("not connected")]
    NotConnected,
    #[error("queue full")]
    QueueFull,
    #[error("disconnected")]
    Disconnected,
    #[error("non-loopback address {0}")]
    NonLoopbackAddress(String),
    #[error("invalid config {0}")]
    InvalidConfig(String),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error("io {kind}: {message}")]
    Io { kind: io::ErrorKind, message: String },
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ScopeId(pub u16);

#[derive(Default, Clone, Debug)]
pub struct FrameProfile {
    frame: u64,
    names: Vec<(ScopeId, String)>,
    scopes: Vec<ScopeTotal>,
    counters: Vec<(&'static str, u64)>,
}

impl FrameProfile {
    /// Clears scopes and counters of the previous frame; the name table stays.
    pub fn begin(&mut self, frame: u64) {
        self.frame = frame;
        self.scopes.clear();
        self.counters.clear();
    }
    /// Copies `name` only the first time `scope` appears; later calls do not allocate.
    pub fn record(&mut self, scope: ScopeId, name: &str, duration: Duration) {
        if !self.names.iter().any(|(id, _)| *id == scope) {
            self.names.push((scope, name.to_owned()));
        }
        if let Some(total) = self.scopes.iter_mut().find(|total| total.scope == scope) {
            total.total += duration;
            total.calls += 1;
        } else {
            self.scopes.push(ScopeTotal {
                scope,
                total: duration,
                calls: 1,
            });
        }
    }
    pub fn add_counter(&mut self, name: &'static str, value: u64) {
        self.counters.push((name, value));
    }
    pub fn scopes(&self) -> &[ScopeTotal] {
        &self.scopes
    }
    pub fn scope_name(&self, scope: ScopeId) -> Option<&str> {
        self.names
            .iter()
            .find(|(id, _)| *id == scope)
            .map(|(_, name)| name.as_str())
    }
    pub fn counters(&self) -> &[(&'static str, u64)] {
        &self.counters
    }
    /// Stub: truncation to 64 entries only; name truncation at a UTF-8 boundary is omitted.
    pub fn to_stats(&self, frame: &StatsFrame) -> Stats {
        let nanos = |duration: Duration| u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        Stats {
            frame: frame.frame,
            sim_tick: frame.sim_tick,
            ticks_this_frame: frame.ticks_this_frame,
            alpha: frame.alpha,
            frame_time_ns: nanos(frame.frame_time),
            fps: frame.fps,
            dropped_time_ns: nanos(frame.dropped_time),
            content_swaps: frame.content_swaps,
            content_manifest: frame.content_manifest,
            scopes: self
                .scopes
                .iter()
                .take(64)
                .map(|total| StatsScope {
                    scope: total.scope.0,
                    name: self.scope_name(total.scope).unwrap_or_default().to_owned(),
                    total_ns: nanos(total.total),
                    calls: total.calls,
                    budget_ns: 0,
                    estimate: false,
                })
                .collect(),
            counters: self
                .counters
                .iter()
                .take(64)
                .map(|(name, value)| StatsCounter {
                    name: (*name).to_owned(),
                    value: *value,
                })
                .collect(),
        }
    }
}

/// Only produced by `FrameProfile::record` (§2 rule 13 exemption).
#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScopeTotal {
    pub scope: ScopeId,
    pub total: Duration,
    pub calls: u32,
}

/// Frame values for `Stats` that `FrameProfile` does not hold (§13).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StatsFrame {
    pub frame: u64,
    pub sim_tick: u64,
    pub ticks_this_frame: u32,
    pub alpha: f32,
    pub frame_time: Duration,
    pub fps: f32,
    pub dropped_time: Duration,
    pub content_swaps: u32,
    pub content_manifest: u64,
}

#[cfg(feature = "conformance")]
pub mod conformance {
    use super::DebugTransport;

    pub fn transport_pair(a: &mut dyn DebugTransport, b: &mut dyn DebugTransport) {
        let _ = (a, b);
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_safety() {
        let _: Option<Box<dyn DebugTransport>> = None;
        fn send<T: Send>() {}
        send::<InProcessTransport>();
        #[cfg(feature = "tcp")]
        send::<TcpServerTransport>();
    }
}
