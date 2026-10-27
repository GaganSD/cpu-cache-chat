use std::collections::VecDeque;
use std::time::Duration;

use ccc_core::{AppKind, ChannelMode, Delivery, Error, LinkState, Role, RuntimeConfig};
use ccc_protocol::{
    decode_frame, encode_frame, AppMessage, Frame, FrameType, Link, LinkEvent, Wire,
};

#[derive(Clone, Copy)]
enum SendOutcome {
    Fail,
    FailButAck,
    SuccessButNoAck,
    SuccessAndNack,
    SuccessAndAck,
    Interrupt,
}

struct TestWire {
    incoming: VecDeque<Vec<u8>>,
    outcomes: VecDeque<SendOutcome>,
    sent: Vec<Frame>,
    fail_acks: usize,
}

impl Wire for TestWire {
    fn send_blob(&mut self, bytes: &[u8], _: Duration) -> Result<(), Error> {
        let frame = decode_frame(bytes)?;
        self.sent.push(frame.clone());
        if frame.ftype == FrameType::Ack && self.fail_acks > 0 {
            self.fail_acks -= 1;
            return Err(Error::Corrupt("missed physical slot"));
        }
        if frame.ftype != FrameType::Data {
            return Ok(());
        }
        let outcome = self.outcomes.pop_front().expect("unexpected data attempt");
        if matches!(
            outcome,
            SendOutcome::FailButAck | SendOutcome::SuccessAndAck
        ) {
            self.incoming.push_back(encode_frame(&Frame::control(
                FrameType::Ack,
                frame.session,
                0,
                frame.seq,
            ))?);
        }
        if matches!(outcome, SendOutcome::SuccessAndNack) {
            self.incoming.push_back(encode_frame(&Frame::control(
                FrameType::Nack,
                frame.session,
                0,
                frame.seq,
            ))?);
        }
        match outcome {
            SendOutcome::Fail | SendOutcome::FailButAck => {
                Err(Error::Corrupt("missed physical slot"))
            }
            SendOutcome::Interrupt => Err(Error::Interrupted),
            SendOutcome::SuccessButNoAck
            | SendOutcome::SuccessAndAck
            | SendOutcome::SuccessAndNack => Ok(()),
        }
    }
    fn recv_blob(&mut self, _: Duration) -> Result<Vec<u8>, Error> {
        self.incoming.pop_front().ok_or(Error::Timeout)
    }
    fn interrupt(&mut self) {}
}

#[test]
fn failed_nack_retransmission_preserves_frame_for_a_bounded_retry() {
    let mut link = connected(
        &[
            SendOutcome::SuccessAndNack,
            SendOutcome::Fail,
            SendOutcome::SuccessAndAck,
        ],
        2,
    );
    let first = link.drive(Duration::from_millis(1)).unwrap();
    assert!(!has(&first, Delivery::Unknown));
    assert!(!has(&first, Delivery::Delivered));
    assert!(first
        .iter()
        .any(|e| matches!(e, LinkEvent::Error(s) if s.contains("attempt 2 failed"))));
    std::thread::sleep(Duration::from_millis(4));
    let second = link.drive(Duration::from_millis(1)).unwrap();
    assert!(has(&second, Delivery::Delivered));
    let wire = link.into_wire();
    let data: Vec<_> = wire
        .sent
        .iter()
        .filter(|f| f.ftype == FrameType::Data)
        .collect();
    assert_eq!(data.len(), 3);
    assert!(data.iter().all(|f| *f == data[0]));
}

fn connected(outcomes: &[SendOutcome], max_retries: u32) -> Link<TestWire> {
    let mut cfg = RuntimeConfig::new(Role::A, ChannelMode::Sim, AppKind::Chat);
    cfg.ack_timeout = Duration::from_millis(2);
    cfg.max_retries = max_retries;
    let wire = TestWire {
        incoming: VecDeque::from([
            encode_frame(&Frame::control(FrameType::SyncAck, 42, 0, 0)).unwrap()
        ]),
        outcomes: outcomes.iter().copied().collect(),
        sent: Vec::new(),
        fail_acks: 0,
    };
    let mut link = Link::new(cfg, wire, 42);
    link.handshake().unwrap();
    link.enqueue(AppMessage::Chat {
        id: 1,
        text: "hello".into(),
    })
    .unwrap();
    link
}

