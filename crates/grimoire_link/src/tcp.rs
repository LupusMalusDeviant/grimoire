//! [`TcpClientTransport`]: the tool side of the debug link's TCP transport (contract §13 "TCP",
//! engine ADR-0008: "Client-Transport von `grimoire-link`").
//!
//! The engine binds and serves (`grimoire_debug::TcpServerTransport`); this connects to it. Like
//! the server it accepts only `127.0.0.1`, keeps the socket non-blocking so `poll` and `send`
//! never block, and turns every byte through a `FrameDecoder`, so a malformed stream ends the
//! connection instead of panicking (contract §2 rule 9).

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::time::{Duration, Instant};

use grimoire_debug::{DebugTransport, Frame, FrameDecoder, TransportError, encode_frame};

/// Longest a blocked socket write is retried before the connection counts as lost. The server's
/// own write timeout (`grimoire_debug::IO_WRITE_TIMEOUT`) is the same idea from the other side.
const WRITE_TIMEOUT: Duration = Duration::from_millis(500);

/// The client end of a debug-link TCP connection (contract §13).
#[derive(Debug)]
pub struct TcpClientTransport {
    stream: TcpStream,
    decoder: FrameDecoder,
    connected: bool,
}

impl TcpClientTransport {
    /// Connects to an engine listening on `addr`.
    ///
    /// # Errors
    /// - [`TransportError::NonLoopbackAddress`] for any address other than `127.0.0.1`, before a
    ///   socket is created;
    /// - [`TransportError::Io`] if the connection or the socket setup fails (for example because
    ///   no engine is listening).
    pub fn connect(addr: SocketAddrV4) -> Result<Self, TransportError> {
        if *addr.ip() != Ipv4Addr::LOCALHOST {
            return Err(TransportError::NonLoopbackAddress(addr.ip().to_string()));
        }
        let stream = TcpStream::connect(addr).map_err(io_error)?;
        stream.set_nonblocking(true).map_err(io_error)?;
        Ok(Self {
            stream,
            decoder: FrameDecoder::new(),
            connected: true,
        })
    }

    /// The address this client is connected to.
    ///
    /// # Errors
    /// [`TransportError::Io`] if the socket has no peer address any more.
    pub fn peer_addr(&self) -> Result<SocketAddrV4, TransportError> {
        match self.stream.peer_addr().map_err(io_error)? {
            std::net::SocketAddr::V4(addr) => Ok(addr),
            // `connect` only accepts an IPv4 loopback address, so a connected socket is V4.
            std::net::SocketAddr::V6(addr) => {
                Ok(SocketAddrV4::new(Ipv4Addr::LOCALHOST, addr.port()))
            }
        }
    }

    /// Reads whatever the socket has buffered into the frame decoder.
    fn fill(&mut self) -> Result<(), TransportError> {
        let mut buf = [0u8; 8192];
        loop {
            match self.stream.read(&mut buf) {
                Ok(0) => {
                    self.connected = false;
                    return Err(TransportError::Disconnected);
                }
                Ok(read) => self.decoder.push(&buf[..read]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) => {
                    self.connected = false;
                    return Err(io_error(error));
                }
            }
        }
    }
}

impl DebugTransport for TcpClientTransport {
    fn poll(&mut self, inbox: &mut Vec<Frame>) -> Result<(), TransportError> {
        // Frames already decoded reach the caller even when the peer closed in the same call: the
        // error is returned after them, on this or the next poll.
        let filled = self.fill();
        loop {
            match self.decoder.next_frame() {
                Ok(Some(frame)) => inbox.push(frame),
                Ok(None) => return filled,
                Err(error) => {
                    self.disconnect();
                    return Err(TransportError::from(error));
                }
            }
        }
    }

    fn send(&mut self, frame: &Frame) -> Result<(), TransportError> {
        if !self.connected {
            return Err(TransportError::Disconnected);
        }
        let mut bytes = Vec::new();
        encode_frame(frame, &mut bytes)?;
        let deadline = Instant::now() + WRITE_TIMEOUT;
        let mut offset = 0;
        while offset < bytes.len() {
            match self.stream.write(&bytes[offset..]) {
                Ok(0) => {
                    self.connected = false;
                    return Err(TransportError::Disconnected);
                }
                Ok(written) => offset += written,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(TransportError::QueueFull);
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => {
                    self.connected = false;
                    return Err(io_error(error));
                }
            }
        }
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn disconnect(&mut self) {
        if self.connected {
            self.connected = false;
            let _ = self.stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

fn io_error(error: io::Error) -> TransportError {
    TransportError::Io {
        kind: error.kind(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grimoire_debug::socket_tests_enabled;

    #[test]
    fn a_non_loopback_address_is_rejected_without_a_socket() {
        let addr = SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 1), 47_474);
        assert!(matches!(
            TcpClientTransport::connect(addr),
            Err(TransportError::NonLoopbackAddress(_))
        ));
    }

    #[test]
    fn connecting_to_a_closed_port_is_an_io_error() {
        if !socket_tests_enabled() {
            eprintln!("skipping: set GRIMOIRE_SOCKET_TESTS=1");
            return;
        }
        // Port 0 never listens: `connect` to it fails without any engine running.
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);
        assert!(matches!(
            TcpClientTransport::connect(addr),
            Err(TransportError::Io { .. })
        ));
    }
}
