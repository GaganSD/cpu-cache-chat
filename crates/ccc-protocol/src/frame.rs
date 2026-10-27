use ccc_core::{Error, Mark, MAX_CHAT_BYTES, MAX_PAYLOAD};

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    Sync = 0,
    SyncAck = 1,
    Data = 2,
    Ack = 3,
    Nack = 4,
    Heartbeat = 5,
    Resync = 6,
    Close = 7,
}

impl FrameType {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::Sync,
            1 => Self::SyncAck,
            2 => Self::Data,
            3 => Self::Ack,
            4 => Self::Nack,
            5 => Self::Heartbeat,
            6 => Self::Resync,
            7 => Self::Close,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub ftype: FrameType,
    pub flags: u8,
    pub session: u32,
    pub seq: u16,
    pub ack_seq: u16,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(ftype: FrameType, session: u32, seq: u16, payload: Vec<u8>) -> Result<Self, Error> {
        if payload.len() > MAX_PAYLOAD {
            return Err(Error::TooLarge(payload.len()));
        }
        Ok(Self {
            ftype,
            flags: 0,
            session,
            seq,
            ack_seq: 0,
            payload,
        })
    }

    pub fn control(ftype: FrameType, session: u32, seq: u16, ack_seq: u16) -> Self {
        Self {
            ftype,
            flags: 0,
            session,
            seq,
            ack_seq,
            payload: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameCtrl {
    Start = 1,
    Abort = 2,
}

impl GameCtrl {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Start),
            2 => Some(Self::Abort),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMessage {
    Chat { id: u16, text: String },
    Move { ply: u8, square: u8, mark: Mark },
    Ctrl(GameCtrl),
}

impl AppMessage {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        match self {
            AppMessage::Chat { id, text } => {
                let raw = text.as_bytes();
                if raw.len() > MAX_CHAT_BYTES {
                    return Err(Error::TooLarge(raw.len()));
                }
                if raw.contains(&0) {
                    return Err(Error::InvalidInput("chat text contains NUL".into()));
                }
                let mut out = Vec::with_capacity(3 + raw.len());
                out.push(0x01);
                out.extend_from_slice(&id.to_le_bytes());
                out.extend_from_slice(raw);
                Ok(out)
            }
            AppMessage::Move { ply, square, mark } => {
                if !(1..=9).contains(square) {
                    return Err(Error::InvalidInput("square must be 1-9".into()));
                }
                Ok(vec![0x02, *ply, *square, mark.as_u8()])
            }
            AppMessage::Ctrl(kind) => Ok(vec![0x03, *kind as u8]),
        }
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.is_empty() {
            return Err(Error::Corrupt("empty app payload"));
        }
        match bytes[0] {
            0x01 => {
                if bytes.len() < 3 {
                    return Err(Error::Corrupt("short chat"));
                }
                let id = u16::from_le_bytes([bytes[1], bytes[2]]);
                let text = std::str::from_utf8(&bytes[3..])
                    .map_err(|_| Error::Corrupt("chat is not utf-8"))?
                    .to_string();
                if text.len() > MAX_CHAT_BYTES {
                    return Err(Error::TooLarge(text.len()));
                }
                if text.as_bytes().contains(&0) {
                    return Err(Error::Corrupt("chat contains NUL"));
                }
                Ok(AppMessage::Chat { id, text })
            }
            0x02 => {
                if bytes.len() != 4 {
                    return Err(Error::Corrupt("bad move length"));
                }
                let mark = Mark::from_u8(bytes[3]).ok_or(Error::Corrupt("bad mark"))?;
                if !(1..=9).contains(&bytes[2]) {
                    return Err(Error::Corrupt("bad square"));
                }
                Ok(AppMessage::Move {
                    ply: bytes[1],
                    square: bytes[2],
                    mark,
                })
            }
            0x03 => {
                if bytes.len() != 2 {
                    return Err(Error::Corrupt("bad ctrl length"));
                }
                let kind = GameCtrl::from_u8(bytes[1]).ok_or(Error::Corrupt("bad ctrl"))?;
                Ok(AppMessage::Ctrl(kind))
            }
            _ => Err(Error::Corrupt("unknown app tag")),
        }
    }
}
