//! [`InProcessTransport`]: an in-process pair that still exercises the full byte codec
//! (contract §13 "Transporte").

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use super::{DebugTransport, TransportError};
use crate::frame::{Frame, FrameDecoder, encode_frame};

/// One end of an in-process [`DebugTransport`] pair (contract §13).
///
/// `send` encodes the frame with [`encode_frame`] and writes the resulting bytes into the
/// peer's inbound channel in chunks of at most [`InProcessOptions::max_chunk`] bytes; `poll`
/// reads whatever bytes are currently available and feeds them through an internal
/// [`FrameDecoder`]. This means an in-process pair exercises exactly the same codec and framing
/// path a real transport would, regardless of chunk size.
///
/// Disconnecting either end sets `is_connected()` to `false` on both, because the two ends
/// share one connection flag.
#[derive(Debug)]
pub struct InProcessTransport {
    outbound: mpsc::SyncSender<Vec<u8>>,
    inbound: mpsc::Receiver<Vec<u8>>,
    connected: Arc<AtomicBool>,
    decoder: FrameDecoder,
    max_chunk: usize,
}

impl InProcessTransport {
    /// Creates a connected pair with the default [`InProcessOptions`].
    pub fn pair() -> (Self, Self) {
        Self::pair_with(InProcessOptions::default())
    }

    /// Creates a connected pair using `options` to control chunking and channel capacity.
    pub fn pair_with(options: InProcessOptions) -> (Self, Self) {
        let capacity = options.capacity_frames.max(1);
        let (tx_a_to_b, rx_a_to_b) = mpsc::sync_channel::<Vec<u8>>(capacity);
        let (tx_b_to_a, rx_b_to_a) = mpsc::sync_channel::<Vec<u8>>(capacity);
        let connected = Arc::new(AtomicBool::new(true));
        let max_chunk = options.max_chunk.max(1);

        let a = Self {
            outbound: tx_a_to_b,
            inbound: rx_b_to_a,
            connected: Arc::clone(&connected),
            decoder: FrameDecoder::new(),
            max_chunk,
        };
        let b = Self {
            outbound: tx_b_to_a,
            inbound: rx_a_to_b,
            connected,
            decoder: FrameDecoder::new(),
            max_chunk,
        };
        (a, b)
    }
}

impl DebugTransport for InProcessTransport {
    fn poll(&mut self, inbox: &mut Vec<Frame>) -> Result<(), TransportError> {
        while let Ok(chunk) = self.inbound.try_recv() {
            self.decoder.push(&chunk);
        }
        loop {
            match self.decoder.next_frame() {
                Ok(Some(frame)) => inbox.push(frame),
                Ok(None) => return Ok(()),
                Err(error) => {
                    // A framing error found while decoding inbound bytes ends the connection;
                    // it never panics and never corrupts frames already pushed to `inbox`.
                    self.disconnect();
                    return Err(TransportError::from(error));
                }
            }
        }
    }

    fn send(&mut self, frame: &Frame) -> Result<(), TransportError> {
        if !self.is_connected() {
            return Err(TransportError::Disconnected);
        }
        let mut bytes = Vec::new();
        encode_frame(frame, &mut bytes)?;
        for chunk in bytes.chunks(self.max_chunk) {
            self.outbound
                .try_send(chunk.to_vec())
                .map_err(|error| match error {
                    mpsc::TrySendError::Full(_) => TransportError::QueueFull,
                    mpsc::TrySendError::Disconnected(_) => TransportError::Disconnected,
                })?;
        }
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    fn disconnect(&mut self) {
        self.connected.store(false, Ordering::SeqCst);
    }
}

/// Tuning knobs for [`InProcessTransport::pair_with`] (contract §13).
///
/// `#[non_exhaustive]`: built via [`Default::default`] plus field assignment (§2 rule 13), so
/// adding a field later is not a breaking change.
#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InProcessOptions {
    /// Maximum number of bytes `send` writes into the peer's channel per chunk. A small value
    /// forces the codec to run its multi-chunk path even for small frames; `usize::MAX` sends
    /// each frame's bytes as a single chunk.
    pub max_chunk: usize,
    /// Bounded capacity, in chunks, of each direction's channel.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MessageId;

