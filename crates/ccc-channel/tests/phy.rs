use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ccc_channel::phy::{
    bits_msb, crc32_of, lock_quality, matches_training, train_lock, train_lock_erased,
    train_preamble,
};
use ccc_channel::{run_silent_control, Phy, PhyKind};
use ccc_core::{ChannelMode, Error, Role};

fn training(inverted: bool) -> Vec<Duration> {
    let carrier = [true; 8].into_iter();
    carrier
        .chain(bits_msb(0xaaaa_aaaa, 32))
        .enumerate()
        .map(|(i, active)| {
            Duration::from_nanos(if active ^ inverted {
                200 + (i % 3) as u64
            } else {
                100 + (i % 3) as u64
            })
        })
        .collect()
}

#[test]
fn wire_bits_and_crc() {
    assert_eq!(
        bits_msb(0xa5, 8).collect::<Vec<_>>(),
        vec![true, false, true, false, false, true, false, true]
    );
    assert_eq!(
        bits_msb(0x1234, 16).fold(0, |v, bit| (v << 1) | u32::from(bit)),
        0x1234
    );
    assert_eq!(crc32_of(b"123456789"), 0xcbf4_3926);
    assert_eq!(crc32_of(b""), 0);
}

#[test]
fn trained_threshold_handles_both_polarities_and_rejects_noise() {
    for inverted in [false, true] {
        let decoder = train_preamble(&training(inverted)).unwrap();
        assert!(decoder.decode(Duration::from_nanos(if inverted { 101 } else { 201 })));
        assert!(!decoder.decode(Duration::from_nanos(if inverted { 201 } else { 101 })));
    }
    assert!(train_preamble(&vec![Duration::from_nanos(100); 40]).is_none());
    let mut one = training(false);
    one[12] = Duration::from_nanos(100);
    assert!(train_preamble(&one).is_some());
    let mut bad = training(false);
    for slot in bad.iter_mut().skip(8).take(16) {
        *slot = Duration::from_nanos(100);
    }
    assert!(train_preamble(&bad).is_none());
}

#[test]
fn interrupt_stops_waits_and_silent_control_cannot_be_usable() {
    let stop = Arc::new(AtomicBool::new(false));
    let mut phy = Phy::new(PhyKind::Cpu, Duration::from_secs(1), stop.clone());
    let signal = stop.clone();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(10));
        signal.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let start = Instant::now();
    assert!(matches!(
        phy.send_blob(b"x", Duration::from_secs(3)),
        Err(Error::Interrupted)
    ));
    assert!(start.elapsed() < Duration::from_millis(500));
    thread.join().unwrap();
    let report = run_silent_control(
        ChannelMode::Cpu,
        Role::B,
        Duration::from_millis(1),
        Arc::new(AtomicBool::new(false)),
        Duration::from_millis(10),
    )
    .unwrap();
    assert!(!report.usable);
    assert!(report.notes.contains("Silent control"));
}

#[test]
fn hunt_resynchronizes_when_the_probe_exceeds_its_quantum() {
    // A CPU probe is much slower than a quarter microsecond, forcing missed
    // intervals; losing acquisition samples must not be reported as a frame.
    let report = run_silent_control(
        ChannelMode::Cpu,
        Role::B,
        Duration::from_micros(50),
        Arc::new(AtomicBool::new(false)),
        Duration::from_millis(20),
    )
    .unwrap();
    assert_eq!(
        report.trials[0].outcome,
        ccc_core::ProbeOutcome::SilentNoFrame
    );
    assert_eq!(report.silent_false_ones, None);
}

#[test]
fn training_match_requires_carrier_gap_and_preamble() {
    let training = (u128::from(u32::MAX) << 40) | u128::from(0xaaaa_aaaau32);
    assert!(matches_training(training));
    assert!(matches_training(training ^ 0b1111));
    assert!(!matches_training(training ^ 0b1_1111));
    assert!(!matches_training(u128::from(0xaaaa_aaaau32)));
    assert!(!matches_training(training | (0xffu128 << 32)));
}

