use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use ccc_app::{Session, Snapshot};
use ccc_core::{AppKind, Delivery, Error, RuntimeConfig};
use ccc_protocol::{decode_frame, FrameType, Wire};

pub enum Update {
    Snapshot(Snapshot),
    Note(String),
    Accepted,
    Delivery(u16, Delivery),
    Fatal(Error),
}

struct ObservedWire {
    inner: Box<dyn Wire>,
    output: mpsc::Sender<Update>,
}

impl Wire for ObservedWire {
    fn send_blob(&mut self, bytes: &[u8], timeout: Duration) -> Result<(), Error> {
        let seq = decode_frame(bytes)
            .ok()
            .filter(|f| f.ftype == FrameType::Data)
            .map(|f| f.seq);
        if let Some(seq) = seq {
            let _ = self
                .output
                .send(Update::Delivery(seq, Delivery::Transmitting));
        }
        // A failed attempt is not a final delivery state: the link owns the
        // retry budget and reports Unknown only when it is exhausted.
        self.inner.send_blob(bytes, timeout)
    }
    fn recv_blob(&mut self, timeout: Duration) -> Result<Vec<u8>, Error> {
        self.inner.recv_blob(timeout)
    }
    fn receive_in_progress(&self) -> bool {
        self.inner.receive_in_progress()
    }
    fn interrupt(&mut self) {
        self.inner.interrupt();
    }
}

#[cfg(test)]
#[path = "../tests/internal/worker.rs"]
mod tests;

