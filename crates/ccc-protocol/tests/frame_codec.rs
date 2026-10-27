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

#[test]
fn rejects_invalid_frame_headers_and_lengths() {
    let bytes = encode_frame(&Frame::new(FrameType::Data, 1, 1, vec![42]).unwrap()).unwrap();
    for (offset, value) in [(0, 0), (2, 2), (3, 99), (13, 2)] {
        let mut bad = bytes.clone();
        bad[offset] = value;
        assert!(decode_frame(&bad).is_err(), "offset {offset}");
    }
    assert!(decode_frame(&bytes[..bytes.len() - 1]).is_err());
    let mut extra = bytes.clone();
    extra.push(0);
    assert!(decode_frame(&extra).is_err());
}

#[test]
fn chat_validation_and_control_bytes() {
    use ccc_core::MAX_CHAT_BYTES;
    for text in ["bad\0text".to_string(), "x".repeat(MAX_CHAT_BYTES + 1)] {
        assert!(AppMessage::Chat { id: 1, text }.encode().is_err());
    }
    for raw in [
        vec![1, 0, 0, 0],
        vec![1, 0, 0, 0xff],
        [vec![1, 0, 0], vec![b'x'; MAX_CHAT_BYTES + 1]].concat(),
    ] {
        assert!(AppMessage::decode(&raw).is_err());
    }
    let msg = AppMessage::Chat {
        id: 4,
        text: "\x1b[2J\x07\n".into(),
    };
    let frame = Frame::new(FrameType::Data, 1, 1, msg.encode().unwrap()).unwrap();
    let decoded = decode_frame(&encode_frame(&frame).unwrap()).unwrap();
    assert_eq!(AppMessage::decode(&decoded.payload).unwrap(), msg);
}
