//! Strict, bounded JSONL decoding for post-run evidence.
use std::io::Read;

use ccc_core::{Mark, Role};
use serde::Deserialize;

use crate::evidence::{Acceptance, AcceptedMove, AgentChoice, PeerEvidence};
use crate::Outcome;

pub const MAX_BYTES: usize = 16 * 1024;
pub const MAX_LINE_BYTES: usize = 1024;
pub const MAX_EVENTS: usize = 21;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Event {
    Session {
        version: u8,
        session: u32,
        role: EvidenceRole,
    },
    Move {
        ply: u8,
        square: u8,
        mark: EvidenceMark,
        sequence: u16,
        acceptance: EvidenceAcceptance,
    },
    AgentChoice {
        ply: u8,
        square: u8,
    },
    Final {
        board: String,
        ply: u8,
        outcome: EvidenceOutcome,
    },
    Stopped {
        successful: bool,
        evidence_complete: bool,
    },
}
#[derive(Deserialize)]
enum EvidenceRole {
    A,
    B,
}
#[derive(Deserialize)]
enum EvidenceMark {
    X,
    O,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum EvidenceAcceptance {
    LocalAck,
    PeerAccepted,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum EvidenceOutcome {
    XWin,
    OWin,
    Draw,
}

/// Read one complete session, rejecting truncated, extra, or malformed records.
pub fn read_evidence(mut input: impl Read) -> Result<PeerEvidence, String> {
    let mut bytes = Vec::new();
    input
        .by_ref()
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("reading evidence: {e}"))?;
    if bytes.len() > MAX_BYTES {
        return Err("evidence exceeds byte limit".into());
    }
    if !bytes.ends_with(b"\n") {
        return Err("missing terminating newline".into());
    }
    let mut peer = None;
    let mut final_seen = false;
    let mut stopped = false;
    let mut count = 0;
    for line in bytes[..bytes.len() - 1].split(|&b| b == b'\n') {
        count += 1;
        if count > MAX_EVENTS || line.len() > MAX_LINE_BYTES {
            return Err("event or line limit exceeded".into());
        }
        let event: Event =
            serde_json::from_slice(line).map_err(|e| format!("invalid event {count}: {e}"))?;
        match event {
            Event::Session {
                version: 1,
                session,
                role,
            } if count == 1 => {
                peer = Some(PeerEvidence {
                    session,
                    role: match role {
                        EvidenceRole::A => Role::A,
                        EvidenceRole::B => Role::B,
                    },
                    moves: vec![],
                    choices: vec![],
                    final_board: String::new(),
                    final_ply: 0,
                    outcome: Outcome::InProgress,
                    evidence_complete: false,
                    exited_successfully: false,
                });
            }
            Event::Move {
                ply,
                square,
                mark,
                sequence,
                acceptance,
            } if peer.is_some() && !final_seen => {
                peer.as_mut().unwrap().moves.push(AcceptedMove {
                    ply,
                    square,
                    mark: match mark {
                        EvidenceMark::X => Mark::X,
                        EvidenceMark::O => Mark::O,
                    },
                    sequence,
                    acceptance: match acceptance {
                        EvidenceAcceptance::LocalAck => Acceptance::LocalAck,
                        EvidenceAcceptance::PeerAccepted => Acceptance::PeerAccepted,
                    },
                });
            }
            Event::AgentChoice { ply, square } if peer.is_some() && !final_seen => {
                let choices = &mut peer.as_mut().unwrap().choices;
                if choices.iter().any(|choice| choice.ply == ply) {
                    return Err("duplicate agent choice".into());
                }
                choices.push(AgentChoice { ply, square });
            }
            Event::Final {
                board,
                ply,
                outcome,
            } if peer.is_some() && !final_seen => {
                let p = peer.as_mut().unwrap();
                p.final_board = board;
                p.final_ply = ply;
                p.outcome = match outcome {
                    EvidenceOutcome::XWin => Outcome::Win(Mark::X),
                    EvidenceOutcome::OWin => Outcome::Win(Mark::O),
                    EvidenceOutcome::Draw => Outcome::Draw,
                };
                final_seen = true;
            }
            Event::Stopped {
                successful: true,
                evidence_complete: true,
            } if final_seen && !stopped => {
                let p = peer.as_mut().unwrap();
                p.exited_successfully = true;
                p.evidence_complete = true;
                stopped = true;
            }
            _ => return Err(format!("unexpected event {count}")),
        }
        if stopped
            && count
                != bytes[..bytes.len() - 1]
                    .iter()
                    .filter(|&&b| b == b'\n')
                    .count()
                    + 1
        {
            return Err("events after stopped".into());
        }
    }
    if !final_seen || !stopped {
        return Err("missing final or successful stopped event".into());
    }
    Ok(peer.unwrap())
}
