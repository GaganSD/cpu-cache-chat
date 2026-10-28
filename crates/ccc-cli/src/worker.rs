use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use ccc_app::{Session, Snapshot};
use ccc_core::{AppKind, Delivery, Error, RuntimeConfig};
use ccc_protocol::{decode_frame, FrameType, Wire};

pub struct Command {
    pub id: u64,
    pub line: String,
}

pub enum Update {
    Snapshot(Snapshot),
    Note(String),
    Accepted(u64),
    Rejected(u64, String),
    Delivery(u16, Delivery),
    Fatal(Error),
}

#[derive(Clone)]
struct Output {
    events: mpsc::SyncSender<Update>,
    snapshot: Arc<Mutex<Option<Snapshot>>>,
    fatal: mpsc::SyncSender<Error>,
    failure: Arc<Mutex<Option<Error>>>,
    stop: Arc<AtomicBool>,
}

pub struct Updates {
    events: mpsc::Receiver<Update>,
    snapshot: Arc<Mutex<Option<Snapshot>>>,
    fatal: mpsc::Receiver<Error>,
}

impl Output {
    fn successful(&self) -> bool {
        self.failure.lock().unwrap().is_none()
    }

    fn take_failure(&self) -> Option<Error> {
        self.failure.lock().unwrap().take()
    }

    fn fail(&self, error: Error) {
        let mut failure = self.failure.lock().unwrap();
        if failure.is_none() {
            let _ = self.fatal.try_send(Error::Io(error.to_string()));
            *failure = Some(error);
        }
        self.stop.store(true, Ordering::SeqCst);
    }

