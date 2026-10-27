use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ccc_app::Session;
use ccc_core::{AppKind, Error, LinkState, RuntimeConfig};
use ccc_protocol::Wire;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Terminal;

pub fn run(
    mut session: Session<Box<dyn Wire>>,
    cfg: RuntimeConfig,
    stop: Arc<AtomicBool>,
) -> Result<(), Error> {
    enable_raw_mode().map_err(|e| Error::Io(e.to_string()))?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(|e| Error::Io(e.to_string()))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|e| Error::Io(e.to_string()))?;

    let result = drive(&mut terminal, &mut session, &cfg, stop);
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    session.interrupt();
    result
}

fn drive(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    session: &mut Session<Box<dyn Wire>>,
    cfg: &RuntimeConfig,
    stop: Arc<AtomicBool>,
) -> Result<(), Error> {
    let mut input = String::new();
    let mut log: Vec<String> = Vec::new();
    log.push(format!("mode: {}", cfg.channel));
    if !cfg.channel.is_physical() {
        log.push("WARNING: simulation transport — not a hardware channel".into());
    }
    match session.connect() {
        Ok(notes) => log.extend(notes),
        Err(e) => {
            log.push(format!("connect failed: {e}"));
        }
    }
    if cfg.kind == AppKind::Game
        && cfg.role.is_initiator()
        && session.snapshot().link == LinkState::Connected
    {
        if let Err(e) = session.request_start() {
            log.push(format!("start: {e}"));
        }
    }

    while !stop.load(Ordering::SeqCst) {
        if event::poll(Duration::from_millis(50)).map_err(|e| Error::Io(e.to_string()))? {
            if let Event::Key(key) = event::read().map_err(|e| Error::Io(e.to_string()))? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            break
                        }
                        KeyCode::Esc => break,
                        KeyCode::Enter => {
                            let line = std::mem::take(&mut input);
                            handle_line(session, cfg, &line, &mut log);
                        }
                        KeyCode::Backspace => {
                            input.pop();
                        }
                        KeyCode::Char(c) => input.push(c),
                        _ => {}
                    }
                }
            }
        }
        match session.tick(Duration::from_millis(20)) {
            Ok(notes) => log.extend(notes),
            Err(e) => log.push(e.to_string()),
        }
        if cfg.agent && session.our_turn() {
            match session.try_agent_move() {
                Ok(Some(sq)) => match session.queue_move(sq) {
                    Ok(_) => log.push(format!("agent chose {sq}")),
                    Err(e) => log.push(format!("illegal agent move: {e}")),
                },
                Ok(None) => {}
                Err(e) => log.push(format!("agent: {e}")),
            }
        }
        let snap = session.snapshot();
        terminal
            .draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Min(8),
                        Constraint::Length(6),
                        Constraint::Length(3),
                    ])
                    .split(f.area());
                let title = format!(
                    " cpu-cache-chat / {} / {} ",
                    snap.role,
                    snap.role.mark().glyph()
                );
                let head = format!(
                    "mode: {}   link: {}   {}",
                    snap.channel_label,
                    snap.link,
                    if snap.simulated { "SIMULATION" } else { "" }
                );
                f.render_widget(
                    Paragraph::new(head).block(Block::default().title(title).borders(Borders::ALL)),
                    chunks[0],
                );
                let body = match &snap.board {
                    Some(b) => b.clone(),
                    None => snap
                        .chat
                        .iter()
                        .map(|c| format!("{} [{}] {}", c.from, c.delivery, c.text))
                        .collect::<Vec<_>>()
                        .join("\n"),
                };
                f.render_widget(
                    Paragraph::new(body).block(
                        Block::default()
                            .title(" board / chat ")
                            .borders(Borders::ALL),
                    ),
                    chunks[1],
                );
                let tail = log.iter().rev().take(8).cloned().rev().collect::<Vec<_>>();
                f.render_widget(
                    Paragraph::new(tail.join("\n"))
                        .wrap(Wrap { trim: true })
                        .block(
                            Block::default()
                                .title(format!(" {}", snap.status))
                                .borders(Borders::ALL),
                        ),
                    chunks[2],
                );
                let hint = if cfg.kind == AppKind::Game {
                    "type 1-9 + enter   Ctrl-C stop"
                } else {
                    "type message + enter   Ctrl-C stop"
                };
                f.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::raw("> "),
                        Span::styled(&input, Style::default().add_modifier(Modifier::BOLD)),
                    ]))
                    .block(Block::default().title(hint).borders(Borders::ALL)),
                    chunks[3],
                );
            })
            .map_err(|e| Error::Io(e.to_string()))?;
        if snap.outcome.is_some() && cfg.kind == AppKind::Game {
            // keep the final board visible until the user quits
        }
    }
    Ok(())
}

fn handle_line(
    session: &mut Session<Box<dyn Wire>>,
    cfg: &RuntimeConfig,
    line: &str,
    log: &mut Vec<String>,
) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    match cfg.kind {
        AppKind::Chat => match session.queue_chat(line.to_string()) {
            Ok(_) => {}
            Err(e) => log.push(e.to_string()),
        },
        AppKind::Game => match line.parse::<u8>() {
            Ok(sq) => match session.queue_move(sq) {
                Ok(_) => {}
                Err(e) => log.push(e.to_string()),
            },
            Err(_) => log.push("enter a square 1-9".into()),
        },
    }
}
