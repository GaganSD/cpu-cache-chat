//! Session orchestration. No terminal drawing.

use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use ccc_channel::open_wire;
use ccc_core::{AppKind, Delivery, Error, LinkState, Mark, Role, RuntimeConfig};
use ccc_game::{Board, Outcome};
use ccc_protocol::{new_session_id, AppMessage, GameCtrl, Link, LinkEvent, Wire};

#[derive(Debug, Clone)]
pub struct ChatLine {
    pub from: Role,
    pub text: String,
    pub delivery: Delivery,
    pub seq: u16,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub role: Role,
    pub channel_label: String,
    pub simulated: bool,
    pub link: LinkState,
    pub chat: Vec<ChatLine>,
    pub draft_note: String,
    pub board: Option<String>,
    pub turn: Option<Mark>,
    pub outcome: Option<String>,
    pub status: String,
}

pub struct Session<W: Wire> {
    cfg: RuntimeConfig,
    link: Link<W>,
    chat: Vec<ChatLine>,
    board: Board,
    started: bool,
    status: String,
    next_chat_id: u16,
}

impl Session<Box<dyn Wire>> {
    pub fn open(cfg: RuntimeConfig, stop: Arc<AtomicBool>) -> Result<Self, Error> {
        cfg.validate()?;
        let wire = open_wire(&cfg, stop)?;
        let session = if cfg.role.is_initiator() {
            new_session_id()
        } else {
            0
        };
        Ok(Self::wrap(cfg, wire, session))
    }
}

impl<W: Wire> Session<W> {
    pub fn wrap(cfg: RuntimeConfig, wire: W, session: u32) -> Self {
        let status = if cfg.channel.is_physical() {
            format!("calibrate/connect over {}", cfg.channel)
        } else {
            "simulation transport — not a hardware channel".into()
        };
        let link = Link::new(cfg.clone(), wire, session);
        Self {
            cfg,
            link,
            chat: Vec::new(),
            board: Board::new(),
            started: false,
            status,
            next_chat_id: 1,
        }
    }

    pub fn connect(&mut self) -> Result<Vec<String>, Error> {
        let events = self.link.handshake()?;
        Ok(self.apply(events))
    }

    pub fn queue_chat(&mut self, text: String) -> Result<u16, Error> {
        let text = sanitize_user_text(&text)?;
        let id = self.next_chat_id;
        self.next_chat_id = self.next_chat_id.saturating_add(1);
        let seq = self.link.enqueue(AppMessage::Chat {
            id,
            text: text.clone(),
        })?;
        self.chat.push(ChatLine {
            from: self.cfg.role,
            text,
            delivery: Delivery::Queued,
            seq,
        });
        Ok(seq)
    }

    pub fn queue_move(&mut self, square: u8) -> Result<Outcome, Error> {
        let mark = self.cfg.role.mark();
        let ply = self.board.ply();
        let outcome = self.board.play(square, mark)?;
        let seq = self.link.enqueue(AppMessage::Move { ply, square, mark })?;
        self.status = format!("queued move {square} (seq {seq})");
        Ok(outcome)
    }

    pub fn request_start(&mut self) -> Result<(), Error> {
        self.link.enqueue(AppMessage::Ctrl(GameCtrl::Start))?;
        self.started = true;
        self.status = "game start sent".into();
        Ok(())
    }

    pub fn tick(&mut self, wait: Duration) -> Result<Vec<String>, Error> {
        let events = self.link.drive(wait)?;
        Ok(self.apply(events))
    }

