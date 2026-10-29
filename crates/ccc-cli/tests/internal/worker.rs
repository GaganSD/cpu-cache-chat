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
        output.send(Update::Accepted(id, String::new())).unwrap();
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
        output.send(Update::Accepted(id, String::new())).unwrap();
    }
    assert!(output.send(Update::Accepted(128, String::new())).is_err());
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

// --- Complete worker path over a sim socket pair ---------------------------

use std::os::unix::fs::DirBuilderExt;
use std::time::Instant;

use ccc_channel::SimSocket;
use ccc_core::{AppKind, ChannelMode, Role, RuntimeConfig};
use ccc_protocol::{AppMessage, GameCtrl, Link, LinkEvent};

use super::{start, Command, Events, Updates};

struct SimDir(std::path::PathBuf);

impl SimDir {
    fn new(tag: &str) -> Self {
        // Keep the path short: unix socket names are bounded by SUN_LEN.
        let dir = std::env::temp_dir().join(format!("ccc-w{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        Self(dir)
    }

    fn base(&self) -> String {
        self.0.join("link").to_string_lossy().into_owned()
    }
}

impl Drop for SimDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Accept-all raw link peer; optionally aborts once the game start arrives.
fn raw_peer(
    base: String,
    stop: Arc<AtomicBool>,
    abort_on_start: bool,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let cfg = RuntimeConfig::new(Role::B, ChannelMode::Sim, AppKind::Game);
        let Ok(wire) = SimSocket::bind(Role::B, &base, stop.clone()) else {
            return;
        };
        let mut link = Link::new(cfg, wire, 0);
        if link.handshake().is_err() {
            return;
        }
        let mut aborted = false;
        while !stop.load(Ordering::SeqCst) {
            let Ok(events) = link.drive(Duration::from_millis(5)) else {
                return;
            };
            for event in events {
                if abort_on_start
                    && !aborted
                    && matches!(event, LinkEvent::Recv(AppMessage::Ctrl(GameCtrl::Start)))
                    && link.enqueue(AppMessage::Ctrl(GameCtrl::Abort)).is_ok()
                {
                    aborted = true;
                }
            }
        }
    })
}

fn agent_game_cfg(base: &str, agent_cmd: &str) -> RuntimeConfig {
    let mut cfg = RuntimeConfig::new(Role::A, ChannelMode::Sim, AppKind::Game);
    cfg.sim_path = Some(base.into());
    cfg.agent = true;
    cfg.agent_cmd = Some(agent_cmd.into());
    cfg
}

#[derive(Default)]
struct Collected {
    notes: Vec<String>,
    fatals: Vec<String>,
    snapshots: Vec<ccc_app::Snapshot>,
    accepted: Vec<u64>,
}

fn pump(updates: &Updates, out: &mut Collected) {
    loop {
        match updates.try_recv() {
            Ok(Update::Note(note)) => out.notes.push(note),
            Ok(Update::Fatal(e)) => out.fatals.push(e.to_string()),
            Ok(Update::Snapshot(s)) => out.snapshots.push(s),
            Ok(Update::Accepted(id, _)) => out.accepted.push(id),
            Ok(Update::Rejected(id, why)) => out.notes.push(format!("rejected {id}: {why}")),
            Ok(Update::Delivery(..) | Update::EventJSON(_)) => {}
            Err(_) => break,
        }
    }
}

fn wait_for(
    updates: &Updates,
    out: &mut Collected,
    what: &str,
    mut cond: impl FnMut(&Collected) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !cond(out) {
        pump(updates, out);
        assert!(
            out.fatals.is_empty(),
            "fatal during {what}: {:?}",
            out.fatals
        );
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; notes: {:?}",
            out.notes
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn stale_agent_failure_after_manual_move_does_not_end_session() {
    let dir = SimDir::new("stale-failure");
    let stop = Arc::new(AtomicBool::new(false));
    let peer = raw_peer(dir.base(), stop.clone(), false);
    let (commands, updates, worker) = start(
        agent_game_cfg(&dir.base(), "sh -c 'sleep 1; exit 3'"),
        stop.clone(),
        Events::default(),
    );
    let mut out = Collected::default();
    wait_for(&updates, &mut out, "agent spawn", |o| {
        o.notes.iter().any(|n| n.contains("agent choosing a move"))
    });
    // The user makes a legal move manually while inference is still running.
    commands
        .send(Command {
            id: 7,
            line: "1".into(),
        })
        .unwrap();
    wait_for(&updates, &mut out, "manual move commit", |o| {
        o.snapshots.iter().any(|s| s.ply == Some(1))
    });
    // The obsolete job failure must be ignored with a visible note, not fatal.
    wait_for(&updates, &mut out, "stale failure note", |o| {
        o.notes.iter().any(|n| n.contains("stale agent failure"))
    });
    assert!(out.accepted.contains(&7));
    assert!(out.fatals.is_empty());
    // The session is still alive and the worker quits cleanly.
    stop.store(true, Ordering::SeqCst);
    assert!(worker.join().unwrap().is_ok());
    peer.join().unwrap();
}

#[test]
fn stale_agent_success_after_abort_never_commits() {
    let dir = SimDir::new("stale-success");
    let stop = Arc::new(AtomicBool::new(false));
    let peer = raw_peer(dir.base(), stop.clone(), true);
    let (_commands, updates, worker) = start(
        agent_game_cfg(
            &dir.base(),
            "sh -c 'sleep 1; printf \"{\\\"source\\\":\\\"pi\\\",\\\"square\\\":5}\"'",
        ),
        stop.clone(),
        Events::default(),
    );
    let mut out = Collected::default();
    wait_for(&updates, &mut out, "agent spawn", |o| {
        o.notes.iter().any(|n| n.contains("agent choosing a move"))
    });
    // The peer aborts during inference; the late success must not commit.
    wait_for(&updates, &mut out, "peer abort", |o| {
        o.snapshots.iter().any(|s| s.frozen)
    });
    // Give the slow fixture time to finish and be discarded as stale.
    std::thread::sleep(Duration::from_millis(1500));
    pump(&updates, &mut out);
    assert!(out.fatals.is_empty(), "fatals: {:?}", out.fatals);
    let last = out.snapshots.last().expect("snapshots observed");
    assert!(last.frozen);
    assert_eq!(last.ply, Some(0));
    stop.store(true, Ordering::SeqCst);
    assert!(worker.join().unwrap().is_ok());
    peer.join().unwrap();
}
