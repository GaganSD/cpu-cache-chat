use std::time::Duration;

use ccc_channel::SimPair;
use ccc_protocol::Wire;

#[test]
fn sim_pair_delivers_bytes() {
    let mut pair = SimPair::new();
    pair.a.send_blob(b"abc", Duration::from_millis(10)).unwrap();
    assert_eq!(pair.b.recv_blob(Duration::from_millis(10)).unwrap(), b"abc");
}

#[test]
fn sim_pair_can_drop() {
    let mut pair = SimPair::lossy(Some(1));
    pair.a.send_blob(b"x", Duration::from_millis(10)).unwrap();
    assert!(pair.b.recv_blob(Duration::from_millis(15)).is_err());
}