    pub fn interrupt(&mut self) {
        self.link.interrupt();
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            role: self.cfg.role,
            channel_label: self.cfg.channel.label().to_string(),
            simulated: !self.cfg.channel.is_physical(),
            link: self.link.state(),
            chat: self.chat.clone(),
            draft_note: if self.link.queue_len() > 1 {
                format!("{} outbound waiting", self.link.queue_len())
            } else {
                String::new()
            },
            board: (self.cfg.kind == AppKind::Game).then(|| self.board.render()),
            turn: (self.cfg.kind == AppKind::Game
                && matches!(self.board.outcome(), Outcome::InProgress))
            .then(|| self.board.turn()),
            outcome: match self.board.outcome() {
                Outcome::InProgress => None,
                Outcome::Win(m) => Some(format!("{} wins", m.glyph())),
                Outcome::Draw => Some("draw".into()),
            },
            status: self.status.clone(),
        }
    }

    pub fn our_turn(&self) -> bool {
        self.cfg.kind == AppKind::Game
            && self.board.outcome() == Outcome::InProgress
            && self.board.turn() == self.cfg.role.mark()
            && self.link.state() == LinkState::Connected
    }

    pub fn board(&self) -> &Board {
        &self.board
    }

    pub fn try_agent_move(&mut self) -> Result<Option<u8>, Error> {
        if !self.cfg.agent || !self.our_turn() {
            return Ok(None);
        }
        let cmd = self.cfg.agent_cmd.clone().unwrap_or_else(|| "node".into());
        let mut child = Command::new(&cmd)
            .arg(
                self.cfg
                    .agent_cmd
                    .as_ref()
                    .map(|_| "")
                    .unwrap_or("/opt/ccc/agent/play.mjs"),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error::Agent(e.to_string()))?;
        use std::io::Write;
        if let Some(stdin) = child.stdin.as_mut() {
            let body = format!(
                "{{\"board\":\"{}\",\"turn\":\"{}\",\"legal\":{:?}}}\n",
                self.board.compact(),
                self.board.turn().glyph(),
                self.board.legal_squares()
            );
            stdin
                .write_all(body.as_bytes())
                .map_err(|e| Error::Agent(e.to_string()))?;
        }
        let out = child
            .wait_with_output()
            .map_err(|e| Error::Agent(e.to_string()))?;
        if !out.status.success() {
            return Err(Error::Agent(format!(
                "agent exit {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            )));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let square = parse_square(&text)?;
        Ok(Some(square))
    }

    fn apply(&mut self, events: Vec<LinkEvent>) -> Vec<String> {
        let mut notes = Vec::new();
        for ev in events {
            match ev {
                LinkEvent::State(s) => {
                    self.status = format!("link {s}");
                    notes.push(self.status.clone());
                }
                LinkEvent::Delivery { seq, state } => {
                    if let Some(line) = self.chat.iter_mut().find(|c| c.seq == seq) {
                        line.delivery = state;
                    }
                    notes.push(format!("seq {seq}: {state}"));
                }
                LinkEvent::Recv(AppMessage::Chat { text, .. }) => {
                    let shown = escape_controls(&text);
                    self.chat.push(ChatLine {
                        from: self.cfg.role.peer(),
                        text: shown.clone(),
                        delivery: Delivery::Delivered,
                        seq: 0,
                    });
                    notes.push(format!("peer: {shown}"));
                }
                LinkEvent::Recv(AppMessage::Move { ply, square, mark }) => {
                    match self.board.play(square, mark) {
                        Ok(outcome) => {
                            self.status = format!("peer move {square} (ply {ply})");
                            if let Outcome::Win(m) = outcome {
                                self.status = format!("{} wins", m.glyph());
                            } else if outcome == Outcome::Draw {
                                self.status = "draw".into();
                            }
                        }
                        Err(e) => {
                            self.status = format!("rejected peer move: {e}");
                        }
                    }
                    notes.push(self.status.clone());
                }
                LinkEvent::Recv(AppMessage::Ctrl(GameCtrl::Start)) => {
                    self.started = true;
                    self.status = "peer started a match".into();
                    notes.push(self.status.clone());
                }
                LinkEvent::Recv(AppMessage::Ctrl(GameCtrl::Abort)) => {
                    self.status = "peer aborted".into();
                    notes.push(self.status.clone());
                }
                LinkEvent::Info(s) | LinkEvent::Error(s) => {
                    self.status = s.clone();
                    notes.push(s);
                }
            }
        }
        notes
    }
}

pub fn sanitize_user_text(text: &str) -> Result<String, Error> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(Error::InvalidInput("empty message".into()));
    }
    if trimmed.len() > ccc_core::MAX_CHAT_BYTES {
        return Err(Error::TooLarge(trimmed.len()));
    }
    if trimmed.bytes().any(|b| b == 0) {
        return Err(Error::InvalidInput("NUL is not allowed".into()));
    }
    Ok(trimmed.to_string())
}

pub fn escape_controls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:02x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn parse_square(text: &str) -> Result<u8, Error> {
    let t = text.trim();
    if let Ok(n) = t.parse::<u8>() {
        if (1..=9).contains(&n) {
            return Ok(n);
        }
    }
    if let Some(rest) = t.strip_prefix("{\"square\":") {
        let n: u8 = rest
            .trim_start()
            .trim_end_matches(|c: char| c == '}' || c == ',' || c.is_whitespace())
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .map_err(|_| Error::Agent("bad square json".into()))?;
        if (1..=9).contains(&n) {
            return Ok(n);
        }
    }
    Err(Error::Agent(format!("could not parse move from {t:?}")))
}
