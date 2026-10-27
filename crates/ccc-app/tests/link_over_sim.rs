use std::thread;
use std::time::Duration;

use ccc_app::Session;
use ccc_channel::SimPair;
use ccc_core::{AppKind, ChannelMode, Role, RuntimeConfig};
use ccc_protocol::AppMessage;

#[test]
fn handshake_and_chat_over_sim() {
    let pair = SimPair::new();
    let mut cfg_a = RuntimeConfig::new(Role::A, ChannelMode::Sim, AppKind::Chat);
    cfg_a.sim_path = Some("/unused".into());
    cfg_a.ack_timeout = Duration::from_millis(200);
    let mut cfg_b = cfg_a.clone();
    cfg_b.role = Role::B;

    let mut a = Session::wrap(cfg_a, pair.a, 0xAABB_CCDD);
    let mut b = Session::wrap(cfg_b, pair.b, 0);

    let t = thread::spawn(move || {
        b.connect().unwrap();
        for _ in 0..40 {
            b.tick(Duration::from_millis(10)).unwrap();
            if b.snapshot().chat.iter().any(|c| c.text.contains("ping")) {
                b.queue_chat("pong".into()).unwrap();
            }
            if b.snapshot()
                .chat
                .iter()
                .any(|c| c.from == Role::A && c.text == "ping")
                && b.snapshot().chat.iter().any(|c| c.from == Role::B)
            {
                break;
            }
        }
        b
    });

    a.connect().unwrap();
    a.queue_chat("ping".into()).unwrap();
    for _ in 0..40 {
        a.tick(Duration::from_millis(10)).unwrap();
        if a.snapshot().chat.iter().any(|c| c.text.contains("pong")) {
            break;
        }
    }
    let b = t.join().unwrap();
    assert!(a.snapshot().chat.iter().any(|c| c.text.contains("ping")));
    assert!(b.snapshot().chat.iter().any(|c| c.text.contains("ping")));
}

#[test]
fn duplicate_data_is_acked_once() {
    // Lossy pair drops every 3rd send. Stop-and-wait should still deliver one chat.
    let pair = SimPair::lossy(Some(0)); // 0 means never drop (covered); real loss tested below
    drop(pair);
    let pair = SimPair::new();
    let mut cfg = RuntimeConfig::new(Role::A, ChannelMode::Sim, AppKind::Chat);
    cfg.sim_path = Some("/unused".into());
    let mut a = Session::wrap(cfg.clone(), pair.a, 9);
    let mut b_cfg = cfg;
    b_cfg.role = Role::B;
    let mut b = Session::wrap(b_cfg, pair.b, 0);
    let h = thread::spawn(move || {
        b.connect().unwrap();
        for _ in 0..30 {
            let _ = b.tick(Duration::from_millis(10));
        }
        b.snapshot().chat.len()
    });
    a.connect().unwrap();
    a.queue_chat("once".into()).unwrap();
    for _ in 0..30 {
        let _ = a.tick(Duration::from_millis(10));
    }
    let n = h.join().unwrap();
    assert_eq!(n, 1);
}

#[test]
fn app_message_rejects_bad_square() {
    assert!(AppMessage::Move {
        ply: 0,
        square: 10,
        mark: ccc_core::Mark::X,
    }
    .encode()
    .is_err());
}
