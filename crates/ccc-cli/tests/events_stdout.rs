//! Black-box: `game --plain --events-stdout` mirrors the canonical evidence
//! stream on stdout. Prefix extraction must yield each role's independent
//! JSONL log, replayable and verifiable exactly like the retained file.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ccc_core::{Mark, Role};
use ccc_game::evidence::verify_game;
use ccc_game::evidence_jsonl::read_evidence;
use ccc_game::Outcome;

const PREFIX: &str = "CCC_EVENT ";
const TIMEOUT: Duration = Duration::from_secs(15);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("ccc-events-stdout-{}", std::process::id()));
        std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Peer {
    name: &'static str,
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
    reader: Option<std::thread::JoinHandle<()>>,
    captured: Vec<String>,
}

impl Peer {
    fn spawn(role: &'static str, dir: &Path, link: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_cpu-cache-chat"))
            .args([
                "game",
                "--role",
                role,
                "--channel",
                "cpu",
                "--sim",
                "--plain",
                "--events-stdout",
            ])
            .arg("--events-path")
            .arg(dir.join(format!("events-{role}.jsonl")))
            .arg("--sim-path")
            .arg(link)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            name: role,
            child,
            stdin,
            lines: rx,
            reader: Some(reader),
            captured: Vec::new(),
        }
    }

    fn send(&mut self, line: &str) {
        self.stdin.write_all(line.as_bytes()).unwrap();
        self.stdin.write_all(b"\n").unwrap();
        self.stdin.flush().unwrap();
    }

    fn pull(&mut self, timeout: Duration) {
        if let Ok(line) = self.lines.recv_timeout(timeout) {
            self.captured.push(line);
        }
    }

    fn wait_for(&mut self, what: &str, pred: impl Fn(&str) -> bool) {
        let deadline = Instant::now() + TIMEOUT;
        while !self.captured.iter().any(|line| pred(line)) {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or_else(|| panic!("{}: timed out waiting for {what}", self.name));
            self.pull(remaining.min(Duration::from_millis(100)));
        }
    }

    fn wait_exit_success(&mut self) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "{} exited with {status}", self.name);
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{}: did not exit after /q\ncaptured:\n{}",
                self.name,
                self.captured.join("\n")
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn finish(&mut self) {
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
        while let Ok(line) = self.lines.try_recv() {
            self.captured.push(line);
        }
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn is_move_event(line: &str, ply: u8) -> bool {
    line.starts_with(PREFIX)
        && line.contains("\"type\":\"move\"")
        && line.contains(&format!("\"ply\":{ply}"))
}

fn events_jsonl(peer: &Peer) -> String {
    // The worker must never write event JSON to stdout directly: every line
    // carrying an event object must use the exact machine prefix.
    for line in &peer.captured {
        if line.contains("\"type\":\"") {
            assert!(
                line.starts_with(PREFIX),
                "{}: unprefixed event line: {line}",
                peer.name
            );
        }
    }
    let mut jsonl = peer
        .captured
        .iter()
        .filter_map(|line| line.strip_prefix(PREFIX))
        .collect::<Vec<_>>()
        .join("\n");
    jsonl.push('\n');
    jsonl
}

#[test]
fn events_stdout_mirrors_full_verifiable_game() {
    let dir = TempDir::new();
    let link = dir.0.join("link");
    let mut a = Peer::spawn("a", &dir.0, &link);
    let mut b = Peer::spawn("b", &dir.0, &link);

    a.wait_for("role a connected", |l| l.contains("link: connected"));
    b.wait_for("role b connected", |l| l.contains("link: connected"));
    b.wait_for("start note", |l| l.contains("peer started a match"));

    // Ply 0: the initiator may still be committing the game start, so retry
    // until the peer applies the move. Later plies are gated on the mover
    // having applied the previous ply, so they are sent exactly once.
    let deadline = Instant::now() + TIMEOUT;
    while !b.captured.iter().any(|l| is_move_event(l, 0)) {
        a.send("1");
        let step = Instant::now() + Duration::from_millis(500);
        while Instant::now() < step && !b.captured.iter().any(|l| is_move_event(l, 0)) {
            b.pull(Duration::from_millis(50));
        }
        assert!(
            b.captured.iter().any(|l| is_move_event(l, 0)) || Instant::now() < deadline,
            "role a never committed ply 0\ncaptured:\n{}",
            a.captured.join("\n")
        );
    }
    for (mover_is_a, square, ply) in [
        (false, "4", 1u8),
        (true, "2", 2),
        (false, "5", 3),
        (true, "3", 4),
    ] {
        let mover = if mover_is_a { &mut a } else { &mut b };
        mover.wait_for("previous ply applied", |l| is_move_event(l, ply - 1));
        mover.send(square);
    }

    a.wait_for("role a final", |l| {
        l.starts_with(PREFIX) && l.contains("\"type\":\"final\"")
    });
    b.wait_for("role b final", |l| {
        l.starts_with(PREFIX) && l.contains("\"type\":\"final\"")
    });
    a.send("/q");
    b.send("/q");
    a.wait_exit_success();
    b.wait_exit_success();
    a.finish();
    b.finish();

    let jsonl_a = events_jsonl(&a);
    let jsonl_b = events_jsonl(&b);

    // The stdout mirror and the retained 0600 file must be byte-identical.
    let path_a = dir.0.join("events-a.jsonl");
    let path_b = dir.0.join("events-b.jsonl");
    assert_eq!(jsonl_a, std::fs::read_to_string(&path_a).unwrap());
    assert_eq!(jsonl_b, std::fs::read_to_string(&path_b).unwrap());
    for path in [&path_a, &path_b] {
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    // Strict replay of each role's independent log, then cross-verification.
    let evidence_a = read_evidence(jsonl_a.as_bytes()).unwrap();
    let evidence_b = read_evidence(jsonl_b.as_bytes()).unwrap();
    assert_eq!(evidence_a.role, Role::A);
    assert_eq!(evidence_b.role, Role::B);
    for evidence in [&evidence_a, &evidence_b] {
        assert!(evidence.evidence_complete);
        assert!(evidence.exited_successfully);
    }
    let verified = verify_game(&evidence_a, &evidence_b, false).unwrap();
    assert_eq!(verified.outcome, Outcome::Win(Mark::X));
    assert_eq!(verified.ply, 5);
    assert_eq!(verified.board, "XXXOO6789");
}