pub fn start(
    cfg: RuntimeConfig,
    stop: Arc<AtomicBool>,
    events_path: Option<String>,
) -> (
    mpsc::SyncSender<String>,
    mpsc::Receiver<Update>,
    std::thread::JoinHandle<()>,
) {
    let (commands, input) = mpsc::sync_channel::<String>(64);
    let (output, updates) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        use std::io::Write;
        let mut evidence = match events_path {
            Some(path) => match {
                use std::os::unix::fs::OpenOptionsExt;
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(path)
            } {
                Ok(file) => Some(file),
                Err(e) => {
                    let _ = output.send(Update::Fatal(Error::Io(e.to_string())));
                    return;
                }
            },
            None => None,
        };
        let mut emit = |event: serde_json::Value| -> Result<(), Error> {
            if let Some(file) = &mut evidence {
                writeln!(file, "{event}").map_err(|e| Error::Io(format!("event evidence: {e}")))?;
            }
            Ok(())
        };
        let wire = match ccc_channel::open_wire(&cfg, stop.clone()) {
            Ok(wire) => wire,
            Err(e) => {
                let _ = output.send(Update::Fatal(e));
                return;
            }
        };
        let wire: Box<dyn Wire> = Box::new(ObservedWire {
            inner: wire,
            output: output.clone(),
        });
        let id = if cfg.role.is_initiator() {
            ccc_protocol::new_session_id()
        } else {
            0
        };
        let mut session = Session::wrap(cfg.clone(), wire, id);
        session.set_stop_flag(stop.clone());
        let _ = output.send(Update::Snapshot(session.snapshot()));
        let _ = output.send(Update::Note("connecting".into()));
        match session.connect() {
            Ok(notes) => {
                for note in notes {
                    let _ = output.send(Update::Note(note));
                }
                if cfg.kind == AppKind::Game && cfg.role.is_initiator() {
                    if let Err(e) = session.request_start() {
                        let _ = output.send(Update::Note(e.to_string()));
                    }
                }
            }
            Err(e) => {
                let _ = output.send(Update::Fatal(e));
                return;
            }
        }
        let initial = session.snapshot();
        if let Err(e) = emit(
            serde_json::json!({"type":"session","version":1,"session":initial.session,"role":cfg.role.to_string()}),
        ) {
            let _ = output.send(Update::Fatal(e));
            return;
        }
        let _ = output.send(Update::Snapshot(initial));
        let mut final_emitted = false;
        let mut successful = true;
        let mut job: Option<ccc_app::agent::AgentJob> = None;
        let mut request_id = 0u64;
        while !stop.load(Ordering::SeqCst) {
            for _ in 0..64 {
                let Ok(line) = input.try_recv() else { break };
                let result = match cfg.kind {
                    AppKind::Chat => session.queue_chat(line).map(|_| ()),
                    AppKind::Game => line
                        .trim()
                        .parse::<u8>()
                        .map_err(|_| ccc_core::Error::InvalidInput("enter a square 1-9".into()))
                        .and_then(|sq| session.queue_move(sq).map(|_| ())),
                };
                if let Err(e) = result {
                    let _ = output.send(Update::Note(e.to_string()));
                    let _ = output.send(Update::Accepted);
                } else {
                    let _ = output.send(Update::Accepted);
                }
                let _ = output.send(Update::Snapshot(session.snapshot()));
            }
            if let Some(active) = &job {
                match active.try_result() {
                    Ok(Some(result)) => {
                        job = None;
                        match result {
                            Ok(result) => {
                                if std::env::var_os("CCC_AGENT_DEBUG").is_some()
                                    && !result.stderr.is_empty()
                                {
                                    let transcript = ccc_app::safe_diagnostic(
                                        &String::from_utf8_lossy(&result.stderr),
                                    );
                                    let _ = output.send(Update::Note(format!(
                                        "agent transcript: {transcript}"
                                    )));
                                }
                                if let Some(current) = session.agent_request(request_id) {
                                    if current.session == result.session
                                        && current.generation == result.generation
                                        && current.request_id == result.request_id
                                    {
                                        if let Err(e) = emit(
                                            serde_json::json!({"type":"agent_choice","ply":current.generation,"square":result.square}),
                                        ) {
                                            let _ = output.send(Update::Fatal(e));
                                            successful = false;
                                            break;
                                        }
                                        if let Err(e) = session.queue_move(result.square) {
                                            let _ = output.send(Update::Fatal(e));
                                            successful = false;
                                            break;
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = output.send(Update::Fatal(e));
                                successful = false;
                                break;
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(_) => {
                        let _ = output.send(Update::Fatal(Error::Agent(
                            "agent supervisor disconnected".into(),
                        )));
                        successful = false;
                        break;
                    }
                }
            }
            if job.is_none() {
                request_id = request_id.wrapping_add(1);
                if let Some(request) = session.agent_request(request_id) {
                    let created = ccc_app::agent::CommandSpec::legacy(cfg.agent_cmd.as_deref())
                        .and_then(|command| {
                            ccc_app::agent::AgentJob::spawn(
                                command,
                                request,
                                ccc_app::agent::Limits::default(),
                                Some(stop.clone()),
                            )
                        });
                    match created {
                        Ok(active) => {
                            job = Some(active);
                            let _ = output.send(Update::Note("agent choosing a move".into()));
                        }
                        Err(e) => {
                            let _ = output.send(Update::Fatal(e));
                            successful = false;
                            break;
                        }
                    }
                }
            }
            match session.tick(cfg.recv_poll()) {
                Ok(notes) => {
                    for note in notes {
                        let _ = output.send(Update::Note(note));
                    }
                    if session.snapshot().link == ccc_core::LinkState::Failed {
                        let _ = output.send(Update::Fatal(Error::Io(
                            "link failed; delivery unknown".into(),
                        )));
                        successful = false;
                        break;
                    }
                }
                Err(e) => {
                    let _ = output.send(Update::Fatal(e));
                    successful = false;
                    break;
                }
            }
            for mv in session.take_accepted_moves() {
                if let Err(e) = emit(
                    serde_json::json!({"type":"move","ply":mv.ply,"square":mv.square,"mark":mv.mark.glyph().to_string(),"sequence":mv.sequence,"acceptance":if mv.local_ack {"local_ack"} else {"peer_accepted"}}),
                ) {
                    let _ = output.send(Update::Fatal(e));
                    successful = false;
                    break;
                }
            }
            if !successful {
                break;
            }
            let snapshot = session.snapshot();
            if !final_emitted {
                if let Some(outcome) = &snapshot.outcome {
                    let outcome = match outcome.as_str() {
                        "X wins" => "x_win",
                        "O wins" => "o_win",
                        "draw" => "draw",
                        _ => "unknown",
                    };
                    if let Err(e) = emit(
                        serde_json::json!({"type":"final","board":snapshot.board_compact,"ply":snapshot.ply,"outcome":outcome}),
                    ) {
                        let _ = output.send(Update::Fatal(e));
                        successful = false;
                        break;
                    }
                    final_emitted = true;
                }
            }
            let _ = output.send(Update::Snapshot(snapshot));
        }
        session.interrupt();
        if let Err(e) = emit(
            serde_json::json!({"type":"stopped","successful":successful,"evidence_complete":successful}),
        ) {
            let _ = output.send(Update::Fatal(e));
        }
    });
    (commands, updates, handle)
}
