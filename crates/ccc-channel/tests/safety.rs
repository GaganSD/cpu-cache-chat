use ccc_channel::{probe_payload_len, random_probe_payload, SimPair, SimSocket};
use ccc_core::{AppKind, ChannelMode, Role, RuntimeConfig, MAX_PHY_BYTES};
use ccc_protocol::Wire;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};

fn stop() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}
fn dir() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "ccc-test-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}
#[test]
fn socket_collisions_and_ownership() {
    let d = dir();
    let base = d.join("run");
    let path = base.to_str().unwrap();
    let a = SimSocket::bind(Role::A, path, stop()).unwrap();
    assert!(SimSocket::bind(Role::A, path, stop()).is_err());
    let b = SimSocket::bind(Role::B, path, stop()).unwrap();
    let mut other = SimSocket::bind(Role::A, d.join("other").to_str().unwrap(), stop()).unwrap();
    other
        .send_blob(b"x", Duration::from_millis(10))
        .unwrap_err();
    let mine = d.join("run.a.sock");
    fs::remove_file(&mine).unwrap();
    fs::write(&mine, b"replacement").unwrap();
    drop(a);
    assert_eq!(fs::read(&mine).unwrap(), b"replacement");
    drop(b);
    drop(other);
    fs::remove_dir_all(d).unwrap();
}
#[test]
fn refuses_existing_file_and_insecure_parent() {
    let d = dir();
    let base = d.join("run");
    let mine = d.join("run.a.sock");
    fs::write(&mine, b"keep").unwrap();
    assert!(SimSocket::bind(Role::A, base.to_str().unwrap(), stop()).is_err());
    assert_eq!(fs::read(&mine).unwrap(), b"keep");
    fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(SimSocket::bind(Role::B, base.to_str().unwrap(), stop()).is_err());
    fs::remove_dir_all(d).unwrap();
}
#[test]
fn independent_socket_pairs() {
    let d = dir();
    let p = d.join("one");
    let q = d.join("two");
    let mut a = SimSocket::bind(Role::A, p.to_str().unwrap(), stop()).unwrap();
    let mut b = SimSocket::bind(Role::B, p.to_str().unwrap(), stop()).unwrap();
    let mut c = SimSocket::bind(Role::A, q.to_str().unwrap(), stop()).unwrap();
    let mut e = SimSocket::bind(Role::B, q.to_str().unwrap(), stop()).unwrap();
    a.send_blob(b"one", Duration::from_secs(1)).unwrap();
    c.send_blob(b"two", Duration::from_secs(1)).unwrap();
    assert_eq!(b.recv_blob(Duration::from_secs(1)).unwrap(), b"one");
    assert_eq!(e.recv_blob(Duration::from_secs(1)).unwrap(), b"two");
    drop((a, b, c, e));
    assert_eq!(fs::read_dir(&d).unwrap().count(), 0);
    fs::remove_dir(d).unwrap();
}
#[test]
fn queue_is_bounded() {
    let mut pair = SimPair::new();
    for _ in 0..256 {
        pair.a.send_blob(b"x", Duration::ZERO).unwrap();
    }
    assert!(pair.a.send_blob(b"x", Duration::ZERO).is_err());
    pair.b.recv_blob(Duration::ZERO).unwrap();
    pair.a.send_blob(b"x", Duration::ZERO).unwrap();
}
#[test]
fn size_and_config_validation() {
    assert!(probe_payload_len(0).is_err());
    assert!(probe_payload_len(9).is_err());
    assert_eq!(
        probe_payload_len((MAX_PHY_BYTES * 8) as u32).unwrap(),
        MAX_PHY_BYTES
    );
    assert!(probe_payload_len((MAX_PHY_BYTES * 8 + 8) as u32).is_err());
    assert_eq!(random_probe_payload(64).unwrap().len(), 8);
    let mut c = RuntimeConfig::new(Role::A, ChannelMode::Cpu, AppKind::Chat);
    assert!(c.validate().is_ok());
    c.slot = Duration::ZERO;
    assert!(c.validate().is_err());
    c.slot = Duration::MAX;
    assert!(c.validate().is_err());
}
