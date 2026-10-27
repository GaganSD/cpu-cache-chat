use ccc_core::Mark;
use ccc_protocol::{decode_frame, encode_frame, AppMessage, Frame, FrameType};

#[test]
fn roundtrip_data_frame() {
    let msg = AppMessage::Chat {
        id: 7,
        text: "hello".into(),
    };
    let frame = Frame::new(FrameType::Data, 0x1111_2222, 3, msg.encode().unwrap()).unwrap();
    let bytes = encode_frame(&frame).unwrap();
    let back = decode_frame(&bytes).unwrap();
    assert_eq!(back, frame);
    assert_eq!(AppMessage::decode(&back.payload).unwrap(), msg);
}

#[test]
fn rejects_bad_checksum() {
    let frame = Frame::control(FrameType::Ack, 1, 0, 4);
    let mut bytes = encode_frame(&frame).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    assert!(decode_frame(&bytes).is_err());
}

#[test]
fn rejects_truncated() {
    assert!(decode_frame(&[0xC3, 0xC4, 1]).is_err());
}

#[test]
fn move_payload() {
    let msg = AppMessage::Move {
        ply: 0,
        square: 5,
        mark: Mark::X,
    };
    let bytes = msg.encode().unwrap();
    assert_eq!(AppMessage::decode(&bytes).unwrap(), msg);
}

#[test]
fn oversized_payload() {
    let big = vec![0u8; 300];
    assert!(Frame::new(FrameType::Data, 1, 1, big).is_err());
}
