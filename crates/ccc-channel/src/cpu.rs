use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use ccc_core::Error;
use ccc_protocol::Wire;

use crate::phy::{Phy, PhyKind};

pub struct CpuChannel {
    phy: Phy,
}

impl CpuChannel {
    pub fn new(slot: Duration, stop: Arc<AtomicBool>) -> Self {
        Self {
            phy: Phy::new(PhyKind::Cpu, slot, stop),
        }
    }
}

impl Wire for CpuChannel {
    fn send_blob(&mut self, bytes: &[u8], timeout: Duration) -> Result<(), Error> {
        self.phy.send_blob(bytes, timeout)
    }

    fn recv_blob(&mut self, timeout: Duration) -> Result<Vec<u8>, Error> {
        self.phy.recv_blob(timeout)
    }

    fn interrupt(&mut self) {
        self.phy.interrupt();
    }
}
