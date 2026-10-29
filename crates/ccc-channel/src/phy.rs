use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use ccc_core::{Error, PREAMBLE};

use crate::timing::measure;

const CARRIER_ONES: u32 = 8;
const LOCK_ONES: u32 = 32;
const PREAMBLE_BITS: usize = 32;
const TRAINING_BITS: usize = CARRIER_ONES as usize + PREAMBLE_BITS;
// The first interleaved SECDED bit is parity position 1 of the high length
// byte. For lengths below 512 that byte is 0 or 1, and this parity is zero.
const _: () = assert!(ccc_core::MAX_PAYLOAD + 64 < 512);
const FEC_BITS: usize = 13;
const FEC_BLOCK_BYTES: usize = 8;
// Per word, SECDED distance 4 guarantees recovery from up to three erasures
// (2*errors + erasures < 4 with zero errors). A block holding more than
// MAX_WORD_ERASURES erasures per word is beyond that envelope even before
// allowing bit errors; discard such frames fail-closed. A frame may also
// occupy at most FRAME_LIFETIME_MULTIPLIER times its expected airtime.
const MAX_WORD_ERASURES: usize = 3;
const FRAME_LIFETIME_MULTIPLIER: u32 = 4;

// Minimum observed (non-erased) quarter-samples per training region before a
// lock candidate is even considered. Quarter-slot sampling loses individual
// measurements under load, so a sparse but internally consistent window must
// still train. These floors were lowered (carrier 20, gap 6, preamble 24) so
// that 25-40% erasure hunts can lock; alignment safety comes from the
// mandatory gap anchors, the zero-gap-error rule and the error caps scaling
// with how much of each region was observed, not from these counts alone.
const MIN_CARRIER_OBSERVED: usize = 16; // of 32 carrier quarter-samples
const MIN_GAP_OBSERVED: usize = 4; // of 8 idle-gap quarter-samples
const MIN_PREAMBLE_OBSERVED: usize = 16; // of 32 preamble quarter-samples

// CCC_DEBUG diagnostics: tally which acquisition gate rejected each full
// window so loaded hunts can attribute failures without per-quanta logging.
static REJECT_CARRIER: AtomicU64 = AtomicU64::new(0);
static REJECT_GAP: AtomicU64 = AtomicU64::new(0);
static REJECT_PREAMBLE: AtomicU64 = AtomicU64::new(0);
static REJECT_ANCHORS: AtomicU64 = AtomicU64::new(0);
static REJECT_SEPARATION: AtomicU64 = AtomicU64::new(0);

