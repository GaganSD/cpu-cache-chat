use super::*;

#[test]
fn polling_preserves_acquisition_history_and_clears_on_interrupt() {
    let stop = Arc::new(AtomicBool::new(false));
    let mut phy = Phy::new(PhyKind::Cpu, Duration::from_millis(20), stop.clone());
    assert!(matches!(
        phy.recv_blob(Duration::from_millis(13)),
        Err(Error::Timeout)
    ));
    let first = phy.hunt.as_ref().unwrap().count;
    assert!(first > 0);
    assert!(matches!(
        phy.recv_blob(Duration::from_millis(13)),
        Err(Error::Timeout)
    ));
    assert!(phy.hunt.as_ref().unwrap().count > first);
    stop.store(true, Ordering::SeqCst);
    assert!(matches!(
        phy.recv_blob(Duration::from_millis(10)),
        Err(Error::Interrupted)
    ));
    assert!(phy.hunt.is_none());
}

#[test]
fn slot_integral_resumes_and_rejects_a_missed_interval() {
    let mut phy = Phy::new(
        PhyKind::Cpu,
        Duration::from_millis(40),
        Arc::new(AtomicBool::new(false)),
    );
    let mut integral = Integral::default();
    let end = Instant::now() + Duration::from_millis(30);
    assert!(matches!(
        phy.integrate_resumable(
            end,
            Instant::now() + Duration::from_millis(3),
            &mut integral
        ),
        Err(Error::Timeout)
    ));
    assert!(integral.count > 0);
    assert!(phy
        .integrate_resumable(
            end,
            Instant::now() + Duration::from_millis(60),
            &mut integral
        )
        .is_ok());
    let end = Instant::now() + Duration::from_millis(15);
    assert!(matches!(
        phy.integrate_resumable(
            end,
            Instant::now() + Duration::from_millis(2),
            &mut integral
        ),
        Err(Error::Timeout)
    ));
    std::thread::sleep(Duration::from_millis(20));
    assert!(matches!(
        phy.integrate_resumable(
            end,
            Instant::now() + Duration::from_millis(30),
            &mut integral
        ),
        Err(Error::Corrupt("empty probe interval"))
    ));
}

#[test]
fn sender_rejects_missed_slot_instead_of_shifting_frame() {
    let slot = Duration::from_millis(20);
    let mut phy = Phy::new(PhyKind::Cpu, slot, Arc::new(AtomicBool::new(false)));
    phy.tx_slots = Some(7);
    phy.next_slot = Some(Instant::now() - slot * 2);
    assert!(matches!(
        phy.slot_end(Instant::now() + Duration::from_secs(1)),
        Err(Error::Corrupt("missed physical slot"))
    ));
    assert_eq!(phy.tx_slots, Some(7));
}

fn encoded_block(bytes: &[u8]) -> Vec<bool> {
    let words: Vec<_> = bytes.iter().map(|&byte| fec_encode(byte)).collect();
    (0..FEC_BITS)
        .flat_map(|position| words.iter().map(move |word| (word >> position) & 1 != 0))
        .collect()
}

fn encoded_frame(payload: &[u8], crc: u32) -> Vec<bool> {
    let mut bits = encoded_block(&(payload.len() as u16).to_be_bytes());
    let mut body = payload.to_vec();
    body.extend_from_slice(&crc.to_be_bytes());
    for block in body.chunks(FEC_BLOCK_BYTES) {
        bits.extend(encoded_block(block));
    }
    bits
}

const TEST_SLOT: Duration = Duration::from_millis(10);

fn test_decoder() -> Decoder {
    Decoder {
        threshold: Duration::from_micros(15),
        active_high: true,
        active: Duration::from_micros(20),
        idle: Duration::from_micros(10),
    }
}

// Drive a frame receiver with explicit per-slot samples: Some(bit) is a clean
// measurement, None is an erasure at that absolute slot position.
fn decode_frame_slots(slots: &[Option<bool>], chunk_size: usize) -> Result<Option<Vec<u8>>, Error> {
    let mut receiver = FrameReceiver::default();
    let decoder = test_decoder();
    let mut decoded = None;
    for chunk in slots.chunks(chunk_size) {
        for &slot in chunk {
            let sample = slot.map(|bit| if bit { decoder.active } else { decoder.idle });
            let result = receiver.push_sample(sample, decoder, TEST_SLOT)?;
            if result.is_some() {
                decoded = result;
            }
        }
    }
    Ok(decoded)
}

