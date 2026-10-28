use ccc_core::{AppKind, ChannelMode, Delivery, Error, Role, RuntimeConfig};
use ccc_protocol::{
    decode_frame, encode_frame, AppMessage, Frame, FrameType, Link, LinkEvent, Wire,
};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

struct MockWire {
    tx: mpsc::Sender<Vec<u8>>,
    rx: mpsc::Receiver<Vec<u8>>,
    sent: Arc<Mutex<Vec<Frame>>>,
    drop_type: Option<FrameType>,
    drops: usize,
}
impl Wire for MockWire {
    fn send_blob(&mut self, bytes: &[u8], _: Duration) -> Result<(), Error> {
        let frame = decode_frame(bytes).unwrap();
        self.sent.lock().unwrap().push(frame.clone());
        if self.drop_type == Some(frame.ftype) && self.drops > 0 {
            self.drops -= 1;
        } else {
            self.tx
                .send(bytes.to_vec())
                .map_err(|_| Error::Disconnected)?;
        }
        Ok(())
    }
    fn recv_blob(&mut self, timeout: Duration) -> Result<Vec<u8>, Error> {
        self.rx.recv_timeout(timeout).map_err(|e| match e {
            mpsc::RecvTimeoutError::Timeout => Error::Timeout,
            mpsc::RecvTimeoutError::Disconnected => Error::Disconnected,
        })
    }
    fn interrupt(&mut self) {}
}
type SentFrames = Arc<Mutex<Vec<Frame>>>;
type MockPair = (Link<MockWire>, Link<MockWire>, SentFrames, SentFrames);

fn pair(drop_a: Option<(FrameType, usize)>, drop_b: Option<(FrameType, usize)>) -> MockPair {
    pair_with_timeout(drop_a, drop_b, Duration::from_millis(2))
}

fn pair_with_timeout(
    drop_a: Option<(FrameType, usize)>,
    drop_b: Option<(FrameType, usize)>,
    timeout: Duration,
) -> MockPair {
    let (at, br) = mpsc::channel();
    let (bt, ar) = mpsc::channel();
    let sa = Arc::new(Mutex::new(Vec::new()));
    let sb = Arc::new(Mutex::new(Vec::new()));
    let wire = |tx, rx, sent, drop: Option<(FrameType, usize)>| MockWire {
        tx,
        rx,
        sent,
        drop_type: drop.map(|d| d.0),
        drops: drop.map_or(0, |d| d.1),
    };
    let config = |role| {
        let mut c = RuntimeConfig::new(role, ChannelMode::Sim, AppKind::Chat);
        c.ack_timeout = timeout;
        c.sync_timeout = Duration::from_secs(1);
        c.max_retries = 3;
        c
    };
    let mut a = Link::new(config(Role::A), wire(at, ar, sa.clone(), drop_a), 42);
    let mut b = Link::new(config(Role::B), wire(bt, br, sb.clone(), drop_b), 0);
    let thread = std::thread::spawn(move || {
        b.handshake().unwrap();
        b
    });
    a.handshake().unwrap();
    (a, thread.join().unwrap(), sa, sb)
}
fn chat(text: &str) -> AppMessage {
    AppMessage::Chat {
        id: 1,
        text: text.into(),
    }
}
fn drive_pair(
    a: &mut Link<MockWire>,
    b: &mut Link<MockWire>,
    rounds: usize,
) -> (Vec<LinkEvent>, Vec<LinkEvent>) {
    let mut ae = Vec::new();
    let mut be = Vec::new();
    for _ in 0..rounds {
        ae.extend(a.drive(Duration::from_millis(3)).unwrap());
        be.extend(b.drive(Duration::from_millis(3)).unwrap());
    }
    (ae, be)
}
fn delivered(events: &[LinkEvent], state: Delivery) -> bool {
    events
        .iter()
        .any(|e| matches!(e, LinkEvent::Delivery { state: s, .. } if *s == state))
}
fn received(events: &[LinkEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, LinkEvent::Recv(_)))
        .count()
}

#[test]
fn dropped_data_and_ack_are_retried_without_duplicate_delivery() {
    for (da, db) in [
        (Some((FrameType::Data, 1)), None),
        (None, Some((FrameType::Ack, 1))),
    ] {
        let (mut a, mut b, sa, sb) = pair(da, db);
        a.enqueue(chat("hello")).unwrap();
        let (ae, be) = drive_pair(&mut a, &mut b, 8);
        assert!(delivered(&ae, Delivery::Delivered));
        assert_eq!(received(&be), 1);
        assert!(
            sa.lock()
                .unwrap()
                .iter()
                .filter(|f| f.ftype == FrameType::Data)
                .count()
                >= 2
        );
        if db.is_some() {
            assert!(
                sb.lock()
                    .unwrap()
                    .iter()
                    .filter(|f| f.ftype == FrameType::Ack)
                    .count()
                    >= 2
            );
        }
    }
}

