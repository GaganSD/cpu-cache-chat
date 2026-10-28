use ccc_core::{AppKind, ChannelMode, Error, LinkState, Role, RuntimeConfig};
use ccc_protocol::{decode_frame, encode_frame, AppMessage, Frame, FrameType, Link, Wire};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
struct Shared {
    incoming: Arc<Mutex<VecDeque<Vec<u8>>>>,
    sent: Arc<Mutex<Vec<Frame>>>,
}
struct TestWire(Shared);
impl Wire for TestWire {
    fn send_blob(&mut self, bytes: &[u8], _: Duration) -> Result<(), Error> {
        self.0.sent.lock().unwrap().push(decode_frame(bytes)?);
        Ok(())
    }
    fn recv_blob(&mut self, _: Duration) -> Result<Vec<u8>, Error> {
        self.0
            .incoming
            .lock()
            .unwrap()
            .pop_front()
            .ok_or(Error::Timeout)
    }
    fn interrupt(&mut self) {}
}
fn feed(s: &Shared, t: FrameType, seq: u16, ack: u16) {
    s.incoming
        .lock()
        .unwrap()
        .push_back(encode_frame(&Frame::control(t, 42, seq, ack)).unwrap());
}
fn link(role: Role) -> (Link<TestWire>, Shared) {
    let s = Shared::default();
    feed(
        &s,
        if role == Role::A {
            FrameType::SyncAck
        } else {
            FrameType::Sync
        },
        1,
        0,
    );
    let mut cfg = RuntimeConfig::new(role, ChannelMode::Sim, AppKind::Chat);
    cfg.ack_timeout = Duration::from_millis(2);
    cfg.max_retries = 1;
    let mut l = Link::new(cfg, TestWire(s.clone()), 42);
    l.handshake().unwrap();
    (l, s)
}
fn run(l: &mut Link<TestWire>, ms: u64) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(ms) {
        l.drive(Duration::ZERO).unwrap();
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn delayed_offer_after_return_does_not_regrant() {
    let (mut l, s) = link(Role::B);
    feed(&s, FrameType::Heartbeat, 1, 0);
    l.drive(Duration::ZERO).unwrap();
    run(&mut l, 90); // B returns token with identity 2
    feed(&s, FrameType::Heartbeat, 1, 0);
    l.drive(Duration::ZERO).unwrap();
    l.enqueue(AppMessage::Chat {
        id: 1,
        text: "queued".into(),
    })
    .unwrap();
    l.drive(Duration::ZERO).unwrap();
    assert!(!s
        .sent
        .lock()
        .unwrap()
        .iter()
        .any(|f| f.ftype == FrameType::Data));
    feed(&s, FrameType::Ack, 1, 0);
    l.drive(Duration::ZERO).unwrap(); // wrong confirmation
    feed(&s, FrameType::Heartbeat, 3, 0);
    l.drive(Duration::ZERO).unwrap();
    l.drive(Duration::ZERO).unwrap();
    assert!(s
        .sent
        .lock()
        .unwrap()
        .iter()
        .any(|f| f.ftype == FrameType::Data));
}
#[test]
fn repeated_sync_and_duplicate_offer_do_not_reset_or_regrant() {
    let (mut l, s) = link(Role::B);
    feed(&s, FrameType::Heartbeat, 1, 0);
    l.drive(Duration::ZERO).unwrap();
    feed(&s, FrameType::Sync, 1, 0);
    l.drive(Duration::ZERO).unwrap();
    feed(&s, FrameType::Heartbeat, 1, 0);
    l.drive(Duration::ZERO).unwrap();
    assert_eq!(
        s.sent
            .lock()
            .unwrap()
            .iter()
            .filter(|f| f.ftype == FrameType::Ack && f.seq == 1)
            .count(),
        2
    );
    run(&mut l, 90);
    feed(&s, FrameType::Heartbeat, 1, 0);
    l.drive(Duration::ZERO).unwrap();
    l.enqueue(AppMessage::Chat {
        id: 1,
        text: "queued".into(),
    })
    .unwrap();
    l.drive(Duration::ZERO).unwrap();
    assert!(!s
        .sent
        .lock()
        .unwrap()
        .iter()
        .any(|f| f.ftype == FrameType::Data));
}
#[test]
fn absent_peer_fails_for_both_roles_without_speculative_offer() {
    for role in [Role::A, Role::B] {
        let (mut l, s) = link(role);
        run(&mut l, 750);
        assert_eq!(l.state(), LinkState::Failed);
        let offers: Vec<_> = s
            .sent
            .lock()
            .unwrap()
            .iter()
            .filter(|f| f.ftype == FrameType::Heartbeat)
            .map(|f| f.seq)
            .collect();
        if role == Role::A {
            assert!(!offers.is_empty());
            assert!(offers.iter().all(|x| *x == 1));
        } else {
            assert!(offers.is_empty());
        }
    }
}
#[test]
fn mismatched_and_stale_confirmations_cannot_clear_handoff() {
    let (mut l, s) = link(Role::A);
    run(&mut l, 85);
    feed(&s, FrameType::Ack, 2, 0);
    l.drive(Duration::ZERO).unwrap();
    run(&mut l, 25);
    let offers: Vec<_> = s
        .sent
        .lock()
        .unwrap()
        .iter()
        .filter(|f| f.ftype == FrameType::Heartbeat)
        .map(|f| f.seq)
        .collect();
    assert!(offers.len() >= 2 && offers.iter().all(|x| *x == 1));
    feed(&s, FrameType::Ack, 1, 0);
    l.drive(Duration::ZERO).unwrap();
    assert_eq!(l.state(), LinkState::Connected);
}
