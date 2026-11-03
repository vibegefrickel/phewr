//! The marks of the grid.

use crate::test_support::app_with;

#[test]
fn mark_lookup_skips_empty_selection() {
    let mut app = app_with(&["a.png", "b.png"]);
    assert!(app.selection.is_empty());
    assert!(!app.is_marked(0) && !app.is_marked(1));
    app.toggle_selection_at_cursor();
    assert!(app.is_marked(0) && !app.is_marked(1));
}

#[test]
fn selection_marks_and_clears() {
    let mut app = app_with(&["a.png", "b.png", "c.png"]);
    app.toggle_selection_at_cursor();
    assert!(app.is_marked(0));
    app.toggle_selection_at_cursor();
    assert!(!app.is_marked(0));
    assert!(!app.is_marked(99));

    app.gallery.filtered_indices = vec![0, 2];
    app.select_all_filtered();
    assert!(app.is_marked(0) && !app.is_marked(1) && app.is_marked(2));
    app.clear_selection();
    assert!(app.selection.is_empty());

    app.gallery.filtered_indices.clear();
    app.toggle_selection_at_cursor();
    assert!(app.selection.is_empty());
}
