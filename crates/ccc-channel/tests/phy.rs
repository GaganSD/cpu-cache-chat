use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ccc_channel::phy::{bits_msb, crc32_of, train_preamble};
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
    let mut bad = training(false);
    bad[12] = Duration::from_nanos(100);
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