fn decode_frame_bits(bits: &[bool], chunk_size: usize) -> Result<Option<Vec<u8>>, Error> {
    let slots: Vec<_> = bits.iter().map(|&bit| Some(bit)).collect();
    decode_frame_slots(&slots, chunk_size)
}

// Window with explicit erasures at absolute positions and optionally flipped
// (wrong-symbol) samples, for probing the internal training statistics.
fn stats_window(inverted: bool, erase: &[usize], corrupt: &[usize]) -> Vec<Option<Duration>> {
    let bits = std::iter::repeat_n(true, 32)
        .chain(std::iter::repeat_n(false, 8))
        .chain(bits_msb(0xaaaa_aaaa, 32));
    let mut window: Vec<_> = bits
        .map(|active| {
            Some(Duration::from_micros(if active ^ inverted {
                20
            } else {
                10
            }))
        })
        .collect();
    for &i in corrupt {
        window[i] = window[i].map(|sample| {
            if sample == Duration::from_micros(20) {
                Duration::from_micros(10)
            } else {
                Duration::from_micros(20)
            }
        });
    }
    for &i in erase {
        window[i] = None;
    }
    window
}

#[test]
fn error_budget_scales_with_observations_at_the_lowered_bounds() {
    // Sixteen carrier observations: the count gate passes at the new floor
    // and the error budget is min(4, 16/8) = 2. Two carrier errors lock;
    // three reject while the statistics gate still accepts, proving the
    // scaled error cap, not the count floor, does the rejecting.
    let sparse: Vec<usize> = (0..32)
        .step_by(2)
        .chain([34, 35, 36, 37])
        .chain(40..56)
        .collect();
    for inverted in [false, true] {
        let two_errors = stats_window(inverted, &sparse, &[1, 3]);
        let stats = training_stats_erased(&two_errors).expect("statistics accept two errors");
        assert_eq!(stats.3, 2);
        assert!(train_lock_erased(&two_errors).is_some());
        let three_errors = stats_window(inverted, &sparse, &[1, 3, 5]);
        let stats = training_stats_erased(&three_errors).expect("statistics accept three errors");
        assert_eq!(stats.3, 3);
        assert!(
            train_lock_erased(&three_errors).is_none(),
            "min(4, 16/8) = 2 budget rejects three carrier errors"
        );
    }
    // A fully observed window keeps the historical budget of four.
    let four_errors = stats_window(false, &[], &[0, 2, 4, 6]);
    assert_eq!(training_stats_erased(&four_errors).unwrap().3, 4);
    assert!(train_lock_erased(&four_errors).is_some());
    let five_errors = stats_window(false, &[], &[0, 2, 4, 6, 8]);
    assert!(train_lock_erased(&five_errors).is_none());
}

#[test]
fn fec_corrects_every_single_bit_and_rejects_every_double_bit() {
    let decoder = Decoder {
        threshold: Duration::from_micros(15),
        active_high: true,
        active: Duration::from_micros(20),
        idle: Duration::from_micros(10),
    };
    for byte in 0..=u8::MAX {
        let word = fec_encode(byte);
        assert_eq!(fec_decode(word).unwrap(), (byte, false));
        for a in 0..FEC_BITS {
            assert_eq!(fec_decode(word ^ (1 << a)).unwrap(), (byte, true));
            let sample_word = word ^ (1 << a);
            let samples = std::array::from_fn(|pos| {
                if (sample_word >> pos) & 1 != 0 {
                    decoder.active
                } else {
                    decoder.idle
                }
            });
            assert_eq!(soft_fec_decode(&samples, decoder).unwrap(), (byte, true));
            for b in a + 1..FEC_BITS {
                assert!(matches!(
                    fec_decode(word ^ (1 << a) ^ (1 << b)),
                    Err(Error::Corrupt("uncorrectable FEC"))
                ));
                let sample_word = word ^ (1 << a) ^ (1 << b);
                let samples = std::array::from_fn(|pos| {
                    if (sample_word >> pos) & 1 != 0 {
                        decoder.active
                    } else {
                        decoder.idle
                    }
                });
                assert!(matches!(
                    soft_fec_decode(&samples, decoder),
                    Err(Error::Corrupt("uncorrectable FEC"))
                ));
            }
        }
    }
}

