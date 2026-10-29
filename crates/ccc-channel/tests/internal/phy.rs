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
