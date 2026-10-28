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
    next_seq: u32,
    token_epoch: u16,
    last_rx_seq: Option<u16>,
    inflight: Option<Inflight>,
    queue: VecDeque<(u16, AppMessage)>,
    peer_seen: bool,
    have_token: bool,
    token_since: Instant,
    handoff: Option<TokenHandoff>,
    tokenless_since: Option<Instant>,
}

struct TokenHandoff {
    sent_at: Instant,
    confirm_wait: Duration,
    epoch: u16,
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
            token_epoch: 0,
            last_rx_seq: None,
            inflight: None,
            queue: VecDeque::new(),
            peer_seen: false,
            have_token,
            token_since: Instant::now(),
            handoff: None,
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
        if matches!(self.state, LinkState::Failed | LinkState::Closed) {
            return Err(Error::InvalidInput("session is no longer usable".into()));
        }
        if self.queue_len() >= 256 {
            return Err(Error::InvalidInput("outbound queue full".into()));
        }
        let seq = u16::try_from(self.next_seq)
            .map_err(|_| Error::InvalidInput("sequence exhausted; start a new session".into()))?;
        msg.encode()?;
        self.next_seq += 1;
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
                if let Err(e) = self.send_handshake(FrameType::Sync) {
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
            self.send_handshake(FrameType::SyncAck)?;
        }
        self.peer_seen = true;
        self.have_token = self.cfg.role.is_initiator();
        self.token_since = Instant::now();
        self.handoff = None;
        self.tokenless_since = if self.have_token {
            None
        } else {
            Some(Instant::now())
        };
        self.state = LinkState::Connected;
        events.push(LinkEvent::State(self.state));
        events.push(LinkEvent::Info(format!(
            "session {:#010x} established",
            self.session
        )));
        Ok(events)
    }

    pub fn drive(&mut self, wait: Duration) -> Result<Vec<LinkEvent>, Error> {
        self.drive_with_accept(wait, |_| Ok(()))
    }

    /// Validate and apply a new message synchronously before acknowledging it.
    /// The callback must not block on external I/O. It is not called for duplicates.
    /// The legacy `drive` accepts all syntactically valid application messages.
    pub fn drive_with_accept<F>(
        &mut self,
        wait: Duration,
        mut accept: F,
    ) -> Result<Vec<LinkEvent>, Error>
    where
        F: FnMut(&AppMessage) -> Result<(), Error>,
    {
        self.drive_with_accept_seq(wait, |_, msg| accept(msg))
    }

    /// As `drive_with_accept`, with the actual incoming sequence for evidence.
    pub fn drive_with_accept_seq<F>(
        &mut self,
        wait: Duration,
        mut accept: F,
    ) -> Result<Vec<LinkEvent>, Error>
    where
        F: FnMut(u16, &AppMessage) -> Result<(), Error>,
    {
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
                let Some(epoch) = self.token_epoch.checked_add(1) else {
                    self.state = LinkState::Failed;
                    events.push(LinkEvent::State(self.state));
                    events.push(LinkEvent::Error("token offer identity exhausted".into()));
                    return Ok(events);
                };
                self.token_epoch = epoch;
                let result = self.send_offer(epoch);
                let confirm_wait = if result.is_err() {
                    self.cfg
                        .failed_send_recovery_budget(FRAME_OVERHEAD, FRAME_OVERHEAD)
                } else {
                    self.cfg.response_budget(FRAME_OVERHEAD)
                };
                self.handoff = Some(TokenHandoff {
                    sent_at: Instant::now(),
                    confirm_wait,
                    epoch,
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

        // Only retransmit the exact outstanding offer; never invent a new grant.
        if self
            .tokenless_since
            .is_some_and(|since| since.elapsed() >= self.token_deadline())
        {
            self.state = LinkState::Failed;
            events.push(LinkEvent::State(self.state));
            events.push(LinkEvent::Error("token recovery deadline exceeded".into()));
            return Ok(events);
        }
        if !receiving
            && self
                .handoff
                .as_ref()
                .is_some_and(|h| h.sent_at.elapsed() >= h.confirm_wait)
        {
            let epoch = self.handoff.as_ref().unwrap().epoch;
            if self.state != LinkState::Degraded {
                self.state = LinkState::Degraded;
                events.push(LinkEvent::State(self.state));
            }
            let result = self.send_offer(epoch);
            let wait = if result.is_err() {
                self.cfg
                    .failed_send_recovery_budget(FRAME_OVERHEAD, FRAME_OVERHEAD)
            } else {
                self.cfg.response_budget(FRAME_OVERHEAD)
            };
            let h = self.handoff.as_mut().unwrap();
            h.sent_at = Instant::now();
            h.confirm_wait = wait;
            if matches!(result, Err(Error::Interrupted)) {
                return Err(Error::Interrupted);
            }
            if let Err(e) = result {
                events.push(LinkEvent::Error(format!("token retry failed: {e}")));
            }
        }

        if let Some(inf) = self.inflight.as_ref().filter(|_| !receiving) {
            if inf.sent_at.elapsed() >= inf.ack_wait {
                let seq = inf.seq;
                if inf.attempts > u64::from(self.cfg.max_retries) {
                    // Retain the unresolved frame and stop this session permanently.
                    self.state = LinkState::Failed;
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
        let wait = self.tokenless_since.map_or(wait, |since| {
            wait.min(self.token_deadline().saturating_sub(since.elapsed()))
        });
        match self.wire.recv_blob(wait) {
            Ok(bytes) => {
                if let Some(more) = self.on_bytes(&bytes, &mut accept)? {
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

    fn on_bytes<F>(&mut self, bytes: &[u8], accept: &mut F) -> Result<Option<Vec<LinkEvent>>, Error>
    where
        F: FnMut(u16, &AppMessage) -> Result<(), Error>,
    {
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
                    if self.handoff.as_ref().is_some_and(|h| h.epoch == frame.seq) {
                        self.handoff = None;
                        self.tokenless_since = Some(Instant::now());
                        let mut events = vec![LinkEvent::Info("token handoff confirmed".into())];
                        if self.state == LinkState::Degraded {
                            self.state = LinkState::Connected;
                            events.push(LinkEvent::State(self.state));
                        }
                        return Ok(Some(events));
                    }
                    return Ok(Some(vec![LinkEvent::Info(
                        "ignored unmatched token confirmation".into(),
                    )]));
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
                if self.have_token {
                    return Ok(Some(vec![LinkEvent::Error(
                        "peer data while holding token".into(),
                    )]));
                }
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
                let expected = self.last_rx_seq.map_or(Some(1), |n| n.checked_add(1));
                if Some(frame.seq) != expected {
                    return Ok(Some(vec![LinkEvent::Error(format!(
                        "unexpected data seq {}; expected {expected:?}",
                        frame.seq
                    ))]));
                }
                match AppMessage::decode(&frame.payload) {
                    Ok(msg) => {
                        if let Err(e) = accept(frame.seq, &msg) {
                            self.send_control(FrameType::Nack, frame.seq)?;
                            return Ok(Some(vec![LinkEvent::Error(format!(
                                "application rejected seq {}: {e}",
                                frame.seq
                            ))]));
                        }
                        self.handoff = None;
                        self.tokenless_since = Some(Instant::now());
                        self.last_rx_seq = Some(frame.seq);
                        // Valid in-sequence peer data proves the peer holds
                        // the token and the link works; recover from a
                        // degraded handoff without claiming the token.
                        let mut events = Vec::new();
                        if self.state == LinkState::Degraded {
                            self.state = LinkState::Connected;
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
                if frame.seq == self.token_epoch && !self.have_token && self.handoff.is_some() {
                    // This epoch was our outgoing offer, not a peer offer.
                    return Ok(None);
                }
                if frame.seq == self.token_epoch && self.have_token {
                    let result = self.send_token_confirmation(frame.seq);
                    if matches!(result, Err(Error::Interrupted)) {
                        return Err(Error::Interrupted);
                    }
                    return Ok(None);
                }
                if self.have_token || self.token_epoch.checked_add(1) != Some(frame.seq) {
                    return Ok(Some(vec![LinkEvent::Info(
                        "ignored stale or unexpected token offer".into(),
                    )]));
                }
                self.token_epoch = frame.seq;
                self.grant_token();
                let result = self.send_token_confirmation(frame.seq);
                if matches!(result, Err(Error::Interrupted)) {
                    return Err(Error::Interrupted);
                }
                let mut events = Vec::new();
                if self.state == LinkState::Degraded {
                    self.state = LinkState::Connected;
                    events.push(LinkEvent::State(self.state));
                }
                if let Err(e) = result {
                    events.push(LinkEvent::Error(format!("token confirmation failed: {e}")));
                }
                Ok(Some(events))
            }
            FrameType::Close => {
                self.state = LinkState::Closed;
                Ok(Some(vec![LinkEvent::State(self.state)]))
            }
            FrameType::Sync => {
                let kind = match self.cfg.kind {
                    ccc_core::AppKind::Chat => 1,
                    ccc_core::AppKind::Game => 2,
                };
                if !self.cfg.role.is_initiator() && frame.seq == kind {
                    let _ = self.send_handshake(FrameType::SyncAck);
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

    fn token_deadline(&self) -> Duration {
        let budget = self.cfg.response_budget(FRAME_OVERHEAD).max(
            self.cfg
                .failed_send_recovery_budget(FRAME_OVERHEAD, FRAME_OVERHEAD),
        );
        budget
            .saturating_mul(self.cfg.max_retries.saturating_add(3))
            .max(if self.cfg.channel.is_physical() {
                Duration::from_secs(30)
            } else {
                Duration::from_millis(500)
            })
    }

    fn send_handshake(&mut self, ftype: FrameType) -> Result<(), Error> {
        let kind = match self.cfg.kind {
            ccc_core::AppKind::Chat => 1,
            ccc_core::AppKind::Game => 2,
        };
        self.transmit(&Frame::control(ftype, self.session, kind, 0))
    }

    fn send_offer(&mut self, epoch: u16) -> Result<(), Error> {
        self.transmit(&Frame::control(
            FrameType::Heartbeat,
            self.session,
            epoch,
            0,
        ))
    }

    fn send_token_confirmation(&mut self, epoch: u16) -> Result<(), Error> {
        self.transmit(&Frame::control(FrameType::Ack, self.session, epoch, 0))
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
                        && (!self.cfg.role.is_initiator() || frame.session == self.session)
                        && frame.seq
                            == match self.cfg.kind {
                                ccc_core::AppKind::Chat => 1,
                                ccc_core::AppKind::Game => 2,
                            } =>
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
    u32::from_le_bytes(b)
}

fn fill_random(buf: &mut [u8]) {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(buf))
        .expect("OS entropy unavailable");
}
