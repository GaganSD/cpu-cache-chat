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

#[test]
fn slow_agent_does_not_prevent_link_from_receiving_abort() {
    use ccc_app::agent::{AgentJob, CommandSpec, Limits};
    use ccc_protocol::{AppMessage, GameCtrl, Link, LinkEvent};
    let pair = SimPair::new();
    let mut cfg = RuntimeConfig::new(Role::A, ChannelMode::Sim, AppKind::Game);
    cfg.sim_path = Some("/unused".into());
    cfg.agent = true;
    let mut session = Session::wrap(cfg.clone(), pair.a, 99);
    cfg.role = Role::B;
    let mut peer = Link::new(cfg, pair.b, 0);
    let peer_thread = thread::spawn(move || {
        peer.handshake().unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        let mut started = false;
        while Instant::now() < until {
            for event in peer.drive(Duration::from_millis(5)).unwrap() {
                if matches!(event, LinkEvent::Recv(AppMessage::Ctrl(GameCtrl::Start))) {
                    started = true;
                }
            }
            if started {
                peer.enqueue(AppMessage::Ctrl(GameCtrl::Abort)).unwrap();
                break;
            }
        }
        assert!(started);
        for _ in 0..100 {
            let _ = peer.drive(Duration::from_millis(5));
        }
    });
    session.connect().unwrap();
    session.request_start().unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    while !session.our_turn() && Instant::now() < until {
        session.tick(Duration::from_millis(5)).unwrap();
    }
    let request = session.agent_request(1).unwrap();
    let job = AgentJob::spawn(
        CommandSpec::TrustedShell("sleep 2; printf '{\"source\":\"pi\",\"square\":1}'".into()),
        request,
        Limits::default(),
        None,
    )
    .unwrap();
    while !session.snapshot().frozen && Instant::now() < until {
        session.tick(Duration::from_millis(5)).unwrap();
    }
    assert!(session.snapshot().frozen);
    assert!(job.try_result().unwrap().is_none());
    assert!(session.agent_request(1).is_none());
    drop(job);
    peer_thread.join().unwrap();
}

// --- Deterministic scripted-wire schedules -------------------------------
// Every peer frame is injected at an exact point and every local frame is
// captured, so token/ack ordering is fully deterministic.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use ccc_core::Mark;
use ccc_protocol::{decode_frame, encode_frame, AppMessage, Frame, FrameType, GameCtrl, Wire};

const SCRIPT_SESSION: u32 = 0x0DEC_0DE0;

#[derive(Clone, Default)]
struct Script {
    incoming: Arc<Mutex<VecDeque<Vec<u8>>>>,
    sent: Arc<Mutex<Vec<Vec<u8>>>>,
}

struct ScriptedWire(Script);

impl Wire for ScriptedWire {
    fn send_blob(&mut self, bytes: &[u8], _timeout: Duration) -> Result<(), ccc_core::Error> {
        self.0.sent.lock().unwrap().push(bytes.to_vec());
        Ok(())
    }

    fn recv_blob(&mut self, _timeout: Duration) -> Result<Vec<u8>, ccc_core::Error> {
        self.0
            .incoming
            .lock()
            .unwrap()
            .pop_front()
            .ok_or(ccc_core::Error::Timeout)
    }

    fn interrupt(&mut self) {}
}

fn scripted_session() -> (Session<ScriptedWire>, Script) {
    let script = Script::default();
    let mut cfg = RuntimeConfig::new(Role::A, ChannelMode::Sim, AppKind::Game);
    cfg.sim_path = Some("/unused".into());
    (
        Session::wrap(cfg, ScriptedWire(script.clone()), SCRIPT_SESSION),
        script,
    )
}

fn push(script: &Script, bytes: Vec<u8>) {
    script.incoming.lock().unwrap().push_back(bytes);
}

fn framed(frame: &Frame) -> Vec<u8> {
    encode_frame(frame).unwrap()
}