#[test]
fn lock_training_uses_carrier_gap_and_both_polarities() {
    for inverted in [false, true] {
        let bits = std::iter::repeat_n(true, 32)
            .chain(std::iter::repeat_n(false, 8))
            .chain(bits_msb(0xaaaa_aaaa, 32));
        // A 50% increase must work: requiring twice the idle value would fail.
        let mut samples: Vec<_> = bits
            .enumerate()
            .map(|(i, active)| {
                let ns = if active ^ inverted { 15_000 } else { 10_000 };
                Duration::from_nanos(ns + (i % 7) as u64 * 10)
            })
            .collect();
        // A few isolated scheduler outliers must not move the medians.
        samples[2] = Duration::from_micros(200);
        let decoder = train_lock(&samples).unwrap();
        assert!(decoder.decode(Duration::from_nanos(if inverted { 10_000 } else { 15_000 })));
        assert!(!decoder.decode(Duration::from_nanos(if inverted { 15_000 } else { 10_000 })));
        samples[32..40].fill(if inverted {
            Duration::from_micros(10)
        } else {
            Duration::from_micros(15)
        });
        assert!(
            train_lock(&samples).is_none(),
            "carrier without the idle gap is not training"
        );
    }
    assert!(train_lock(&[Duration::from_micros(10); 72]).is_none());
    assert!(train_lock(&[Duration::from_micros(10); 71]).is_none());
}

#[test]
fn lock_training_rejects_stationary_noise_and_slow_drift() {
    let mut seed = 42u64;
    let mut noise = Vec::new();
    for _ in 0..1024 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        noise.push(Duration::from_nanos(10_000 + ((seed >> 32) % 1_000)));
    }
    assert!(noise.windows(72).all(|window| train_lock(window).is_none()));
    let drift: Vec<_> = (0..200)
        .map(|i| Duration::from_nanos(10_000 + i * 50))
        .collect();
    assert!(drift.windows(72).all(|window| train_lock(window).is_none()));
}

#[test]
fn lock_training_rejects_shifted_periodic_preamble() {
    for inverted in [false, true] {
        let bits = std::iter::repeat_n(false, 4)
            .chain(std::iter::repeat_n(true, 32))
            .chain(std::iter::repeat_n(false, 8))
            .chain(bits_msb(0xaaaa_aaaa, 32))
            .chain(std::iter::repeat_n(false, 8));
        let samples: Vec<_> = bits
            .map(|active| Duration::from_micros(if active ^ inverted { 20 } else { 10 }))
            .collect();
        assert!(train_lock(&samples[4..76]).is_some());
        for start in (0..=8).filter(|&start| start != 4) {
            assert!(
                train_lock(&samples[start..start + 72]).is_none(),
                "incorrectly locked at offset {}",
                start as isize - 4
            );
        }
    }
}

#[test]
fn phase_quality_prefers_well_centered_preamble() {
    let bits = std::iter::repeat_n(true, 32)
        .chain(std::iter::repeat_n(false, 8))
        .chain(bits_msb(0xaaaa_aaaa, 32));
    let centered: Vec<_> = bits
        .map(|active| Duration::from_micros(if active { 20 } else { 10 }))
        .collect();
    let mut boundary = centered.clone();
    for (i, sample) in boundary.iter_mut().enumerate().skip(40) {
        *sample = Duration::from_micros(if i % 2 == 0 { 18 } else { 12 });
    }
    assert!(train_lock(&centered).is_some());
    assert!(train_lock(&boundary).is_some());
    assert!(lock_quality(&centered).unwrap() < lock_quality(&boundary).unwrap());
}

#[test]
fn acquisition_preserves_phase_across_explicit_erasures() {
    for inverted in [false, true] {
        let bits = std::iter::repeat_n(false, 4)
            .chain(std::iter::repeat_n(true, 32))
            .chain(std::iter::repeat_n(false, 8))
            .chain(bits_msb(0xaaaa_aaaa, 32))
            .chain(std::iter::repeat_n(false, 4));
        let samples: Vec<_> = bits
            .map(|active| {
                Some(Duration::from_micros(if active ^ inverted {
                    20
                } else {
                    10
                }))
            })
            .collect();
        let mut aligned = samples[4..76].to_vec();
        for i in [3, 9, 18, 25, 34, 36, 42, 47, 52, 61, 69] {
            aligned[i] = None;
        }
        assert!(train_lock_erased(&aligned).is_some());
        let mut early = samples[2..74].to_vec();
        for i in [3, 9, 18, 25, 34, 36, 42, 47, 52, 61, 69] {
            early[i] = None;
        }
        assert!(train_lock_erased(&early).is_none());
        aligned[32] = None;
        assert!(
            train_lock_erased(&aligned).is_none(),
            "gap edge is needed to rule out a shifted lock"
        );
    }
    let mostly_missing = vec![None; 72];
    assert!(train_lock_erased(&mostly_missing).is_none());
}

