use std::collections::VecDeque;
use std::time::Duration;

use ccc_core::{AppKind, ChannelMode, Delivery, Error, Role, RuntimeConfig};
use ccc_protocol::{
    decode_frame, encode_frame, AppMessage, Frame, FrameType, Link, LinkEvent, Wire,
};

#[derive(Default)]
struct TimingWire {
    incoming: VecDeque<Vec<u8>>,
    sends: Vec<(Frame, usize, Duration)>,
    waits: Vec<Duration>,
    fail_data: bool,
}

impl Wire for TimingWire {
    fn send_blob(&mut self, bytes: &[u8], timeout: Duration) -> Result<(), Error> {
        let frame = decode_frame(bytes)?;
        let fail = self.fail_data && frame.ftype == FrameType::Data;
        self.sends.push((frame, bytes.len(), timeout));
        if fail {
            return Err(Error::Corrupt("missed physical slot"));
        }
        Ok(())
    }

    fn recv_blob(&mut self, timeout: Duration) -> Result<Vec<u8>, Error> {
        self.waits.push(timeout);
        self.incoming.pop_front().ok_or(Error::Timeout)
    }

    fn interrupt(&mut self) {}
}

#[test]
fn failed_physical_send_waits_for_receiver_to_drain_before_retry() {
    let cfg = config(Role::A, 50);
    let wire = TimingWire {
        incoming: VecDeque::from([
            encode_frame(&Frame::control(FrameType::SyncAck, 42, 1, 0)).unwrap()
        ]),
        fail_data: true,
        ..TimingWire::default()
    };
    let mut link = Link::new(cfg.clone(), wire, 42);
    link.handshake().unwrap();
    link.enqueue(AppMessage::Chat {
        id: 1,
        text: "a".repeat(200),
    })
    .unwrap();
    let events = link.drive(Duration::MAX).unwrap();
    assert!(events.iter().any(|e| matches!(
        e,
        LinkEvent::Delivery {
            state: Delivery::AwaitingAck,
            ..
        }
    )));
    assert!(!events.iter().any(|e| matches!(
        e,
        LinkEvent::Delivery {
            state: Delivery::Unknown,
            ..
        }
    )));
    let wire = link.into_wire();
    assert!(wire.waits[1] > cfg.physical_frame_duration(222) + cfg.response_budget(19));
    // On top of the receiver-drain time sits a bounded randomized backoff of
    // 0.5-2s; the recorded wait must stay inside that window.
    let drain = cfg.failed_send_recovery_budget(222, 19);
    assert!(wire.waits[1] >= drain + Duration::from_millis(400));
    assert!(wire.waits[1] <= drain + Duration::from_millis(2000));
}

fn config(role: Role, millis: u64) -> RuntimeConfig {
    let mut cfg = RuntimeConfig::new(role, ChannelMode::Cpu, AppKind::Chat);
    cfg.slot = Duration::from_millis(millis);
    cfg.ack_timeout = Duration::from_millis(1);
    cfg.sync_timeout = Duration::from_millis(1);
    cfg.max_retries = 0;
    cfg
}

#[test]
fn initiator_waits_for_complete_sync_ack_and_budgets_actual_data_size() {
    for millis in [10, 20, 50] {
        let cfg = config(Role::A, millis);
        let wire = TimingWire {
            incoming: VecDeque::from([
                encode_frame(&Frame::control(FrameType::SyncAck, 42, 1, 0)).unwrap()
            ]),
            ..TimingWire::default()
        };
        let mut link = Link::new(cfg.clone(), wire, 42);
        link.handshake().unwrap();
        link.enqueue(AppMessage::Chat {
            id: 1,
            text: "a".repeat(200),
        })
        .unwrap();
        let events = link.drive(Duration::MAX).unwrap();
        assert!(events.iter().any(|e| matches!(
            e,
            LinkEvent::Delivery {
                state: Delivery::AwaitingAck,
                ..
            }
        )));
        let wire = link.into_wire();
        assert_eq!(wire.sends[0].0.ftype, FrameType::Sync);
        assert_eq!(wire.sends[0].1, 19);
        assert_eq!(wire.sends[0].2, cfg.send_budget(19));
        assert!(wire.waits[0] > cfg.physical_frame_duration(19));
        assert_eq!(wire.sends[1].0.ftype, FrameType::Data);
        assert_eq!(wire.sends[1].1, 222);
        assert_eq!(wire.sends[1].2, cfg.send_budget(222));
        assert!(wire.sends[1].2 > cfg.physical_frame_duration(222));
        // The drive wait is clamped by an ACK budget long enough for the whole response.
        assert!(wire.waits[1] > cfg.physical_frame_duration(19));
        // A clean send pays no post-failure backoff.
        assert!(wire.waits[1] <= cfg.response_budget(19));
    }
}

#[test]
fn responder_listens_long_enough_for_a_complete_sync() {
    let cfg = config(Role::B, 50);
    let wire = TimingWire {
        incoming: VecDeque::from([
            encode_frame(&Frame::control(FrameType::Sync, 42, 1, 0)).unwrap()
        ]),
        ..TimingWire::default()
    };
    let mut link = Link::new(cfg.clone(), wire, 0);
    link.handshake().unwrap();
    assert_eq!(link.session(), 42);
    let wire = link.into_wire();
    assert!(wire.waits[0] > cfg.physical_frame_duration(19));
    assert_eq!(wire.sends[0].0.ftype, FrameType::SyncAck);
    assert_eq!(wire.sends[0].2, cfg.send_budget(19));
}
