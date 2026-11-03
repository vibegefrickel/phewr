//! The marks and the delete question of the grid.

use super::{DeleteMode, PendingDelete};
use crate::app::ViewMode;
use crate::test_support::{app_with, app_with_files, test_dir, wait_until};
use std::fs;
use std::path::{Path, PathBuf};

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

#[test]
fn begin_delete_picks_view_paths() {
    let mut app = app_with(&[]);
    app.begin_delete(DeleteMode::Trash);
    assert!(
        app.pending_delete.is_none(),
        "an empty grid deletes nothing"
    );

    let mut app = app_with(&["a.png", "b.png", "c.png"]);
    app.gallery.cursor = 1;
    app.delete_error = Some("old".to_owned());
    app.begin_delete(DeleteMode::Trash);
    let pending = app.pending_delete.take().unwrap();
    assert_eq!(pending.paths, vec![PathBuf::from("b.png")]);
    assert!(!pending.from_fullscreen);
    assert_eq!(pending.mode, DeleteMode::Trash);
    assert!(app.delete_error.is_none());

    // Marked files go in grid order.
    app.selection.insert(PathBuf::from("c.png"));
    app.selection.insert(PathBuf::from("a.png"));
    app.begin_delete(DeleteMode::Permanent);
    let pending = app.pending_delete.take().unwrap();
    assert_eq!(
        pending.paths,
        vec![PathBuf::from("a.png"), PathBuf::from("c.png")]
    );
    assert_eq!(pending.mode, DeleteMode::Permanent);

    // Marked files outside the filter still go.
    app.gallery.filtered_indices = vec![1];
    app.selection.remove(Path::new("a.png"));
    app.begin_delete(DeleteMode::Trash);
    assert_eq!(
        app.pending_delete.take().unwrap().paths,
        vec![PathBuf::from("c.png")]
    );

    app.mode = ViewMode::Fullscreen;
    app.current = 2;
    app.begin_delete(DeleteMode::Trash);
    let pending = app.pending_delete.take().unwrap();
    assert_eq!(pending.paths, vec![PathBuf::from("c.png")]);
    assert!(pending.from_fullscreen);

    app.current = 7;
    app.begin_delete(DeleteMode::Trash);
    assert!(app.pending_delete.is_none());

    app.current = 0;
    app.begin_delete(DeleteMode::Trash);
    app.cancel_delete();
    assert!(app.pending_delete.is_none());
}

#[test]
fn confirm_removes_files_from_grid() {
    let dir = test_dir();
    let mut app = app_with_files(dir.path(), &["a.png", "b.png", "c.png"]);
    app.confirm_delete();
    assert_eq!(app.images.len(), 3, "nothing pending");

    let removed = app.images[2].clone();
    app.info_for(&removed);
    app.gallery.cursor = 2;
    app.current = 2;
    app.selection.insert(removed.clone());
    app.begin_delete(DeleteMode::Permanent);
    app.confirm_delete();

    assert!(!removed.exists());
    assert_eq!(app.filenames, ["a.png", "b.png"]);
    assert_eq!(app.gallery.filtered_indices, vec![0, 1]);
    assert_eq!(app.gallery.cursor, 1, "the new last image");
    assert_eq!(app.current, 1);
    assert!(app.selection.is_empty());
    assert!(app.delete_error.is_none());
    assert!(app.graphics_storage_dirty);
    assert_eq!(app.shared_list.len(), 2);
    assert_eq!(app.known_len, 2);
}

