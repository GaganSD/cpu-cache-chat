//! Physical and labeled-simulation transports.

mod cache;
mod cpu;
mod phy;
mod probe;
mod sim;
mod timing;

pub use cache::CacheChannel;
pub use cpu::CpuChannel;
pub use phy::{Phy, PhyKind};
pub use probe::{run_probe_receiver, run_probe_sender};
pub use sim::{SimPair, SimSocket, SimWire};
pub use timing::{ticks, Timer};

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use ccc_core::{ChannelMode, Error, RuntimeConfig};
use ccc_protocol::Wire;

pub fn open_wire(cfg: &RuntimeConfig, stop: Arc<AtomicBool>) -> Result<Box<dyn Wire>, Error> {
    match cfg.channel {
        ChannelMode::Sim => {
            let path = cfg
                .sim_path
                .as_ref()
                .ok_or_else(|| Error::InvalidInput("missing --sim-path".into()))?;
            Ok(Box::new(SimSocket::bind(cfg.role, path, stop)?))
        }
        ChannelMode::Cache => Ok(Box::new(CacheChannel::new(cfg.slot, stop))),
        ChannelMode::Cpu => Ok(Box::new(CpuChannel::new(cfg.slot, stop))),
    }
}

pub fn open_phy(mode: ChannelMode, slot: Duration, stop: Arc<AtomicBool>) -> Result<Phy, Error> {
    match mode {
        ChannelMode::Cache => Ok(Phy::new(PhyKind::Cache, slot, stop)),
        ChannelMode::Cpu => Ok(Phy::new(PhyKind::Cpu, slot, stop)),
        ChannelMode::Sim => Err(Error::InvalidInput(
            "sim transport is not a physical PHY".into(),
        )),
    }
}