    fn frame(id: u16, seq: u32, payload: Vec<u8>) -> Frame {
        Frame {
            id: MessageId(id),
            seq,
            payload,
        }
    }

    #[test]
    fn pair_starts_connected() {
        let (a, b) = InProcessTransport::pair();
        assert!(a.is_connected());
        assert!(b.is_connected());
    }

    #[test]
    fn round_trip_with_default_options() {
        let (mut a, mut b) = InProcessTransport::pair();
        let sent = frame(1, 1, vec![1, 2, 3]);
        a.send(&sent).unwrap();

        let mut inbox = Vec::new();
        b.poll(&mut inbox).unwrap();
        assert_eq!(inbox, vec![sent]);
    }

    #[test]
    fn round_trip_survives_byte_sized_chunks() {
        let options = InProcessOptions {
            max_chunk: 1,
            ..InProcessOptions::default()
        };
        let (mut a, mut b) = InProcessTransport::pair_with(options);
        let sent = frame(3, 7, vec![9, 8, 7, 6, 5]);
        a.send(&sent).unwrap();

        let mut inbox = Vec::new();
        b.poll(&mut inbox).unwrap();
        assert_eq!(inbox, vec![sent]);
    }

    #[test]
    fn round_trip_survives_seven_byte_chunks() {
        let options = InProcessOptions {
            max_chunk: 7,
            ..InProcessOptions::default()
        };
        let (mut a, mut b) = InProcessTransport::pair_with(options);
        let sent = frame(4, 1, vec![0xAB; 40]);
        a.send(&sent).unwrap();

        let mut inbox = Vec::new();
        b.poll(&mut inbox).unwrap();
        assert_eq!(inbox, vec![sent]);
    }

    #[test]
    fn order_is_preserved_and_no_frame_is_duplicated() {
        let (mut a, mut b) = InProcessTransport::pair();
        let frames: Vec<Frame> = (0..10).map(|i| frame(1, i, vec![i as u8])).collect();
        for f in &frames {
            a.send(f).unwrap();
        }
        let mut inbox = Vec::new();
        b.poll(&mut inbox).unwrap();
        assert_eq!(&inbox, &frames);
    }

    #[test]
    fn disconnect_on_one_side_is_visible_on_both() {
        let (mut a, b) = InProcessTransport::pair();
        a.disconnect();
        assert!(!a.is_connected());
        assert!(!b.is_connected());
        // Idempotent.
        a.disconnect();
        assert!(!a.is_connected());
    }

    #[test]
    fn send_after_disconnect_fails() {
        let (mut a, _b) = InProcessTransport::pair();
        a.disconnect();
        let result = a.send(&frame(1, 1, vec![]));
        assert_eq!(result, Err(TransportError::Disconnected));
    }

    #[test]
    fn full_outbound_channel_reports_queue_full() {
        let options = InProcessOptions {
            capacity_frames: 1,
            ..InProcessOptions::default()
        };
        let (mut a, _b) = InProcessTransport::pair_with(options);
        // Never polled from `b`, so the bounded channel of capacity 1 fills up immediately.
        a.send(&frame(1, 1, vec![])).unwrap();
        let result = a.send(&frame(1, 2, vec![]));
        assert_eq!(result, Err(TransportError::QueueFull));
    }

    #[test]
    fn garbage_bytes_disconnect_without_panicking() {
        let (a, mut b) = InProcessTransport::pair();
        // An over-long declared frame length is a framing error, not something reachable
        // through `send` (which validates via `encode_frame`), so inject it directly into the
        // shared channel to simulate garbage bytes arriving from a real peer. This is only
        // possible because this test lives inside the module and can see the private field.
        let huge_len = crate::MAX_FRAME_LEN + 1;
        a.outbound
            .try_send(huge_len.to_le_bytes().to_vec())
            .unwrap();

        let mut inbox = Vec::new();
        let result = b.poll(&mut inbox);
        assert!(result.is_err());
        assert!(inbox.is_empty());
        assert!(!b.is_connected());
    }
}