#[test]
fn acquisition_tolerates_scheduler_gap_bursts() {
    // Scheduler descheduling loses consecutive quarter-slot measurements,
    // not scattered ones. Bursts inside the carrier and preamble must still
    // train; a burst that erases the idle-gap edges must not.
    for inverted in [false, true] {
        let bits = std::iter::repeat_n(true, 32)
            .chain(std::iter::repeat_n(false, 8))
            .chain(bits_msb(0xaaaa_aaaa, 32));
        let mut samples: Vec<_> = bits
            .map(|active| {
                Some(Duration::from_micros(if active ^ inverted {
                    20
                } else {
                    10
                }))
            })
            .collect();
        for start in [4, 20, 48, 60] {
            for sample in &mut samples[start..start + 4] {
                *sample = None;
            }
        }
        let decoder = train_lock_erased(&samples).expect("burst erasures still train");
        assert!(decoder.decode(Duration::from_micros(if inverted { 10 } else { 20 })));
        assert!(!decoder.decode(Duration::from_micros(if inverted { 20 } else { 10 })));
        samples[32] = None;
        samples[33] = None;
        assert!(
            train_lock_erased(&samples).is_none(),
            "a burst across the gap edge must not lock"
        );
    }
}

// Sparse 72-slot hunt windows for the acquisition hardening tests: erasures
// and corruptions are absolute window positions, never shifts. The lowered
// observation floors are 16 carrier, 4 gap and 16 preamble quarter-samples.
fn sparse_training_window(
    inverted: bool,
    erase: &[usize],
    corrupt: &[usize],
) -> Vec<Option<Duration>> {
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

// Erase sets leaving exactly the new minima: 16 carrier, the four mandatory
// gap anchors, and 16 preamble quarter-samples.
const SPARSE_CARRIER_ERASE: [usize; 16] =
    [0, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28, 30];
const SPARSE_GAP_ERASE: [usize; 4] = [34, 35, 36, 37];
const SPARSE_PREAMBLE_ERASE: [usize; 16] = [
    40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55,
];

fn sparse_erase_set() -> Vec<usize> {
    SPARSE_CARRIER_ERASE
        .into_iter()
        .chain(SPARSE_GAP_ERASE)
        .chain(SPARSE_PREAMBLE_ERASE)
        .collect()
}

#[test]
fn acquisition_locks_at_lowered_erasure_bounds() {
    for inverted in [false, true] {
        let window = sparse_training_window(inverted, &sparse_erase_set(), &[]);
        let decoder = train_lock_erased(&window).expect("sparse window at the new bounds locks");
        assert!(decoder.decode(Duration::from_micros(if inverted { 10 } else { 20 })));
        assert!(!decoder.decode(Duration::from_micros(if inverted { 20 } else { 10 })));
    }
}

#[test]
fn acquisition_still_requires_mandatory_gap_anchors() {
    for inverted in [false, true] {
        for anchor in [32, 33, 38, 39] {
            // Every other gap slot stays observed, so the gap count floor
            // passes; the missing anchor alone must reject.
            let erase: Vec<usize> = SPARSE_CARRIER_ERASE
                .into_iter()
                .chain(SPARSE_PREAMBLE_ERASE)
                .chain([anchor])
                .collect();
            let window = sparse_training_window(inverted, &erase, &[]);
            assert!(
                train_lock_erased(&window).is_none(),
                "anchor {anchor} is mandatory even at the lowered bounds"
            );
        }
    }
}

#[test]
fn acquisition_rejects_two_slot_aliases_at_lowered_bounds() {
    for inverted in [false, true] {
        // Stream: four idle slots, the training sequence, four idle slots.
        let stream: Vec<_> = std::iter::repeat_n(false, 4)
            .chain(std::iter::repeat_n(true, 32))
            .chain(std::iter::repeat_n(false, 8))
            .chain(bits_msb(0xaaaa_aaaa, 32))
            .chain(std::iter::repeat_n(false, 4))
            .map(|active| {
                Some(Duration::from_micros(if active ^ inverted {
                    20
                } else {
                    10
                }))
            })
            .collect();
        // Control: the aligned window, sparsely erased to the new bounds.
        let mut aligned = stream[4..76].to_vec();
        for &i in &sparse_erase_set() {
            aligned[i] = None;
        }
        assert!(train_lock_erased(&aligned).is_some());
        // Two slots early: every count gate passes (16 carrier, the four
        // anchors, 16 preamble) with zero carrier and preamble errors; only
        // the zero-gap-error guard can reject, because anchors 32 and 33
        // hold carrier spillover.
        let mut early = stream[2..74].to_vec();
        let early_erase: Vec<usize> = [0, 1, 40, 41]
            .into_iter()
            .chain((2..32).step_by(2).take(14))
            .chain([34, 35, 36, 37])
            .chain((42..72).step_by(2).take(14))
            .collect();
        for &i in &early_erase {
            early[i] = None;
        }
        assert!(
            train_lock_erased(&early).is_none(),
            "two-slot-early alias must not lock at the lowered bounds"
        );
        // Two slots late: again every count gate passes with zero carrier
        // and preamble errors; anchor 38 holds preamble spillover, so the
        // gap guard alone rejects.
        let mut late = stream[6..78].to_vec();
        let late_erase: Vec<usize> = [30, 31, 70]
            .into_iter()
            .chain((0..30).step_by(2).take(14))
            .chain([34, 35, 36, 37])
            .chain((40..70).step_by(2).take(15))
            .collect();
        for &i in &late_erase {
            late[i] = None;
        }
        assert!(
            train_lock_erased(&late).is_none(),
            "two-slot-late alias must not lock at the lowered bounds"
        );
    }
}

#[test]
fn acquisition_never_locks_noise_or_silence_under_erasures() {
    // Pure silence: no measurements at all, or a constant-cost line, fully
    // observed or thinned out.
    assert!(train_lock_erased(&vec![None; 72]).is_none());
    assert!(train_lock_erased(&vec![Some(Duration::from_micros(10)); 72]).is_none());
    let mut sparse_flat = vec![Some(Duration::from_micros(10)); 72];
    for i in (0..72).step_by(3) {
        sparse_flat[i] = None;
    }
    assert!(train_lock_erased(&sparse_flat).is_none());
    // Stationary noise with a third of the quarter-samples erased: no window
    // of the stream may lock, however the erasures line up.
    let mut seed = 7u64;
    let mut stream = Vec::new();
    for _ in 0..512 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        stream.push(if (seed >> 33).is_multiple_of(3) {
            None
        } else {
            Some(Duration::from_nanos(10_000 + ((seed >> 32) % 1_000)))
        });
    }
    assert!(stream
        .windows(72)
        .all(|window| train_lock_erased(window).is_none()));
}

