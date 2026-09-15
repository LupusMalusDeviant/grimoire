//! Conformance suite for [`crate::DebugTransport`] implementations (contract §2a, §13),
//! behind the non-default `conformance` feature.
//!
//! Every implementation's own tests call the function(s) here that apply to it. The suite
//! checks only the properties every implementation must satisfy — order preservation, no
//! duplication, panic-freedom around disconnects, and graceful handling of an oversized frame —
//! never implementation-specific behaviour (handshake, loopback binding, thread shutdown), which
//! belongs to that implementation's own tests (§13 "Verhaltenstests", §2 rule 12).

use crate::frame::{Frame, MessageId};
use crate::transport::{DebugTransport, TransportError};

/// Checks the [`crate::NullTransport`] contract specifically: never connected, `poll` never
/// yields a frame, `send` always fails with [`TransportError::NotConnected`] (contract §2a).
pub fn null_transport(transport: &mut dyn DebugTransport) {
    assert!(
        !transport.is_connected(),
        "NullTransport must never be connected"
    );

    let mut inbox = Vec::new();
    assert_eq!(transport.poll(&mut inbox), Ok(()), "poll must never fail");
    assert!(inbox.is_empty(), "poll must never yield a frame");

    let probe = Frame {
        id: MessageId(1),
        seq: 1,
        payload: Vec::new(),
    };
    assert_eq!(
        transport.send(&probe),
        Err(TransportError::NotConnected),
        "send must always report NotConnected"
    );
}

/// Checks that a connected pair preserves frame order end to end with no duplication and no
/// dropped frame, for a batch sent entirely before the peer polls (contract §2a "Reihenfolge
/// bleibt erhalten, kein Frame wird verdoppelt").
pub fn order_preserved_no_duplicates(
    sender: &mut dyn DebugTransport,
    receiver: &mut dyn DebugTransport,
) {
    let frames: Vec<Frame> = (0..16)
        .map(|i| Frame {
            id: MessageId(1),
            seq: i + 1,
            payload: vec![i as u8; (i as usize % 5) + 1],
        })
        .collect();
    for frame in &frames {
        sender
            .send(frame)
            .expect("send to a freshly connected peer must succeed");
    }

    let mut inbox = Vec::new();
    // A real transport may deliver asynchronously on a background IO thread (e.g. TCP polls its
    // socket every `IO_POLL_INTERVAL`), so give it a bounded amount of *wall-clock* time, not
    // just a bounded number of immediate calls, to observe everything a peer already sent.
    for attempt in 0..64 {
        receiver
            .poll(&mut inbox)
            .expect("poll must not fail on well-formed input");
        if inbox.len() >= frames.len() {
            break;
        }
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    assert_eq!(
        inbox, frames,
        "frame order must be preserved with no duplicates and no gaps"
    );
}

/// Checks that disconnecting mid-exchange never panics either side, and that `is_connected`
/// reflects the disconnect and is idempotent (contract §2a "Abriss mitten im Frame ohne
/// Panic").
pub fn disconnect_does_not_panic(a: &mut dyn DebugTransport, b: &mut dyn DebugTransport) {
    // Start an exchange, then sever the connection before the receiver has necessarily seen
    // everything — the point is that neither side panics, however far the bytes got.
    let frame = Frame {
        id: MessageId(2),
        seq: 1,
        payload: vec![0xAB; 64],
    };
    let _ = a.send(&frame);

    a.disconnect();
    assert!(
        !a.is_connected(),
        "is_connected must become false after disconnect"
    );
    a.disconnect(); // idempotent
    assert!(!a.is_connected());

    // Polling the peer after a disconnect must not panic, whatever partial bytes arrived.
    let mut inbox = Vec::new();
    let _ = b.poll(&mut inbox);
}

/// Checks that a frame too large to encode is either rejected up front, leaving the connection
/// usable, or causes a clean disconnect — but never corrupts the stream or panics (contract
/// §2a "Überlänge schließt die Verbindung").
pub fn oversized_frame_rejected_or_disconnects(
    a: &mut dyn DebugTransport,
    b: &mut dyn DebugTransport,
) {
    let oversized = Frame {
        id: MessageId(1),
        seq: 1,
        payload: vec![0xAA; crate::MAX_FRAME_LEN as usize],
    };

    match a.send(&oversized) {
        Ok(()) => {
            // Accepted despite being oversized (should not happen for the shipped
            // implementations, but is not itself a contract violation as long as nothing
            // panics or fabricates frames): draining the peer must stay panic-free.
            let mut inbox = Vec::new();
            for _ in 0..8 {
                let _ = b.poll(&mut inbox);
            }
        }
        Err(_) => {
            // Rejected cleanly: the connection must still carry a normal frame afterwards.
            let follow_up = Frame {
                id: MessageId(3),
                seq: 2,
                payload: vec![7, 7, 7],
            };
            a.send(&follow_up)
                .expect("transport must stay usable after rejecting an oversized frame");

            let mut inbox = Vec::new();
            for attempt in 0..64 {
                b.poll(&mut inbox)
                    .expect("poll must not fail after an oversized send was rejected");
                if !inbox.is_empty() {
                    break;
                }
                if attempt > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
            assert_eq!(inbox, vec![follow_up]);
        }
    }
}
