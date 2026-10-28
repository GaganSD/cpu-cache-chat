use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::phy::{Phy, PhyKind};
use ccc_core::{
    validate_slot, AppKind, ChannelMode, Error, ProbeOutcome, ProbeReport, ProbeTrial, Role,
    RuntimeConfig, MAX_PHY_BYTES, PROBE_SCHEMA_VERSION,
};
use rand::RngCore;

/// Validate exact byte-aligned probe size (one through MAX_PHY_BYTES).
pub fn probe_payload_len(bits: u32) -> Result<usize, Error> {
    if bits == 0 || !bits.is_multiple_of(8) || bits > (MAX_PHY_BYTES as u32) * 8 {
        return Err(Error::InvalidInput(format!(
            "probe bits must be byte-aligned and between 8 and {}",
            MAX_PHY_BYTES * 8
        )));
    }
    Ok((bits / 8) as usize)
}

/// Generate sender-only random probe bytes; share separately only for post-decode comparison.
pub fn random_probe_payload(bits: u32) -> Result<Vec<u8>, Error> {
    let mut bytes = vec![0; probe_payload_len(bits)?];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|e| Error::Channel(format!("random payload: {e}")))?;
    Ok(bytes)
}

fn budget(mode: ChannelMode, slot: Duration, bytes: usize) -> Duration {
    let mut cfg = RuntimeConfig::new(Role::A, mode, AppKind::Chat);
    cfg.slot = slot;
    cfg.send_budget(bytes)
}

pub fn run_probe_sender(
    mode: ChannelMode,
    bits: u32,
    slot: Duration,
    stop: Arc<AtomicBool>,
    payload: &[u8],
) -> Result<(), Error> {
    validate_slot(slot)?;
    if payload.len() != probe_payload_len(bits)? {
        return Err(Error::InvalidInput(
            "payload length must exactly match probe bits".into(),
        ));
    }
    phy(mode, slot, stop)?.send_blob(payload, budget(mode, slot, payload.len()))
}

/// One bounded receive attempt; a CRC-passing payload is not a raw BER measurement.
pub fn run_probe_receiver(
    mode: ChannelMode,
    role: Role,
    trials: u32,
    bits: u32,
    slot: Duration,
    stop: Arc<AtomicBool>,
    expected: Option<&[u8]>,
) -> Result<ProbeReport, Error> {
    validate_slot(slot)?;
    if trials != 1 {
        return Err(Error::InvalidInput(
            "probe supports exactly one trial; use --trials 1".into(),
        ));
    }
    let len = probe_payload_len(bits)?;
    if expected.is_some_and(|e| e.len() != len) {
        return Err(Error::InvalidInput(
            "expected payload length must match probe bits".into(),
        ));
    }
    let mut phy = phy(mode, slot, stop)?;
    let start = Instant::now();
    let result = phy.recv_blob(budget(mode, slot, len).saturating_mul(2));
    if matches!(result, Err(Error::Interrupted)) {
        return Err(Error::Interrupted);
    }
    let mut trial = trial(result, start.elapsed(), expected, false);
    if trial.decoded_bytes.is_some_and(|actual| actual != len) {
        trial.error = Some(format!(
            "decoded payload length differs from requested {len} bytes"
        ));
    }
    Ok(report(
        mode,
        role,
        bits,
        trial,
        start.elapsed(),
        "One frame observation; raw BER and link reliability are not established.",
    ))
}

/// Observe for at least two full default-sized frame windows. Sender silence must
/// be independently controlled. A decoded frame is a false positive.
pub fn run_silent_control(
    mode: ChannelMode,
    role: Role,
    slot: Duration,
    stop: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<ProbeReport, Error> {
    validate_slot(slot)?;
    let mut phy = phy(mode, slot, stop)?;
    let mut cfg = RuntimeConfig::new(role, mode, AppKind::Chat);
    cfg.slot = slot;
    let window = timeout.max(
        cfg.physical_frame_duration((ccc_core::DEFAULT_PROBE_BITS / 8) as usize)
            .saturating_mul(2),
    );
    let start = Instant::now();
    let result = phy.recv_blob(window);
    if matches!(result, Err(Error::Interrupted)) {
        return Err(Error::Interrupted);
    }
    let trial = trial(result, start.elapsed(), None, true);
    Ok(report(mode, role, 0, trial, start.elapsed(), &format!("Silent control window {window:?}; sender silence must be independently ensured. Interference is not separately classified.")))
}

fn trial(
    result: Result<Vec<u8>, Error>,
    elapsed: Duration,
    expected: Option<&[u8]>,
    silent: bool,
) -> ProbeTrial {
    match result {
        Ok(bytes) => {
            let errors = expected.map(|exp| {
                bytes
                    .iter()
                    .zip(exp)
                    .map(|(a, b)| (a ^ b).count_ones())
                    .sum::<u32>()
                    + (bytes.len().abs_diff(exp.len()) as u32) * 8
            });
            ProbeTrial {
                outcome: if silent {
                    ProbeOutcome::SilentFrame
                } else {
                    ProbeOutcome::Decoded
                },
                decoded_bytes: Some(bytes.len()),
                decoded_hex: Some(bytes.iter().map(|b| format!("{b:02x}")).collect()),
                crc_valid: Some(true),
                payload_bit_errors: errors,
                elapsed,
                error: None,
            }
        }
        Err(e) => {
            let outcome = match e {
                Error::Timeout if silent => ProbeOutcome::SilentNoFrame,
                Error::Timeout => ProbeOutcome::Timeout,
                Error::Corrupt(_) => ProbeOutcome::Corrupt,
                _ => ProbeOutcome::Error,
            };
            let crc_valid = if matches!(e, Error::Corrupt("channel crc")) {
                Some(false)
            } else {
                None
            };
            ProbeTrial {
                outcome,
                decoded_bytes: None,
                decoded_hex: None,
                crc_valid,
                payload_bit_errors: None,
                elapsed,
                error: if outcome == ProbeOutcome::SilentNoFrame {
                    None
                } else {
                    Some(e.to_string())
                },
            }
        }
    }
}
fn report(
    mode: ChannelMode,
    role: Role,
    bits: u32,
    trial: ProbeTrial,
    elapsed: Duration,
    notes: &str,
) -> ProbeReport {
    let completed = u32::from(matches!(
        trial.outcome,
        ProbeOutcome::Decoded | ProbeOutcome::SilentFrame | ProbeOutcome::SilentNoFrame
    ));
    ProbeReport {
        schema_version: PROBE_SCHEMA_VERSION,
        mode,
        role,
        requested_trials: 1,
        attempted_trials: 1,
        completed_trials: completed,
        requested_bits: bits,
        elapsed,
        trials: vec![trial],
        raw_bit_errors: None,
        silent_false_ones: None,
        usable: false,
        notes: notes.into(),
    }
}
fn phy(mode: ChannelMode, slot: Duration, stop: Arc<AtomicBool>) -> Result<Phy, Error> {
    let kind = match mode {
        ChannelMode::Cache => PhyKind::Cache,
        ChannelMode::Cpu => PhyKind::Cpu,
        ChannelMode::Sim => {
            return Err(Error::InvalidInput(
                "probe on sim is not a hardware measurement".into(),
            ))
        }
    };
    Ok(Phy::new(kind, slot, stop))
}
