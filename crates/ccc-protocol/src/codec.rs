use ccc_core::{Error, FRAME_MAGIC, MAX_PAYLOAD, PROTOCOL_VERSION};

use crate::frame::{Frame, FrameType};

const HEADER: usize = 2 + 1 + 1 + 1 + 4 + 2 + 2 + 2;
const CRC: usize = 4;

pub fn encode_frame(frame: &Frame) -> Result<Vec<u8>, Error> {
    if frame.payload.len() > MAX_PAYLOAD {
        return Err(Error::TooLarge(frame.payload.len()));
    }
    let mut buf = Vec::with_capacity(HEADER + frame.payload.len() + CRC);
    buf.extend_from_slice(&FRAME_MAGIC);
    buf.push(PROTOCOL_VERSION);
    buf.push(frame.ftype as u8);
    buf.push(frame.flags);
    buf.extend_from_slice(&frame.session.to_le_bytes());
    buf.extend_from_slice(&frame.seq.to_le_bytes());
    buf.extend_from_slice(&frame.ack_seq.to_le_bytes());
    let len =
        u16::try_from(frame.payload.len()).map_err(|_| Error::TooLarge(frame.payload.len()))?;
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(&frame.payload);
    let crc = crc32fast::hash(&buf);
    buf.extend_from_slice(&crc.to_le_bytes());
    Ok(buf)
}

pub fn decode_frame(bytes: &[u8]) -> Result<Frame, Error> {
    if bytes.len() < HEADER + CRC {
        return Err(Error::Corrupt("truncated"));
    }
    if bytes[0] != FRAME_MAGIC[0] || bytes[1] != FRAME_MAGIC[1] {
        return Err(Error::Corrupt("bad magic"));
    }
    if bytes[2] != PROTOCOL_VERSION {
        return Err(Error::Corrupt("bad version"));
    }
    let ftype = FrameType::from_u8(bytes[3]).ok_or(Error::Corrupt("bad type"))?;
    let flags = bytes[4];
    let session = u32::from_le_bytes(bytes[5..9].try_into().unwrap());
    let seq = u16::from_le_bytes(bytes[9..11].try_into().unwrap());
    let ack_seq = u16::from_le_bytes(bytes[11..13].try_into().unwrap());
    let len = u16::from_le_bytes(bytes[13..15].try_into().unwrap()) as usize;
    if len > MAX_PAYLOAD {
        return Err(Error::TooLarge(len));
    }
    let need = HEADER + len + CRC;
    if bytes.len() != need {
        return Err(Error::Corrupt("length mismatch"));
    }
    let payload = bytes[HEADER..HEADER + len].to_vec();
    let got = u32::from_le_bytes(bytes[HEADER + len..].try_into().unwrap());
    let expect = crc32fast::hash(&bytes[..HEADER + len]);
    if got != expect {
        return Err(Error::Corrupt("checksum"));
    }
    Ok(Frame {
        ftype,
        flags,
        session,
        seq,
        ack_seq,
        payload,
    })
}