fn sync_ack() -> Vec<u8> {
    framed(&Frame::control(FrameType::SyncAck, SCRIPT_SESSION, 2, 0))
}

fn ack_data(seq: u16) -> Vec<u8> {
    framed(&Frame::control(FrameType::Ack, SCRIPT_SESSION, 0, seq))
}

fn token_confirm(epoch: u16) -> Vec<u8> {
    framed(&Frame::control(FrameType::Ack, SCRIPT_SESSION, epoch, 0))
}

fn token_offer(epoch: u16) -> Vec<u8> {
    framed(&Frame::control(
        FrameType::Heartbeat,
        SCRIPT_SESSION,
        epoch,
        0,
    ))
}

fn peer_data(seq: u16, msg: &AppMessage) -> Vec<u8> {
    framed(&Frame::new(FrameType::Data, SCRIPT_SESSION, seq, msg.encode().unwrap()).unwrap())
}

fn sent_frames(script: &Script) -> Vec<Frame> {
    script
        .sent
        .lock()
        .unwrap()
        .iter()
        .map(|bytes| decode_frame(bytes).unwrap())
        .collect()
}

fn send_data_seen(script: &Script, seq: u16) -> bool {
    sent_frames(script)
        .iter()
        .any(|f| f.ftype == FrameType::Data && f.seq == seq)
}

fn drive_until(
    a: &mut Session<ScriptedWire>,
    mut cond: impl FnMut(&Session<ScriptedWire>) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !cond(a) {
        a.tick(Duration::from_millis(5)).unwrap();
        assert!(
            Instant::now() < deadline,
            "condition not met; status: {:?}",
            a.snapshot().status
        );
        thread::sleep(Duration::from_millis(2));
    }
}

/// Wait for A's idle token offer of exactly `epoch`, then confirm it so A is
/// tokenless and the peer holds the token.
fn hand_token_to_peer(a: &mut Session<ScriptedWire>, script: &Script, epoch: u16) {
    drive_until(a, |_| {
        sent_frames(script)
            .iter()
            .filter(|f| f.ftype == FrameType::Heartbeat)
            .any(|f| f.seq == epoch)
    });
    push(script, token_confirm(epoch));
    a.tick(Duration::from_millis(5)).unwrap();
}

/// The peer offers the next epoch back so A holds the token again.
fn give_token_to_a(a: &mut Session<ScriptedWire>, script: &Script, epoch: u16) {
    push(script, token_offer(epoch));
    a.tick(Duration::from_millis(5)).unwrap();
}

/// Connect, deliver the game start, and see it acknowledged.
fn start_game(a: &mut Session<ScriptedWire>, script: &Script) {
    push(script, sync_ack());
    a.connect().unwrap();
    a.request_start().unwrap();
    drive_until(a, |_| send_data_seen(script, 1));
    push(script, ack_data(1));
    drive_until(a, |s| s.snapshot().started);
}

/// A queues `square` (link sequence `seq`) and the peer acks it; the board
/// reaches `ply`.
fn a_move(a: &mut Session<ScriptedWire>, script: &Script, square: u8, seq: u16, ply: u8) {
    a.queue_move(square).unwrap();
    drive_until(a, |_| send_data_seen(script, seq));
    push(script, ack_data(seq));
    drive_until(a, |s| s.snapshot().ply == Some(ply));
}

/// The peer plays `square` at `ply` with link sequence `peer_seq`.
fn peer_move(a: &mut Session<ScriptedWire>, script: &Script, square: u8, ply: u8, peer_seq: u16) {
    push(
        script,
        peer_data(
            peer_seq,
            &AppMessage::Move {
                ply,
                square,
                mark: Mark::O,
            },
        ),
    );
    drive_until(a, |s| s.snapshot().ply == Some(ply + 1));
}

