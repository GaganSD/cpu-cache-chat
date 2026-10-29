use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use crate::worker::{Command, Update, Updates};
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

#[cfg(test)]
#[path = "../tests/internal/ui.rs"]
mod tests;

fn take_pending(pending: &mut VecDeque<(u64, String)>, id: u64) -> Option<String> {
    pending
        .iter()
        .position(|(key, _)| *key == id)
        .and_then(|pos| pending.remove(pos))
        .map(|(_, line)| line)
}

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
    commands: mpsc::SyncSender<Command>,
    updates: Updates,
) -> Result<(), Error> {
    enable_raw_mode().map_err(|e| Error::Io(e.to_string()))?;
    let _guard = TerminalGuard;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(|e| Error::Io(e.to_string()))?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(stdout)).map_err(|e| Error::Io(e.to_string()))?;
    let mut input = String::new();
    let mut pending: VecDeque<(u64, String)> = VecDeque::new();
    let mut next_id = 0u64;
    let mut snapshot: Option<Snapshot> = None;
    let mut log = Vec::new();
    let mut failed = false;
    loop {
        loop {
            let update = match updates.try_recv() {
                Ok(update) => update,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    return Err(Error::Io("worker disconnected".into()));
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
                Update::EventJSON(_) => {
                    // --events-stdout requires --plain; never mirrored here.
                }
                Update::Accepted(id, _) => {
                    take_pending(&mut pending, id);
                }
                Update::Rejected(id, reason) => {
                    if let Some(line) = take_pending(&mut pending, id) {
                        if input.is_empty() {
                            input = line;
                        } else {
                            log.push(format!("rejected input: {}", escape_controls(&line)));
                        }
                    }
                    log.push(format!("rejected: {}", escape_controls(&reason)));
                }
            }
        }
        while log.len() > 100 || log.iter().map(String::len).sum::<usize>() > 16_384 {
            log.remove(0);
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
                for (_, line) in &pending {
                    body.push_str(&format!(
                        "\n{} [queued] {}",
                        cfg.role,
                        escape_controls(line)
                    ));
                }
                f.render_widget(
                    Paragraph::new(body.clone())
                        .scroll((
                            body.lines()
                                .count()
                                .saturating_sub(chunks[0].height.saturating_sub(2) as usize)
                                .min(u16::MAX as usize) as u16,
                            0,
                        ))
                        .wrap(Wrap { trim: false })
                        .block(
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
                            if pending.len() >= 32 {
                                log.push("command queue full; retry later".into());
                            } else {
                                let id = next_id;
                                let line = input.clone();
                                match commands.try_send(Command {
                                    id,
                                    line: line.clone(),
                                }) {
                                    Ok(()) => {
                                        next_id = next_id.wrapping_add(1);
                                        pending.push_back((id, line));
                                        input.clear();
                                    }
                                    Err(mpsc::TrySendError::Full(_)) => {
                                        log.push("command queue full; retry later".into())
                                    }
                                    Err(mpsc::TrySendError::Disconnected(_)) => {
                                        return Err(Error::Disconnected)
                                    }
                                }
                            }
                        }
                        KeyCode::Backspace => {
                            input.pop();
                        }
                        KeyCode::Char(c)
                            if !key.modifiers.contains(KeyModifiers::CONTROL)
                                && input.len() + c.len_utf8() <= ccc_core::MAX_CHAT_BYTES =>
                        {
                            input.push(c);
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