#[test]
fn exhausted_retries_are_unknown_and_stop_queued_sends() {
    for (da, db) in [
        (Some((FrameType::Data, 99)), None),
        (None, Some((FrameType::Ack, 99))),
    ] {
        let (mut a, mut b, sa, _) = pair(da, db);
        assert_eq!(a.enqueue(chat("one")).unwrap(), 1);
        assert_eq!(a.enqueue(chat("two")).unwrap(), 2);
        let (ae, be) = drive_pair(&mut a, &mut b, 12);
        assert!(delivered(&ae, Delivery::Unknown));
        assert!(!delivered(&ae, Delivery::Delivered));
        assert_eq!(received(&be), usize::from(db.is_some()));
        let frames = sa.lock().unwrap();
        let data: Vec<_> = frames
            .iter()
            .filter(|f| f.ftype == FrameType::Data)
            .collect();
        assert_eq!(data.len(), 4);
        assert!(data.iter().all(|f| f.seq == 1));
    }
}

#[test]
fn unknown_sequence_never_resumes_after_prolonged_driving() {
    let (mut a, mut b, sent, _) =
        pair_with_timeout(Some((FrameType::Data, 99)), None, Duration::from_millis(50));
    a.enqueue(chat("one")).unwrap();
    a.enqueue(chat("two")).unwrap();
    let (events, _) = drive_pair(&mut a, &mut b, 400);
    assert!(delivered(&events, Delivery::Unknown));
    assert!(sent
        .lock()
        .unwrap()
        .iter()
        .filter(|f| f.ftype == FrameType::Data)
        .all(|f| f.seq == 1));
}

#[test]
fn stale_and_old_data_are_rejected_and_duplicate_is_acked() {
    let (mut a, mut b, _, sb) = pair(None, None);
    for text in ["one", "two"] {
        a.enqueue(chat(text)).unwrap();
    }
    let (_, events) = drive_pair(&mut a, &mut b, 8);
    assert_eq!(received(&events), 2);
    let mut wire = a.into_wire();
    for (session, seq) in [(42, 2), (42, 1), (99, 3)] {
        let frame =
            Frame::new(FrameType::Data, session, seq, chat("old").encode().unwrap()).unwrap();
        wire.send_blob(&encode_frame(&frame).unwrap(), Duration::ZERO)
            .unwrap();
    }
    let mut events = Vec::new();
    for _ in 0..3 {
        events.extend(b.drive(Duration::from_millis(1)).unwrap());
    }
    assert_eq!(received(&events), 0);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, LinkEvent::Error(_)))
            .count(),
        2
    );
    assert_eq!(
        sb.lock()
            .unwrap()
            .iter()
            .filter(|f| f.ftype == FrameType::Ack && f.ack_seq == 2)
            .count(),
        2
    );
}

#[test]
fn application_rejection_is_not_acknowledged_or_delivered() {
    let (mut a, mut b, _, sent) = pair(None, None);
    a.enqueue(chat("reject")).unwrap();
    a.drive(Duration::from_millis(1)).unwrap();
    let events = b
        .drive_with_accept(Duration::from_millis(1), |_| {
            Err(Error::InvalidInput("rejected".into()))
        })
        .unwrap();
    assert_eq!(received(&events), 0);
    let frames = sent.lock().unwrap();
    assert!(frames
        .iter()
        .any(|f| f.ftype == FrameType::Nack && f.ack_seq == 1));
    assert!(!frames
        .iter()
        .any(|f| f.ftype == FrameType::Ack && f.ack_seq == 1));
}

#[test]
fn initiator_ignores_stale_handshake_response() {
    let (outgoing, _out_rx) = mpsc::channel();
    let (incoming, in_rx) = mpsc::channel();
    for session in [99, 42] {
        incoming
            .send(encode_frame(&Frame::control(FrameType::SyncAck, session, 1, 0)).unwrap())
            .unwrap();
    }
    let wire = MockWire {
        tx: outgoing,
        rx: in_rx,
        sent: Arc::new(Mutex::new(Vec::new())),
        drop_type: None,
        drops: 0,
    };
    let mut cfg = RuntimeConfig::new(Role::A, ChannelMode::Sim, AppKind::Chat);
    cfg.sync_timeout = Duration::from_millis(20);
    let mut link = Link::new(cfg, wire, 42);
    link.handshake().unwrap();
    assert_eq!(link.session(), 42);
    assert!(matches!(
        link.into_wire().rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}
