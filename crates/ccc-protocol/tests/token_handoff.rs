use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ccc_core::{AppKind, ChannelMode, Error, LinkState, Role, RuntimeConfig};
use ccc_protocol::{
    decode_frame, encode_frame, AppMessage, Frame, FrameType, Link, LinkEvent, Wire,
};

type Inbox = Arc<Mutex<VecDeque<Vec<u8>>>>;
struct HandoffWire {
    inbox: Inbox,
    sent: Vec<Frame>,
    fail_offer: bool,
    fail_ack: bool,
}
impl Wire for HandoffWire {
    fn send_blob(&mut self, bytes: &[u8], _: Duration) -> Result<(), Error> {
        let frame = decode_frame(bytes)?;
        let fail = (self.fail_offer && frame.ftype == FrameType::Heartbeat)
            || (self.fail_ack && frame.ftype == FrameType::Ack);
        self.sent.push(frame);
        if fail {
            Err(Error::Corrupt("missed physical slot"))
        } else {
            Ok(())
        }
    }
    fn recv_blob(&mut self, _: Duration) -> Result<Vec<u8>, Error> {
        self.inbox.lock().unwrap().pop_front().ok_or(Error::Timeout)
    }
    fn interrupt(&mut self) {}
}
fn push(inbox: &Inbox, kind: FrameType, ack: u16) {
    inbox
        .lock()
        .unwrap()
        .push_back(encode_frame(&Frame::control(kind, 42, 0, ack)).unwrap());
}
fn connected(role: Role, fail_offer: bool, fail_ack: bool) -> (Link<HandoffWire>, Inbox) {
    let inbox = Arc::new(Mutex::new(VecDeque::new()));
    push(
        &inbox,
        if role == Role::A {
            FrameType::SyncAck
        } else {
            FrameType::Sync
        },
        0,
    );
    let wire = HandoffWire {
        inbox: inbox.clone(),
        sent: Vec::new(),
        fail_offer,
        fail_ack,
    };
    let mut cfg = RuntimeConfig::new(role, ChannelMode::Sim, AppKind::Chat);
    cfg.ack_timeout = Duration::from_millis(5);
    let mut link = Link::new(cfg, wire, 42);
    link.handshake().unwrap();
    (link, inbox)
}
fn wait_for_offer() {
    std::thread::sleep(Duration::from_millis(90));
}
fn queue(link: &mut Link<HandoffWire>) {
    link.enqueue(AppMessage::Chat {
        id: 1,
        text: "hello".into(),
    })
    .unwrap();
}
#[test]
fn token_offer_is_confirmed_and_old_owner_waits_for_return() {
    let (mut link, inbox) = connected(Role::A, false, false);
    wait_for_offer();
    link.drive(Duration::ZERO).unwrap();
    queue(&mut link);
    push(&inbox, FrameType::Ack, 0);
    let confirmation = link.drive(Duration::ZERO).unwrap();
    assert!(confirmation
        .iter()
        .any(|e| matches!(e, LinkEvent::Info(s) if s == "token handoff confirmed")));
    link.drive(Duration::ZERO).unwrap();
    push(&inbox, FrameType::Heartbeat, 0);
    link.drive(Duration::ZERO).unwrap();
    link.drive(Duration::ZERO).unwrap();
    let sent: Vec<_> = link
        .into_wire()
        .sent
        .into_iter()
        .map(|f| (f.ftype, f.ack_seq))
        .collect();
    assert_eq!(
        sent,
        [
            (FrameType::Sync, 0),
            (FrameType::Heartbeat, 0),
            (FrameType::Ack, 0),
            (FrameType::Data, 0)
        ]
    );
}
#[test]
fn ambiguous_or_lost_offer_fails_closed_without_retry_or_reclaim() {
    for fail_offer in [false, true] {
        let (mut link, _) = connected(Role::A, fail_offer, false);
        wait_for_offer();
        let offer = link.drive(Duration::ZERO).unwrap();
        if fail_offer {
            assert!(offer
                .iter()
                .any(|e| matches!(e, LinkEvent::Error(s) if s.contains("handoff send failed"))));
        }
        queue(&mut link);
        std::thread::sleep(Duration::from_millis(8));
        let events = link.drive(Duration::ZERO).unwrap();
        assert!(events
            .iter()
            .any(|e| matches!(e, LinkEvent::Error(s) if s.contains("handoff unconfirmed"))));
        assert_eq!(link.state(), LinkState::Degraded);
        link.drive(Duration::ZERO).unwrap();
        let sent: Vec<_> = link.into_wire().sent.into_iter().map(|f| f.ftype).collect();
        assert_eq!(sent, [FrameType::Sync, FrameType::Heartbeat]);
    }
}
#[test]
fn new_owner_sends_confirmation_before_data_even_if_confirmation_fails() {
    for fail_ack in [false, true] {
        let (mut link, inbox) = connected(Role::B, false, fail_ack);
        queue(&mut link);
        push(&inbox, FrameType::Heartbeat, 0);
        let events = link.drive(Duration::ZERO).unwrap();
        if fail_ack {
            assert!(events
                .iter()
                .any(|e| matches!(e, LinkEvent::Error(s) if s.contains("confirmation failed"))));
        }
        link.drive(Duration::ZERO).unwrap();
        let sent: Vec<_> = link
            .into_wire()
            .sent
            .into_iter()
            .map(|f| (f.ftype, f.ack_seq))
            .collect();
        assert_eq!(
            sent,
            [
                (FrameType::SyncAck, 0),
                (FrameType::Ack, 0),
                (FrameType::Data, 0)
            ]
        );
    }
}
