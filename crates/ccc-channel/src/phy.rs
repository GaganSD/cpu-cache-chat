use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use ccc_core::{Error, PREAMBLE};

use crate::timing::measure;

const CARRIER_ONES: u32 = 8;
const PREAMBLE_BITS: usize = 32;
const TRAINING_BITS: usize = CARRIER_ONES as usize + PREAMBLE_BITS;

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
    decoder: Option<Decoder>,
    next_slot: Option<Instant>,
}

struct CacheWork {
    data: Vec<u64>,
    stride: usize,
    cursor: usize,
}

impl Phy {
    pub fn new(kind: PhyKind, slot: Duration, stop: Arc<AtomicBool>) -> Self {
        let bytes = match kind {
            PhyKind::Cache => 8 * 1024 * 1024,
            PhyKind::Cpu => 0,
        };
        let n = bytes / 8;
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
                cursor: 0,
            },
            cpu_iters: 50_000,
            decoder: None,
            next_slot: None,
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
        let len = u16::try_from(bytes.len()).map_err(|_| Error::TooLarge(bytes.len()))?;
        self.next_slot = None;
        // Inter-frame gap.
        self.idle_slots(4, deadline)?;
        for _ in 0..CARRIER_ONES {
            self.slot_activity(deadline)?;
        }
        self.send_u32(PREAMBLE, deadline)?;
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
        self.decoder = None;
        self.next_slot = None;
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

    // Sliding windows require a carrier followed by a complete alternating preamble.
    // Each candidate trains two clusters from its own samples, so no absolute
    // timing threshold or assumed direction of contention is required.
    fn hunt_preamble(&mut self, deadline: Instant) -> Result<(), Error> {
        let mut samples = std::collections::VecDeque::with_capacity(TRAINING_BITS);
        loop {
            let cost = self.sample_cost(deadline)?;
            if samples.len() == TRAINING_BITS {
                samples.pop_front();
            }
            samples.push_back(cost);
            if samples.len() == TRAINING_BITS {
                let candidate: Vec<_> = samples.iter().copied().collect();
                if let Some(decoder) = train_preamble(&candidate) {
                    self.decoder = Some(decoder);
                    return Ok(());
                }
            }
        }
    }

    fn send_u32(&mut self, v: u32, deadline: Instant) -> Result<(), Error> {
        for bit in bits_msb(v, 32) {
            self.send_bit(bit, deadline)?;
        }
        Ok(())
    }

    fn send_u16(&mut self, v: u16, deadline: Instant) -> Result<(), Error> {
        for bit in bits_msb(u32::from(v), 16) {
            self.send_bit(bit, deadline)?;
        }
        Ok(())
    }

    fn send_u8(&mut self, v: u8, deadline: Instant) -> Result<(), Error> {
        for bit in bits_msb(u32::from(v), 8) {
            self.send_bit(bit, deadline)?;
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
        let cost = self.sample_cost(deadline)?;
        let decoder = self.decoder.ok_or(Error::Corrupt("untrained receiver"))?;
        Ok((decoder.decode(cost), cost))
    }

    fn sample_cost(&mut self, deadline: Instant) -> Result<Duration, Error> {
        self.check(deadline)?;
        let end = self.slot_end(deadline)?;
        let cost = self.observe();
        if Instant::now() > end {
            self.check(deadline)?;
            return Err(Error::Channel(
                "probe exceeded slot; increase slot duration".into(),
            ));
        }
        self.finish_slot(end, deadline)?;
        Ok(cost)
    }

    fn slot_activity(&mut self, deadline: Instant) -> Result<(), Error> {
        self.check(deadline)?;
        let end = self.slot_end(deadline)?;
        while Instant::now() < end {
            self.check(deadline)?;
            self.activity();
        }
        self.check(deadline)
    }

    fn idle_slots(&mut self, n: u32, deadline: Instant) -> Result<(), Error> {
        for _ in 0..n {
            self.check(deadline)?;
            let end = self.slot_end(deadline)?;
            self.finish_slot(end, deadline)?;
        }
        Ok(())
    }

    fn slot_end(&mut self, deadline: Instant) -> Result<Instant, Error> {
        self.check(deadline)?;
        if self.slot.is_zero() {
            return Err(Error::InvalidInput("slot must be nonzero".into()));
        }
        let start = self.next_slot.unwrap_or_else(Instant::now);
        let end = start + self.slot;
        if Instant::now() >= end {
            return Err(Error::Channel("missed slot; increase slot duration".into()));
        }
        self.next_slot = Some(end);
        Ok(end)
    }

    fn finish_slot(&self, end: Instant, deadline: Instant) -> Result<(), Error> {
        loop {
            self.check(deadline)?;
            let now = Instant::now();
            if now >= end {
                return Ok(());
            }
            let remaining = end
                .saturating_duration_since(now)
                .min(deadline.saturating_duration_since(now));
            if remaining > Duration::from_micros(100) {
                thread::sleep(
                    (remaining - Duration::from_micros(50)).min(Duration::from_millis(1)),
                );
            } else {
                std::hint::spin_loop();
            }
        }
    }

    fn activity(&mut self) {
        match self.kind {
            PhyKind::Cache => {
                walk_chunk(&mut self.cache);
            }
            PhyKind::Cpu => {
                burn(1_000);
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

// Keep each activity batch short while cycling across the complete occupancy array.
fn walk_chunk(work: &mut CacheWork) {
    let mut sum = 0u64;
    for _ in 0..256 {
        sum = sum.wrapping_add(unsafe { core::ptr::read_volatile(&work.data[work.cursor]) });
        work.cursor = (work.cursor + work.stride) % work.data.len();
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

pub fn crc32_of(bytes: &[u8]) -> u32 {
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

/// A threshold and the direction in which an active slot lies.
#[derive(Clone, Copy, Debug)]
pub struct Decoder {
    threshold: Duration,
    active_high: bool,
}

impl Decoder {
    pub fn decode(self, sample: Duration) -> bool {
        (sample > self.threshold) == self.active_high
    }
}

/// Train from eight carrier slots followed by the 32-bit alternating preamble.
/// Reject overlapping clusters and candidates that do not match the complete pattern.
pub fn train_preamble(samples: &[Duration]) -> Option<Decoder> {
    if samples.len() != TRAINING_BITS {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let mut best = None;
    let mut largest_gap = Duration::ZERO;
    for i in 1..sorted.len() {
        let gap = sorted[i] - sorted[i - 1];
        if gap > largest_gap {
            largest_gap = gap;
            best = Some(i);
        }
    }
    let split = best?;
    // Reject tiny gaps that would turn ordinary timer jitter into a carrier.
    if largest_gap < sorted[split - 1] / 10 || largest_gap < Duration::from_nanos(1) {
        return None;
    }
    let threshold = sorted[split - 1] + largest_gap / 2;
    let active_high = samples[0] > threshold;
    let decoder = Decoder {
        threshold,
        active_high,
    };
    if samples[..CARRIER_ONES as usize]
        .iter()
        .any(|&s| !decoder.decode(s))
    {
        return None;
    }
    for (i, &sample) in samples[CARRIER_ONES as usize..].iter().enumerate() {
        let expected = (PREAMBLE >> (31 - i)) & 1 != 0;
        if decoder.decode(sample) != expected {
            return None;
        }
    }
    Some(decoder)
}

/// Bits in wire order, most significant first.
pub fn bits_msb(value: u32, width: u32) -> impl Iterator<Item = bool> {
    (0..width.min(32)).rev().map(move |i| (value >> i) & 1 != 0)
}
