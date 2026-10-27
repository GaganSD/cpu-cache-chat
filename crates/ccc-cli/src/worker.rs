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
) -> (
    mpsc::Sender<String>,
    mpsc::Receiver<Update>,
    std::thread::JoinHandle<()>,
) {
    let (commands, input) = mpsc::channel::<String>();
    let (output, updates) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let wire = match ccc_channel::open_wire(&cfg, stop.clone()) {
            Ok(wire) => wire,
            Err(e) => {
                let _ = output.send(Update::Note(format!("connect failed: {e}")));
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
                let _ = output.send(Update::Note(format!("connect failed: {e}")));
                return;
            }
        }
        let _ = output.send(Update::Snapshot(session.snapshot()));
        let mut agent_paused = false;
        let mut agent_failures = 0u32;
        while !stop.load(Ordering::SeqCst) {
            while let Ok(line) = input.try_recv() {
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
                } else {
                    let _ = output.send(Update::Accepted);
                }
                let _ = output.send(Update::Snapshot(session.snapshot()));
            }
            if cfg.agent && !agent_paused && session.our_turn() {
                let _ = output.send(Update::Note("agent choosing a move".into()));
                match session.try_agent_move().and_then(|sq| match sq {
                    Some(sq) => session.queue_move(sq).map(|_| sq),
                    None => Err(ccc_core::Error::Agent("no move".into())),
                }) {
                    Ok(sq) => {
                        agent_failures = 0;
                        let _ = output.send(Update::Note(format!("agent chose {sq}")));
                    }
                    Err(e) => {
                        agent_failures += 1;
                        if agent_failures >= 3 {
                            agent_paused = true;
                            let _ = output.send(Update::Note(format!(
                                "agent paused after {agent_failures} failed attempts: {e}"
                            )));
                        } else {
                            let _ = output.send(Update::Note(format!(
                                "agent attempt failed ({agent_failures}/3): {e}"
                            )));
                        }
                    }
                }
                let _ = output.send(Update::Snapshot(session.snapshot()));
            }
            match session.tick(cfg.recv_poll()) {
                Ok(notes) => {
                    for note in notes {
                        let _ = output.send(Update::Note(note));
                    }
                }
                Err(e) => {
                    let _ = output.send(Update::Note(format!("link failed: {e}")));
                    break;
                }
            }
            let _ = output.send(Update::Snapshot(session.snapshot()));
        }
        session.interrupt();
    });
    (commands, updates, handle)
}
