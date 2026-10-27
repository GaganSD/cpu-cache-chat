use ccc_core::Mark;
use ccc_game::{Board, Outcome};

#[test]
fn x_starts_and_center_is_legal() {
    let mut b = Board::new();
    assert_eq!(b.turn(), Mark::X);
    assert_eq!(b.play(5, Mark::X).unwrap(), Outcome::InProgress);
    assert!(b.play(5, Mark::O).is_err());
}

#[test]
fn rejects_out_of_turn() {
    let mut b = Board::new();
    assert!(b.play(1, Mark::O).is_err());
}

#[test]
fn win_and_draw() {
    let mut b = Board::new();
    // X 1, O 4, X 2, O 5, X 3
    b.play(1, Mark::X).unwrap();
    b.play(4, Mark::O).unwrap();
    b.play(2, Mark::X).unwrap();
    b.play(5, Mark::O).unwrap();
    assert_eq!(b.play(3, Mark::X).unwrap(), Outcome::Win(Mark::X));
    assert!(b.play(6, Mark::O).is_err());

    let mut d = Board::new();
    // 1X 2O 3X 4X 5O 6X 7O 8X 9O -> not a real draw sequence, do a known draw
    let seq = [
        (1, Mark::X),
        (2, Mark::O),
        (3, Mark::X),
        (5, Mark::O),
        (4, Mark::X),
        (6, Mark::O),
        (8, Mark::X),
        (7, Mark::O),
        (9, Mark::X),
    ];
    let mut last = Outcome::InProgress;
    for (s, m) in seq {
        last = d.play(s, m).unwrap();
    }
    assert_eq!(last, Outcome::Draw);
}

#[test]
fn render_uses_numbers_for_empty() {
    let b = Board::new();
    assert!(b.render().contains('5'));
    assert_eq!(b.compact(), "123456789");
}

#[test]
fn all_win_lines_and_finished_boards() {
    let lines = [
        [1, 2, 3],
        [4, 5, 6],
        [7, 8, 9],
        [1, 4, 7],
        [2, 5, 8],
        [3, 6, 9],
        [1, 5, 9],
        [3, 5, 7],
    ];
    for line in lines {
        // Pick two non-line squares that cannot block the winning line.
        let other: Vec<_> = (1..=9).filter(|s| !line.contains(s)).take(2).collect();
        let mut board = Board::new();
        for (square, mark) in [
            (line[0], Mark::X),
            (other[0], Mark::O),
            (line[1], Mark::X),
            (other[1], Mark::O),
        ] {
            assert_eq!(board.play(square, mark).unwrap(), Outcome::InProgress);
        }
        assert_eq!(board.play(line[2], Mark::X).unwrap(), Outcome::Win(Mark::X));
        assert!(board.legal_squares().is_empty());
        assert!(board.play(other[0], Mark::O).is_err());
    }
    let mut board = Board::new();
    for (i, square) in [1, 2, 3, 5, 4, 6, 8, 7, 9].into_iter().enumerate() {
        assert_eq!(
            board
                .play(square, if i % 2 == 0 { Mark::X } else { Mark::O })
                .unwrap(),
            if i == 8 {
                Outcome::Draw
            } else {
                Outcome::InProgress
            }
        );
    }
    assert!(board.play(1, Mark::O).is_err());
    assert!(board.legal_squares().is_empty());
}

#[test]
fn illegal_moves_leave_board_unchanged() {
    let mut board = Board::new();
    for (square, mark) in [(0, Mark::X), (10, Mark::X), (1, Mark::O)] {
        let before = board.clone();
        assert!(board.play(square, mark).is_err());
        assert_eq!(board, before);
    }
    board.play(1, Mark::X).unwrap();
    let before = board.clone();
    assert!(board.play(1, Mark::O).is_err());
    assert_eq!(board, before);
}
