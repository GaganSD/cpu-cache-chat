use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ccc_core::{Delivery, Error, LinkState, RuntimeConfig};

use crate::codec::{decode_frame, encode_frame, FRAME_OVERHEAD};
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
    handoff: Option<TokenHandoff>,
    degraded_since: Option<Instant>,
    reoffer_backoff: Duration,
    tokenless_since: Option<Instant>,
}

struct TokenHandoff {
    sent_at: Instant,
    confirm_wait: Duration,
}

struct Inflight {
    seq: u16,
    frame: Frame,
    attempts: u64,
    sent_at: Instant,
    ack_wait: Duration,
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
            handoff: None,
            degraded_since: None,
            reoffer_backoff: Duration::ZERO,
            tokenless_since: if have_token {
                None
            } else {
                Some(Instant::now())
            },
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
            let slice = if self.cfg.channel.is_physical() {
                self.cfg.response_budget(FRAME_OVERHEAD)
            } else {
                (self.cfg.sync_timeout / self.cfg.max_retries.max(1)).max(Duration::from_millis(50))
            };
            let mut last = Error::Timeout;
            let mut ok = false;
            for _ in 0..=self.cfg.max_retries {
                if let Err(e) = self.send_control(FrameType::Sync, 0) {
                    if matches!(e, Error::Interrupted) {
                        return Err(e);
                    }
                    last = e;
                    continue;
                }
                match self.recv_typed(FrameType::SyncAck, slice) {
                    Ok(frame) => {
                        self.accept_session(frame.session);
                        ok = true;
                        break;
                    }
                    Err(Error::Interrupted) => return Err(Error::Interrupted),
                    Err(e) => last = e,
                }
            }
            if !ok {
                self.state = LinkState::Failed;
                return Err(last);
            }
        } else {
            let frame =
                self.recv_typed(FrameType::Sync, self.cfg.sync_listen_budget(FRAME_OVERHEAD))?;
            self.accept_session(frame.session);
            self.send_control(FrameType::SyncAck, 0)?;
        }
        self.peer_seen = true;
        self.have_token = self.cfg.role.is_initiator();
        self.token_since = Instant::now();
        self.handoff = None;
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
        let receiving = self.wire.receive_in_progress();
        if !receiving
            && self.inflight.is_none()
            && self.state == LinkState::Connected
            && self.have_token
        {
            if let Some((seq, msg)) = self.queue.pop_front() {
                let payload = msg.encode()?;
                let frame = Frame::new(FrameType::Data, self.session, seq, payload)?;
                // Even an error can occur after enough of the frame reached
                // the peer. Preserve the sequence and wait for a possible ACK
                // before retrying; an error never implies delivery either way.
                let result = self.transmit(&frame);
                let ack_wait = self.acknowledgement_wait(&frame, result.is_err());
                self.inflight = Some(Inflight {
                    seq,
                    frame,
                    attempts: 1,
                    sent_at: Instant::now(),
                    ack_wait,
                });
                if matches!(result, Err(Error::Interrupted)) {
                    return Err(Error::Interrupted);
                }
                events.push(LinkEvent::Delivery {
                    seq,
                    state: Delivery::AwaitingAck,
                });
                if let Err(e) = result {
                    events.push(LinkEvent::Error(format!(
                        "send seq {seq} attempt 1 failed: {e}; waiting for ack before retry"
                    )));
                }
            } else if self.token_since.elapsed() >= Duration::from_millis(80) {
                // A send error is ambiguous: the peer may have received the
                // token. Relinquish it before attempting the handoff and do
                // not reclaim it if confirmation is lost.
                self.have_token = false;
                self.tokenless_since = Some(Instant::now());
                let result = self.send_control(FrameType::Heartbeat, 0);
                let confirm_wait = if result.is_err() {
                    self.cfg
                        .failed_send_recovery_budget(FRAME_OVERHEAD, FRAME_OVERHEAD)
                } else {
                    self.cfg.response_budget(FRAME_OVERHEAD)
                };
                self.handoff = Some(TokenHandoff {
                    sent_at: Instant::now(),
                    confirm_wait,
                });
                if matches!(result, Err(Error::Interrupted)) {
                    return Err(Error::Interrupted);
                }
                if let Err(e) = result {
                    events.push(LinkEvent::Error(format!(
                        "token handoff send failed: {e}; awaiting confirmation"
                    )));
                }
            }
        }

        if !receiving
            && self
                .handoff
                .as_ref()
                .is_some_and(|handoff| handoff.sent_at.elapsed() >= handoff.confirm_wait)
        {
            self.handoff = None;
            self.enter_degraded();
            events.push(LinkEvent::State(self.state));
            events.push(LinkEvent::Error(
                "token handoff unconfirmed; peer may hold token".into(),
            ));
            return Ok(events);
        }

        // Token re-arbitration after an unconfirmed handoff. Only the
        // initiator re-offers, and re-offering never claims the token: the
        // peer acquires it only by receiving the heartbeat, so this path
        // cannot create two owners. Without it, a lost handoff plus a lost
        // confirmation leaves both sides tokenless forever. An initiator
        // that never degraded can still end up tokenless while the peer
        // waits degraded for a heartbeat it cannot re-offer itself, so a
        // long tokenless silence also permits a re-offer.
        let initiator_reoffer = self.cfg.role.is_initiator()
            && self.inflight.is_none()
            && self.handoff.is_none()
            && ((self.state == LinkState::Degraded
                && self
                    .degraded_since
                    .is_some_and(|since| since.elapsed() >= self.reoffer_backoff))
                || (self.state == LinkState::Connected
                    && !self.have_token
                    && self
                        .tokenless_since
                        .is_some_and(|since| since.elapsed() >= self.connected_reoffer_idle())));
        if !receiving && initiator_reoffer {
            // Relinquish before offering. Holding the token here means no
            // heartbeat from us was ever confirmed, so the peer cannot hold
            // it; re-offering still never claims ownership.
            self.have_token = false;
            self.tokenless_since = Some(Instant::now());
            let result = self.send_control(FrameType::Heartbeat, 0);
            let confirm_wait = if result.is_err() {
                self.cfg
                    .failed_send_recovery_budget(FRAME_OVERHEAD, FRAME_OVERHEAD)
            } else {
                self.cfg.response_budget(FRAME_OVERHEAD)
            };
            self.handoff = Some(TokenHandoff {
                sent_at: Instant::now(),
                confirm_wait,
            });
            self.degraded_since = Some(Instant::now());
            let doubled = self.reoffer_backoff.saturating_mul(2);
            self.reoffer_backoff = doubled
                .max(self.cfg.response_budget(FRAME_OVERHEAD))
                .min(Duration::from_secs(60));
            if matches!(result, Err(Error::Interrupted)) {
                return Err(Error::Interrupted);
            }
            match result {
                Ok(()) => events.push(LinkEvent::Info(
                    "re-offering token after unconfirmed handoff".into(),
                )),
                Err(e) => events.push(LinkEvent::Error(format!(
                    "token re-offer send failed: {e}; awaiting confirmation"
                ))),
            }
        }

        if let Some(inf) = self.inflight.as_ref().filter(|_| !receiving) {
            if inf.sent_at.elapsed() >= inf.ack_wait {
                let seq = inf.seq;
                if inf.attempts > u64::from(self.cfg.max_retries) {
                    self.inflight = None;
                    self.enter_degraded();
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
                events.extend(self.retransmit_inflight("retransmit")?);
            }
        }

        let wait = self
            .inflight
            .as_ref()
            .filter(|_| !receiving)
            .map_or(wait, |inf| {
                wait.min(inf.ack_wait.saturating_sub(inf.sent_at.elapsed()))
            });
        let wait = if receiving {
            wait
        } else {
            self.handoff.as_ref().map_or(wait, |handoff| {
                wait.min(
                    handoff
                        .confirm_wait
                        .saturating_sub(handoff.sent_at.elapsed()),
                )
            })
        };
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
                if frame.ack_seq == 0 {
                    let confirmed = self.handoff.take().is_some();
                    // A stale or duplicate token confirmation still evidences
                    // that the peer acquired the token; recover a degraded
                    // link without claiming ownership. If that belief is
                    // wrong, the tokenless re-offer bounds the wait.
                    let mut events = Vec::new();
                    if confirmed {
                        events.push(LinkEvent::Info("token handoff confirmed".into()));
                    }
                    if self.state == LinkState::Degraded {
                        self.state = LinkState::Connected;
                        self.degraded_since = None;
                        events.push(LinkEvent::State(self.state));
                    }
                    if !events.is_empty() {
                        return Ok(Some(events));
                    }
                    return Ok(Some(vec![LinkEvent::Info("ignored unmatched ack".into())]));
                }
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
                let retry = self.inflight.as_ref().is_some_and(|inf| {
                    frame.ack_seq == inf.seq && inf.attempts <= u64::from(self.cfg.max_retries)
                });
                if retry {
                    return Ok(Some(self.retransmit_inflight("nack retransmit")?));
                }
                Ok(None)
            }
            FrameType::Data => {
                // A valid peer data frame also proves the peer acquired the
                // token even if its handoff ACK was lost.
                self.handoff = None;
                if let Some(prev) = self.last_rx_seq {
                    if frame.seq == prev {
                        let result = self.send_control(FrameType::Ack, frame.seq);
                        if matches!(result, Err(Error::Interrupted)) {
                            return Err(Error::Interrupted);
                        }
                        let mut events = vec![LinkEvent::Info(format!(
                            "duplicate seq {} suppressed",
                            frame.seq
                        ))];
                        if let Err(e) = result {
                            events.push(LinkEvent::Error(format!("ack failed: {e}")));
                        }
                        return Ok(Some(events));
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
                        // Valid in-sequence peer data proves the peer holds
                        // the token and the link works; recover from a
                        // degraded handoff without claiming the token.
                        let mut events = Vec::new();
                        if self.state == LinkState::Degraded {
                            self.state = LinkState::Connected;
                            self.degraded_since = None;
                            events.push(LinkEvent::State(self.state));
                        }
                        events.push(LinkEvent::Recv(msg));
                        if let Err(e) = self.send_control(FrameType::Ack, frame.seq) {
                            if matches!(e, Error::Interrupted) {
                                return Err(e);
                            }
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
                let result = self.send_control(FrameType::Ack, 0);
                if matches!(result, Err(Error::Interrupted)) {
                    return Err(Error::Interrupted);
                }
                self.grant_token();
                // A valid offer resolves our ownership uncertainty too: the
                // sender relinquished before offering, so accepting the token
                // cannot create two owners, and the link demonstrably works.
                let mut events = Vec::new();
                if self.state == LinkState::Degraded {
                    self.state = LinkState::Connected;
                    self.degraded_since = None;
                    events.push(LinkEvent::State(self.state));
                }
                if let Err(e) = result {
                    events.push(LinkEvent::Error(format!("token confirmation failed: {e}")));
                }
                if events.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(events))
                }
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
        self.handoff = None;
        self.tokenless_since = None;
    }

    /// How long a connected, tokenless initiator waits for the peer's token
    /// use before re-offering. Long enough that an inferring or transmitting
    /// peer almost always finishes first; collisions degrade to ordinary
    // bounded retries, never to dual ownership.
    fn connected_reoffer_idle(&self) -> Duration {
        let budget = self.cfg.response_budget(FRAME_OVERHEAD).saturating_mul(4);
        if self.cfg.channel.is_physical() {
            budget.max(Duration::from_secs(30))
        } else {
            budget
        }
    }

    fn enter_degraded(&mut self) {
        self.state = LinkState::Degraded;
        if self.degraded_since.is_none() {
            self.degraded_since = Some(Instant::now());
            self.reoffer_backoff = self.cfg.response_budget(FRAME_OVERHEAD);
        }
    }

    fn retransmit_inflight(&mut self, reason: &str) -> Result<Vec<LinkEvent>, Error> {
        let mut inf = self.inflight.take().expect("inflight frame exists");
        inf.attempts += 1;
        let seq = inf.seq;
        let attempts = inf.attempts;
        let result = self.transmit(&inf.frame);
        inf.ack_wait = self.acknowledgement_wait(&inf.frame, result.is_err());
        inf.sent_at = Instant::now();
        // Restore before examining the result, including interruption. An
        // ambiguous send must never lose or replace the original sequence.
        self.inflight = Some(inf);
        if matches!(result, Err(Error::Interrupted)) {
            return Err(Error::Interrupted);
        }
        let mut events = vec![LinkEvent::Info(format!(
            "{reason} seq {seq} attempt {attempts}"
        ))];
        if let Err(e) = result {
            events.push(LinkEvent::Error(format!(
                "send seq {seq} attempt {attempts} failed: {e}; waiting for ack before retry"
            )));
        }
        Ok(events)
    }

    fn acknowledgement_wait(&self, frame: &Frame, send_failed: bool) -> Duration {
        if send_failed {
            self.cfg.failed_send_recovery_budget(
                FRAME_OVERHEAD.saturating_add(frame.payload.len()),
                FRAME_OVERHEAD,
            )
        } else {
            self.cfg.response_budget(FRAME_OVERHEAD)
        }
    }

    fn send_control(&mut self, ftype: FrameType, ack_seq: u16) -> Result<(), Error> {
        let frame = Frame::control(ftype, self.session, 0, ack_seq);
        self.transmit(&frame)
    }

    fn transmit(&mut self, frame: &Frame) -> Result<(), Error> {
        let bytes = encode_frame(frame)?;
        self.wire
            .send_blob(&bytes, self.cfg.send_budget(bytes.len()))
    }

    fn recv_typed(&mut self, want: FrameType, timeout: Duration) -> Result<Frame, Error> {
        let began = Instant::now();
        loop {
            let remain = timeout.saturating_sub(began.elapsed());
            if remain.is_zero() {
                return Err(Error::Timeout);
            }
            let bytes = match self.wire.recv_blob(remain) {
                Ok(bytes) => bytes,
                Err(Error::Timeout) => return Err(Error::Timeout),
                Err(Error::Interrupted) => return Err(Error::Interrupted),
                Err(_) => continue,
            };
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