#[test]
fn grid_delete_keeps_cursor_place() {
    let dir = test_dir();
    let names = ["a.png", "b.png", "c.png", "d.png", "e.png"];
    let mut app = app_with_files(dir.path(), &names);
    app.gallery.visible_rows = 1;
    app.gallery.grid_cols = 2;

    // In the middle, the next image moves up into the place of the cursor.
    app.gallery.cursor = 1;
    app.begin_delete(DeleteMode::Permanent);
    app.confirm_delete();
    assert_eq!(app.filenames, ["a.png", "c.png", "d.png", "e.png"]);
    assert_eq!(app.gallery.cursor, 1);
    assert_eq!(app.gallery.selected_index(), Some(1), "c.png");

    // At the end, the cursor goes to the new last image and the grid scrolls to it.
    app.gallery.cursor = 3;
    app.gallery.scroll_offset = 1;
    app.begin_delete(DeleteMode::Permanent);
    app.confirm_delete();
    assert_eq!(app.filenames, ["a.png", "c.png", "d.png"]);
    assert_eq!(app.gallery.cursor, 2);
    assert_eq!(app.gallery.scroll_offset, 1);

    // A selection before the cursor: the cursor stays on its image.
    app.selection.insert(app.images[0].clone());
    app.selection.insert(app.images[1].clone());
    app.begin_delete(DeleteMode::Permanent);
    app.confirm_delete();
    assert_eq!(app.filenames, ["d.png"]);
    assert_eq!(app.gallery.cursor, 0);
    assert_eq!(app.gallery.scroll_offset, 0);
}

#[test]
fn confirm_reports_kept_files() {
    let dir = test_dir();
    let mut app = app_with_files(dir.path(), &["a.png", "b.png"]);
    let missing = dir.path().join("gone.png");
    app.pending_delete = Some(PendingDelete {
        paths: vec![missing.clone()],
        from_fullscreen: false,
        mode: DeleteMode::Permanent,
    });
    app.confirm_delete();
    let error = app.delete_error.clone().unwrap();
    assert!(error.contains("gone.png"), "{error}");
    assert_eq!(app.images.len(), 2);

    // The trash route fails for a missing file before it asks the system.
    app.pending_delete = Some(PendingDelete {
        paths: vec![missing.clone()],
        from_fullscreen: false,
        mode: DeleteMode::Trash,
    });
    app.confirm_delete();
    assert!(app.delete_error.as_ref().unwrap().contains("gone.png"));

    // One file goes, the other one reports its error.
    let first = app.images[0].clone();
    app.pending_delete = Some(PendingDelete {
        paths: vec![first.clone(), missing],
        from_fullscreen: false,
        mode: DeleteMode::Permanent,
    });
    app.confirm_delete();
    assert!(!first.exists());
    assert_eq!(app.filenames, ["b.png"]);
    assert!(app.delete_error.as_ref().unwrap().contains("gone.png"));
}

#[test]
fn fullscreen_delete_moves_on_to_gallery() {
    let dir = test_dir();
    let mut app = app_with_files(dir.path(), &["a.png", "b.png"]);
    app.enter_fullscreen_at(0);
    app.start_slideshow();
    app.begin_delete(DeleteMode::Permanent);
    app.confirm_delete();
    assert_eq!(app.mode, ViewMode::Fullscreen);
    assert_eq!(app.filenames, ["b.png"]);
    assert_eq!(app.current, 0);

    app.begin_delete(DeleteMode::Permanent);
    app.confirm_delete();
    assert!(app.images.is_empty());
    assert_eq!(app.mode, ViewMode::Gallery);
    assert!(!app.slideshow_running());
    assert_eq!(app.current, 0);
}

#[test]
fn grid_delete_keeps_index_valid() {
    let dir = test_dir();
    let mut app = app_with_files(dir.path(), &["a.png"]);
    app.begin_delete(DeleteMode::Permanent);
    app.confirm_delete();
    assert!(app.images.is_empty());
    assert_eq!(app.current, 0);
    assert_eq!(app.mode, ViewMode::Gallery);
}

#[test]
fn oversized_delete_does_not_underflow() {
    let dir = test_dir();
    let mut app = app_with_files(dir.path(), &["a.png"]);
    let extra: Vec<PathBuf> = ["x.png", "y.png"]
        .iter()
        .map(|name| dir.path().join(name))
        .collect();
    for path in &extra {
        fs::write(path, b"synthetic").unwrap();
    }
    app.pending_delete = Some(PendingDelete {
        paths: extra,
        from_fullscreen: false,
        mode: DeleteMode::Permanent,
    });
    app.confirm_delete();
    assert_eq!(app.images.len(), 1);
}