#[test]
fn soft_decoding_recovers_two_weak_decisions_for_both_polarities() {
    for active_high in [true, false] {
        let (active, idle) = if active_high {
            (20_000, 10_000)
        } else {
            (10_000, 20_000)
        };
        let decoder = Decoder {
            threshold: Duration::from_nanos(15_000),
            active_high,
            active: Duration::from_nanos(active),
            idle: Duration::from_nanos(idle),
        };
        let byte = 19u8;
        let word = fec_encode(byte);
        assert_eq!((word >> 10) & 1, 1);
        assert_eq!((word >> 11) & 1, 1);
        let mut samples = std::array::from_fn(|pos| {
            if (word >> pos) & 1 != 0 {
                decoder.active
            } else {
                decoder.idle
            }
        });
        let weak = if active_high { 14_900 } else { 15_100 };
        samples[10] = Duration::from_nanos(weak);
        samples[11] = Duration::from_nanos(weak);
        assert_eq!(soft_fec_decode(&samples, decoder).unwrap(), (byte, true));
        assert!(matches!(
            soft_fec_decode(&[Duration::from_nanos(15_000); FEC_BITS], decoder),
            Err(Error::Corrupt("ambiguous FEC"))
        ));
    }
}

#[test]
fn soft_decoding_recovers_observed_weak_ack_length() {
    let costs = [
        12_377, 12_484, 12_618, 11_978, 11_884, 12_175, 12_121, 12_012, 11_944, 11_847, 11_866,
        11_941, 15_151, 18_566, 11_987, 11_943, 12_070, 12_151, 11_952, 11_819, 14_424, 16_847,
        14_308, 17_282, 14_303, 22_556,
    ];
    let decoder = Decoder {
        threshold: Duration::from_nanos(17_505),
        active_high: true,
        active: Duration::from_nanos(23_195),
        idle: Duration::from_nanos(11_815),
    };
    let mut receiver = FrameReceiver::default();
    for cost in costs {
        assert!(receiver
            .push_sample(Some(Duration::from_nanos(cost)), decoder, TEST_SLOT)
            .unwrap()
            .is_none());
    }
    assert_eq!(receiver.length, Some(19));
}

#[test]
fn frame_fields_and_crc_survive_arbitrary_poll_boundaries() {
    for len in [0, 4, 8, 9, 17, ccc_core::MAX_PAYLOAD + 64] {
        let payload: Vec<_> = (0..len).map(|i| (i * 37) as u8).collect();
        let bits = encoded_frame(&payload, crc32_of(&payload));
        for chunk in [1, 3, 7, 17, 31, 104] {
            assert_eq!(
                decode_frame_bits(&bits, chunk).unwrap(),
                Some(payload.clone())
            );
        }
        assert_eq!(
            bits.len() + 4 + 32 + 8 + 32 + 2,
            ccc_core::PHYSICAL_FRAME_OVERHEAD_SLOTS as usize
                + ccc_core::PHYSICAL_CODEWORD_SLOTS as usize * len
        );
        assert!(!bits[0], "first length parity bit must be zero");
        let bad_crc = encoded_frame(&payload, crc32_of(&payload) ^ 1);
        assert!(matches!(
            decode_frame_bits(&bad_crc, 7),
            Err(Error::Corrupt("channel crc"))
        ));
    }
}

#[test]
fn interleaving_corrects_bursts_and_rejects_double_errors_in_one_word() {
    let payload: Vec<_> = (0..20).map(|i| i as u8 * 7).collect();
    let bits = encoded_frame(&payload, crc32_of(&payload));
    // Length is 26 slots, followed by two complete 8-byte blocks and a final
    // partial block. An 8-slot burst in a full block hits each word once.
    for start in 26..=26 + 104 - 8 {
        let mut damaged = bits.clone();
        for bit in &mut damaged[start..start + 8] {
            *bit = !*bit;
        }
        assert_eq!(
            decode_frame_bits(&damaged, 3).unwrap(),
            Some(payload.clone())
        );
    }
    for start in 0..=24 {
        let mut damaged = bits.clone();
        for bit in &mut damaged[start..start + 2] {
            *bit = !*bit;
        }
        assert_eq!(
            decode_frame_bits(&damaged, 11).unwrap(),
            Some(payload.clone())
        );
    }
    let mut damaged = bits;
    damaged[26] = !damaged[26];
    damaged[26 + 8] = !damaged[26 + 8];
    assert!(matches!(
        decode_frame_bits(&damaged, 5),
        Err(Error::Corrupt("uncorrectable FEC"))
    ));
}

