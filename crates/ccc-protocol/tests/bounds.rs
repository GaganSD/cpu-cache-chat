use ccc_core::{AppKind, ChannelMode, Error, Role, RuntimeConfig};
use ccc_protocol::{decode_frame, encode_frame, AppMessage, Frame, FrameType, Link, Wire};
use std::collections::VecDeque;
use std::time::Duration;

struct AutoWire {
    inbox: VecDeque<Vec<u8>>,
}
impl Wire for AutoWire {
    fn send_blob(&mut self, bytes: &[u8], _: Duration) -> Result<(), Error> {
        let f = decode_frame(bytes)?;
        if f.ftype == FrameType::Data {
            self.inbox.push_back(encode_frame(&Frame::control(
                FrameType::Ack,
                f.session,
                0,
                f.seq,
            ))?);
        }
        Ok(())
    }
    fn recv_blob(&mut self, _: Duration) -> Result<Vec<u8>, Error> {
        self.inbox.pop_front().ok_or(Error::Timeout)
    }
    fn interrupt(&mut self) {}
}
fn msg() -> AppMessage {
    AppMessage::Chat {
        id: 0,
        text: String::new(),
    }
}
fn link() -> Link<AutoWire> {
    let cfg = RuntimeConfig::new(Role::A, ChannelMode::Sim, AppKind::Chat);
    let wire = AutoWire {
        inbox: VecDeque::from([
            encode_frame(&Frame::control(FrameType::SyncAck, 42, 1, 0)).unwrap()
        ]),
    };
    let mut l = Link::new(cfg, wire, 42);
    l.handshake().unwrap();
    l
}
#[test]
fn sequence_exhaustion_and_queue_capacity() {
    let mut l = link();
    for n in 1..=256 {
        assert_eq!(l.enqueue(msg()).unwrap(), n);
    }
    assert!(l.enqueue(msg()).is_err());
    for _ in 0..256 {
        l.drive(Duration::ZERO).unwrap();
    }
    for n in 257..=u16::MAX {
        assert_eq!(l.enqueue(msg()).unwrap(), n);
        l.drive(Duration::ZERO).unwrap();
    }
    assert!(l.enqueue(msg()).is_err());
}
#[test]
fn incompatible_app_kind_is_not_accepted() {
    let mut cfg = RuntimeConfig::new(Role::A, ChannelMode::Sim, AppKind::Chat);
    cfg.sync_timeout = Duration::from_millis(1);
    cfg.max_retries = 0;
    let wire = AutoWire {
        inbox: VecDeque::from([
            encode_frame(&Frame::control(FrameType::SyncAck, 42, 2, 0)).unwrap()
        ]),
    };
    assert!(Link::new(cfg, wire, 42).handshake().is_err());
}
#[test]
fn codec_rejects_invalid_semantics_even_with_valid_crc() {
    let base = encode_frame(&Frame::control(FrameType::Ack, 42, 0, 1)).unwrap();
    for (offset, value) in [(4, 1), (9, 1), (11, 0)] {
        let mut b = base.clone();
        b[offset] = value;
        let n = b.len();
        let crc = crc32fast::hash(&b[..n - 4]);
        b[n - 4..].copy_from_slice(&crc.to_le_bytes());
        assert!(decode_frame(&b).is_err());
    }
    for f in [
        Frame::control(FrameType::Heartbeat, 1, 0, 0),
        Frame::control(FrameType::Nack, 1, 1, 0),
        Frame::new(FrameType::Close, 1, 0, vec![1]).unwrap(),
        Frame::new(FrameType::Data, 1, 0, vec![1]).unwrap(),
    ] {
        assert!(encode_frame(&f).is_err());
    }
}
