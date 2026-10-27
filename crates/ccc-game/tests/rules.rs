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
