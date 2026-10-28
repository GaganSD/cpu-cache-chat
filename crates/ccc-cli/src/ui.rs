use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use crate::worker::Update;
use ccc_app::{escape_controls, Snapshot};
use ccc_core::{AppKind, Error, RuntimeConfig};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Terminal;

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            std::io::stdout(),
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
    }
}

pub fn run(
    cfg: RuntimeConfig,
    stop: Arc<AtomicBool>,
    commands: mpsc::Sender<String>,
    updates: mpsc::Receiver<Update>,
) -> Result<(), Error> {
    enable_raw_mode().map_err(|e| Error::Io(e.to_string()))?;
    let _guard = TerminalGuard;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(|e| Error::Io(e.to_string()))?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(stdout)).map_err(|e| Error::Io(e.to_string()))?;
    let mut input = String::new();
    let mut pending: VecDeque<String> = VecDeque::new();
    let mut snapshot: Option<Snapshot> = None;
    let mut log = Vec::new();
    let mut failed = false;
    loop {
        loop {
            let update = match updates.try_recv() {
                Ok(update) => update,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(Error::Io("worker disconnected".into()))
                }
            };
            match update {
                Update::Snapshot(s) => snapshot = Some(s),
                Update::Note(n) => {
                    if n.starts_with("connect failed:") || n.starts_with("link failed:") {
                        failed = true;
                    }
                    log.push(escape_controls(&n));
                }
                Update::Delivery(seq, state) => {
                    if let Some(s) = &mut snapshot {
                        if let Some(line) = s
                            .chat
                            .iter_mut()
                            .find(|line| line.seq == seq && line.from == cfg.role)
                        {
                            line.delivery = state;
                        }
                    }
                    log.push(format!("seq {seq}: {state}"));
                }
                Update::Fatal(e) => return Err(e),
                Update::Accepted => {
                    pending.pop_front();
                }
            }
        }
        if log.len() > 100 {
            log.drain(..log.len() - 100);
        }
        terminal
            .draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(8),
                        Constraint::Length(3),
                        Constraint::Length(6),
                        Constraint::Length(3),
                    ])
                    .split(f.area());
                let mut body = snapshot
                    .as_ref()
                    .map(|s| match &s.board {
                        Some(board) => format!("{}\n{}", board, s.outcome.as_deref().unwrap_or("")),
                        None => s
                            .chat
                            .iter()
                            .map(|c| {
                                format!("{} [{}] {}", c.from, c.delivery, escape_controls(&c.text))
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    })
                    .unwrap_or_default();
                for line in &pending {
                    body.push_str(&format!(
                        "\n{} [queued] {}",
                        cfg.role,
                        escape_controls(line)
                    ));
                }
                f.render_widget(
                    Paragraph::new(body).wrap(Wrap { trim: false }).block(
                        Block::default()
                            .title(if cfg.kind == AppKind::Game {
                                " board "
                            } else {
                                " chat "
                            })
                            .borders(Borders::ALL),
                    ),
                    chunks[0],
                );
                let link = if failed {
                    "failed".to_string()
                } else {
                    snapshot
                        .as_ref()
                        .map(|s| {
                            if s.link == ccc_core::LinkState::Idle {
                                "connecting".to_string()
                            } else {
                                s.link.to_string()
                            }
                        })
                        .unwrap_or_else(|| "connecting".into())
                };
                let mode = if cfg.channel.is_physical() {
                    cfg.channel.to_string()
                } else {
                    "SIMULATION (software only)".into()
                };
                f.render_widget(
                    Paragraph::new(format!(
                        "{} / {}   mode: {mode}   link: {link}",
                        cfg.role,
                        cfg.role.mark().glyph()
                    ))
                    .block(Block::default().borders(Borders::ALL)),
                    chunks[1],
                );
                let mut notes = log.iter().rev().take(4).cloned().collect::<Vec<_>>();
                notes.reverse();
                if let Some(s) = &snapshot {
                    if let Some(outcome) = &s.outcome {
                        notes.push(outcome.clone());
                    }
                }
                f.render_widget(
                    Paragraph::new(notes.join("\n"))
                        .wrap(Wrap { trim: false })
                        .block(Block::default().title(" status ").borders(Borders::ALL)),
                    chunks[2],
                );
                f.render_widget(
                    Paragraph::new(format!("> {}", escape_controls(&input))).block(
                        Block::default()
                            .title(" Enter to send • Esc / Ctrl-C to quit ")
                            .borders(Borders::ALL),
                    ),
                    chunks[3],
                );
            })
            .map_err(|e| Error::Io(e.to_string()))?;
        if stop.load(Ordering::SeqCst) {
            break;
        }
        if event::poll(Duration::from_millis(50)).map_err(|e| Error::Io(e.to_string()))? {
            if let Event::Key(key) = event::read().map_err(|e| Error::Io(e.to_string()))? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Esc => break,
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            break
                        }
                        KeyCode::Enter if !input.trim().is_empty() => {
                            let line = std::mem::take(&mut input);
                            if commands.send(line.clone()).is_ok() {
                                pending.push_back(line);
                            } else {
                                log.push("session unavailable".into());
                            }
                        }
                        KeyCode::Backspace => {
                            input.pop();
                        }
                        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if input.len() + c.len_utf8() <= ccc_core::MAX_CHAT_BYTES {
                                input.push(c);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    stop.store(true, Ordering::SeqCst);
    Ok(())
}
