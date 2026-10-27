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
    assert!(report.notes.contains("unproven"));
}

#[test]
fn hunt_resynchronizes_when_the_probe_exceeds_its_quantum() {
    // A CPU probe is much slower than a quarter microsecond, forcing missed
    // intervals; losing acquisition samples must not be reported as a frame.
    let report = run_silent_control(
        ChannelMode::Cpu,
        Role::B,
        Duration::from_micros(1),
        Arc::new(AtomicBool::new(false)),
        Duration::from_millis(20),
    )
    .unwrap();
    assert_eq!(report.timeouts, 1);
    assert!(report.notes.contains("result=Err(Timeout)"));
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