fn tally_reject(counter: &AtomicU64) {
    if std::env::var_os("CCC_DEBUG").is_some() {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

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
    hunt: Option<HuntState>,
    frame: Option<FrameReceiver>,
    integral: Option<Integral>,
    tx_slots: Option<u64>,
}

#[derive(Default)]
struct Integral {
    end: Option<Instant>,
    total: Duration,
    count: u32,
    paused_at: Option<Instant>,
}

struct HuntState {
    windows: [VecDeque<Option<Duration>>; 4],
    phase: usize,
    end: Instant,
    count: u64,
    erasures: u64,
    began: Instant,
    candidate: Option<(Decoder, Instant, u128, u8)>,
    integral: Integral,
}

struct FrameReceiver {
    samples: [[Option<Duration>; FEC_BITS]; FEC_BLOCK_BYTES],
    bits: usize,
    block_bytes: usize,
    length: Option<usize>,
    bytes: Vec<u8>,
    costs: Vec<Duration>,
    // Cumulative counts for diagnostics, and the current block's erasure
    // count for the frame erasure budget.
    slots: usize,
    erasures: usize,
    block_erasures: usize,
    // First sample time; a frame older than its lifetime bound is stale.
    began: Option<Instant>,
}

impl Default for FrameReceiver {
    fn default() -> Self {
        Self {
            samples: [[None; FEC_BITS]; FEC_BLOCK_BYTES],
            bits: 0,
            block_bytes: 2,
            length: None,
            bytes: Vec::new(),
            costs: Vec::new(),
            slots: 0,
            erasures: 0,
            block_erasures: 0,
            began: None,
        }
    }
}

impl FrameReceiver {
    /// Consume one physical frame slot: either a measured slot cost or an
    /// explicit erasure. The push sequence is the absolute slot grid, so an
    /// erasure stays at its original position and later symbols never shift.
    /// Incomplete fields remain in this object, so a polling deadline never
    /// drops a partially received frame.
    fn push_sample(
        &mut self,
        sample: Option<Duration>,
        decoder: Decoder,
        slot: Duration,
    ) -> Result<Option<Vec<u8>>, Error> {
        let began = self.began.get_or_insert_with(Instant::now);
        if began.elapsed() > frame_lifetime(self.length, slot) {
            return Err(Error::Corrupt("frame lifetime exceeded"));
        }
        self.slots += 1;
        self.erasures += usize::from(sample.is_none());
        self.block_erasures += usize::from(sample.is_none());
        // A block holding more erasures than the interleaved distance-4
        // guarantee envelope is beyond recovery policy; fail closed.
        if self.block_erasures > MAX_WORD_ERASURES * self.block_bytes {
            return Err(Error::Corrupt("too many erasures"));
        }
        let word = self.bits % self.block_bytes;
        let position = self.bits / self.block_bytes;
        self.samples[word][position] = sample;
        self.bits += 1;
        if self.bits < FEC_BITS * self.block_bytes {
            return Ok(None);
        }
        let mut decoded = Vec::with_capacity(self.block_bytes);
        for samples in &self.samples[..self.block_bytes] {
            let (byte, corrected) = soft_fec_decode_erased(samples, decoder)?;
            if corrected && std::env::var_os("CCC_DEBUG").is_some() {
                eprintln!("phy FEC selected a different codeword");
            }
            decoded.push(byte);
        }
        self.bits = 0;
        self.block_erasures = 0;
        match self.length {
            None => {
                let len = u16::from_be_bytes([decoded[0], decoded[1]]) as usize;
                if len > ccc_core::MAX_PAYLOAD + 64 {
                    return Err(Error::Corrupt("bad length"));
                }
                self.length = Some(len);
                self.block_bytes = (len + 4).min(FEC_BLOCK_BYTES);
                Ok(None)
            }
            Some(len) => {
                self.bytes.extend_from_slice(&decoded);
                let remaining = len + 4 - self.bytes.len();
                if remaining > 0 {
                    self.block_bytes = remaining.min(FEC_BLOCK_BYTES);
                    return Ok(None);
                }
                let actual_crc =
                    u32::from_be_bytes(self.bytes[len..].try_into().expect("four CRC bytes"));
                self.bytes.truncate(len);
                let expected_crc = crc32_of(&self.bytes);
                if actual_crc != expected_crc {
                    if std::env::var_os("CCC_DEBUG").is_some() {
                        eprintln!("phy CRC mismatch received={actual_crc:08x} expected={expected_crc:08x}");
                    }
                    return Err(Error::Corrupt("channel crc"));
                }
                Ok(Some(std::mem::take(&mut self.bytes)))
            }
        }
    }
}

/// One frame-slot observation: whole slots skipped by scheduling (reported
/// so they can be recorded as erasures in place) and the measured or erased
/// sample for the current slot.
struct SlotSample {
    missed: u32,
    sample: Option<Duration>,
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
            hunt: None,
            frame: None,
            integral: None,
            tx_slots: None,
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

    /// Whether a recognized acquisition or partially decoded frame is still
    /// within its sampling window. Ordinary idle hunting does not hold the
    /// link token; a stale receive cannot block sends indefinitely.
    pub fn receive_in_progress(&self) -> bool {
        let now = Instant::now();
        if self.decoder.is_some() {
            return self
                .next_slot
                .is_some_and(|at| now.saturating_duration_since(at) < self.slot);
        }
        self.hunt
            .as_ref()
            .and_then(|hunt| hunt.candidate)
            .is_some_and(|(_, at, _, _)| {
                now.saturating_duration_since(at) < self.slot.saturating_mul(2)
            })
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
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| Error::InvalidInput("timeout too large".into()))?;
        self.check(deadline)?;
        // The first encoded length bit is always zero for a valid frame.
        if bytes.len() > ccc_core::MAX_PAYLOAD + 64 {
            return Err(Error::TooLarge(bytes.len()));
        }
        let len = u16::try_from(bytes.len()).map_err(|_| Error::TooLarge(bytes.len()))?;
        // A half-duplex send interrupts any pending receive. The link should
        // avoid this while a frame is being decoded; discard stale samples if
        // it nevertheless asks to transmit.
        self.hunt = None;
        self.frame = None;
        self.integral = None;
        self.decoder = None;
        self.next_slot = None;
        if std::env::var_os("CCC_DEBUG").is_some() {
            eprintln!("phy send {} bytes", bytes.len());
        }
        self.tx_slots = Some(0);
        let began = Instant::now();
        let result = (|| {
            // Inter-frame gap.
            self.idle_slots(4, deadline)?;
            for _ in 0..LOCK_ONES {
                self.slot_activity(deadline)?;
            }
            self.idle_slots(8, deadline)?;
            self.send_u32(PREAMBLE, deadline)?;
            self.send_fec_block(&len.to_be_bytes(), deadline)?;
            let mut encoded = Vec::with_capacity(bytes.len() + 4);
            encoded.extend_from_slice(bytes);
            encoded.extend_from_slice(&crc32_of(bytes).to_be_bytes());
            for block in encoded.chunks(FEC_BLOCK_BYTES) {
                self.send_fec_block(block, deadline)?;
            }
            self.idle_slots(2, deadline)
        })();
        if let Err(ref error) = result {
            if std::env::var_os("CCC_DEBUG").is_some() {
                eprintln!(
                    "phy send failed after {:?}, slot {:?}: {error}",
                    began.elapsed(),
                    self.tx_slots
                );
            }
        }
        self.tx_slots = None;
        result
    }

    pub fn recv_blob(&mut self, timeout: Duration) -> Result<Vec<u8>, Error> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| Error::InvalidInput("timeout too large".into()))?;
        let result = (|| {
            if self.decoder.is_none() {
                self.hunt_preamble(deadline)?;
            }
            self.read_trained_frame(deadline)
        })();
        // Timeout is a bounded poll: preserve all acquisition and frame state.
        if !matches!(result, Err(Error::Timeout)) {
            self.decoder = None;
            self.next_slot = None;
            self.hunt = None;
            self.frame = None;
            self.integral = None;
        }
        result
    }

    fn read_trained_frame(&mut self, deadline: Instant) -> Result<Vec<u8>, Error> {
        let slot = self.slot;
        loop {
            let SlotSample { missed, sample } = self.sample_frame_slot(deadline)?;
            let decoder = self.decoder.ok_or(Error::Corrupt("untrained receiver"))?;
            let frame = self.frame.get_or_insert_with(FrameReceiver::default);
            // Missed slots are older than the measured one: record their
            // erasures first so every symbol keeps its absolute position.
            let mut result = Ok(None);
            for _ in 0..missed {
                result = frame.push_sample(None, decoder, slot);
                if !matches!(result, Ok(None)) {
                    break;
                }
            }
            if matches!(result, Ok(None)) {
                if std::env::var_os("CCC_DEBUG").is_some() {
                    if let Some(cost) = sample {
                        frame.costs.push(cost);
                    }
                }
                result = frame.push_sample(sample, decoder, slot);
            }
            if (frame.bits == 0 || result.is_err()) && !frame.costs.is_empty() {
                eprintln!("phy decoded field costs={:?}", frame.costs);
                frame.costs.clear();
            }
            if let Err(ref error) = result {
                if std::env::var_os("CCC_DEBUG").is_some() {
                    eprintln!(
                        "phy frame rejected: {error} (erasures={} slots={})",
                        frame.erasures, frame.slots
                    );
                }
            }
            if let Some(bytes) = result? {
                return Ok(bytes);
            }
        }
    }

    // Oversample the channel, looking for the complete preamble at every
    // quarter-slot offset. This is iterative so repeated noise cannot grow the stack.
    fn hunt_preamble(&mut self, deadline: Instant) -> Result<(), Error> {
        let quantum = self.slot / 4;
        if quantum.is_zero() {
            return Err(Error::InvalidInput("slot too short to oversample".into()));
        }
        let now = Instant::now();
        let end = now
            .checked_add(quantum)
            .ok_or_else(|| Error::InvalidInput("slot too large".into()))?;
        let mut hunt = self.hunt.take().unwrap_or_else(|| HuntState {
            windows: std::array::from_fn(|_| VecDeque::with_capacity(72)),
            phase: 0,
            end,
            count: 0,
            erasures: 0,
            began: now,
            candidate: None,
            integral: Integral::default(),
        });
        let result = self.continue_hunt(deadline, quantum, &mut hunt);
        if matches!(result, Err(Error::Timeout)) {
            self.hunt = Some(hunt);
        }
        result
    }

    fn continue_hunt(
        &mut self,
        deadline: Instant,
        quantum: Duration,
        hunt: &mut HuntState,
    ) -> Result<(), Error> {
        loop {
            // Scheduling can carry us past the next interval before we start
            // observing it. Treat this as a lost phase, not a corrupt frame.
            self.check(deadline)?;
            let sample = match self.integrate_resumable(hunt.end, deadline, &mut hunt.integral) {
                Ok(sample) => Some(sample),
                Err(Error::Corrupt("empty probe interval")) => {
                    hunt.integral = Integral::default();
                    None
                }
                Err(e) => return Err(e),
            };
            // Preserve clock phase through scheduling gaps. A measurement that
            // spans a whole extra quantum cannot represent this interval, so
            // mark it and each subsequent elapsed interval as erasures.
            let sample = if Instant::now().saturating_duration_since(hunt.end) >= quantum {
                None
            } else {
                sample
            };
            if sample.is_none() {
                hunt.erasures += 1;
                if std::env::var_os("CCC_DEBUG").is_some()
                    && (hunt.erasures <= 3 || hunt.erasures.is_multiple_of(32))
                {
                    eprintln!(
                        "phy hunt erasure elapsed={:?} count={} erasures={} phase={}",
                        hunt.began.elapsed(),
                        hunt.count,
                        hunt.erasures,
                        hunt.phase
                    );
                }
            }
            let window = &mut hunt.windows[hunt.phase];
            if window.len() == 72 {
                window.pop_front();
            }
            window.push_back(sample);
            hunt.count += 1;
            if window.len() == 72 {
                let samples = window.make_contiguous();
                let trained = train_lock_erased(samples);
                if std::env::var_os("CCC_DEBUG").is_some() && hunt.count.is_multiple_of(128) {
                    eprintln!(
                        "phy hunt elapsed={:?} count={} erasures={} \
                         rejects(carrier/gap/preamble/anchors/separation)={}/{}/{}/{}/{} \
                         candidate: {:?}",
                        hunt.began.elapsed(),
                        hunt.count,
                        hunt.erasures,
                        REJECT_CARRIER.load(Ordering::Relaxed),
                        REJECT_GAP.load(Ordering::Relaxed),
                        REJECT_PREAMBLE.load(Ordering::Relaxed),
                        REJECT_ANCHORS.load(Ordering::Relaxed),
                        REJECT_SEPARATION.load(Ordering::Relaxed),
                        training_stats_erased(samples)
                    );
                }
                if let Some(decoder) = trained {
                    let quality =
                        lock_quality_erased(samples).expect("trained candidate has quality");
                    if std::env::var_os("CCC_DEBUG").is_some() {
                        eprintln!(
                            "phy lock candidate quality={quality}: {:?}",
                            training_stats_erased(samples)
                        );
                    }
                    if let Some((best_decoder, best_end, best_quality, remaining)) =
                        &mut hunt.candidate
                    {
                        if quality < *best_quality {
                            *best_decoder = decoder;
                            *best_end = hunt.end;
                            *best_quality = quality;
                        }
                        *remaining -= 1;
                    } else {
                        hunt.candidate = Some((decoder, hunt.end, quality, 3));
                        hunt.phase = (hunt.phase + 1) % 4;
                        hunt.end = hunt
                            .end
                            .checked_add(quantum)
                            .ok_or_else(|| Error::InvalidInput("slot too large".into()))?;
                        continue;
                    }
                } else if let Some((_, _, _, remaining)) = &mut hunt.candidate {
                    *remaining -= 1;
                }
            }
            if let Some((decoder, candidate_end, quality, 0)) = hunt.candidate {
                if std::env::var_os("CCC_DEBUG").is_some() {
                    eprintln!(
                        "phy lock selected quality={quality} threshold={:?} active_high={}",
                        decoder.threshold, decoder.active_high
                    );
                }
                // The last quarter-slot sample is centred one eighth of a
                // slot before its end; the following bit starts half a slot
                // after that centre.
                self.next_slot = Some(
                    candidate_end
                        .checked_add(self.slot.saturating_mul(3) / 8)
                        .ok_or_else(|| Error::InvalidInput("slot too large".into()))?,
                );
                self.decoder = Some(decoder);
                return Ok(());
            }
            hunt.phase = (hunt.phase + 1) % 4;
            hunt.end = hunt
                .end
                .checked_add(quantum)
                .ok_or_else(|| Error::InvalidInput("slot too large".into()))?;
        }
    }

    fn send_u32(&mut self, v: u32, deadline: Instant) -> Result<(), Error> {
        for bit in bits_msb(v, 32) {
            self.send_bit(bit, deadline)?;
        }
        Ok(())
    }

    fn send_fec_block(&mut self, bytes: &[u8], deadline: Instant) -> Result<(), Error> {
        let words: Vec<_> = bytes.iter().map(|&byte| fec_encode(byte)).collect();
        for position in 0..FEC_BITS {
            for &word in &words {
                self.send_bit((word >> position) & 1 != 0, deadline)?;
            }
        }
        Ok(())
    }

    fn send_bit(&mut self, one: bool, deadline: Instant) -> Result<(), Error> {
        if one {
            self.slot_activity(deadline)
        } else {
            self.idle_slots(1, deadline)
        }
    }

    /// Sample the next frame slot on the absolute grid anchored at
    /// `next_slot`. Whole slots lost to scheduling are reported as `missed`
    /// so the caller can record erasures at their original positions; an
    /// interval without a usable measurement is a `None` sample. The grid
    /// itself never moves, so later symbols are never shifted.
    fn sample_frame_slot(&mut self, deadline: Instant) -> Result<SlotSample, Error> {
        self.check(deadline)?;
        let mut integral = self.integral.take().unwrap_or_default();
        let mut missed = 0u32;
        let end = if let Some(end) = integral.end {
            end
        } else {
            let start = self.next_slot.ok_or(Error::Corrupt("untrained receiver"))?;
            if let Err(error) = self.finish_slot(start, deadline) {
                self.integral = Some(integral);
                return Err(error);
            }
            if self.slot.is_zero() {
                return Err(Error::InvalidInput("slot must be nonzero".into()));
            }
            let now = Instant::now();
            let elapsed = now.saturating_duration_since(start).as_nanos();
            missed = u32::try_from(elapsed / self.slot.as_nanos()).unwrap_or(u32::MAX);
            let start = start
                .checked_add(self.slot.saturating_mul(missed))
                .ok_or_else(|| Error::InvalidInput("slot too large".into()))?;
            let end = start
                .checked_add(self.slot)
                .ok_or_else(|| Error::InvalidInput("slot too large".into()))?;
            self.next_slot = Some(end);
            integral.end = Some(end);
            end
        };
        match self.integrate_resumable(end, deadline, &mut integral) {
            Ok(sample) => {
                // A measurement that overran its slot end by a whole slot
                // straddled a boundary and cannot represent this interval.
                let sample = if Instant::now().saturating_duration_since(end) >= self.slot {
                    None
                } else {
                    Some(sample)
                };
                Ok(SlotSample { missed, sample })
            }
            Err(Error::Corrupt("empty probe interval")) => Ok(SlotSample {
                missed,
                sample: None,
            }),
            Err(error) => {
                if matches!(error, Error::Timeout) {
                    self.integral = Some(integral);
                }
                Err(error)
            }
        }
    }

    fn sample_cost(&mut self, deadline: Instant) -> Result<Duration, Error> {
        self.check(deadline)?;
        let mut integral = self.integral.take().unwrap_or_default();
        let end = if let Some(end) = integral.end {
            end
        } else {
            if let Some(start) = self.next_slot {
                if let Err(error) = self.finish_slot(start, deadline) {
                    self.integral = Some(integral);
                    return Err(error);
                }
            }
            match self.slot_end(deadline) {
                Ok(end) => {
                    integral.end = Some(end);
                    end
                }
                Err(error) => {
                    self.integral = Some(integral);
                    return Err(error);
                }
            }
        };
        let result = self.integrate_resumable(end, deadline, &mut integral);
        if matches!(result, Err(Error::Timeout)) {
            self.integral = Some(integral);
        }
        result
    }

    fn integrate_resumable(
        &mut self,
        end: Instant,
        deadline: Instant,
        integral: &mut Integral,
    ) -> Result<Duration, Error> {
        if integral.paused_at.take().is_some() && Instant::now() >= end {
            return Err(Error::Corrupt("empty probe interval"));
        }
        while Instant::now() < end {
            if let Err(error) = self.check(deadline) {
                if matches!(error, Error::Timeout) {
                    integral.paused_at = Some(Instant::now());
                }
                return Err(error);
            }
            integral.total = integral.total.saturating_add(self.observe());
            integral.count = integral.count.saturating_add(1);
        }
        if integral.count == 0 {
            return Err(Error::Corrupt("empty probe interval"));
        }
        let average = integral.total / integral.count;
        *integral = Integral::default();
        Ok(average)
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
        let end = start
            .checked_add(self.slot)
            .ok_or_else(|| Error::InvalidInput("slot too large".into()))?;
        // Dropping a slot changes every subsequent bit's meaning.
        let now = Instant::now();
        if now >= end {
            if std::env::var_os("CCC_DEBUG").is_some() {
                eprintln!(
                    "phy slot overrun elapsed={:?} slot={:?}",
                    now.duration_since(end),
                    self.slot
                );
            }
            return Err(Error::Corrupt("missed physical slot"));
        }
        if let Some(slots) = &mut self.tx_slots {
            *slots += 1;
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

// Extended Hamming (13,8): parity positions 1, 2, 4, 8 and overall
// parity position 13. Bits are transmitted in position order, interleaved by
// position across up to eight independently encoded bytes.
fn fec_encode(byte: u8) -> u16 {
    let mut word = 0u16;
    for (index, position) in [3, 5, 6, 7, 9, 10, 11, 12].into_iter().enumerate() {
        if byte & (1 << (7 - index)) != 0 {
            word |= 1 << (position - 1);
        }
    }
    for parity in [1, 2, 4, 8] {
        let mut odd = false;
        for position in 1..=12 {
            if position & parity != 0 && word & (1 << (position - 1)) != 0 {
                odd = !odd;
            }
        }
        if odd {
            word |= 1 << (parity - 1);
        }
    }
    if !word.count_ones().is_multiple_of(2) {
        word |= 1 << 12;
    }
    word
}

fn fec_decode(mut word: u16) -> Result<(u8, bool), Error> {
    let mut syndrome = 0usize;
    for position in 1..=12 {
        if word & (1 << (position - 1)) != 0 {
            syndrome ^= position;
        }
    }
    let odd = !word.count_ones().is_multiple_of(2);
    let corrected = match (syndrome, odd) {
        (0, false) => false,
        (0, true) => {
            word ^= 1 << 12;
            true
        }
        (1..=12, true) => {
            word ^= 1 << (syndrome - 1);
            true
        }
        _ => return Err(Error::Corrupt("uncorrectable FEC")),
    };
    let mut byte = 0u8;
    for position in [3, 5, 6, 7, 9, 10, 11, 12] {
        byte = (byte << 1) | u8::from((word >> (position - 1)) & 1 != 0);
    }
    Ok((byte, corrected))
}

/// Wall-clock bound for one frame: a multiple of its expected on-air slot
/// count (26 length slots plus 13 per byte of payload and CRC). A partial
/// frame older than this is stale and must be discarded.
fn frame_lifetime(length: Option<usize>, slot: Duration) -> Duration {
    let slots = length.map_or(FEC_BITS * 2, |len| FEC_BITS * (len + 6));
    slot.saturating_mul(FRAME_LIFETIME_MULTIPLIER.saturating_mul(slots as u32))
}

// Decode from measured slot costs, without using expected payload bytes. A
// capped absolute distance limits the effect of extreme scheduling outliers.
// CRC remains mandatory: a unique nearest codeword is not proof of correctness.
#[cfg(test)]
fn soft_fec_decode(samples: &[Duration; FEC_BITS], decoder: Decoder) -> Result<(u8, bool), Error> {
    soft_fec_decode_erased(&samples.map(Some), decoder)
}

/// Soft decode with explicit erasures. An erased slot costs the same for
/// either bit value, so it never prefers one codeword over another; with
/// SECDED distance 4 a word still decodes while 2*errors + erasures < 4,
/// and genuinely ambiguous words are rejected instead of guessed.
fn soft_fec_decode_erased(
    samples: &[Option<Duration>; FEC_BITS],
    decoder: Decoder,
) -> Result<(u8, bool), Error> {
    let spread = decoder.active.abs_diff(decoder.idle).as_nanos();
    if spread == 0 {
        return Err(Error::Corrupt("untrained receiver"));
    }
    let mut hard_word = 0u16;
    let mut all_strong = true;
    for (position, &sample) in samples.iter().enumerate() {
        let Some(sample) = sample else {
            all_strong = false;
            continue;
        };
        let active = decoder.decode(sample);
        if active {
            hard_word |= 1 << position;
        }
        let reference = if active { decoder.active } else { decoder.idle };
        all_strong &= sample.abs_diff(reference).as_nanos() <= spread / 8;
    }
    // Preserve SECDED's double-error detection for clearly separated samples;
    // only use soft recovery when at least one decision was uncertain.
    if all_strong && fec_decode(hard_word).is_err() {
        return Err(Error::Corrupt("uncorrectable FEC"));
    }
    // Equal cost for either bit value: an erasure is neutral evidence.
    let erasure = spread / 2;
    let mut best = u128::MAX;
    let mut best_byte = 0u8;
    let mut tied = false;
    for byte in 0..=u8::MAX {
        let word = fec_encode(byte);
        let score: u128 = samples
            .iter()
            .enumerate()
            .map(|(position, &sample)| {
                let Some(sample) = sample else {
                    return erasure;
                };
                let reference = if (word >> position) & 1 != 0 {
                    decoder.active
                } else {
                    decoder.idle
                };
                sample.abs_diff(reference).as_nanos().min(spread)
            })
            .sum();
        if score < best {
            best = score;
            best_byte = byte;
            tied = false;
        } else if score == best {
            tied = true;
        }
    }
    if tied {
        return Err(Error::Corrupt("ambiguous FEC"));
    }
    let best_word = fec_encode(best_byte);
    let corrected = samples.iter().enumerate().any(|(position, &sample)| {
        sample.is_none_or(|sample| decoder.decode(sample) != ((best_word >> position) & 1 != 0))
    });
    Ok((best_byte, corrected))
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
    active: Duration,
    idle: Duration,
}

impl Decoder {
    pub fn decode(self, sample: Duration) -> bool {
        (sample > self.threshold) == self.active_high
    }
}

// The sender transmits 32 active slots, eight idle slots, then the alternating
// preamble. These known symbols are used only to calibrate the physical layer;
// payload bytes are never supplied to the receiver.
fn training_stats_erased(
    samples: &[Option<Duration>],
) -> Option<(Duration, Duration, Decoder, u32, u32, u32)> {
    if samples.len() != 72 {
        return None;
    }
    let carrier_count = samples[..32].iter().flatten().count();
    let gap_count = samples[32..40].iter().flatten().count();
    let preamble_count = samples[40..].iter().flatten().count();
    // Retain both edges of the gap: otherwise missing symbols can conceal a
    // two-slot-early lock on the periodic preamble. The anchors at the gap
    // edges stay mandatory even though the observation minima are lower.
    if carrier_count < MIN_CARRIER_OBSERVED {
        tally_reject(&REJECT_CARRIER);
        return None;
    }
    if gap_count < MIN_GAP_OBSERVED {
        tally_reject(&REJECT_GAP);
        return None;
    }
    if preamble_count < MIN_PREAMBLE_OBSERVED {
        tally_reject(&REJECT_PREAMBLE);
        return None;
    }
    if [32, 33, 38, 39].iter().any(|&i| samples[i].is_none()) {
        tally_reject(&REJECT_ANCHORS);
        return None;
    }
    let mut active = Vec::with_capacity(48);
    let mut idle = Vec::with_capacity(24);
    for (i, &sample) in samples.iter().enumerate() {
        let Some(sample) = sample else { continue };
        let expected = i < 32 || (i >= 40 && ((PREAMBLE >> (71 - i)) & 1 != 0));
        if expected {
            active.push(sample);
        } else {
            idle.push(sample);
        }
    }
    active.sort_unstable();
    idle.sort_unstable();
    let a = active[active.len() / 2];
    let b = idle[idle.len() / 2];
    let (low, high) = if a < b { (a, b) } else { (b, a) };
    if high - low < low / 20 || high == low {
        tally_reject(&REJECT_SEPARATION);
        return None;
    }
    let decoder = Decoder {
        threshold: low + (high - low) / 2,
        active_high: a > b,
        active: a,
        idle: b,
    };
    let mut carrier_errors = 0;
    let mut gap_errors = 0;
    let mut preamble_errors = 0;
    for (i, &sample) in samples.iter().enumerate() {
        let Some(sample) = sample else { continue };
        let expected = i < 32 || (i >= 40 && ((PREAMBLE >> (71 - i)) & 1 != 0));
        if decoder.decode(sample) != expected {
            if i < 32 {
                carrier_errors += 1;
            } else if i < 40 {
                gap_errors += 1;
            } else {
                preamble_errors += 1;
            }
        }
    }
    Some((a, b, decoder, carrier_errors, gap_errors, preamble_errors))
}

/// Train on a complete carrier, idle gap and preamble at a candidate phase.
/// Medians tolerate occasional scheduling outliers without imposing a fixed
/// threshold or assuming which polarity the machine produces.
pub fn train_lock(samples: &[Duration]) -> Option<Decoder> {
    let samples: Vec<_> = samples.iter().copied().map(Some).collect();
    train_lock_erased(&samples)
}

/// Train while preserving explicitly missing quarter-slot measurements.
pub fn train_lock_erased(samples: &[Option<Duration>]) -> Option<Decoder> {
    let (_, _, decoder, carrier, gap, preamble) = training_stats_erased(samples)?;
    // The preamble repeats every two slots. A candidate two slots early can
    // otherwise pass with exactly two carrier errors, two gap errors and one
    // preamble error, then decode every frame bit at the wrong offset. Require
    // the entire idle gap so that the gap identifies the phase unambiguously.
    //
    // Error budgets scale with how much of each region survived erasure:
    // min(4, observed/8) keeps a fully observed window at the historical cap
    // of four, while a sparsely observed window must show proportionally
    // cleaner evidence. Fewer observations never weaken error evidence.
    let carrier_observed = samples[..32].iter().flatten().count() as u32;
    let preamble_observed = samples[40..].iter().flatten().count() as u32;
    let carrier_cap = (carrier_observed / 8).min(4);
    let preamble_cap = (preamble_observed / 8).min(4);
    if carrier <= carrier_cap && gap == 0 && preamble <= preamble_cap {
        Some(decoder)
    } else {
        None
    }
}

/// Rank valid phases by how tightly their alternating preamble samples fit
/// the trained active/idle medians. This uses only known training symbols.
pub fn lock_quality(samples: &[Duration]) -> Option<u128> {
    let samples: Vec<_> = samples.iter().copied().map(Some).collect();
    lock_quality_erased(&samples)
}

fn lock_quality_erased(samples: &[Option<Duration>]) -> Option<u128> {
    let (active, idle, _, _, _, _) = training_stats_erased(samples)?;
    let spread = active.abs_diff(idle).as_nanos();
    if spread == 0 {
        return None;
    }
    let mut error = 0u128;
    let mut observed = 0u128;
    for (i, sample) in samples.iter().enumerate().skip(40) {
        let Some(sample) = sample else { continue };
        observed += 1;
        let expected = if (PREAMBLE >> (71 - i)) & 1 != 0 {
            active
        } else {
            idle
        };
        error += sample.abs_diff(expected).as_nanos().min(spread) * 1000 / spread;
    }
    // Comparable scores even when phases contain different erasure counts;
    // prefer a better-observed phase when their fit is otherwise similar.
    Some(error * 32 / observed + (32 - observed) * 10)
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
        let low = sorted[split / 2];
        let high = sorted[split + (sorted.len() - split) / 2];
        let decoder = Decoder {
            threshold,
            active_high,
            active: if active_high { high } else { low },
            idle: if active_high { low } else { high },
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

#[cfg(test)]
#[path = "../tests/internal/phy.rs"]
mod tests;