#[test]
fn receive_guard_excludes_idle_and_stale_acquisition() {
    let mut phy = Phy::new(
        PhyKind::Cpu,
        Duration::from_millis(20),
        Arc::new(AtomicBool::new(false)),
    );
    assert!(!phy.receive_in_progress());
    assert!(matches!(
        phy.recv_blob(Duration::from_millis(6)),
        Err(Error::Timeout)
    ));
    assert!(!phy.receive_in_progress());
    let decoder = Decoder {
        threshold: Duration::from_micros(1),
        active_high: true,
        active: Duration::from_micros(2),
        idle: Duration::ZERO,
    };
    phy.hunt.as_mut().unwrap().candidate = Some((decoder, Instant::now(), 0, 3));
    assert!(phy.receive_in_progress());
    phy.hunt.as_mut().unwrap().candidate =
        Some((decoder, Instant::now() - Duration::from_secs(1), 0, 3));
    assert!(!phy.receive_in_progress());
    phy.decoder = Some(decoder);
    phy.next_slot = Some(Instant::now());
    assert!(phy.receive_in_progress());
    phy.next_slot = Some(Instant::now() - Duration::from_secs(1));
    assert!(!phy.receive_in_progress());
}

#[test]
fn frame_recovers_burst_and_scattered_erasures_without_shifting() {
    let payload: Vec<_> = (0..20).map(|i| i as u8 * 7 + 3).collect();
    let bits = encoded_frame(&payload, crc32_of(&payload));
    // A burst of erased slots inside one full block hits every interleaved
    // word equally: 8, 16 or 24 consecutive gaps erase 1, 2 or 3 positions
    // per word, all inside the SECDED distance-4 recovery guarantee.
    for burst in [8, 16, 24] {
        let mut slots: Vec<_> = bits.iter().map(|&bit| Some(bit)).collect();
        for slot in &mut slots[26..26 + burst] {
            *slot = None;
        }
        assert_eq!(
            decode_frame_slots(&slots, 5).unwrap(),
            Some(payload.clone()),
            "burst of {burst} erasures must decode without shifting"
        );
    }
    // Six consecutive gaps in the two-word length block give each word three
    // erased positions, the guaranteed limit; the field must not shift.
    let mut slots: Vec<_> = bits.iter().map(|&bit| Some(bit)).collect();
    for slot in &mut slots[4..10] {
        *slot = None;
    }
    assert_eq!(
        decode_frame_slots(&slots, 7).unwrap(),
        Some(payload.clone())
    );
    // Scattered gaps spread across words and stay under the frame budget.
    let mut slots: Vec<_> = bits.iter().map(|&bit| Some(bit)).collect();
    for i in (0..slots.len()).step_by(17) {
        slots[i] = None;
    }
    assert_eq!(decode_frame_slots(&slots, 3).unwrap(), Some(payload));
}

#[test]
fn absolute_slot_positions_preserved_under_gaps() {
    let payload: Vec<_> = (0..9).map(|i| 0xA0u8 + i as u8).collect();
    let bits = encoded_frame(&payload, crc32_of(&payload));
    let decoder = test_decoder();
    let mut receiver = FrameReceiver::default();
    // One erasure in each length word; positions must not shift the field.
    let mut slots: Vec<_> = bits.iter().map(|&bit| Some(bit)).collect();
    slots[6] = None; // position 3, word 0
    slots[15] = None; // position 7, word 1
    for &slot in &slots[..26] {
        let sample = slot.map(|bit| if bit { decoder.active } else { decoder.idle });
        assert!(receiver
            .push_sample(sample, decoder, TEST_SLOT)
            .unwrap()
            .is_none());
    }
    assert_eq!(receiver.length, Some(payload.len()));
    assert_eq!(receiver.slots, 26);
    assert_eq!(receiver.erasures, 2);
    // Any compression of the two gaps would misalign every later symbol and
    // fail the CRC; exact delivery proves absolute indexing.
    let mut decoded = None;
    for &slot in &slots[26..] {
        let sample = slot.map(|bit| if bit { decoder.active } else { decoder.idle });
        if let Some(bytes) = receiver.push_sample(sample, decoder, TEST_SLOT).unwrap() {
            decoded = Some(bytes);
        }
    }
    assert_eq!(decoded, Some(payload));
}

#[test]
fn erasure_and_error_within_fec_distance_recover() {
    // SECDED distance 4: one hard error plus one erasure (2*1 + 1 < 4).
    let payload: Vec<_> = (0..12).map(|i| i as u8 * 11 + 1).collect();
    let bits = encoded_frame(&payload, crc32_of(&payload));
    let mut slots: Vec<_> = bits.iter().map(|&bit| Some(bit)).collect();
    // Word 0 of the first body block: flip position 0 strongly, erase
    // position 3. Slots are interleaved as position * words + word.
    slots[26] = slots[26].map(|bit| !bit);
    slots[26 + 3 * FEC_BLOCK_BYTES] = None;
    assert_eq!(decode_frame_slots(&slots, 5).unwrap(), Some(payload));
}