#[test]
fn abort_before_delayed_token_ack_does_not_commit_move() {
    let (mut a, script) = scripted_session();
    start_game(&mut a, &script);
    // A passes the token to the peer and becomes tokenless.
    hand_token_to_peer(&mut a, &script, 1);
    // A queues a move while tokenless; the peer aborts before the token returns.
    a.queue_move(1).unwrap();
    push(&script, peer_data(1, &AppMessage::Ctrl(GameCtrl::Abort)));
    drive_until(&mut a, |s| s.snapshot().frozen);
    // Fail closed: no new application data is accepted once frozen.
    assert!(a.queue_move(2).is_err());
    // The delayed token return transmits the already-queued move and the peer
    // acks it; the late ack must not mutate the frozen board.
    give_token_to_a(&mut a, &script, 2);
    drive_until(&mut a, |_| send_data_seen(&script, 2));
    push(&script, ack_data(2));
    let mut notes = Vec::new();
    for _ in 0..3 {
        notes.extend(a.tick(Duration::from_millis(5)).unwrap());
    }
    assert!(
        notes.iter().any(|n| n.contains("not committed")),
        "notes: {notes:?}"
    );
    let snap = a.snapshot();
    assert!(snap.frozen);
    assert!(snap.started);
    assert_eq!(snap.ply, Some(0));
    assert_eq!(snap.board_compact.as_deref(), Some("123456789"));
    assert!(snap.outcome.is_none());
}

#[test]
fn delayed_start_ack_does_not_start_frozen_session() {
    let (mut a, script) = scripted_session();
    push(&script, sync_ack());
    a.connect().unwrap();
    // Become tokenless before any start request is queued.
    hand_token_to_peer(&mut a, &script, 1);
    a.request_start().unwrap();
    // An abort before any game start is invalid and freezes the session.
    push(&script, peer_data(1, &AppMessage::Ctrl(GameCtrl::Abort)));
    drive_until(&mut a, |s| s.snapshot().frozen);
    assert!(a.request_start().is_err());
    // The token returns; the queued start is transmitted and acked late. The
    // late ack must not start the frozen game.
    give_token_to_a(&mut a, &script, 2);
    drive_until(&mut a, |_| send_data_seen(&script, 1));
    push(&script, ack_data(1));
    let mut notes = Vec::new();
    for _ in 0..3 {
        notes.extend(a.tick(Duration::from_millis(5)).unwrap());
    }
    assert!(
        notes.iter().any(|n| n.contains("not started")),
        "notes: {notes:?}"
    );
    let snap = a.snapshot();
    assert!(snap.frozen);
    assert!(!snap.started);
    assert_eq!(snap.ply, Some(0));
}

#[test]
fn abort_after_finished_game_is_rejected() {
    let (mut a, script) = scripted_session();
    start_game(&mut a, &script);
    // X wins with the top row: X1 O4 X2 O5 X3.
    a_move(&mut a, &script, 1, 2, 1);
    hand_token_to_peer(&mut a, &script, 1);
    peer_move(&mut a, &script, 4, 1, 1);
    give_token_to_a(&mut a, &script, 2);
    a_move(&mut a, &script, 2, 3, 3);
    hand_token_to_peer(&mut a, &script, 3);
    peer_move(&mut a, &script, 5, 3, 2);
    give_token_to_a(&mut a, &script, 4);
    a_move(&mut a, &script, 3, 4, 5);
    assert_eq!(a.snapshot().outcome.as_deref(), Some("X wins"));
    // An abort after the game finished is illegal and must be rejected.
    hand_token_to_peer(&mut a, &script, 5);
    push(&script, peer_data(3, &AppMessage::Ctrl(GameCtrl::Abort)));
    drive_until(&mut a, |s| s.snapshot().frozen);
    let snap = a.snapshot();
    assert!(snap.frozen);
    assert_eq!(snap.ply, Some(5));
    assert_eq!(snap.outcome.as_deref(), Some("X wins"));
    assert!(snap.status.contains("rejected"), "status: {}", snap.status);
}
