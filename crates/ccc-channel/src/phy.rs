use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use ccc_core::{Error, PREAMBLE};

use crate::timing::measure;

const CARRIER_ONES: u32 = 8;
const LOCK_ONES: u32 = 32;
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
            cpu_iters: 8_000,
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

    /// Role A: 32 busy slots then 32 idle. Role B: sample 64 slot costs.
    pub fn bench_pattern(&mut self, send: bool) -> Result<Vec<Duration>, Error> {
        let deadline = Instant::now() + Duration::from_secs(30);
        self.next_slot = None;
        if send {
            thread::sleep(Duration::from_millis(400));
            self.idle_slots(8, deadline)?;
            for _ in 0..64 {
                self.slot_activity(deadline)?;
            }
            self.idle_slots(64, deadline)?;
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            out.push(self.sample_cost(deadline)?);
        }
        Ok(out)
    }

    pub fn send_blob(&mut self, bytes: &[u8], timeout: Duration) -> Result<(), Error> {
        let deadline = Instant::now() + timeout;
        self.check(deadline)?;
        let len = u16::try_from(bytes.len()).map_err(|_| Error::TooLarge(bytes.len()))?;
        self.next_slot = None;
        if std::env::var_os("CCC_DEBUG").is_some() {
            eprintln!("phy send {} bytes", bytes.len());
        }
        // Inter-frame gap.
        self.idle_slots(4, deadline)?;
        for _ in 0..LOCK_ONES {
            self.slot_activity(deadline)?;
        }
        self.idle_slots(8, deadline)?;
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
        if self.decoder.is_none() {
            self.hunt_preamble(deadline)?;
        }
        let result = self.read_trained_frame(deadline);
        match &result {
            Ok(_) => {
                self.decoder = None;
            }
            Err(Error::Timeout) if self.decoder.is_none() => {}
            Err(_) => {
                self.decoder = None;
                self.next_slot = None;
            }
        }
        result
    }

    fn read_trained_frame(&mut self, deadline: Instant) -> Result<Vec<u8>, Error> {
        let len = self.recv_u16(deadline)? as usize;
        if len > ccc_core::MAX_PAYLOAD + 64 {
            return Err(Error::Corrupt("bad length"));
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

    // Oversample the channel, looking for the complete preamble at every
    // quarter-slot offset. This is iterative so repeated noise cannot grow the stack.
    fn hunt_preamble(&mut self, deadline: Instant) -> Result<(), Error> {
        let quantum = self.slot / 4;
        if quantum.is_zero() {
            return Err(Error::InvalidInput("slot too short to oversample".into()));
        }
        let mut baseline = Duration::MAX;
        for _ in 0..8 {
            let end = Instant::now() + quantum;
            baseline = baseline.min(self.integrate_until(end, deadline)?);
        }
        let threshold = baseline
            .saturating_mul(2)
            .saturating_add(Duration::from_micros(5));
        let decoder = Decoder {
            threshold,
            active_high: true,
        };
        let mut windows = [0u128; 4];
        let mut counts = [0u32; 4];
        let mut phase = 0usize;
        loop {
            let end = Instant::now() + quantum;
            let sample = self.integrate_until(end, deadline)?;
            windows[phase] = (windows[phase] << 1) | u128::from(decoder.decode(sample));
            counts[phase] = counts[phase].saturating_add(1);
            if counts[phase] >= LOCK_ONES + 8 + PREAMBLE_BITS as u32 {
                let hamming = ((windows[phase] as u32) ^ PREAMBLE).count_ones();
                if std::env::var_os("CCC_DEBUG").is_some() && counts[phase] % 32 == 0 {
                    eprintln!(
                        "phy hunt window={:#010x} hamming={hamming} threshold={threshold:?}",
                        windows[phase]
                    );
                }
                if matches_training(windows[phase]) {
                    // The last quarter-slot sample is centred one eighth of a
                    // slot before now; centre the next integral one slot later.
                    self.next_slot = Some(Instant::now() + self.slot * 3 / 8);
                    self.decoder = Some(decoder);
                    return Ok(());
                }
            }
            phase = (phase + 1) % 4;
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
        if let Some(start) = self.next_slot {
            self.finish_slot(start, deadline)?;
        }
        let end = self.slot_end(deadline)?;
        self.integrate_until(end, deadline)
    }

    // Average the probe latency throughout the entire interval. Repeated probes
    // reduce sensitivity to one scheduler interruption or unusually fast sample.
    fn integrate_until(&mut self, end: Instant, deadline: Instant) -> Result<Duration, Error> {
        let mut total = Duration::ZERO;
        let mut count = 0u32;
        while Instant::now() < end {
            self.check(deadline)?;
            total = total.saturating_add(self.observe());
            count = count.saturating_add(1);
        }
        self.check(deadline)?;
        if count == 0 {
            return Err(Error::Corrupt("empty probe interval"));
        }
        Ok(total / count)
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
        let mut end = start + self.slot;
        // Skip missed slots instead of aborting the frame.
        while Instant::now() >= end {
            end += self.slot;
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
    let mut best: Option<(u32, Decoder)> = None;
    for active_high in [true, false] {
        let decoder = Decoder {
            threshold,
            active_high,
        };
        let mut errors = 0u32;
        for &sample in &samples[..CARRIER_ONES as usize] {
            if !decoder.decode(sample) {
                errors += 1;
            }
        }
        for (i, &sample) in samples[CARRIER_ONES as usize..].iter().enumerate() {
            let expected = (PREAMBLE >> (31 - i)) & 1 != 0;
            if decoder.decode(sample) != expected {
                errors += 1;
            }
        }
        if errors <= 6 && best.is_none_or(|(e, _)| errors < e) {
            best = Some((errors, decoder));
        }
    }
    best.map(|(_, decoder)| decoder)
}

/// Match the carrier, eight idle slots, and alternating preamble in wire order.
/// Requiring all three reduces accidental locks on alternating payload or noise.
pub fn matches_training(window: u128) -> bool {
    let carrier = (window >> 40) as u32;
    let gap = ((window >> 32) & 0xff) as u8;
    let preamble = window as u32;
    carrier.count_ones() >= 28 && gap.count_ones() <= 2 && (preamble ^ PREAMBLE).count_ones() <= 4
}

/// Bits in wire order, most significant first.
pub fn bits_msb(value: u32, width: u32) -> impl Iterator<Item = bool> {
    (0..width.min(32)).rev().map(move |i| (value >> i) & 1 != 0)
}