#[test]
fn erasures_beyond_budget_discard_frame() {
    let payload = b"budget".to_vec();
    let bits = encoded_frame(&payload, crc32_of(&payload));
    // Alternating measured and erased slots exceeds one erasure in four.
    let mut slots: Vec<_> = bits.iter().map(|&bit| Some(bit)).collect();
    for i in (0..slots.len()).step_by(2) {
        slots[i] = None;
    }
    assert!(matches!(
        decode_frame_slots(&slots, 1),
        Err(Error::Corrupt("too many erasures"))
    ));
    // Exactly one in four stays within budget; the frame then stands or
    // falls on FEC and CRC alone, never on budget accounting.
    let mut slots: Vec<_> = bits.iter().map(|&bit| Some(bit)).collect();
    for i in (3..slots.len()).step_by(4) {
        slots[i] = None;
    }
    assert!(!matches!(
        decode_frame_slots(&slots, 1),
        Err(Error::Corrupt("too many erasures"))
    ));
}

#[test]
fn ambiguous_erasure_decode_never_delivers() {
    let decoder = test_decoder();
    // Two codewords at distance 4 with all four differing positions erased
    // are exactly tied: the decoder must refuse to guess.
    let byte = 0x5Au8;
    let word = fec_encode(byte);
    let other = (0..=u8::MAX)
        .find(|&b| (fec_encode(b) ^ word).count_ones() == 4)
        .expect("distance-4 codeword exists");
    let differing = fec_encode(other) ^ word;
    let mut samples = std::array::from_fn(|position| {
        Some(if (word >> position) & 1 != 0 {
            decoder.active
        } else {
            decoder.idle
        })
    });
    for (position, sample) in samples.iter_mut().enumerate() {
        if (differing >> position) & 1 != 0 {
            *sample = None;
        }
    }
    assert!(matches!(
        soft_fec_decode_erased(&samples, decoder),
        Err(Error::Corrupt("ambiguous FEC"))
    ));
    // The same ambiguity inside a frame rejects it without delivering: the
    // xor of two codewords is a codeword, so erasing its support in an
    // all-zero word ties the all-zero byte against a real alternative.
    let payload: Vec<_> = (0..16).map(|i| i as u8).collect();
    let bits = encoded_frame(&payload, crc32_of(&payload));
    let mut slots: Vec<_> = bits.iter().map(|&bit| Some(bit)).collect();
    for position in 0..FEC_BITS {
        if (differing >> position) & 1 != 0 {
            slots[26 + position * FEC_BLOCK_BYTES] = None;
        }
    }
    assert!(matches!(
        decode_frame_slots(&slots, 1),
        Err(Error::Corrupt("ambiguous FEC"))
    ));
}

#[test]
fn frame_lifetime_bound_discards_stale_frames() {
    let decoder = test_decoder();
    let mut receiver = FrameReceiver::default();
    assert!(receiver
        .push_sample(Some(decoder.idle), decoder, TEST_SLOT)
        .unwrap()
        .is_none());
    // A partial frame older than four times its expected airtime is stale.
    receiver.began = Some(Instant::now() - Duration::from_secs(60));
    assert!(matches!(
        receiver.push_sample(Some(decoder.idle), decoder, TEST_SLOT),
        Err(Error::Corrupt("frame lifetime exceeded"))
    ));
}

// ---- TX overrun skip-and-resume (data region) ----

// Capture slots sized so the busy-wait and short sleeps of the send path
// keep the schedule; an injected rewind then deterministically synthesizes
// a deschedule of whole slots.
const SKIP_SLOT: Duration = Duration::from_millis(3);

/// One capture attempt: the send result plus whether the armed injection
/// fired before the frame completed or aborted.
struct Capture {
    result: Result<(Vec<bool>, u32), Error>,
    inject_fired: bool,
}

