use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use ccc_core::{Error, PREAMBLE};

use crate::timing::measure;

const CARRIER_ONES: u32 = 8;
const PREAMBLE_BITS: u32 = 32;

#[derive(Clone, Copy, Debug)]
pub enum PhyKind {
    Cache,
    Cpu,
}

pub struct Phy {
    kind: PhyKind,
    slot: Duration,
    stop: Arc<AtomicBool>,
    cache: CacheWork,
    cpu_iters: u32,
}

struct CacheWork {
    data: Vec<u64>,
    stride: usize,
}

impl Phy {
    pub fn new(kind: PhyKind, slot: Duration, stop: Arc<AtomicBool>) -> Self {
        let bytes = match kind {
            PhyKind::Cache => 8 * 1024 * 1024,
            PhyKind::Cpu => 4096,
        };
        let n = (bytes / 8).max(64);
        Self {
            kind,
            slot,
            stop,
            cache: CacheWork {
                data: vec![0xA5A5_A5A5_A5A5_A5A5; n],
                stride: match kind {
                    PhyKind::Cache => 8,
                    PhyKind::Cpu => 1,
                },
            },
            cpu_iters: 50_000,
        }
    }

    pub fn kind(&self) -> PhyKind {
        self.kind
    }

    pub fn slot(&self) -> Duration {
        self.slot
    }

