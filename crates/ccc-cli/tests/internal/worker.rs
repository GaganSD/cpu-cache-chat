use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use ccc_core::{Delivery, Error};
use ccc_protocol::{encode_frame, Frame, FrameType, Wire};

use super::{spawn_worker, stopped_event, update_channel, ObservedWire, Update};

#[test]
fn slow_ui_snapshots_coalesce_and_overflow_is_fatal() {
    let stop = Arc::new(AtomicBool::new(false));
    let (output, updates) = update_channel(stop.clone());
    let cfg = ccc_core::RuntimeConfig::new(
        ccc_core::Role::A,
        ccc_core::ChannelMode::Sim,
        ccc_core::AppKind::Chat,
    );
    let session = ccc_app::Session::wrap(
        cfg,
        TestWire {
            fail_send: Arc::new(AtomicBool::new(false)),
            receiving: Arc::new(AtomicBool::new(false)),
        },
        1,
    );
    for _ in 0..1000 {
        output.send(Update::Snapshot(session.snapshot())).unwrap();
    }
    assert!(matches!(updates.try_recv(), Ok(Update::Snapshot(_))));
    assert!(matches!(updates.try_recv(), Err(mpsc::TryRecvError::Empty)));
    for id in 0..128 {
        output.send(Update::Accepted(id)).unwrap();
    }
    assert!(output.send(Update::Rejected(999, "no".into())).is_err());
    assert!(stop.load(Ordering::SeqCst));
    assert!(matches!(
        updates.try_recv(),
        Ok(Update::Fatal(Error::Io(_)))
    ));
}

#[test]
fn fatal_outcome_survives_frontend_quit_and_overflow_marks_evidence_unsuccessful() {
    let stop = Arc::new(AtomicBool::new(false));
    let (output, updates) = update_channel(stop);
    for id in 0..128 {
        output.send(Update::Accepted(id)).unwrap();
    }
    assert!(output.send(Update::Accepted(128)).is_err());
    drop(updates);
    assert_eq!(
        stopped_event(&output, true),
        serde_json::json!({"type":"stopped","successful":false,"evidence_complete":false})
    );
    assert!(!output.successful());
    assert!(matches!(output.take_failure(), Some(Error::Channel(_))));

    let (output, updates) = update_channel(Arc::new(AtomicBool::new(false)));
    drop(updates);
    output
        .send(Update::Fatal(Error::Corrupt("racing fatal")))
        .unwrap();
    assert!(!output.successful());
    assert!(matches!(
        output.take_failure(),
        Some(Error::Corrupt("racing fatal"))
    ));
}

#[test]
fn worker_join_reports_fatal_after_frontend_quits() {
    let (output, updates) = update_channel(Arc::new(AtomicBool::new(false)));
    let (resume, wait) = mpsc::channel();
    let worker = spawn_worker(output, move |output| {
        wait.recv().unwrap();
        output
            .send(Update::Fatal(Error::Corrupt("racing fatal")))
            .unwrap();
    });
    drop(updates);
    resume.send(()).unwrap();
    assert!(matches!(
        worker.join().unwrap(),
        Err(Error::Corrupt("racing fatal"))
    ));
}

struct TestWire {
    fail_send: Arc<AtomicBool>,
    receiving: Arc<AtomicBool>,
}

impl Wire for TestWire {
    fn send_blob(&mut self, _bytes: &[u8], _timeout: Duration) -> Result<(), Error> {
        if self.fail_send.load(Ordering::SeqCst) {
            Err(Error::Corrupt("test send failure"))
        } else {
            Ok(())
        }
    }

    fn recv_blob(&mut self, _timeout: Duration) -> Result<Vec<u8>, Error> {
        Err(Error::Timeout)
    }

    fn receive_in_progress(&self) -> bool {
        self.receiving.load(Ordering::SeqCst)
    }

    fn interrupt(&mut self) {}
}

#[test]
fn failed_data_attempt_is_not_reported_as_a_final_delivery_state() {
    let fail_send = Arc::new(AtomicBool::new(true));
    let receiving = Arc::new(AtomicBool::new(false));
    let (output, updates) = update_channel(Arc::new(AtomicBool::new(false)));
    let mut wire = ObservedWire {
        inner: Box::new(TestWire {
            fail_send: fail_send.clone(),
            receiving,
        }),
        output,
    };
    let data = encode_frame(&Frame::new(FrameType::Data, 12, 7, vec![1]).unwrap()).unwrap();
    assert!(matches!(
        wire.send_blob(&data, Duration::from_secs(1)),
        Err(Error::Corrupt("test send failure"))
    ));
    assert!(matches!(
        updates.try_recv(),
        Ok(Update::Delivery(7, Delivery::Transmitting))
    ));
    assert!(matches!(updates.try_recv(), Err(mpsc::TryRecvError::Empty)));

    fail_send.store(false, Ordering::SeqCst);
    wire.send_blob(&data, Duration::from_secs(1)).unwrap();
    assert!(matches!(
        updates.try_recv(),
        Ok(Update::Delivery(7, Delivery::Transmitting))
    ));
    assert!(matches!(updates.try_recv(), Err(mpsc::TryRecvError::Empty)));
}

#[test]
fn wrapper_forwards_partial_receive_and_does_not_label_control_frames() {
    let receiving = Arc::new(AtomicBool::new(false));
    let (output, updates) = update_channel(Arc::new(AtomicBool::new(false)));
    let mut wire = ObservedWire {
        inner: Box::new(TestWire {
            fail_send: Arc::new(AtomicBool::new(false)),
            receiving: receiving.clone(),
        }),
        output,
    };
    assert!(!wire.receive_in_progress());
    receiving.store(true, Ordering::SeqCst);
    assert!(wire.receive_in_progress());
    let heartbeat = encode_frame(&Frame::control(FrameType::Heartbeat, 12, 1, 0)).unwrap();
    wire.send_blob(&heartbeat, Duration::from_secs(1)).unwrap();
    assert!(matches!(updates.try_recv(), Err(mpsc::TryRecvError::Empty)));
}