/// Drive the real send path over the FEC-protected region exactly as
/// send_blob does after the preamble: the two-byte length block, then
/// payload+CRC body blocks. On success the result holds one wire bit per
/// grid slot (skipped slots are false = idle, which is what the receiver
/// measures) plus the frame's skip counter. `inject` arms a one-shot
/// synthetic deschedule: (slot counter, whole slots) applied inside the
/// send path.
fn capture_data_region(payload: &[u8], slot: Duration, inject: Option<(u64, u32)>) -> Capture {
    let mut phy = Phy::new(PhyKind::Cpu, slot, Arc::new(AtomicBool::new(false)));
    phy.tx_slots = Some(0);
    phy.tx_skip = Some(TxSkip {
        data: true,
        ..TxSkip::default()
    });
    phy.tx_skip_inject = inject;
    phy.tx_capture = Some(Vec::new());
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut encoded = payload.to_vec();
    encoded.extend_from_slice(&crc32_of(payload).to_be_bytes());
    let result: Result<(), Error> = (|| {
        phy.send_fec_block(&(payload.len() as u16).to_be_bytes(), deadline)?;
        for block in encoded.chunks(FEC_BLOCK_BYTES) {
            phy.send_fec_block(block, deadline)?;
        }
        Ok(())
    })();
    let inject_fired = inject.is_none() || phy.tx_skip_inject.is_none();
    let result = result.map(|()| {
        let skipped = phy.tx_skip.as_ref().map_or(0, |tx| tx.skipped);
        (phy.tx_capture.take().expect("capture"), skipped)
    });
    Capture {
        result,
        inject_fired,
    }
}

/// Decode captured wire bits through the real receiver path: skipped slots
/// arrive as idle samples at their absolute positions, never as shifts.
fn decode_captured(wire: &[bool], chunk_size: usize) -> Result<Option<Vec<u8>>, Error> {
    let slots: Vec<_> = wire.iter().map(|&bit| Some(bit)).collect();
    decode_frame_slots(&slots, chunk_size)
}

/// Capture with retries: the injected skip is deterministic, but the test
/// machine itself can be descheduled past a 3ms slot and abort the frame
/// exactly like the field traces that motivated this change. An abort
/// caused by the skip logic is deterministic and fails every retry, so
/// retrying environmental aborts cannot mask a logic regression.
fn capture_with_retries(
    payload: &[u8],
    slot: Duration,
    inject: Option<(u64, u32)>,
) -> (Vec<bool>, u32) {
    for _ in 0..24 {
        let attempt = capture_data_region(payload, slot, inject);
        if let Ok(captured) = attempt.result {
            assert!(attempt.inject_fired, "injection never fired");
            return captured;
        }
    }
    panic!("data-region capture kept aborting");
}

/// Assert the armed skip aborts the frame once it fires. Environmental
/// aborts before the armed slot are retried; once the injection has fired,
/// a successful send would prove the budget is not enforced.
fn capture_expect_abort(payload: &[u8], slot: Duration, inject: (u64, u32)) {
    for _ in 0..32 {
        let attempt = capture_data_region(payload, slot, Some(inject));
        if !attempt.inject_fired {
            continue;
        }
        assert!(matches!(
            attempt.result,
            Err(Error::Corrupt("missed physical slot"))
        ));
        return;
    }
    panic!("injection never fired");
}

#[test]
fn data_region_skip_within_budget_decodes_without_shifting() {
    // Body is exactly one full 8-byte block (budget 1); the 26-slot length
    // block precedes it. Data-region slot 26 is the first body symbol; 26,
    // 60 and 129 cover the block's start, middle and last slot.
    let payload: Vec<u8> = (0..4).map(|i| 0x30u8 + i * 7).collect();
    for at in [26u64, 60, 129] {
        let (wire, skipped) = capture_with_retries(&payload, SKIP_SLOT, Some((at, 1)));
        assert_eq!(skipped, 1);
        assert_eq!(wire.len(), 26 + 13 * 8);
        assert!(!wire[at as usize], "a skipped slot emits nothing");
        assert_eq!(
            decode_captured(&wire, 5).unwrap(),
            Some(payload.clone()),
            "skip at {at} must decode exactly"
        );
    }
    // Reference: an undisturbed send decodes identically.
    let (wire, _) = capture_with_retries(&payload, SKIP_SLOT, None);
    assert_eq!(decode_captured(&wire, 5).unwrap(), Some(payload));
}

#[test]
fn data_region_skip_spilling_across_blocks_decodes() {
    // Two full body blocks (budget 1 each): a two-slot overrun starting at
    // the last symbol of the first block drops that symbol and the first
    // symbol of the second block, each within its own block's budget.
    let payload: Vec<u8> = (0..12).map(|i| 0x41u8 + i * 5).collect();
    let last_of_first = 26 + 103;
    let (wire, skipped) = capture_with_retries(&payload, SKIP_SLOT, Some((last_of_first, 2)));
    assert_eq!(skipped, 2);
    assert!(!wire[last_of_first as usize]);
    assert!(!wire[last_of_first as usize + 1]);
    assert_eq!(decode_captured(&wire, 7).unwrap(), Some(payload));
}

