//! Post-run verification only. Never participates in transport or turn selection.

use ccc_core::{Error, Mark, Role};

use crate::{Board, Outcome};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acceptance {
    /// Sender committed after the peer's application-acceptance ACK.
    LocalAck,
    /// Receiver accepted and applied the move before sending its ACK.
    PeerAccepted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedMove {
    pub ply: u8,
    pub square: u8,
    pub mark: Mark,
    pub sequence: u16,
    pub acceptance: Acceptance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentChoice {
    pub ply: u8,
    pub square: u8,
}

#[derive(Debug, Clone)]
pub struct PeerEvidence {
    pub session: u32,
    pub role: Role,
    pub moves: Vec<AcceptedMove>,
    pub choices: Vec<AgentChoice>,
    pub final_board: String,
    pub final_ply: u8,
    pub outcome: Outcome,
    /// True only when the complete event stream and required transcripts exist.
    pub evidence_complete: bool,
    /// True only after observing the successful worker/process outcome.
    pub exited_successfully: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedGame {
    pub session: u32,
    pub board: String,
    pub ply: u8,
    pub outcome: Outcome,
    pub agents_verified: bool,
}

fn invalid(reason: impl Into<String>) -> Error {
    Error::InvalidInput(format!(
        "incomplete or invalid game evidence: {}",
        reason.into()
    ))
}

fn replay(peer: &PeerEvidence, require_agents: bool) -> Result<Board, Error> {
    if !peer.evidence_complete || !peer.exited_successfully {
        return Err(invalid("missing evidence or unsuccessful process outcome"));
    }
    if peer.moves.len() > 9 || peer.choices.len() > 9 {
        return Err(invalid("too many move records"));
    }
    let mut board = Board::new();
    let mut local_seq = 0;
    let mut peer_seq = 0;
    let mut local_moves = Vec::new();
    for accepted in &peer.moves {
        if accepted.ply != board.ply() || accepted.sequence == 0 {
            return Err(invalid("missing, duplicate, or unordered move/sequence"));
        }
        let (expected_mark, last_seq) = match accepted.acceptance {
            Acceptance::LocalAck => {
                local_moves.push(AgentChoice {
                    ply: accepted.ply,
                    square: accepted.square,
                });
                (peer.role.mark(), &mut local_seq)
            }
            Acceptance::PeerAccepted => (peer.role.peer().mark(), &mut peer_seq),
        };
        if accepted.mark != expected_mark || accepted.sequence <= *last_seq {
            return Err(invalid("move origin or sequence disagrees with peer role"));
        }
        *last_seq = accepted.sequence;
        board
            .play(accepted.square, accepted.mark)
            .map_err(|e| invalid(e.to_string()))?;
    }
    if require_agents && peer.choices != local_moves {
        return Err(invalid(
            "agent choices do not match every accepted local move",
        ));
    }
    if board.outcome() == Outcome::InProgress {
        return Err(invalid("game is unfinished"));
    }
    if peer.final_board != board.compact()
        || peer.final_ply != board.ply()
        || peer.outcome != board.outcome()
    {
        return Err(invalid(
            "final board, ply, or outcome differs from legal replay",
        ));
    }
    Ok(board)
}

/// Require equal complete legal histories, not merely matching outcome strings.
/// This attests application evidence, not physical-channel attribution or isolation.
pub fn verify_game(
    a: &PeerEvidence,
    b: &PeerEvidence,
    require_agents: bool,
) -> Result<VerifiedGame, Error> {
    if a.session == 0 || a.session != b.session || a.role == b.role {
        return Err(invalid("session identity or peer roles disagree"));
    }
    let board_a = replay(a, require_agents)?;
    let board_b = replay(b, require_agents)?;
    if board_a != board_b || a.moves.len() != b.moves.len() {
        return Err(invalid("peer boards or move histories disagree"));
    }
    for (left, right) in a.moves.iter().zip(&b.moves) {
        if left.ply != right.ply
            || left.square != right.square
            || left.mark != right.mark
            || left.sequence != right.sequence
            || left.acceptance == right.acceptance
        {
            return Err(invalid("sender ACK and receiver acceptance do not match"));
        }
    }
    Ok(VerifiedGame {
        session: a.session,
        board: board_a.compact(),
        ply: board_a.ply(),
        outcome: board_a.outcome(),
        agents_verified: require_agents,
    })
}
