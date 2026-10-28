use super::take_pending;
use std::collections::VecDeque;

#[test]
fn rejection_then_acceptance_resolves_only_matching_input() {
    let mut pending = VecDeque::from([
        (10, "bad".to_string()),
        (11, "good".to_string()),
        (12, "later".to_string()),
    ]);
    assert_eq!(take_pending(&mut pending, 10).as_deref(), Some("bad"));
    assert_eq!(take_pending(&mut pending, 12).as_deref(), Some("later"));
    assert_eq!(take_pending(&mut pending, 11).as_deref(), Some("good"));
    assert!(pending.is_empty());
}
