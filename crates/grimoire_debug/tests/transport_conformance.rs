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
        Frame, FrameDecoder, MessageId, TcpConfig, TcpServerTransport, TransportError, catalogue,
        encode_frame, socket_tests_enabled,
    };
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
    use std::time::{Duration, Instant};

    /// A minimal [`DebugTransport`] around a raw client socket, so this test can drive
    /// [`TcpServerTransport`] through the same generic `conformance` functions used for the
    /// other transports. This is test-only scaffolding, not part of the crate's public API.
    struct RawTcpClient {
        stream: TcpStream,
        decoder: FrameDecoder,
        connected: bool,
    }

    impl RawTcpClient {
        fn connect(addr: SocketAddrV4) -> Self {
            let stream = TcpStream::connect(addr).expect("connect to the freshly bound server");
            stream.set_nonblocking(true).expect("set_nonblocking");
            Self {
                stream,
                decoder: FrameDecoder::new(),
                connected: true,
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
            self.connected
        }

        fn disconnect(&mut self) {
            self.connected = false;
            let _ = self.stream.shutdown(std::net::Shutdown::Both);
        }
    }

    fn skip_without_sockets() -> bool {
        if socket_tests_enabled() {
            return false;
        }
        eprintln!("skipping: set GRIMOIRE_SOCKET_TESTS=1");
        true
    }

    /// Binds a server, connects a raw client and completes the transport's pre-handshake gate the
    /// way the engine does (contract §13 "TCP"): the server receives the client's first frame and
    /// sends a `Hello` frame, after which it reads every frame. The conformance properties are
    /// about an established connection.
    fn established_pair() -> (TcpServerTransport, RawTcpClient) {
        let config = TcpConfig::new(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0), [0x22; 32]);
        let mut server = TcpServerTransport::bind(config).expect("bind");
        let mut client = RawTcpClient::connect(server.local_addr());
        let first = Frame {
            id: MessageId(catalogue::HELLO),
            seq: 1,
            payload: vec![1, 0],
        };
        client.send(&first).expect("send the first frame");

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut inbox = Vec::new();
        while inbox.is_empty() && Instant::now() < deadline {
            server.poll(&mut inbox).expect("poll the first frame");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(inbox, vec![first]);
        let reply = Frame {
            id: MessageId(catalogue::HELLO),
            seq: 1,
            payload: Vec::new(),
        };
        server.send(&reply).expect("send the engine Hello");

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut replies = Vec::new();
        while replies.is_empty() && Instant::now() < deadline {
            client.poll(&mut replies).expect("poll the engine Hello");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(replies, vec![reply]);
        (server, client)
    }

    #[test]
    fn tcp_server_transport_preserves_order() {
        if skip_without_sockets() {
            return;
        }
        let (mut server, mut client) = established_pair();
        conformance::order_preserved_no_duplicates(&mut client, &mut server);
    }

    #[test]
    fn tcp_server_transport_survives_a_disconnect() {
        if skip_without_sockets() {
            return;
        }
        let (mut server, mut client) = established_pair();
        conformance::disconnect_does_not_panic(&mut client, &mut server);
        let (mut server, mut client) = established_pair();
        conformance::disconnect_does_not_panic(&mut server, &mut client);
    }

    #[test]
    fn tcp_server_transport_rejects_an_oversized_frame() {
        if skip_without_sockets() {
            return;
        }
        let (mut server, mut client) = established_pair();
        conformance::oversized_frame_rejected_or_disconnects(&mut client, &mut server);
    }
}
