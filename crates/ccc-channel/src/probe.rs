use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use ccc_core::{ChannelMode, Error, ProbeReport, Role};

use crate::phy::{Phy, PhyKind};

pub fn run_probe_sender(
    mode: ChannelMode,
    bits: u32,
    slot: Duration,
    stop: Arc<AtomicBool>,
    payload: &[u8],
) -> Result<(), Error> {
    let mut phy = phy(mode, slot, stop)?;
    // Known then unknown payload. Receiver must not be told the unknown bits.
    phy.send_blob(payload, Duration::from_secs(60))?;
    let _ = bits;
    Ok(())
}

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
    let (ones, zeros, silent) = phy.local_ones_zeros(16);
    let mut timeouts = 0;
    let mut bit_errors = 0;
    let mut got: Option<Vec<u8>> = None;
    match phy.recv_blob(Duration::from_secs(60)) {
        Ok(bytes) => got = Some(bytes),
        Err(Error::Timeout) => timeouts += 1,
        Err(e) => {
            return Ok(ProbeReport {
                mode,
                role,
                trials,
                bits,
                bit_errors: bits,
                timeouts: 1,
                silent_false_ones: 0,
                usable: false,
                notes: format!("recv failed: {e}; local ones={ones:.0}ns zeros={zeros:.0}ns silent={silent:.0}ns"),
            });
        }
    }
    if let (Some(got), Some(exp)) = (got.as_deref(), expected) {
        let n = exp.len().max(got.len());
        for i in 0..n {
            let a = got.get(i).copied().unwrap_or(0);
            let b = exp.get(i).copied().unwrap_or(0);
            bit_errors += (a ^ b).count_ones();
        }
    }
    let usable = timeouts == 0
        && expected
            .map(|exp| got.as_deref() == Some(exp))
            .unwrap_or(false);
    Ok(ProbeReport {
        mode,
        role,
        trials,
        bits,
        bit_errors,
        timeouts,
        silent_false_ones: 0,
        usable,
        notes: format!(
            "local ones={ones:.0}ns zeros={zeros:.0}ns silent={silent:.0}ns; received={} bytes; this is a single trial, not a reliability claim",
            got.as_ref().map(|g| g.len()).unwrap_or(0)
        ),
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
