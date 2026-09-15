//! [`NullTransport`]: a [`super::DebugTransport`] that is never connected (contract §2a).

use super::{DebugTransport, TransportError};
use crate::frame::Frame;

/// A transport that never connects to anything.
///
/// `poll` always succeeds without appending any frame, `send` always fails with
/// [`TransportError::NotConnected`], and `is_connected` always returns `false`. Used as the
/// headless default (PRD-0002 FR-15) wherever no real debug link is wanted.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MessageId;

    #[test]
    fn never_connected_never_yields_frames_and_rejects_send() {
        let mut transport = NullTransport;
        assert!(!transport.is_connected());

        let mut inbox = Vec::new();
        assert_eq!(transport.poll(&mut inbox), Ok(()));
        assert!(inbox.is_empty());

        let frame = Frame {
            id: MessageId(1),
            seq: 1,
            payload: vec![],
        };
        assert_eq!(transport.send(&frame), Err(TransportError::NotConnected));

        // Idempotent no-op.
        transport.disconnect();
        transport.disconnect();
        assert!(!transport.is_connected());
    }
}
