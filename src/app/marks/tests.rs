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

/// A marked Live still brings its hidden motion. The paths come in name order.
#[test]
fn marked_live_still_brings_motion() {
    use std::path::PathBuf;
    let mut app = app_with(&["b.png", "c.heic"]);
    let motion = app.images[1].with_extension("mov");
    app.set_live_photos(crate::scanner::LivePhotos::from_pairs([(
        app.images[1].clone(),
        motion.clone(),
    )]));
    assert!(app.marked_paths().is_empty());

    app.select_all_filtered();
    let expected: Vec<PathBuf> = vec![app.images[0].clone(), app.images[1].clone(), motion];
    assert_eq!(app.marked_paths(), expected);
}
