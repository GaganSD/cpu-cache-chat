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
    queue: VecDeque<(u16, AppMessage)>,
    peer_seen: bool,
    have_token: bool,
    token_since: Instant,
}

struct Inflight {
    seq: u16,
    frame: Frame,
    attempts: u64,
    sent_at: Instant,
}

impl<W: Wire> Link<W> {
    pub fn new(cfg: RuntimeConfig, wire: W, session: u32) -> Self {
        let have_token = cfg.role.is_initiator();
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
            have_token,
            token_since: Instant::now(),
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
        msg.encode()?;
        self.next_seq = next_sequence(seq);
        self.queue.push_back((seq, msg));
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
            let slice = (self.cfg.sync_timeout / self.cfg.max_retries.max(1))
                .max(Duration::from_millis(50));
            let mut last = Error::Timeout;
            let mut ok = false;
            for _ in 0..=self.cfg.max_retries {
                if let Err(e) = self.send_control(FrameType::Sync, 0) {
                    last = e;
                    continue;
                }
                match self.recv_typed(FrameType::SyncAck, slice) {
                    Ok(frame) => {
                        self.accept_session(frame.session);
                        ok = true;
                        break;
                    }
                    Err(e) => last = e,
                }
            }
            if !ok {
                self.state = LinkState::Failed;
                return Err(last);
            }
        } else {
            let frame = self.recv_typed(FrameType::Sync, self.cfg.sync_timeout)?;
            self.accept_session(frame.session);
            self.send_control(FrameType::SyncAck, 0)?;
        }
        self.peer_seen = true;
        self.have_token = self.cfg.role.is_initiator();
        self.token_since = Instant::now();
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
        if self.inflight.is_none() && self.state == LinkState::Connected && self.have_token {
            if let Some((seq, msg)) = self.queue.pop_front() {
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
            } else if self.token_since.elapsed() >= Duration::from_millis(80)
                && self.send_control(FrameType::Heartbeat, 0).is_ok()
            {
                self.have_token = false;
            }
        }

        if let Some(inf) = &self.inflight {
            if inf.sent_at.elapsed() >= self.cfg.ack_timeout {
                let seq = inf.seq;
                if inf.attempts > u64::from(self.cfg.max_retries) {
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

        let wait = self.inflight.as_ref().map_or(wait, |inf| {
            wait.min(self.cfg.ack_timeout.saturating_sub(inf.sent_at.elapsed()))
        });
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
                        self.token_since = Instant::now();
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
                    if frame.ack_seq == inf.seq && inf.attempts <= u64::from(self.cfg.max_retries) {
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
                let expected = self.last_rx_seq.map_or(1, next_sequence);
                if frame.seq != expected {
                    return Ok(Some(vec![LinkEvent::Error(format!(
                        "unexpected data seq {}; expected {expected}",
                        frame.seq
                    ))]));
                }
                match AppMessage::decode(&frame.payload) {
                    Ok(msg) => {
                        self.last_rx_seq = Some(frame.seq);
                        let mut events = vec![LinkEvent::Recv(msg)];
                        if let Err(e) = self.send_control(FrameType::Ack, frame.seq) {
                            events.push(LinkEvent::Error(format!("ack failed: {e}")));
                        }
                        Ok(Some(events))
                    }
                    Err(e) => {
                        self.send_control(FrameType::Nack, frame.seq)?;
                        Ok(Some(vec![LinkEvent::Error(format!("bad payload: {e}"))]))
                    }
                }
            }
            FrameType::Heartbeat => {
                self.grant_token();
                Ok(None)
            }
            FrameType::Close => {
                self.state = LinkState::Closed;
                Ok(Some(vec![LinkEvent::State(self.state)]))
            }
            FrameType::Sync => {
                if !self.cfg.role.is_initiator() {
                    let _ = self.send_control(FrameType::SyncAck, 0);
                }
                Ok(None)
            }
            FrameType::SyncAck | FrameType::Resync => Ok(Some(vec![LinkEvent::Info(
                "unexpected control after handshake".into(),
            )])),
        }
    }

    fn grant_token(&mut self) {
        self.have_token = true;
        self.token_since = Instant::now();
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
                Ok(frame)
                    if frame.ftype == want
                        && (!self.cfg.role.is_initiator() || frame.session == self.session) =>
                {
                    return Ok(frame)
                }
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

fn next_sequence(seq: u16) -> u16 {
    if seq == u16::MAX {
        1
    } else {
        seq + 1
    }
}
