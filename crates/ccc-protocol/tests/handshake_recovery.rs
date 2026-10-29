use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ccc_core::{AppKind, ChannelMode, Error, Role, RuntimeConfig};
use ccc_protocol::{decode_frame, encode_frame, Frame, FrameType, Link, Wire};

/// Responder-side wire: serves queued incoming blobs and applies a scripted
/// outcome to each send. Sends are recorded as raw bytes for comparison.
struct ResponderWire {
    incoming: VecDeque<Vec<u8>>,
    outcomes: VecDeque<Result<(), Error>>,
    sent: Vec<Vec<u8>>,
}

impl Wire for ResponderWire {
    fn send_blob(&mut self, bytes: &[u8], _: Duration) -> Result<(), Error> {
        self.sent.push(bytes.to_vec());
        self.outcomes.pop_front().unwrap_or(Ok(()))
    }
    fn recv_blob(&mut self, _: Duration) -> Result<Vec<u8>, Error> {
        self.incoming.pop_front().ok_or(Error::Timeout)
    }
    fn interrupt(&mut self) {}
}

fn responder(wire: ResponderWire, sync_timeout: Duration) -> Link<ResponderWire> {
    let mut cfg = RuntimeConfig::new(Role::B, ChannelMode::Sim, AppKind::Chat);
    cfg.sync_timeout = sync_timeout;
    Link::new(cfg, wire, 0)
}

fn sync() -> Vec<u8> {
    encode_frame(&Frame::control(FrameType::Sync, 42, 1, 0)).unwrap()
}

#[test]
fn transient_sync_ack_failure_recovers_on_duplicate_sync() {
    let wire = ResponderWire {
        incoming: VecDeque::from([sync(), sync()]),
        outcomes: VecDeque::from([Err(Error::Corrupt("missed physical slot")), Ok(())]),
        sent: Vec::new(),
    };
    let mut link = responder(wire, Duration::from_secs(5));
    // One transient SyncAck send failure must not kill the responder.
    link.handshake().unwrap();
    assert_eq!(link.session(), 42);
    let wire = link.into_wire();
    assert_eq!(wire.sent.len(), 2);
    // The duplicate Sync gets the identical SyncAck response.
    assert_eq!(wire.sent[0], wire.sent[1]);
    let frame = decode_frame(&wire.sent[1]).unwrap();
    assert_eq!(frame.ftype, FrameType::SyncAck);
    assert_eq!(frame.session, 42);
}

#[test]
fn terminal_sync_ack_errors_and_interrupt_propagate_without_retry() {
    for (outcome, want) in [
        (Error::Disconnected, "disconnected"),
        (Error::Io("wire gone".into()), "io: wire gone"),
        (Error::Interrupted, "interrupted"),
    ] {
        let wire = ResponderWire {
            incoming: VecDeque::from([sync(), sync()]),
            outcomes: VecDeque::from([Err(outcome)]),
            sent: Vec::new(),
        };
        let mut link = responder(wire, Duration::from_secs(5));
        let error = link.handshake().unwrap_err();
        assert_eq!(error.to_string(), want);
        // A terminal error is not retried: exactly one SyncAck was attempted.
        assert_eq!(link.into_wire().sent.len(), 1);
    }
}

#[test]
fn repeated_transient_failures_stay_inside_the_listen_deadline() {
    let wire = ResponderWire {
        incoming: (0..64).map(|_| sync()).collect(),
        outcomes: (0..64)
            .map(|_| Err(Error::Corrupt("missed physical slot")))
            .collect(),
        sent: Vec::new(),
    };
    let mut link = responder(wire, Duration::from_millis(50));
    let began = Instant::now();
    let error = link.handshake().unwrap_err();
    let elapsed = began.elapsed();
    // Recovery is bounded by the overall listen deadline, never infinite.
    assert!(matches!(error, Error::Corrupt(_) | Error::Timeout));
    assert!(elapsed >= Duration::from_millis(50));
    assert!(elapsed < Duration::from_secs(5));
}
