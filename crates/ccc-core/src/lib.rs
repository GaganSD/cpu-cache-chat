//! Shared types, errors, and budgets. No I/O.

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u8 = 1;
pub const FRAME_MAGIC: [u8; 2] = [0xC3, 0xC4];
pub const MAX_PAYLOAD: usize = 256;
pub const MAX_CHAT_BYTES: usize = 200;
pub const MAX_RETRIES: u32 = 5;
pub const PREAMBLE: u32 = 0xAAAA_AAAA;
pub const DEFAULT_SLOT: Duration = Duration::from_millis(2);
pub const DEFAULT_ACK_TIMEOUT: Duration = Duration::from_secs(20);
pub const DEFAULT_SYNC_TIMEOUT: Duration = Duration::from_secs(20);
pub const DEFAULT_CALIBRATE: Duration = Duration::from_secs(3);
pub const DEFAULT_PROBE_TRIALS: u32 = 64;
pub const DEFAULT_PROBE_BITS: u32 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    A,
    B,
}

impl Role {
    pub fn peer(self) -> Self {
        match self {
            Role::A => Role::B,
            Role::B => Role::A,
        }
    }

    pub fn is_initiator(self) -> bool {
        matches!(self, Role::A)
    }

    pub fn mark(self) -> Mark {
        match self {
            Role::A => Mark::X,
            Role::B => Mark::O,
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Role::A => write!(f, "A"),
            Role::B => write!(f, "B"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelMode {
    Cache,
    Cpu,
    Sim,
}

impl ChannelMode {
    pub fn label(self) -> &'static str {
        match self {
            ChannelMode::Cache => "cache timing",
            ChannelMode::Cpu => "cpu contention",
            ChannelMode::Sim => "simulation",
        }
    }

    pub fn is_physical(self) -> bool {
        !matches!(self, ChannelMode::Sim)
    }
}

impl fmt::Display for ChannelMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppKind {
    Chat,
    Game,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mark {
    X,
    O,
}

impl Mark {
    pub fn other(self) -> Self {
        match self {
            Mark::X => Mark::O,
            Mark::O => Mark::X,
        }
    }

    pub fn as_u8(self) -> u8 {
        match self {
            Mark::X => 1,
            Mark::O => 2,
        }
    }

    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Mark::X),
            2 => Some(Mark::O),
            _ => None,
        }
    }

    pub fn glyph(self) -> char {
        match self {
            Mark::X => 'X',
            Mark::O => 'O',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkState {
    Idle,
    Calibrating,
    Connecting,
    Connected,
    Degraded,
    Failed,
    Closed,
}

impl fmt::Display for LinkState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            LinkState::Idle => "idle",
            LinkState::Calibrating => "calibrating",
            LinkState::Connecting => "connecting",
            LinkState::Connected => "connected",
            LinkState::Degraded => "degraded",
            LinkState::Failed => "failed",
            LinkState::Closed => "closed",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Delivery {
    Queued,
    Transmitting,
    AwaitingAck,
    Delivered,
    Failed,
    Unknown,
}

impl fmt::Display for Delivery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Delivery::Queued => "queued",
            Delivery::Transmitting => "transmitting",
            Delivery::AwaitingAck => "awaiting ack",
            Delivery::Delivered => "delivered",
            Delivery::Failed => "failed",
            Delivery::Unknown => "delivery unknown",
        };
        f.write_str(s)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("timeout")]
    Timeout,
    #[error("disconnected")]
    Disconnected,
    #[error("interrupted")]
    Interrupted,
    #[error("corrupt frame: {0}")]
    Corrupt(&'static str),
    #[error("stale session {found:#x} (want {want:#x})")]
    StaleSession { found: u32, want: u32 },
    #[error("duplicate seq {0}")]
    Duplicate(u16),
    #[error("retries exhausted")]
    RetriesExhausted,
    #[error("payload too large ({0} > {MAX_PAYLOAD})")]
    TooLarge(usize),
    #[error("illegal move: {0}")]
    IllegalMove(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("calibration failed: {0}")]
    Calibration(String),
    #[error("isolation policy: {0}")]
    Isolation(String),
    #[error("channel: {0}")]
    Channel(String),
    #[error("agent: {0}")]
    Agent(String),
    #[error("io: {0}")]
    Io(String),
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::TimedOut {
            Error::Timeout
        } else if e.kind() == std::io::ErrorKind::Interrupted {
            Error::Interrupted
        } else {
            Error::Io(e.to_string())
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub role: Role,
    pub channel: ChannelMode,
    pub kind: AppKind,
    pub agent: bool,
    pub plain: bool,
    pub slot: Duration,
    pub ack_timeout: Duration,
    pub sync_timeout: Duration,
    pub calibrate: Duration,
    pub max_retries: u32,
    pub sim_path: Option<String>,
    pub agent_cmd: Option<String>,
}

impl RuntimeConfig {
    pub fn new(role: Role, channel: ChannelMode, kind: AppKind) -> Self {
        Self {
            role,
            channel,
            kind,
            agent: false,
            plain: false,
            slot: DEFAULT_SLOT,
            ack_timeout: DEFAULT_ACK_TIMEOUT,
            sync_timeout: DEFAULT_SYNC_TIMEOUT,
            calibrate: DEFAULT_CALIBRATE,
            max_retries: MAX_RETRIES,
            sim_path: None,
            agent_cmd: None,
        }
    }

    pub fn validate(&self) -> Result<(), Error> {
        if self.agent && self.kind != AppKind::Game {
            return Err(Error::InvalidInput(
                "--agent is only valid for game mode".into(),
            ));
        }
        if self.channel == ChannelMode::Sim && self.sim_path.is_none() {
            return Err(Error::InvalidInput(
                "simulation transport requires --sim-path".into(),
            ));
        }
        if self.channel.is_physical() && self.sim_path.is_some() {
            return Err(Error::InvalidInput(
                "physical channel cannot use --sim-path".into(),
            ));
        }
        if self.slot < Duration::from_micros(50) {
            return Err(Error::InvalidInput("slot is too short".into()));
        }
        Ok(())
    }

    /// How long a receive poll should last. Physical hunts need many slots.
    pub fn recv_poll(&self) -> Duration {
        if self.channel.is_physical() {
            self.slot
                .saturating_mul(128)
                .max(Duration::from_millis(400))
        } else {
            Duration::from_millis(20)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationReport {
    pub mode: ChannelMode,
    pub slot: Duration,
    pub threshold: f64,
    pub ones_mean: f64,
    pub zeros_mean: f64,
    pub silent_mean: f64,
    pub separation: f64,
    pub usable: bool,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeReport {
    pub mode: ChannelMode,
    pub role: Role,
    pub trials: u32,
    pub bits: u32,
    pub bit_errors: u32,
    pub timeouts: u32,
    pub silent_false_ones: u32,
    pub usable: bool,
    pub notes: String,
}
