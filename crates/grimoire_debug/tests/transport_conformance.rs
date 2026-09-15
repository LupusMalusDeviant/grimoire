//! Integration test for the transport conformance suite (contract §13 "Konformanz (WP1.3)").
//!
//! Requires the `conformance` feature. The TCP section additionally requires `tcp` and only
//! runs its socket-based check when `GRIMOIRE_SOCKET_TESTS=1` (contract §13 "Socket-Tests").
#![cfg(feature = "conformance")]

use grimoire_debug::{
    DebugTransport, InProcessOptions, InProcessTransport, NullTransport, conformance,
};

#[test]
fn null_transport_is_never_connected() {
    let mut transport = NullTransport;
    conformance::null_transport(&mut transport);
}

#[test]
fn in_process_pair_is_conformant_across_chunk_sizes() {
    // The three chunk sizes the contract calls out by name (§13 "Konformanz"): one byte at a
    // time, an odd small size, and everything in a single chunk.
    for max_chunk in [1usize, 7, usize::MAX] {
        // `InProcessOptions` is `#[non_exhaustive]`, so from outside the crate it is built via
        // `Default` plus field assignment rather than struct-update syntax (contract §13).
        let mut options = InProcessOptions::default();
        options.max_chunk = max_chunk;

        let (mut a, mut b) = InProcessTransport::pair_with(options);
        conformance::order_preserved_no_duplicates(&mut a, &mut b);

        let (mut a, mut b) = InProcessTransport::pair_with(options);
        conformance::disconnect_does_not_panic(&mut a, &mut b);

        let (mut a, mut b) = InProcessTransport::pair_with(options);
        conformance::oversized_frame_rejected_or_disconnects(&mut a, &mut b);
    }
}

#[cfg(feature = "tcp")]
mod tcp_conformance {
    use super::*;
    use grimoire_debug::{
        Frame, FrameDecoder, TcpConfig, TcpServerTransport, TransportError, encode_frame,
        socket_tests_enabled,
    };
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
    use std::time::{Duration, Instant};

    /// A minimal [`DebugTransport`] around a raw client socket, so this test can drive
    /// [`TcpServerTransport`] through the same generic `conformance` functions used for the
    /// other transports. This is test-only scaffolding, not part of the crate's public API: a
    /// real client implementation (with the `Hello` handshake) is WP8.2's job.
    struct RawTcpClient {
        stream: TcpStream,
        decoder: FrameDecoder,
    }

    impl RawTcpClient {
        fn connect(addr: SocketAddrV4) -> Self {
            let stream = TcpStream::connect(addr).expect("connect to the freshly bound server");
            stream.set_nonblocking(true).expect("set_nonblocking");
            Self {
                stream,
                decoder: FrameDecoder::new(),
            }
        }
    }

    impl DebugTransport for RawTcpClient {
        fn poll(&mut self, inbox: &mut Vec<Frame>) -> Result<(), TransportError> {
            let mut buf = [0u8; 4096];
            loop {
                match self.stream.read(&mut buf) {
                    Ok(0) => return Err(TransportError::Disconnected),
                    Ok(n) => self.decoder.push(&buf[..n]),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) => {
                        return Err(TransportError::Io {
                            kind: error.kind(),
                            message: error.to_string(),
                        });
                    }
                }
            }
            loop {
                match self.decoder.next_frame() {
                    Ok(Some(frame)) => inbox.push(frame),
                    Ok(None) => return Ok(()),
                    Err(error) => return Err(TransportError::from(error)),
                }
            }
        }

        fn send(&mut self, frame: &Frame) -> Result<(), TransportError> {
            let mut bytes = Vec::new();
            encode_frame(frame, &mut bytes)?;
            let deadline = Instant::now() + Duration::from_secs(1);
            let mut offset = 0;
            while offset < bytes.len() {
                match self.stream.write(&bytes[offset..]) {
                    Ok(0) => return Err(TransportError::Disconnected),
                    Ok(n) => offset += n,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return Err(TransportError::QueueFull);
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => {
                        return Err(TransportError::Io {
                            kind: error.kind(),
                            message: error.to_string(),
                        });
                    }
                }
            }
            Ok(())
        }

        fn is_connected(&self) -> bool {
            true
        }

        fn disconnect(&mut self) {
            let _ = self.stream.shutdown(std::net::Shutdown::Both);
        }
    }

    #[test]
    fn tcp_server_transport_preserves_order() {
        if !socket_tests_enabled() {
            eprintln!("skipping: set GRIMOIRE_SOCKET_TESTS=1");
            return;
        }

        let config = TcpConfig::new(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0), [0x22; 32]);
        let mut server = TcpServerTransport::bind(config).expect("bind");
        let mut client = RawTcpClient::connect(server.local_addr());
        // Give the IO thread a moment to accept the connection before exercising the pair.
        std::thread::sleep(Duration::from_millis(50));

        conformance::order_preserved_no_duplicates(&mut client, &mut server);
    }
}
