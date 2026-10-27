use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ccc_core::{Delivery, Error, LinkState, RuntimeConfig};

use crate::codec::{decode_frame, encode_frame};
use crate::frame::{AppMessage, Frame, FrameType};
use crate::Wire;

#[derive(Debug, Clone)]
pub struct Outbound {
    pub seq: u16,
    pub message: AppMessage,
    pub delivery: Delivery,
    pub attempts: u32,
}

#[derive(Debug, Clone)]
pub enum LinkEvent {
    State(LinkState),
    Delivery { seq: u16, state: Delivery },
    Recv(AppMessage),
    Info(String),
    Error(String),
}

pub struct Link<W: Wire> {
    cfg: RuntimeConfig,
    wire: W,
    state: LinkState,
    session: u32,
    next_seq: u16,
    last_rx_seq: Option<u16>,
    inflight: Option<Inflight>,
    queue: VecDeque<AppMessage>,
    peer_seen: bool,
}

struct Inflight {
    seq: u16,
    frame: Frame,
    attempts: u32,
    sent_at: Instant,
}

impl<W: Wire> Link<W> {
    pub fn new(cfg: RuntimeConfig, wire: W, session: u32) -> Self {
        Self {
            cfg,
            wire,
            state: LinkState::Idle,
            session,
            next_seq: 1,
            last_rx_seq: None,
            inflight: None,
            queue: VecDeque::new(),
            peer_seen: false,
        }
    }

    pub fn state(&self) -> LinkState {
        self.state
    }

    pub fn session(&self) -> u32 {
        self.session
    }

    pub fn queue_len(&self) -> usize {
        self.queue.len() + usize::from(self.inflight.is_some())
    }

    pub fn enqueue(&mut self, msg: AppMessage) -> Result<u16, Error> {
        let seq = self.next_seq;
        self.queue.push_back(msg);
        Ok(seq)
    }

    pub fn interrupt(&mut self) {
        self.wire.interrupt();
        self.state = LinkState::Closed;
    }

    pub fn handshake(&mut self) -> Result<Vec<LinkEvent>, Error> {
        self.state = LinkState::Connecting;
        let mut events = vec![LinkEvent::State(self.state)];
        if self.cfg.role.is_initiator() {
            self.send_control(FrameType::Sync, 0)?;
            let frame = self.recv_typed(FrameType::SyncAck, self.cfg.sync_timeout)?;
            self.accept_session(frame.session);
            self.peer_seen = true;
        } else {
            let frame = self.recv_typed(FrameType::Sync, self.cfg.sync_timeout)?;
            self.accept_session(frame.session);
            self.send_control(FrameType::SyncAck, 0)?;
            self.peer_seen = true;
        }
        self.state = LinkState::Connected;
        events.push(LinkEvent::State(self.state));
        events.push(LinkEvent::Info(format!(
            "session {:#010x} established",
            self.session
        )));
        Ok(events)
    }

    pub fn drive(&mut self, wait: Duration) -> Result<Vec<LinkEvent>, Error> {
        if self.state != LinkState::Connected && self.state != LinkState::Degraded {
            return Ok(Vec::new());
        }
        let mut events = Vec::new();
        if self.inflight.is_none() {
            if let Some(msg) = self.queue.pop_front() {
                let seq = self.next_seq;
                self.next_seq = self.next_seq.wrapping_add(1);
                if self.next_seq == 0 {
                    self.next_seq = 1;
                }
                let payload = msg.encode()?;
                let frame = Frame::new(FrameType::Data, self.session, seq, payload)?;
                self.transmit(&frame)?;
                self.inflight = Some(Inflight {
                    seq,
                    frame,
                    attempts: 1,
                    sent_at: Instant::now(),
                });
                events.push(LinkEvent::Delivery {
                    seq,
                    state: Delivery::AwaitingAck,
                });
            }
        }

        if let Some(inf) = &self.inflight {
            if inf.sent_at.elapsed() >= self.cfg.ack_timeout {
                let seq = inf.seq;
                if inf.attempts >= self.cfg.max_retries {
                    self.inflight = None;
                    self.state = LinkState::Degraded;
                    events.push(LinkEvent::Delivery {
                        seq,
                        state: Delivery::Unknown,
                    });
                    events.push(LinkEvent::State(self.state));
                    events.push(LinkEvent::Error(format!(
                        "ack missing for seq {seq}; delivery unknown"
                    )));
                    return Ok(events);
                }
                let mut inf = self.inflight.take().unwrap();
                inf.attempts += 1;
                self.transmit(&inf.frame)?;
                inf.sent_at = Instant::now();
                let attempts = inf.attempts;
                self.inflight = Some(inf);
                events.push(LinkEvent::Info(format!(
                    "retransmit seq {seq} attempt {attempts}"
                )));
            }
        }

        match self.wire.recv_blob(wait) {
            Ok(bytes) => {
                if let Some(more) = self.on_bytes(&bytes)? {
                    events.extend(more);
                }
            }
            Err(Error::Timeout) => {}
            Err(Error::Interrupted) => {
                self.state = LinkState::Closed;
                events.push(LinkEvent::State(self.state));
            }
            Err(e) => {
                self.state = LinkState::Degraded;
                events.push(LinkEvent::State(self.state));
                events.push(LinkEvent::Error(e.to_string()));
            }
        }
        Ok(events)
    }

