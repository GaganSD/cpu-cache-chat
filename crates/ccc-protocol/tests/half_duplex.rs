use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use ccc_core::{AppKind, ChannelMode, Delivery, Error, Role, RuntimeConfig};
use ccc_protocol::{
    decode_frame, encode_frame, AppMessage, Frame, FrameType, Link, LinkEvent, Wire,
};

struct ReceivingWire {
    incoming: VecDeque<Vec<u8>>,
    busy: Arc<AtomicBool>,
    sends: Vec<FrameType>,
    waits: Vec<Duration>,
}

impl Wire for ReceivingWire {
    fn send_blob(&mut self, bytes: &[u8], _: Duration) -> Result<(), Error> {
        assert!(
            !self.receive_in_progress(),
            "transmitted during incoming frame"
        );
        self.sends.push(decode_frame(bytes)?.ftype);
        Ok(())
    }
    fn recv_blob(&mut self, timeout: Duration) -> Result<Vec<u8>, Error> {
        self.waits.push(timeout);
        self.incoming.pop_front().ok_or(Error::Timeout)
    }
    fn receive_in_progress(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }
    fn interrupt(&mut self) {}
}

fn connected(max_retries: u32) -> (Link<ReceivingWire>, Arc<AtomicBool>) {
    let busy = Arc::new(AtomicBool::new(false));
    let wire = ReceivingWire {
        incoming: VecDeque::from([
            encode_frame(&Frame::control(FrameType::SyncAck, 42, 1, 0)).unwrap()
        ]),
        busy: busy.clone(),
        sends: Vec::new(),
        waits: Vec::new(),
    };
    let mut cfg = RuntimeConfig::new(Role::A, ChannelMode::Sim, AppKind::Chat);
    cfg.ack_timeout = Duration::from_millis(1);
    cfg.max_retries = max_retries;
    let mut link = Link::new(cfg, wire, 42);
    link.handshake().unwrap();
    (link, busy)
}

#[test]
fn queued_data_waits_until_partial_frame_finishes() {
    let (mut link, busy) = connected(0);
    busy.store(true, Ordering::SeqCst);
    link.enqueue(AppMessage::Chat {
        id: 1,
        text: "hello".into(),
    })
    .unwrap();
    assert!(link.drive(Duration::from_millis(3)).unwrap().is_empty());
    busy.store(false, Ordering::SeqCst);
    let events = link.drive(Duration::from_millis(3)).unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        LinkEvent::Delivery {
            state: Delivery::AwaitingAck,
            ..
        }
    )));
    assert_eq!(link.into_wire().sends, [FrameType::Sync, FrameType::Data]);
}

#[test]
fn expired_ack_does_not_retry_or_go_unknown_during_partial_frame() {
    let (mut link, busy) = connected(0);
    link.enqueue(AppMessage::Chat {
        id: 1,
        text: "hello".into(),
    })
    .unwrap();
    link.drive(Duration::from_millis(3)).unwrap();
    std::thread::sleep(Duration::from_millis(3));
    busy.store(true, Ordering::SeqCst);
    assert!(link.drive(Duration::from_millis(3)).unwrap().is_empty());
    busy.store(false, Ordering::SeqCst);
    let events = link.drive(Duration::from_millis(3)).unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        LinkEvent::Delivery {
            state: Delivery::Unknown,
            ..
        }
    )));
    let wire = link.into_wire();
    assert_eq!(wire.sends, [FrameType::Sync, FrameType::Data]);
    // A partially received frame must still get a bounded wait even after the
    // original ACK deadline passes; a zero wait would busy-spin indefinitely.
    assert_eq!(*wire.waits.last().unwrap(), Duration::from_millis(3));
}

#[test]
fn retry_waits_until_partial_frame_finishes() {
    let (mut link, busy) = connected(1);
    link.enqueue(AppMessage::Chat {
        id: 1,
        text: "hello".into(),
    })
    .unwrap();
    link.drive(Duration::from_millis(3)).unwrap();
    std::thread::sleep(Duration::from_millis(3));
    busy.store(true, Ordering::SeqCst);
    assert!(link.drive(Duration::from_millis(3)).unwrap().is_empty());
    busy.store(false, Ordering::SeqCst);
    let events = link.drive(Duration::from_millis(3)).unwrap();
    assert!(events
        .iter()
        .any(|event| matches!(event, LinkEvent::Info(s) if s.contains("retransmit"))));
    assert_eq!(
        link.into_wire().sends,
        [FrameType::Sync, FrameType::Data, FrameType::Data]
    );
}

#[test]
fn token_heartbeat_waits_for_partial_frame() {
    let (mut link, busy) = connected(0);
    busy.store(true, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(90));
    link.drive(Duration::ZERO).unwrap();
    busy.store(false, Ordering::SeqCst);
    link.drive(Duration::ZERO).unwrap();
    assert_eq!(
        link.into_wire().sends,
        [FrameType::Sync, FrameType::Heartbeat]
    );
}