#[test]
fn confirm_keeps_active_filter() {
    let dir = test_dir();
    let mut app = app_with_files(dir.path(), &["apple.png", "berry.png", "apricot.png"]);
    app.gallery.search_query = "ap".to_owned();
    app.gallery.filtered_indices = vec![0, 2];
    app.gallery.cursor = 1;
    app.begin_delete(DeleteMode::Permanent);
    app.confirm_delete();
    assert_eq!(app.filenames, ["apple.png", "berry.png"]);
    wait_until(|| {
        app.poll_filter();
        app.filter_rx.is_none()
    });
    assert_eq!(app.gallery.filtered_indices, vec![0]);
    assert_eq!(app.gallery.cursor, 0, "the new last match");
}

#[test]
fn filtered_delete_keeps_cursor_place() {
    let dir = test_dir();
    let names = ["ap-1.png", "ap-2.png", "ap-3.png", "berry.png"];
    let mut app = app_with_files(dir.path(), &names);
    app.gallery.search_query = "ap".to_owned();
    app.gallery.filtered_indices = vec![0, 1, 2];
    app.gallery.cursor = 1;
    app.begin_delete(DeleteMode::Permanent);
    app.confirm_delete();
    wait_until(|| {
        app.poll_filter();
        app.filter_rx.is_none()
    });
    assert_eq!(app.gallery.filtered_indices.len(), 2);
    assert_eq!(app.gallery.cursor, 1);

    // A new query starts at the first match again.
    app.gallery.search_query = "ap-".to_owned();
    app.update_filter();
    wait_until(|| {
        app.poll_filter();
        app.filter_rx.is_none()
    });
    assert_eq!(app.gallery.cursor, 0);
}

#[test]
#[cfg(feature = "video")]
fn player_delete_stays_in_player() {
    let dir = test_dir();
    let first = dir.path().join("clip1.mp4");
    let second = dir.path().join("clip2.mp4");
    fs::write(&first, b"").unwrap();
    fs::write(&second, b"").unwrap();

    let mut app = crate::app::App::new((8, 16), crate::image_list::SharedImageList::new());
    app.images = vec![first.clone(), second.clone()];
    app.filenames = vec!["clip1.mp4".to_owned(), "clip2.mp4".to_owned()];
    app.gallery.filtered_indices = vec![0, 1];
    app.scan_complete = true;
    app.mode = ViewMode::Video;
    app.current = 0;

    app.pending_delete = Some(PendingDelete {
        paths: vec![first.clone()],
        from_fullscreen: true,
        mode: DeleteMode::Permanent,
    });
    app.confirm_delete();

    assert_eq!(
        app.mode,
        ViewMode::Video,
        "the player stays, on the next clip"
    );
    assert_eq!(app.current, 0, "the next file moved into place 0");
    assert_eq!(app.images, vec![second]);
    assert!(!first.exists());
}

#[test]
fn delete_during_scan_keeps_later_batches() {
    let dir = test_dir();
    let mut app = app_with_files(dir.path(), &["a.png", "b.png"]);
    let later = dir.path().join("c.png");
    image::RgbaImage::new(8, 8).save(&later).unwrap();

    // A scanner with two files out and one more to come.
    app.scan_complete = false;
    app.shared_list.push_batch(vec![app.images[0].clone()]);
    app.shared_list.push_batch(vec![app.images[1].clone()]);
    app.known_len = 2;

    app.pending_delete = Some(PendingDelete {
        paths: vec![app.images[0].clone()],
        from_fullscreen: false,
        mode: DeleteMode::Permanent,
    });
    let kept = dir.path().join("b.png");
    app.confirm_delete();
    assert_eq!(app.images, vec![kept.clone()]);
    assert_eq!(app.known_len, 1);

    app.shared_list.push_batch(vec![later.clone()]);
    app.refresh_from_scanner();
    assert_eq!(app.images, vec![kept, later]);
    assert_eq!(app.known_len, 2);
}
