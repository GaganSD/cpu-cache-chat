use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use ccc_core::{ChannelMode, Error, ProbeReport, Role};

use crate::phy::{Phy, PhyKind};

/// Send only the supplied bytes; the receiver does not need them to decode.
pub fn run_probe_sender(
    mode: ChannelMode,
    _bits: u32,
    slot: Duration,
    stop: Arc<AtomicBool>,
    payload: &[u8],
) -> Result<(), Error> {
    phy(mode, slot, stop)?.send_blob(payload, Duration::from_secs(60))
}

/// One receive attempt. Expected bytes, if available, are used only after decoding.
pub fn run_probe_receiver(
    mode: ChannelMode,
    role: Role,
    trials: u32,
    bits: u32,
    slot: Duration,
    stop: Arc<AtomicBool>,
    expected: Option<&[u8]>,
) -> Result<ProbeReport, Error> {
    let mut phy = phy(mode, slot, stop)?;
    let result = phy.recv_blob(Duration::from_secs(60));
    if matches!(result, Err(Error::Interrupted)) {
        return Err(Error::Interrupted);
    }
    let mut report = ProbeReport {
        mode, role, trials: 1, bits, bit_errors: 0, timeouts: 0,
        silent_false_ones: 0, usable: false,
        notes: format!("unproven: one receive attempt (requested {trials}); two-process operation and reliability are not established; silent-sender control not measured; "),
    };
    match result {
        Ok(bytes) => {
            report.notes.push_str("received bytes (hex): ");
            for byte in &bytes {
                report.notes.push_str(&format!("{byte:02x}"));
            }
            if let Some(exp) = expected {
                let common = bytes.len().min(exp.len());
                let errors: u64 = bytes[..common]
                    .iter()
                    .zip(&exp[..common])
                    .map(|(a, b)| (a ^ b).count_ones() as u64)
                    .sum::<u64>()
                    + (bytes.len().abs_diff(exp.len()) as u64) * 8;
                report.bit_errors = errors.min(u32::MAX as u64) as u32;
                report.notes.push_str(&format!("; compared after decoding against {} expected bits; bit errors={}; length difference counts as errors", exp.len() * 8, report.bit_errors));
                // A single matching frame is evidence, not proof of a usable link.
            } else {
                report.notes.push_str("; BER unavailable: expected bytes were not supplied; bit_errors field is not a measurement");
            }
        }
        Err(e) => {
            if matches!(e, Error::Timeout) {
                report.timeouts = 1;
            }
            report.notes.push_str(&format!(
                "receive failed: {e}; BER unavailable; bit_errors field is not a measurement"
            ));
        }
    }
    Ok(report)
}

/// Listen with the sender deliberately silent. A decoded frame is a false positive,
/// never evidence of a working link. The operator must ensure the sender is silent.
pub fn run_silent_control(
    mode: ChannelMode,
    role: Role,
    slot: Duration,
    stop: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<ProbeReport, Error> {
    let mut phy = phy(mode, slot, stop)?;
    let result = phy.recv_blob(timeout);
    if matches!(result, Err(Error::Interrupted)) {
        return Err(Error::Interrupted);
    }
    let false_frame = result.is_ok();
    Ok(ProbeReport {
        mode, role, trials: 1, bits: 0, bit_errors: 0,
        timeouts: u32::from(matches!(result, Err(Error::Timeout))),
        silent_false_ones: 0, usable: false,
        notes: format!("unproven: silent-sender control; sender silence must be ensured independently; false decoded frame={false_frame}; result={result:?}; silent_false_ones and bit_errors are not measured"),
    })
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