    pub fn interrupt(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    pub fn send_blob(&mut self, bytes: &[u8], timeout: Duration) -> Result<(), Error> {
        let deadline = Instant::now() + timeout;
        self.check(deadline)?;
        // Inter-frame gap.
        self.idle_slots(4, deadline)?;
        for _ in 0..CARRIER_ONES {
            self.slot_activity(deadline)?;
        }
        self.send_u32(PREAMBLE, deadline)?;
        let len = u16::try_from(bytes.len()).map_err(|_| Error::TooLarge(bytes.len()))?;
        self.send_u16(len, deadline)?;
        for &b in bytes {
            self.send_u8(b, deadline)?;
        }
        let crc = crc32_of(bytes);
        self.send_u32(crc, deadline)?;
        self.idle_slots(2, deadline)
    }

    pub fn recv_blob(&mut self, timeout: Duration) -> Result<Vec<u8>, Error> {
        let deadline = Instant::now() + timeout;
        self.hunt_preamble(deadline)?;
        let len = self.recv_u16(deadline)? as usize;
        if len > ccc_core::MAX_PAYLOAD + 64 {
            return Err(Error::TooLarge(len));
        }
        let mut bytes = vec![0u8; len];
        for slot in &mut bytes {
            *slot = self.recv_u8(deadline)?;
        }
        let crc = self.recv_u32(deadline)?;
        if crc != crc32_of(&bytes) {
            return Err(Error::Corrupt("channel crc"));
        }
        Ok(bytes)
    }

    pub fn sample_bit(&mut self) -> (bool, Duration) {
        let cost = self.observe();
        // High cost means the sender is active (contention / occupancy).
        (cost > self.threshold(), cost)
    }

    pub fn local_ones_zeros(&mut self, n: u32) -> (f64, f64, f64) {
        let mut ones = Vec::new();
        let mut zeros = Vec::new();
        let mut silent = Vec::new();
        for _ in 0..n {
            ones.push(self.activity_cost().as_secs_f64() * 1e9);
            zeros.push(self.idle_cost().as_secs_f64() * 1e9);
            silent.push(self.observe().as_secs_f64() * 1e9);
        }
        (mean(&ones), mean(&zeros), mean(&silent))
    }

    fn threshold(&self) -> Duration {
        // Conservative default: anything clearly slower than a quiet observe.
        self.slot / 8
    }

    fn hunt_preamble(&mut self, deadline: Instant) -> Result<(), Error> {
        // Wait for carrier (several consecutive ones), then collect 32 bits
        // and accept PREAMBLE or its inverse.
        let mut ones = 0u32;
        while ones < CARRIER_ONES {
            self.check(deadline)?;
            let (bit, _) = self.sample_slot(deadline)?;
            if bit {
                ones += 1;
            } else {
                ones = 0;
            }
        }
        let mut window = 0u32;
        for _ in 0..PREAMBLE_BITS {
            let (bit, _) = self.sample_slot(deadline)?;
            window = (window << 1) | u32::from(bit);
        }
        if window == PREAMBLE || window == !PREAMBLE {
            return Ok(());
        }
        // One more sliding hunt over a limited window.
        for _ in 0..PREAMBLE_BITS * 2 {
            let (bit, _) = self.sample_slot(deadline)?;
            window = (window << 1) | u32::from(bit);
            if window == PREAMBLE || window == !PREAMBLE {
                return Ok(());
            }
        }
        Err(Error::Corrupt("preamble"))
    }

    fn send_u32(&mut self, v: u32, deadline: Instant) -> Result<(), Error> {
        for i in (0..32).rev() {
            self.send_bit(((v >> i) & 1) == 1, deadline)?;
        }
        Ok(())
    }

    fn send_u16(&mut self, v: u16, deadline: Instant) -> Result<(), Error> {
        for i in (0..16).rev() {
            self.send_bit(((v >> i) & 1) == 1, deadline)?;
        }
        Ok(())
    }

    fn send_u8(&mut self, v: u8, deadline: Instant) -> Result<(), Error> {
        for i in (0..8).rev() {
            self.send_bit(((v >> i) & 1) == 1, deadline)?;
        }
        Ok(())
    }

    fn recv_u32(&mut self, deadline: Instant) -> Result<u32, Error> {
        let mut v = 0u32;
        for _ in 0..32 {
            let (bit, _) = self.sample_slot(deadline)?;
            v = (v << 1) | u32::from(bit);
        }
        Ok(v)
    }

    fn recv_u16(&mut self, deadline: Instant) -> Result<u16, Error> {
        Ok(self.recv_u32_n(16, deadline)? as u16)
    }

    fn recv_u8(&mut self, deadline: Instant) -> Result<u8, Error> {
        Ok(self.recv_u32_n(8, deadline)? as u8)
    }

    fn recv_u32_n(&mut self, n: u32, deadline: Instant) -> Result<u32, Error> {
        let mut v = 0u32;
        for _ in 0..n {
            let (bit, _) = self.sample_slot(deadline)?;
            v = (v << 1) | u32::from(bit);
        }
        Ok(v)
    }

    fn send_bit(&mut self, one: bool, deadline: Instant) -> Result<(), Error> {
        if one {
            self.slot_activity(deadline)
        } else {
            self.idle_slots(1, deadline)
        }
    }

    fn sample_slot(&mut self, deadline: Instant) -> Result<(bool, Duration), Error> {
        self.check(deadline)?;
        let start = Instant::now();
        let cost = self.observe();
        let bit = cost > self.threshold();
        self.finish_slot(start);
        Ok((bit, cost))
    }

    fn slot_activity(&mut self, deadline: Instant) -> Result<(), Error> {
        self.check(deadline)?;
        let start = Instant::now();
        while start.elapsed() < self.slot {
            self.check(deadline)?;
            self.activity();
        }
        Ok(())
    }

    fn idle_slots(&mut self, n: u32, deadline: Instant) -> Result<(), Error> {
        for _ in 0..n {
            self.check(deadline)?;
            thread::sleep(self.slot);
        }
        Ok(())
    }

    fn finish_slot(&self, start: Instant) {
        let used = start.elapsed();
        if used < self.slot {
            thread::sleep(self.slot - used);
        }
    }

    fn activity(&mut self) {
        match self.kind {
            PhyKind::Cache => {
                walk(&mut self.cache);
            }
            PhyKind::Cpu => {
                burn(self.cpu_iters);
            }
        }
    }

    fn observe(&mut self) -> Duration {
        match self.kind {
            PhyKind::Cache => measure(|| {
                walk(&mut self.cache);
            }),
            PhyKind::Cpu => measure(|| {
                burn(self.cpu_iters);
            }),
        }
    }

    fn activity_cost(&mut self) -> Duration {
        let start = Instant::now();
        self.activity();
        start.elapsed()
    }

    fn idle_cost(&mut self) -> Duration {
        let start = Instant::now();
        thread::sleep(Duration::from_micros(20));
        start.elapsed()
    }

    fn check(&self, deadline: Instant) -> Result<(), Error> {
        if self.stopped() {
            return Err(Error::Interrupted);
        }
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        Ok(())
    }
}

fn walk(work: &mut CacheWork) {
    let mut sum = 0u64;
    let mut i = 0;
    while i < work.data.len() {
        sum = sum.wrapping_add(unsafe { core::ptr::read_volatile(&work.data[i]) });
        i += work.stride;
    }
    core::hint::black_box(sum);
}

fn burn(iters: u32) {
    let mut x = 0x1234_5678_9ABCu64;
    for _ in 0..iters {
        x = x.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
        x ^= x >> 17;
    }
    core::hint::black_box(x);
}

fn crc32_of(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        0.0
    } else {
        xs.iter().sum::<f64>() / xs.len() as f64
    }
}
