use ccc_game::evidence_jsonl::read_evidence;
use serde_json::{json, Value};
use std::{
    fs,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

fn events(role: &str, squares: &[u8]) -> Vec<Value> {
    let mut v = vec![json!({"type":"session","version":1,"session":42,"role":role})];
    let mut board = ccc_game::Board::new();
    for (ply, &square) in squares.iter().enumerate() {
        let mark = if ply % 2 == 0 { "X" } else { "O" };
        let local = (ply % 2 == 0) == (role == "A");
        if local {
            v.push(json!({"type":"agent_choice","ply":ply,"square":square}));
        }
        v.push(json!({"type":"move","ply":ply,"square":square,"mark":mark,"sequence":ply+2,"acceptance":if local {"local_ack"} else {"peer_accepted"}}));
        board.play(square, board.turn()).unwrap();
    }
    v.push(json!({"type":"final","board":board.compact(),"ply":board.ply(),"outcome":"x_win"}));
    v.push(json!({"type":"stopped","successful":true,"evidence_complete":true}));
    v
}
fn encode(events: &[Value]) -> String {
    events.iter().map(|e| format!("{e}\n")).collect()
}
fn verify(a: &[Value], b: &[Value], agents: bool) -> bool {
    static ID: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "ccc-evidence-{}-{}",
        std::process::id(),
        ID.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).unwrap();
    let (pa, pb) = (dir.join("a"), dir.join("b"));
    fs::write(&pa, encode(a)).unwrap();
    fs::write(&pb, encode(b)).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ccc-verify-game"));
    if agents {
        cmd.arg("--require-agents");
    }
    let output = cmd.arg(pa).arg(pb).output().unwrap();
    let verdict: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(output.status.success(), verdict["ok"].as_bool().unwrap());
    fs::remove_dir_all(dir).unwrap();
    output.status.success()
}
#[test]
fn verifier_rejects_incomplete_or_conflicting_evidence() {
    let a = events("A", &[1, 4, 2, 5, 3]);
    let b = events("B", &[1, 4, 2, 5, 3]);
    assert!(verify(&a, &b, true));
    assert!(!verify(&a, &events("B", &[7, 4, 8, 5, 9]), false));
    let mut bad = b.clone();
    bad.remove(1);
    assert!(!verify(&a, &bad, false));
    let mut bad = b.clone();
    bad.remove(bad.len() - 2);
    assert!(!verify(&a, &bad, false));
    let mut bad = b.clone();
    bad.pop();
    assert!(!verify(&a, &bad, false));
    let mut bad = b.clone();
    bad[0]["session"] = json!(43);
    assert!(!verify(&a, &bad, false));
    let mut bad = b.clone();
    let last = bad.len() - 1;
    bad[last]["successful"] = json!(false);
    assert!(!verify(&a, &bad, false));
    let mut bad = b.clone();
    bad[1]["acceptance"] = json!("sent");
    assert!(!verify(&a, &bad, false));
    let mut bad = b.clone();
    let choice = bad
        .iter_mut()
        .find(|e| e["type"] == "agent_choice")
        .unwrap();
    choice["square"] = json!(9);
    assert!(!verify(&a, &bad, true));
    let mut bad = b.clone();
    bad.insert(1, bad[0].clone());
    assert!(!verify(&a, &bad, false));
}
#[test]
fn parser_enforces_framing_limits_and_strict_fields() {
    let a = events("A", &[1, 4, 2, 5, 3]);
    let text = encode(&a);
    assert!(read_evidence(text.as_bytes()).is_ok());
    assert!(read_evidence(&text.as_bytes()[..text.len() - 1]).is_err());
    assert!(read_evidence(b"{\"type\":\"session\"\n".as_slice()).is_err());
    assert!(read_evidence(vec![b'x'; 16385].as_slice()).is_err());
    let mut bad = a.clone();
    bad[0]["extra"] = json!(true);
    assert!(read_evidence(encode(&bad).as_bytes()).is_err());
    let duplicate = text.replacen("\"version\":1", "\"version\":1,\"version\":1", 1);
    assert!(read_evidence(duplicate.as_bytes()).is_err());
}