    fn on_bytes(&mut self, bytes: &[u8]) -> Result<Option<Vec<LinkEvent>>, Error> {
        let frame = match decode_frame(bytes) {
            Ok(f) => f,
            Err(e) => {
                return Ok(Some(vec![LinkEvent::Error(format!("reject frame: {e}"))]));
            }
        };
        if self.peer_seen && frame.session != self.session {
            return Ok(Some(vec![LinkEvent::Error(format!(
                "stale session {:#x}",
                frame.session
            ))]));
        }
        match frame.ftype {
            FrameType::Ack => {
                if let Some(inf) = &self.inflight {
                    if frame.ack_seq == inf.seq {
                        let seq = inf.seq;
                        self.inflight = None;
                        if self.state == LinkState::Degraded {
                            self.state = LinkState::Connected;
                        }
                        return Ok(Some(vec![
                            LinkEvent::Delivery {
                                seq,
                                state: Delivery::Delivered,
                            },
                            LinkEvent::State(self.state),
                        ]));
                    }
                }
                Ok(Some(vec![LinkEvent::Info("ignored unmatched ack".into())]))
            }
            FrameType::Nack => {
                let retry = self.inflight.as_ref().and_then(|inf| {
                    if frame.ack_seq == inf.seq && inf.attempts < self.cfg.max_retries {
                        Some((inf.seq, inf.frame.clone()))
                    } else {
                        None
                    }
                });
                if let Some((seq, framed)) = retry {
                    if let Some(inf) = &mut self.inflight {
                        inf.attempts += 1;
                    }
                    self.transmit(&framed)?;
                    if let Some(inf) = &mut self.inflight {
                        inf.sent_at = Instant::now();
                    }
                    return Ok(Some(vec![LinkEvent::Info(format!(
                        "nack retransmit seq {seq}"
                    ))]));
                }
                Ok(None)
            }
            FrameType::Data => {
                if let Some(prev) = self.last_rx_seq {
                    if frame.seq == prev {
                        self.send_control(FrameType::Ack, frame.seq)?;
                        return Ok(Some(vec![LinkEvent::Info(format!(
                            "duplicate seq {} suppressed",
                            frame.seq
                        ))]));
                    }
                }
                match AppMessage::decode(&frame.payload) {
                    Ok(msg) => {
                        self.last_rx_seq = Some(frame.seq);
                        self.send_control(FrameType::Ack, frame.seq)?;
                        Ok(Some(vec![LinkEvent::Recv(msg)]))
                    }
                    Err(e) => {
                        self.send_control(FrameType::Nack, frame.seq)?;
                        Ok(Some(vec![LinkEvent::Error(format!("bad payload: {e}"))]))
                    }
                }
            }
            FrameType::Heartbeat => Ok(None),
            FrameType::Close => {
                self.state = LinkState::Closed;
                Ok(Some(vec![LinkEvent::State(self.state)]))
            }
            FrameType::Sync | FrameType::SyncAck | FrameType::Resync => {
                Ok(Some(vec![LinkEvent::Info(
                    "unexpected control after handshake".into(),
                )]))
            }
        }
    }

    fn send_control(&mut self, ftype: FrameType, ack_seq: u16) -> Result<(), Error> {
        let frame = Frame::control(ftype, self.session, 0, ack_seq);
        self.transmit(&frame)
    }

    fn transmit(&mut self, frame: &Frame) -> Result<(), Error> {
        let bytes = encode_frame(frame)?;
        self.wire
            .send_blob(&bytes, self.cfg.ack_timeout)
            .map_err(|e| match e {
                Error::Timeout => Error::Timeout,
                other => other,
            })
    }

    fn recv_typed(&mut self, want: FrameType, timeout: Duration) -> Result<Frame, Error> {
        let deadline = Instant::now() + timeout;
        loop {
            let remain = deadline.saturating_duration_since(Instant::now());
            if remain.is_zero() {
                return Err(Error::Timeout);
            }
            let bytes = self.wire.recv_blob(remain)?;
            match decode_frame(&bytes) {
                Ok(frame) if frame.ftype == want => return Ok(frame),
                Ok(_) => continue,
                Err(_) => continue,
            }
        }
    }

    fn accept_session(&mut self, session: u32) {
        if !self.cfg.role.is_initiator() {
            self.session = session;
        }
    }

    pub fn into_wire(self) -> W {
        self.wire
    }
}

/// New session id. Not a secret; only used to drop stale frames.
pub fn new_session_id() -> u32 {
    let mut b = [0u8; 4];
    fill_random(&mut b);
    u32::from_le_bytes(b) | 1
}

fn fill_random(buf: &mut [u8]) {
    let mut x = 0x9E37_79B9u64
        .wrapping_mul(std::process::id() as u64)
        .wrapping_add(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(1),
        );
    for slot in buf {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *slot = (x >> 24) as u8;
    }
}
