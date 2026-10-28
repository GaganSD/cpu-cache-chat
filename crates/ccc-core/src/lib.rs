//! Shared types, errors, and budgets. No I/O.

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u8 = 2;
pub const FRAME_MAGIC: [u8; 2] = [0xC3, 0xC4];
pub const MAX_PAYLOAD: usize = 256;
pub const MAX_CHAT_BYTES: usize = 200;
pub const MAX_RETRIES: u32 = 5;
pub const PREAMBLE: u32 = 0xAAAA_AAAA;
pub const DEFAULT_SLOT: Duration = Duration::from_millis(50);
pub const MIN_SLOT: Duration = Duration::from_micros(50);
pub const MAX_SLOT: Duration = Duration::from_secs(1);
pub const DEFAULT_ACK_TIMEOUT: Duration = Duration::from_secs(45);
pub const DEFAULT_SYNC_TIMEOUT: Duration = Duration::from_secs(90);
pub const DEFAULT_CALIBRATE: Duration = Duration::from_secs(3);
pub const DEFAULT_PROBE_TRIALS: u32 = 1;
pub const MAX_PHY_BYTES: usize = MAX_PAYLOAD + 64;
pub const DEFAULT_PROBE_BITS: u32 = 64;
/// Physical slots outside the encoded protocol blob: gap (4), carrier (32),
/// training gap (8), preamble (32), two FEC length bytes (26), four FEC CRC
/// bytes (52), and gap (2). Interleaving adds no padding.
pub const PHYSICAL_FRAME_OVERHEAD_SLOTS: u32 = 156;
/// Each length, payload, and CRC byte is encoded as a 13-bit SECDED word.
pub const PHYSICAL_CODEWORD_SLOTS: u32 = 13;

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
        validate_slot(self.slot)?;
        if self.ack_timeout.is_zero()
            || self.sync_timeout.is_zero()
            || self.calibrate.is_zero()
            || self.ack_timeout > Duration::from_secs(86400)
            || self.sync_timeout > Duration::from_secs(86400)
            || self.calibrate > Duration::from_secs(86400)
        {
            return Err(Error::InvalidInput(
                "timeouts and calibration must be between zero and 24 hours".into(),
            ));
        }
        if self
            .agent_cmd
            .as_ref()
            .is_some_and(|s| s.trim().is_empty() || s.len() > 4096 || s.contains('\0'))
        {
            return Err(Error::InvalidInput(
                "agent command must be nonempty, at most 4096 bytes, and contain no NUL".into(),
            ));
        }
        if self.agent_cmd.is_some() && !self.agent {
            return Err(Error::InvalidInput("--agent-cmd requires --agent".into()));
        }
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

    /// Estimated physical transmission time, including framing. Calculations
    /// saturate rather than wrap for invalid or extreme configurations.
    pub fn physical_frame_duration(&self, encoded_bytes: usize) -> Duration {
        let data_slots = u32::try_from(encoded_bytes)
            .unwrap_or(u32::MAX)
            .saturating_mul(PHYSICAL_CODEWORD_SLOTS);
        self.slot
            .saturating_mul(PHYSICAL_FRAME_OVERHEAD_SLOTS.saturating_add(data_slots))
    }

    /// Budget a single send from its actual encoded length. The margin covers
    /// scheduling and turn-around, not additional frame retries.
    pub fn send_budget(&self, encoded_bytes: usize) -> Duration {
        if !self.channel.is_physical() {
            return self.ack_timeout;
        }
        let duration = self.physical_frame_duration(encoded_bytes);
        let margin = (duration / 4).saturating_add(self.slot.saturating_mul(8));
        self.ack_timeout.max(
            duration
                .saturating_add(margin)
                .saturating_add(Duration::from_secs(1)),
        )
    }

    /// Give the peer time to transmit a complete control response after the
    /// preceding send has finished. Simulation keeps its configured timeout.
    pub fn response_budget(&self, control_bytes: usize) -> Duration {
        if !self.channel.is_physical() {
            return self.ack_timeout;
        }
        self.send_budget(control_bytes)
            .saturating_add(self.slot.saturating_mul(8))
            .saturating_add(Duration::from_secs(1))
    }

    /// An interrupted physical frame may have been acquired by the receiver,
    /// which must finish or abandon it before another transmission begins.
    pub fn failed_send_recovery_budget(
        &self,
        encoded_bytes: usize,
        control_bytes: usize,
    ) -> Duration {
        let response = self.response_budget(control_bytes);
        if self.channel.is_physical() {
            response.saturating_add(self.send_budget(encoded_bytes))
        } else {
            response
        }
    }

    /// A responder may start listening before its peer begins transmitting.
    pub fn sync_listen_budget(&self, control_bytes: usize) -> Duration {
        if !self.channel.is_physical() {
            return self.sync_timeout;
        }
        self.sync_timeout
            .max(self.response_budget(control_bytes).saturating_mul(2))
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

pub fn validate_slot(slot: Duration) -> Result<(), Error> {
    if !(MIN_SLOT..=MAX_SLOT).contains(&slot) {
        return Err(Error::InvalidInput(
            "slot must be between 50 microseconds and 1 second".into(),
        ));
    }
    Ok(())
}

pub const PROBE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProbeOutcome {
    Decoded,
    Timeout,
    Corrupt,
    Error,
    SilentNoFrame,
    SilentFrame,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeTrial {
    pub outcome: ProbeOutcome,
    pub decoded_bytes: Option<usize>,
    pub decoded_hex: Option<String>,
    /// True only for a decoded frame; CRC failures are reported as false.
    pub crc_valid: Option<bool>,
    /// Post-decode comparison, not raw carrier BER.
    pub payload_bit_errors: Option<u32>,
    pub elapsed: Duration,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeReport {
    pub schema_version: u32,
    pub mode: ChannelMode,
    pub role: Role,
    pub requested_trials: u32,
    pub attempted_trials: u32,
    pub completed_trials: u32,
    pub requested_bits: u32,
    pub elapsed: Duration,
    pub trials: Vec<ProbeTrial>,
    /// Raw carrier BER and false-one rate are not measured by frame decoding.
    pub raw_bit_errors: Option<u64>,
    pub silent_false_ones: Option<u64>,
    pub usable: bool,
    pub notes: String,
}

impl ProbeReport {
    pub fn has_failures(&self) -> bool {
        self.trials.iter().any(|t| {
            !matches!(
                t.outcome,
                ProbeOutcome::Decoded | ProbeOutcome::SilentNoFrame
            ) || t.error.is_some()
                || t.payload_bit_errors.is_some_and(|n| n != 0)
        })
    }
}
