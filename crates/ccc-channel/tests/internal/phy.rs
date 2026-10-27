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

fn decode_frame_bits(bits: &[bool], chunk_size: usize) -> Result<Option<Vec<u8>>, Error> {
    let mut receiver = FrameReceiver::default();
    let decoder = Decoder {
        threshold: Duration::from_micros(15),
        active_high: true,
        active: Duration::from_micros(20),
        idle: Duration::from_micros(10),
    };
    let mut decoded = None;
    for chunk in bits.chunks(chunk_size) {
        for &bit in chunk {
            let sample = if bit { decoder.active } else { decoder.idle };
            let result = receiver.push_sample(sample, decoder)?;
            if result.is_some() {
                decoded = result;
            }
        }
    }
    Ok(decoded)
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
            .push_sample(Duration::from_nanos(cost), decoder)
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
