use std::{env, fs::File, process::ExitCode};

use ccc_core::Mark;
use ccc_game::{evidence::verify_game, evidence_jsonl::read_evidence, Outcome};
use serde_json::json;

fn run() -> Result<serde_json::Value, String> {
    let mut require_agents = false;
    let mut paths = Vec::new();
    for arg in env::args().skip(1) {
        if arg == "--require-agents" && !require_agents {
            require_agents = true;
        } else if arg.starts_with('-') {
            return Err("usage: ccc-verify-game [--require-agents] A.jsonl B.jsonl".into());
        } else {
            paths.push(arg);
        }
    }
    if paths.len() != 2 {
        return Err("usage: ccc-verify-game [--require-agents] A.jsonl B.jsonl".into());
    }
    let read = |path: &str| -> Result<_, String> {
        let file = File::open(path).map_err(|e| format!("opening {path}: {e}"))?;
        read_evidence(file).map_err(|e| format!("{path}: {e}"))
    };
    let a = read(&paths[0])?;
    let b = read(&paths[1])?;
    let game = verify_game(&a, &b, require_agents).map_err(|e| e.to_string())?;
    let outcome = match game.outcome {
        Outcome::Win(Mark::X) => "x_win",
        Outcome::Win(Mark::O) => "o_win",
        Outcome::Draw => "draw",
        Outcome::InProgress => return Err("unfinished game".into()),
    };
    Ok(
        json!({"ok": true, "session": game.session, "board": game.board, "ply": game.ply, "outcome": outcome, "agents_verified": game.agents_verified, "physical_channel_verified": false, "isolation_verified": false}),
    )
}

fn main() -> ExitCode {
    match run() {
        Ok(verdict) => {
            println!("{verdict}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            println!("{}", json!({"ok": false, "error": error}));
            ExitCode::FAILURE
        }
    }
}