#[test]
fn data_region_skip_beyond_budget_aborts() {
    let payload: Vec<u8> = (0..8).map(|i| 0x30u8 + i * 7).collect();
    // Two skipped slots in one full block exceed floor(3*8/13) = 1.
    capture_expect_abort(&payload, SKIP_SLOT, (60, 2));
    // The two-byte length block has budget floor(3*2/13) = 0: any skip
    // aborts immediately.
    capture_expect_abort(&payload, SKIP_SLOT, (0, 1));
    // A skip spilling into the 4-byte tail block aborts: its budget is
    // floor(3*4/13) = 0.
    capture_expect_abort(&payload, SKIP_SLOT, (26 + 103, 2));
}

#[test]
fn training_region_and_trailing_idle_overrun_stays_strict() {
    let slot = Duration::from_millis(3);
    let payload = b"strict";
    // Absolute send_blob slot indices: inter-frame gap 0..4, carrier
    // 4..36, idle gap 36..44, preamble 44..76, then data. Any overrun in
    // the training region aborts even with skip support active.
    // The armed abort is deterministic; an environmental overrun can
    // still abort the send before the armed slot is reached, exactly like
    // the field traces behind this change. Retry those; once the injection
    // fired, the armed slot itself must have done the aborting.
    let send_until_fired = |at: u64, attempts: usize| {
        for _ in 0..attempts {
            let mut phy = Phy::new(PhyKind::Cpu, slot, Arc::new(AtomicBool::new(false)));
            phy.tx_skip_inject = Some((at, 1));
            assert!(matches!(
                phy.send_blob(payload, Duration::from_secs(30)),
                Err(Error::Corrupt("missed physical slot"))
            ));
            if phy.tx_skip_inject.is_none() {
                return;
            }
        }
        panic!("armed slot {at} never reached");
    };
    for at in [1u64, 10, 40, 50] {
        send_until_fired(at, 16);
    }
    // The trailing idle also stays strict: the very last slot of the frame.
    let total = 76 + 26 + 13 * (payload.len() + 4) + 2;
    send_until_fired(total as u64 - 1, 32);
}

#[test]
fn send_blob_tolerates_data_region_skip_end_to_end() {
    let slot = Duration::from_millis(3);
    let payload = b"strict";
    // Absolute slot 112 lands in the full 8-byte body block (budget 1):
    // 76 training slots, 26 length slots, then the body. The injected skip
    // is deterministic; environmental overruns in zero-budget blocks or
    // the strict regions abort the send and are retried.
    for _ in 0..24 {
        let mut phy = Phy::new(PhyKind::Cpu, slot, Arc::new(AtomicBool::new(false)));
        phy.tx_skip_inject = Some((112, 1));
        match phy.send_blob(payload, Duration::from_secs(30)) {
            Ok(()) => {
                assert!(phy.tx_skip_inject.is_none());
                return;
            }
            Err(Error::Corrupt("missed physical slot")) => {}
            Err(other) => panic!("unexpected send error: {other}"),
        }
    }
    panic!("send_blob kept aborting");
}

#[test]
fn tx_skip_preserves_the_absolute_slot_grid() {
    // Large slots: the test never waits and a scheduler pause far below a
    // slot cannot disturb the boundaries.
    let slot = Duration::from_millis(50);
    let mut phy = Phy::new(PhyKind::Cpu, slot, Arc::new(AtomicBool::new(false)));
    phy.tx_slots = Some(0);
    phy.tx_skip = Some(TxSkip {
        data: true,
        ..TxSkip::default()
    });
    phy.tx_begin_block(8);
    let anchor = Instant::now();
    phy.next_slot = Some(anchor);
    let deadline = Instant::now() + Duration::from_secs(30);
    // A normal begin stays on the grid.
    let end = phy.data_slot_end(deadline).unwrap().unwrap();
    assert_eq!(end, anchor + slot);
    // A one-slot deschedule skips the slot and resumes on the grid: the
    // current partial slot still ends at the original boundary.
    phy.tx_skip_inject = Some((1, 1));
    assert!(phy.data_slot_end(deadline).unwrap().is_none());
    assert_eq!(phy.next_slot, Some(anchor + slot * 2));
    assert_eq!(phy.tx_skip.as_ref().unwrap().drop_pending, 0);
    let end = phy.data_slot_end(deadline).unwrap().unwrap();
    assert_eq!(end, anchor + slot * 3);
    assert_eq!(phy.tx_slots, Some(3));
    // A deschedule landing in the last half of the resume slot skips that
    // slot too: an active symbol filling less than half a slot reads as
    // idle at the receiver without being accounted. The grid still never
    // re-anchors: the resume point is start + 2 slots, not now.
    let start = anchor - slot - slot / 4 * 3;
    phy.next_slot = Some(start);
    assert!(phy.data_slot_end(deadline).unwrap().is_none());
    assert_eq!(phy.next_slot, Some(start + slot * 2));
    assert_eq!(phy.tx_skip.as_ref().unwrap().drop_pending, 1);
    assert_eq!(phy.tx_slots, Some(5));
}

