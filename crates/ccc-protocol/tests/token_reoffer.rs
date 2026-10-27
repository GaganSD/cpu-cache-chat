use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ccc_core::{AppKind, ChannelMode, Error, LinkState, Role, RuntimeConfig};
use ccc_protocol::{
    decode_frame, encode_frame, AppMessage, Frame, FrameType, Link, LinkEvent, Wire,
};

#[derive(Clone, Default)]
struct Shared {
    incoming: Arc<Mutex<VecDeque<Vec<u8>>>>,
    sends: Arc<Mutex<Vec<FrameType>>>,
}

struct SharedWire(Shared);

impl Wire for SharedWire {
    fn send_blob(&mut self, bytes: &[u8], _: Duration) -> Result<(), Error> {
        self.0
            .sends
            .lock()
            .unwrap()
            .push(decode_frame(bytes)?.ftype);
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
    fn receive_in_progress(&self) -> bool {
        false
    }
    fn interrupt(&mut self) {}
}

fn link_for(role: Role) -> (Link<SharedWire>, Shared) {
    let shared = Shared::default();
    let first = match role {
        Role::A => Frame::control(FrameType::SyncAck, 42, 0, 0),
        Role::B => Frame::control(FrameType::Sync, 42, 0, 0),
    };
    shared
        .incoming
        .lock()
        .unwrap()
        .push_back(encode_frame(&first).unwrap());
    let mut cfg = RuntimeConfig::new(role, ChannelMode::Sim, AppKind::Chat);
    cfg.ack_timeout = Duration::from_millis(1);
    cfg.max_retries = 1;
    let mut link = Link::new(cfg, SharedWire(shared.clone()), 42);
    link.handshake().unwrap();
    (link, shared)
}

fn drive_for(link: &mut Link<SharedWire>, total: Duration) -> Vec<LinkEvent> {
    let began = std::time::Instant::now();
    let mut events = Vec::new();
    while began.elapsed() < total {
        events.extend(link.drive(Duration::from_millis(2)).unwrap());
        std::thread::sleep(Duration::from_millis(2));
    }
    events
}

fn heartbeats(shared: &Shared) -> usize {
    shared
        .sends
        .lock()
        .unwrap()
        .iter()
        .filter(|t| **t == FrameType::Heartbeat)
        .count()
}

fn feed(shared: &Shared, frame: &Frame) {
    shared
        .incoming
        .lock()
        .unwrap()
        .push_back(encode_frame(frame).unwrap());
}

#[test]
fn initiator_reoffers_token_after_unconfirmed_handoff() {
    let (mut link, shared) = link_for(Role::A);
    // Idle past the handoff delay: the initiator offers the token, and with
    // no confirmation it degrades rather than assuming ownership.
    let events = drive_for(&mut link, Duration::from_millis(120));
    assert!(heartbeats(&shared) >= 1);
    assert!(events
        .iter()
        .any(|e| matches!(e, LinkEvent::State(LinkState::Degraded))));
    // After the bounded backoff the initiator re-offers without claiming the token.
    let before = heartbeats(&shared);
    drive_for(&mut link, Duration::from_millis(40));
    assert!(heartbeats(&shared) > before);
    // A late confirmation recovers the link to connected. Re-offer cycles
    // leave brief windows with no pending handoff, so flood acknowledgments:
    // any pending handoff is confirmed at the next poll, and a stale one
    // still recovers the degraded link without claiming ownership.
    for _ in 0..200 {
        feed(&shared, &Frame::control(FrameType::Ack, 42, 0, 0));
    }
    let events = drive_for(&mut link, Duration::from_millis(400));
    let recovered = events
        .iter()
        .any(|e| matches!(e, LinkEvent::State(LinkState::Connected)));
    let confirmed = events.iter().any(|e| {
        matches!(
            e,
            LinkEvent::Info(s) if s.contains("token handoff confirmed")
        )
    });
    assert!(recovered && confirmed);
}

#[test]
fn non_initiator_never_reoffers_token() {
    let (mut link, shared) = link_for(Role::B);
    // B receives the token, then goes idle and offers it back; no confirmation.
    feed(&shared, &Frame::control(FrameType::Heartbeat, 42, 0, 0));
    let events = drive_for(&mut link, Duration::from_millis(200));
    assert!(events
        .iter()
        .any(|e| matches!(e, LinkEvent::State(LinkState::Degraded))));
    let before = heartbeats(&shared);
    drive_for(&mut link, Duration::from_millis(120));
    assert_eq!(
        heartbeats(&shared),
        before,
        "non-initiator must not re-offer"
    );
}

#[test]
fn connected_tokenless_initiator_eventually_reoffers() {
    let (mut link, shared) = link_for(Role::A);
    // Complete one handoff: the initiator ends connected but tokenless.
    drive_for(&mut link, Duration::from_millis(100));
    for _ in 0..50 {
        feed(&shared, &Frame::control(FrameType::Ack, 42, 0, 0));
    }
    drive_for(&mut link, Duration::from_millis(40));
    assert_eq!(link.state(), LinkState::Connected);
    let before = heartbeats(&shared);
    // The peer never uses the token; the initiator must eventually re-offer
    // even though it never degraded.
    drive_for(&mut link, Duration::from_millis(80));
    assert!(heartbeats(&shared) > before);
    assert_eq!(link.state(), LinkState::Connected);
}

#[test]
fn heartbeat_recovers_degraded_peer_and_token_flows_back() {
    // The stall seen on the physical channel: B degraded without a token,
    // the initiator re-offers, B must accept the token AND recover so it can
    // offer the token back to the peer that has data queued.
    let (mut link, shared) = link_for(Role::B);
    // B offers the token, nobody confirms: B degrades.
    feed(&shared, &Frame::control(FrameType::Heartbeat, 42, 0, 0));
    let events = drive_for(&mut link, Duration::from_millis(200));
    assert!(events
        .iter()
        .any(|e| matches!(e, LinkEvent::State(LinkState::Degraded))));
    // The initiator's re-offer arrives: B takes the token and recovers.
    feed(&shared, &Frame::control(FrameType::Heartbeat, 42, 0, 0));
    let events = drive_for(&mut link, Duration::from_millis(20));
    assert!(events
        .iter()
        .any(|e| matches!(e, LinkEvent::State(LinkState::Connected))));
    // With nothing to send, B offers the token back within its idle delay.
    let before = heartbeats(&shared);
    drive_for(&mut link, Duration::from_millis(120));
    assert!(heartbeats(&shared) > before);
}

#[test]
fn valid_peer_data_recovers_degraded_initiator() {
    let (mut link, shared) = link_for(Role::A);
    drive_for(&mut link, Duration::from_millis(120));
    assert_eq!(link.state(), LinkState::Degraded);
    // The peer had in fact acquired the token and sent data: recover.
    let payload = AppMessage::Chat {
        id: 1,
        text: "hi".into(),
    }
    .encode()
    .unwrap();
    feed(
        &shared,
        &Frame::new(FrameType::Data, 42, 1, payload).unwrap(),
    );
    let events = drive_for(&mut link, Duration::from_millis(20));
    assert!(events
        .iter()
        .any(|e| matches!(e, LinkEvent::State(LinkState::Connected))));
    assert!(events.iter().any(|e| matches!(e, LinkEvent::Recv(_))));
}
