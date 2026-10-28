use ccc_core::Role;
use ccc_game::evidence::{verify_game, Acceptance, AcceptedMove, AgentChoice, PeerEvidence};
use ccc_game::{Board, Outcome};

fn evidence(role: Role, squares: &[u8]) -> PeerEvidence {
    let mut board = Board::new();
    let mut moves = Vec::new();
    let mut choices = Vec::new();
    for &square in squares {
        let mark = board.turn();
        let ply = board.ply();
        let local = mark == role.mark();
        moves.push(AcceptedMove {
            ply,
            square,
            mark,
            sequence: u16::from(ply) + 2,
            acceptance: if local {
                Acceptance::LocalAck
            } else {
                Acceptance::PeerAccepted
            },
        });
        if local {
            choices.push(AgentChoice { ply, square });
        }
        board.play(square, mark).unwrap();
    }
    PeerEvidence {
        session: 42,
        role,
        moves,
        choices,
        final_board: board.compact(),
        final_ply: board.ply(),
        outcome: board.outcome(),
        evidence_complete: true,
        exited_successfully: true,
    }
}

#[test]
fn complete_win_and_draw_require_legal_equal_histories() {
    for squares in [&[1, 4, 2, 5, 3][..], &[1, 2, 3, 5, 4, 6, 8, 7, 9][..]] {
        let a = evidence(Role::A, squares);
        let b = evidence(Role::B, squares);
        let verified = verify_game(&a, &b, true).unwrap();
        assert_ne!(verified.outcome, Outcome::InProgress);
        assert!(verified.agents_verified);
        assert_eq!(verified.ply as usize, squares.len());
    }
}

#[test]
fn equal_outcomes_with_different_boards_cannot_pass() {
    let a = evidence(Role::A, &[1, 4, 2, 5, 3]);
    let b = evidence(Role::B, &[7, 4, 8, 5, 9]);
    assert_eq!(a.outcome, b.outcome);
    assert!(verify_game(&a, &b, false).is_err());
}

#[test]
fn missing_ack_partial_transcript_stale_session_and_bad_choices_fail() {
    let a = evidence(Role::A, &[1, 4, 2, 5, 3]);
    let b = evidence(Role::B, &[1, 4, 2, 5, 3]);
    let mut broken = b.clone();
    broken.moves.pop();
    assert!(verify_game(&a, &broken, true).is_err());
    broken = b.clone();
    broken.evidence_complete = false;
    assert!(verify_game(&a, &broken, true).is_err());
    broken = b.clone();
    broken.exited_successfully = false;
    assert!(verify_game(&a, &broken, true).is_err());
    broken = b.clone();
    broken.session += 1;
    assert!(verify_game(&a, &broken, true).is_err());
    broken = b.clone();
    broken.choices[0].square = 9;
    assert!(verify_game(&a, &broken, true).is_err());
    assert!(verify_game(&a, &broken, false).is_ok());
}

#[test]
fn duplicate_moves_wrong_origins_and_sequence_mismatches_fail() {
    let a = evidence(Role::A, &[1, 4, 2, 5, 3]);
    let b = evidence(Role::B, &[1, 4, 2, 5, 3]);
    let mut broken = b.clone();
    broken.moves.insert(1, broken.moves[0].clone());
    assert!(verify_game(&a, &broken, false).is_err());
    broken = b.clone();
    broken.moves[0].acceptance = Acceptance::LocalAck;
    assert!(verify_game(&a, &broken, false).is_err());
    broken = b.clone();
    broken.moves[0].sequence = 600;
    assert!(verify_game(&a, &broken, false).is_err());
}

#[test]
fn unfinished_or_forged_final_state_fails() {
    let a = evidence(Role::A, &[1, 4]);
    let b = evidence(Role::B, &[1, 4]);
    assert!(verify_game(&a, &b, false).is_err());
    let a = evidence(Role::A, &[1, 4, 2, 5, 3]);
    let mut b = evidence(Role::B, &[1, 4, 2, 5, 3]);
    b.final_ply = 9;
    assert!(verify_game(&a, &b, false).is_err());
}