    fn send(&self, update: Update) -> Result<(), ()> {
        fn bound(mut text: String) -> String {
            if text.len() > 4096 {
                let mut end = 4096;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
                text.push_str(" [UI diagnostic truncated; use retained transcript file]");
            }
            text
        }
        let update = match update {
            Update::Note(text) => Update::Note(bound(text)),
            Update::Rejected(id, text) => Update::Rejected(id, bound(text)),
            other => other,
        };
        match update {
            Update::Snapshot(snapshot) => {
                if let Ok(mut slot) = self.snapshot.try_lock() {
                    *slot = Some(snapshot);
                }
            }
            Update::Fatal(error) => {
                self.fail(error);
            }
            event => {
                if let Err(error) = self.events.try_send(event) {
                    match error {
                        mpsc::TrySendError::Full(_) => {
                            self.fail(Error::Channel("UI critical event queue overflow".into()));
                            return Err(());
                        }
                        mpsc::TrySendError::Disconnected(_) if self.stop.load(Ordering::SeqCst) => {
                        }
                        mpsc::TrySendError::Disconnected(_) => {
                            self.fail(Error::Disconnected);
                            return Err(());
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

impl Updates {
    pub fn try_recv(&self) -> Result<Update, mpsc::TryRecvError> {
        if let Ok(error) = self.fatal.try_recv() {
            return Ok(Update::Fatal(error));
        }
        match self.events.try_recv() {
            Ok(event) => Ok(event),
            Err(error) => match self.snapshot.lock().unwrap().take() {
                Some(snapshot) => Ok(Update::Snapshot(snapshot)),
                None => Err(error),
            },
        }
    }
}

fn update_channel(stop: Arc<AtomicBool>) -> (Output, Updates) {
    let (events, receiver) = mpsc::sync_channel(128);
    let snapshot = Arc::new(Mutex::new(None));
    let (fatal, fatal_receiver) = mpsc::sync_channel(1);
    (
        Output {
            events,
            snapshot: snapshot.clone(),
            fatal,
            failure: Arc::new(Mutex::new(None)),
            stop,
        },
        Updates {
            events: receiver,
            snapshot,
            fatal: fatal_receiver,
        },
    )
}

fn stopped_event(output: &Output, successful: bool) -> serde_json::Value {
    let successful = successful && output.successful();
    serde_json::json!({"type":"stopped","successful":successful,"evidence_complete":successful})
}

struct ObservedWire {
    inner: Box<dyn Wire>,
    output: Output,
}

impl Wire for ObservedWire {
    fn send_blob(&mut self, bytes: &[u8], timeout: Duration) -> Result<(), Error> {
        let seq = decode_frame(bytes)
            .ok()
            .filter(|f| f.ftype == FrameType::Data)
            .map(|f| f.seq);
        if let Some(seq) = seq {
            self.output
                .send(Update::Delivery(seq, Delivery::Transmitting))
                .map_err(|_| {
                    Error::Channel("UI critical event queue overflow or disconnected".into())
                })?;
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

fn spawn_worker(
    output: Output,
    run: impl FnOnce(&Output) + Send + 'static,
) -> std::thread::JoinHandle<Result<(), Error>> {
    std::thread::spawn(move || {
        run(&output);
        match output.take_failure() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    })
}

pub fn start(
    cfg: RuntimeConfig,
    stop: Arc<AtomicBool>,
    events_path: Option<String>,
) -> (
    mpsc::SyncSender<Command>,
    Updates,
    std::thread::JoinHandle<Result<(), Error>>,
) {
    let (commands, input) = mpsc::sync_channel::<Command>(32);
    let (output, updates) = update_channel(stop.clone());
    let handle = spawn_worker(output, move |output| {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut evidence = match events_path {
            Some(path) => match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
            {
                Ok(file) => match file.metadata() {
                    Ok(metadata) if metadata.is_file() => Some(file),
                    Ok(_) => {
                        let _ = output.send(Update::Fatal(Error::Io(
                            "event evidence must be a regular file".into(),
                        )));
                        return;
                    }
                    Err(e) => {
                        let _ = output.send(Update::Fatal(Error::Io(e.to_string())));
                        return;
                    }
                },
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
            Err(Error::Interrupted) if stop.load(Ordering::SeqCst) && output.successful() => return,
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
            Err(Error::Interrupted) if stop.load(Ordering::SeqCst) && output.successful() => return,
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
        let mut job: Option<(ccc_app::agent::AgentJob, ccc_app::agent::AgentRequest)> = None;
        let mut request_id = 0u64;
        while !stop.load(Ordering::SeqCst) {
            for _ in 0..64 {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(Command { id, line }) = input.try_recv() else {
                    break;
                };
                let result = match cfg.kind {
                    AppKind::Chat => session.queue_chat(line).map(|_| ()),
                    AppKind::Game => line
                        .trim()
                        .parse::<u8>()
                        .map_err(|_| ccc_core::Error::InvalidInput("enter a square 1-9".into()))
                        .and_then(|sq| session.queue_move(sq).map(|_| ())),
                };
                if let Err(e) = result {
                    let _ = output.send(Update::Rejected(id, e.to_string()));
                } else {
                    let _ = output.send(Update::Accepted(id));
                }
                let _ = output.send(Update::Snapshot(session.snapshot()));
            }
            if stop.load(Ordering::SeqCst) {
                break;
            }
            if let Some((active, _)) = &job {
                match active.try_result() {
                    Ok(Some(result)) => {
                        let (_, request) = job.take().expect("job is active");
                        // A result is fresh only while the session still waits
                        // on the exact request the job was spawned for. Failures
                        // are checked too: a stale job must never end a valid
                        // game, and a stale success must never commit.
                        let fresh =
                            session
                                .agent_request(request.request_id)
                                .is_some_and(|current| {
                                    current.session == request.session
                                        && current.generation == request.generation
                                        && current.request_id == request.request_id
                                });
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
                                if fresh {
                                    if let Err(e) = emit(
                                        serde_json::json!({"type":"agent_choice","ply":request.generation,"square":result.square}),
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
                            Err(Error::Interrupted)
                                if stop.load(Ordering::SeqCst) && output.successful() =>
                            {
                                break
                            }
                            Err(e) if !fresh => {
                                let _ = output.send(Update::Note(format!(
                                    "ignored stale agent failure: {e}"
                                )));
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
                                request.clone(),
                                ccc_app::agent::Limits::default(),
                                Some(stop.clone()),
                            )
                        });
                    match created {
                        Ok(active) => {
                            job = Some((active, request));
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
                Err(Error::Interrupted) if stop.load(Ordering::SeqCst) && output.successful() => {
                    break
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
        if let Err(e) = emit(stopped_event(output, successful)) {
            let _ = output.send(Update::Fatal(e));
        }
    });
    (commands, updates, handle)
}