#[test]
fn acquisition_below_old_bounds_locks_only_when_all_gates_pass() {
    for inverted in [false, true] {
        // 19 carrier observations: below the old floor of 20, above the new
        // floor of 16. Clean evidence at every other gate must now lock.
        let nineteen_carrier: Vec<usize> = (0..32)
            .step_by(2)
            .take(13)
            .chain(SPARSE_GAP_ERASE)
            .chain(SPARSE_PREAMBLE_ERASE)
            .collect();
        let window = sparse_training_window(inverted, &nineteen_carrier, &[]);
        assert!(
            train_lock_erased(&window).is_some(),
            "19/32 carrier with clean gap and preamble must lock"
        );
        // 23 preamble observations: below the old floor of 24.
        let erase: Vec<usize> = SPARSE_CARRIER_ERASE
            .into_iter()
            .chain(SPARSE_GAP_ERASE)
            .chain((40..72).step_by(2).take(9))
            .collect();
        assert!(train_lock_erased(&sparse_training_window(inverted, &erase, &[])).is_some());
        // Five gap observations: below the old floor of six, anchors kept.
        let erase: Vec<usize> = SPARSE_CARRIER_ERASE
            .into_iter()
            .chain([34, 35, 36])
            .chain(SPARSE_PREAMBLE_ERASE)
            .collect();
        assert!(train_lock_erased(&sparse_training_window(inverted, &erase, &[])).is_some());
        // The same 19-carrier window with three carrier errors: the scaled
        // budget min(4, 19/8) = 2 still rejects.
        let window = sparse_training_window(inverted, &nineteen_carrier, &[1, 3, 5]);
        assert!(
            train_lock_erased(&window).is_none(),
            "three carrier errors exceed the budget for 19 observations"
        );
        // Fifteen carrier observations fall below the new floor entirely.
        let erase: Vec<usize> = (0..32)
            .step_by(2)
            .chain([1])
            .chain(SPARSE_GAP_ERASE)
            .chain(SPARSE_PREAMBLE_ERASE)
            .collect();
        assert!(train_lock_erased(&sparse_training_window(inverted, &erase, &[])).is_none());
    }
}

#[test]
fn physical_sender_completes_small_frame_unchanged() {
    // Happy path: with the TX skip machinery armed, an undisturbed send
    // still completes exactly as before.
    let mut phy = Phy::new(
        PhyKind::Cpu,
        Duration::from_millis(5),
        Arc::new(AtomicBool::new(false)),
    );
    phy.send_blob(b"ok", Duration::from_secs(10)).unwrap();
}

#[test]
fn physical_sender_rejects_frames_beyond_receiver_bound() {
    let mut phy = Phy::new(
        PhyKind::Cpu,
        Duration::from_millis(1),
        Arc::new(AtomicBool::new(false)),
    );
    let bytes = vec![0; ccc_core::MAX_PAYLOAD + 65];
    assert!(matches!(
        phy.send_blob(&bytes, Duration::from_secs(1)),
        Err(Error::TooLarge(_))
    ));
}
