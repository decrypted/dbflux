use super::FilterUndo;

fn issued(filters: &[&str]) -> FilterUndo {
    let mut undo = FilterUndo::default();
    for filter in filters {
        undo.issued(filter);
    }
    undo
}

#[test]
fn the_first_read_starts_the_history_and_repeats_are_no_step() {
    let undo = issued(&["", " ", ""]);
    assert!(!undo.can_step(false), "the first read is no step to undo");
}

#[test]
fn undo_and_redo_walk_the_filters_read() {
    let mut undo = issued(&["", "a = 1", "b = 2"]);

    assert_eq!(undo.step(false).as_deref(), Some("a = 1"));
    assert_eq!(undo.step(false).as_deref(), Some(""));
    assert_eq!(undo.step(false), None);
    assert_eq!(undo.step(true).as_deref(), Some("a = 1"));
    assert_eq!(undo.step(true).as_deref(), Some("b = 2"));
    assert_eq!(undo.step(true), None);
}

#[test]
fn a_new_filter_after_undo_drops_the_redo_steps() {
    let mut undo = issued(&["", "a = 1", "b = 2"]);
    undo.step(false);
    // The read of the undone-to filter is noted too, and is no new step.
    undo.issued("a = 1");
    assert!(undo.can_step(true));

    undo.issued("c = 3");
    assert!(!undo.can_step(true));
    assert_eq!(undo.step(false).as_deref(), Some("a = 1"));
}
