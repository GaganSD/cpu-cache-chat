use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use ccc_core::{Delivery, Error};
use ccc_protocol::{encode_frame, Frame, FrameType, Wire};

use super::{ObservedWire, Update};

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
    let (output, updates) = mpsc::channel();
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
    let (output, updates) = mpsc::channel();
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