#[test]
fn failed_ack_for_repeated_data_does_not_terminate_or_redeliver() {
    let mut cfg = RuntimeConfig::new(Role::B, ChannelMode::Sim, AppKind::Chat);
    cfg.ack_timeout = Duration::from_millis(2);
    let data = Frame::new(
        FrameType::Data,
        42,
        1,
        AppMessage::Chat {
            id: 1,
            text: "hello".into(),
        }
        .encode()
        .unwrap(),
    )
    .unwrap();
    let wire = TestWire {
        incoming: VecDeque::from([
            encode_frame(&Frame::control(FrameType::Sync, 42, 0, 0)).unwrap(),
            encode_frame(&data).unwrap(),
            encode_frame(&data).unwrap(),
        ]),
        outcomes: VecDeque::new(),
        sent: Vec::new(),
        fail_acks: 2,
    };
    let mut link = Link::new(cfg, wire, 0);
    link.handshake().unwrap();
    let first = link.drive(Duration::from_millis(1)).unwrap();
    assert_eq!(
        first
            .iter()
            .filter(|e| matches!(e, LinkEvent::Recv(_)))
            .count(),
        1
    );
    assert!(first
        .iter()
        .any(|e| matches!(e, LinkEvent::Error(s) if s.contains("ack failed"))));
    let duplicate = link.drive(Duration::from_millis(1)).unwrap();
    assert!(!duplicate.iter().any(|e| matches!(e, LinkEvent::Recv(_))));
    assert!(duplicate
        .iter()
        .any(|e| matches!(e, LinkEvent::Error(s) if s.contains("ack failed"))));
    assert_eq!(link.state(), LinkState::Connected);
}

fn has(events: &[LinkEvent], expected: Delivery) -> bool {
    events
        .iter()
        .any(|event| matches!(event, LinkEvent::Delivery { seq: 1, state } if *state == expected))
}

#[test]
fn send_error_is_not_unknown_when_a_matching_ack_arrives() {
    let mut link = connected(&[SendOutcome::FailButAck], 2);
    let events = link.drive(Duration::from_millis(1)).unwrap();
    assert!(has(&events, Delivery::AwaitingAck));
    assert!(has(&events, Delivery::Delivered));
    assert!(!has(&events, Delivery::Unknown));
    assert!(events
        .iter()
        .any(|event| matches!(event, LinkEvent::Error(s) if s.contains("missed physical slot"))));
    assert_eq!(link.queue_len(), 0);
    assert_eq!(
        link.into_wire()
            .sent
            .iter()
            .filter(|f| f.ftype == FrameType::Data)
            .count(),
        1
    );
}

#[test]
fn failed_send_retries_same_sequence_then_delivers_only_after_ack() {
    let mut link = connected(&[SendOutcome::Fail, SendOutcome::SuccessAndAck], 2);
    let first = link.drive(Duration::from_millis(1)).unwrap();
    assert!(has(&first, Delivery::AwaitingAck));
    assert!(!has(&first, Delivery::Delivered));
    assert!(!has(&first, Delivery::Unknown));
    std::thread::sleep(Duration::from_millis(4));
    let second = link.drive(Duration::from_millis(1)).unwrap();
    assert!(has(&second, Delivery::Delivered));
    let wire = link.into_wire();
    let data: Vec<_> = wire
        .sent
        .iter()
        .filter(|f| f.ftype == FrameType::Data)
        .collect();
    assert_eq!(data.len(), 2);
    assert_eq!(data[0], data[1]);
}

#[test]
fn repeated_send_failures_are_bounded_and_become_unknown() {
    let mut link = connected(
        &[SendOutcome::Fail, SendOutcome::Fail, SendOutcome::Fail],
        2,
    );
    link.enqueue(AppMessage::Chat {
        id: 2,
        text: "later".into(),
    })
    .unwrap();
    let first = link.drive(Duration::from_millis(1)).unwrap();
    assert!(!has(&first, Delivery::Unknown));
    for _ in 0..2 {
        std::thread::sleep(Duration::from_millis(4));
        let events = link.drive(Duration::from_millis(1)).unwrap();
        assert!(!has(&events, Delivery::Unknown));
    }
    std::thread::sleep(Duration::from_millis(4));
    let events = link.drive(Duration::from_millis(1)).unwrap();
    assert!(has(&events, Delivery::Unknown));
    assert_eq!(link.state(), LinkState::Degraded);
    assert_eq!(link.queue_len(), 1);
    let wire = link.into_wire();
    let data: Vec<_> = wire
        .sent
        .iter()
        .filter(|f| f.ftype == FrameType::Data)
        .collect();
    assert_eq!(data.len(), 3);
    assert!(data.iter().all(|f| f.seq == 1));
}

#[test]
fn interrupted_initial_or_retry_send_propagates_without_discarding_inflight() {
    for outcomes in [
        vec![SendOutcome::Interrupt],
        vec![SendOutcome::SuccessButNoAck, SendOutcome::Interrupt],
    ] {
        let retry = outcomes.len() == 2;
        let mut link = connected(&outcomes, 1);
        if retry {
            assert!(!has(
                &link.drive(Duration::from_millis(1)).unwrap(),
                Delivery::Unknown
            ));
            std::thread::sleep(Duration::from_millis(4));
        }
        assert!(matches!(
            link.drive(Duration::from_millis(1)),
            Err(Error::Interrupted)
        ));
        assert_eq!(link.queue_len(), 1);
        assert_eq!(link.state(), LinkState::Connected);
    }
}
