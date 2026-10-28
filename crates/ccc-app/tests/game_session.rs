use ccc_app::Session;
use ccc_channel::SimPair;
use ccc_core::{AppKind, ChannelMode, Role, RuntimeConfig};
use std::thread;
use std::time::{Duration, Instant};

fn game(moves: &[u8], expected: &str) {
    let pair = SimPair::new();
    let mut cfg = RuntimeConfig::new(Role::A, ChannelMode::Sim, AppKind::Game);
    cfg.sim_path = Some("/unused".into());
    let mut a = Session::wrap(cfg.clone(), pair.a, 51);
    cfg.role = Role::B;
    let mut b = Session::wrap(cfg, pair.b, 0);
    let moves_b = moves.to_vec();
    let peer = thread::spawn(move || {
        b.connect().unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            b.tick(Duration::from_millis(5)).unwrap();
            let ply = b.board().ply() as usize;
            if ply < moves_b.len() && b.our_turn() {
                b.queue_move(moves_b[ply]).unwrap();
            }
            if ply == moves_b.len() {
                for _ in 0..50 {
                    b.tick(Duration::from_millis(5)).unwrap();
                }
                return b.snapshot();
            }
        }
        panic!("peer game timed out: {:?}", b.snapshot());
    });
    a.connect().unwrap();
    assert!(!a.our_turn());
    a.request_start().unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        a.tick(Duration::from_millis(5)).unwrap();
        let ply = a.board().ply() as usize;
        if ply < moves.len() && a.our_turn() {
            a.queue_move(moves[ply]).unwrap();
        }
        if ply == moves.len() {
            break;
        }
    }
    let sa = a.snapshot();
    let sb = peer.join().unwrap();
    assert_eq!(sa.ply, Some(moves.len() as u8));
    assert_eq!(sa.board_compact, sb.board_compact);
    assert_eq!(sa.outcome.as_deref(), Some(expected));
    assert_eq!(sb.outcome, sa.outcome);
    assert!(!a.our_turn());
}

#[test]
fn complete_win_and_draw_over_link() {
    game(&[1, 4, 2, 5, 3], "X wins");
    game(&[1, 2, 3, 5, 4, 6, 8, 7, 9], "draw");
}

#[test]
fn invalid_move_before_start_freezes_receiver() {
    use ccc_core::Mark;
    use ccc_protocol::{AppMessage, Link};
    let pair = SimPair::new();
    let mut cfg = RuntimeConfig::new(Role::A, ChannelMode::Sim, AppKind::Game);
    cfg.sim_path = Some("/unused".into());
    let mut attacker = Link::new(cfg.clone(), pair.a, 88);
    cfg.role = Role::B;
    let mut receiver = Session::wrap(cfg, pair.b, 0);
    let peer = thread::spawn(move || {
        receiver.connect().unwrap();
        for _ in 0..100 {
            receiver.tick(Duration::from_millis(5)).unwrap();
            if receiver.snapshot().frozen {
                break;
            }
        }
        receiver.snapshot()
    });
    attacker.handshake().unwrap();
    attacker
        .enqueue(AppMessage::Move {
            ply: 0,
            square: 1,
            mark: Mark::X,
        })
        .unwrap();
    for _ in 0..20 {
        let _ = attacker.drive(Duration::from_millis(5));
    }
    let snapshot = peer.join().unwrap();
    assert!(snapshot.frozen);
    assert!(!snapshot.started);
    assert_eq!(snapshot.ply, Some(0));
}

#[test]
fn abort_freezes_started_receiver() {
    use ccc_core::Delivery;
    use ccc_protocol::{AppMessage, GameCtrl, Link, LinkEvent};
    let pair = SimPair::new();
    let mut cfg = RuntimeConfig::new(Role::A, ChannelMode::Sim, AppKind::Game);
    cfg.sim_path = Some("/unused".into());
    let mut attacker = Link::new(cfg.clone(), pair.a, 89);
    cfg.role = Role::B;
    let mut receiver = Session::wrap(cfg, pair.b, 0);
    let peer = thread::spawn(move || {
        receiver.connect().unwrap();
        for _ in 0..200 {
            receiver.tick(Duration::from_millis(5)).unwrap();
            if receiver.snapshot().frozen {
                break;
            }
        }
        receiver.snapshot()
    });
    attacker.handshake().unwrap();
    let start = attacker.enqueue(AppMessage::Ctrl(GameCtrl::Start)).unwrap();
    let mut delivered = false;
    for _ in 0..100 {
        if attacker.drive(Duration::from_millis(5)).unwrap().iter().any(|e| matches!(e, LinkEvent::Delivery {seq, state: Delivery::Delivered} if *seq == start)) { delivered = true; break; }
    }
    assert!(delivered);
    attacker.enqueue(AppMessage::Ctrl(GameCtrl::Abort)).unwrap();
    for _ in 0..100 {
        let _ = attacker.drive(Duration::from_millis(5));
    }
    let snapshot = peer.join().unwrap();
    assert!(snapshot.started && snapshot.frozen);
    assert_eq!(snapshot.ply, Some(0));
}