#[test]
fn skip_of_an_active_symbol_is_corrected_by_fec() {
    let payload: Vec<u8> = (0..4).map(|i| 0xA1u8 ^ (i * 13)).collect();
    let bits = encoded_frame(&payload, crc32_of(&payload));
    // Capture indices match encoded_frame: both start at the length block.
    let at = (26..26 + 104).find(|&i| bits[i]).expect("active body bit");
    let (wire, skipped) = capture_with_retries(&payload, SKIP_SLOT, Some((at as u64, 1)));
    assert_eq!(skipped, 1);
    assert!(bits[at], "the intended symbol was active");
    assert!(!wire[at], "the skipped active slot reads idle");
    assert_eq!(decode_captured(&wire, 3).unwrap(), Some(payload));
}

#[test]
fn budget_forbids_two_skips_in_one_word_by_construction() {
    // Interleave map: block slot i feeds word i % block_bytes at position
    // i / block_bytes, so two slots of the same word sit in the same block.
    // A second skip in one word therefore needs two skips in one block,
    // which the per-block budget forbids for every possible block size.
    for block_bytes in 1..=FEC_BLOCK_BYTES {
        let budget = MAX_WORD_ERASURES * block_bytes / FEC_BITS;
        assert!(
            budget <= 1,
            "block of {block_bytes} bytes allows {budget} skips"
        );
    }
    // Enforcement: the first skip is accounted; a second anywhere in the
    // block aborts, whether it hits the same word or a different one.
    let mut phy = Phy::new(
        PhyKind::Cpu,
        Duration::from_millis(3),
        Arc::new(AtomicBool::new(false)),
    );
    phy.tx_skip = Some(TxSkip {
        data: true,
        ..TxSkip::default()
    });
    phy.tx_begin_block(8);
    phy.tx_account_skip(0).unwrap();
    assert!(
        matches!(
            phy.tx_account_skip(8),
            Err(Error::Corrupt("missed physical slot"))
        ),
        "second skip in the same word must abort"
    );
    let mut phy = Phy::new(
        PhyKind::Cpu,
        Duration::from_millis(3),
        Arc::new(AtomicBool::new(false)),
    );
    phy.tx_skip = Some(TxSkip {
        data: true,
        ..TxSkip::default()
    });
    phy.tx_begin_block(8);
    phy.tx_account_skip(0).unwrap();
    assert!(
        matches!(
            phy.tx_account_skip(1),
            Err(Error::Corrupt("missed physical slot"))
        ),
        "second skip in the same block must abort"
    );
}

#[test]
fn sample_frame_slot_counts_missed_slots_on_the_absolute_grid() {
    // Anchors are only ever in the past, so the test never waits and a
    // scheduler pause needs to exceed half a slot to disturb the counts.
    let slot = Duration::from_millis(20);
    let mut phy = Phy::new(PhyKind::Cpu, slot, Arc::new(AtomicBool::new(false)));
    phy.decoder = Some(test_decoder());
    // Three and a half slots elapsed since the grid anchor: three erasures
    // in place, and the current partial slot is still measured, never
    // skipped or shifted.
    phy.next_slot = Some(Instant::now() - slot * 3 - slot / 2);
    let observed = phy
        .sample_frame_slot(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert_eq!(observed.missed, 3);
    assert!(observed.sample.is_some());
    assert!(phy.next_slot.unwrap() > Instant::now() - slot);
    // An anchor inside the current slot misses nothing.
    phy.next_slot = Some(Instant::now() - slot / 2);
    let observed = phy
        .sample_frame_slot(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert_eq!(observed.missed, 0);
    assert!(observed.sample.is_some());
    // A resumed integral whose interval already ended is an erasure, not an
    // error; the grid advances to the next slot untouched.
    let past = Instant::now() - slot;
    phy.integral = Some(Integral {
        end: Some(past),
        total: Duration::ZERO,
        count: 0,
        paused_at: Some(past),
    });
    let observed = phy
        .sample_frame_slot(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert_eq!(observed.missed, 0);
    assert!(observed.sample.is_none());
}
