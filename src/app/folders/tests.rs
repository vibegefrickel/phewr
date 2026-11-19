//! The folder entries, Enter into a folder and the step to the parent.

use crate::app::{EntryKind, ViewMode};
use crate::test_support::{app_with, folder_app, wait_until};
use image::RgbaImage;
use ratatui::layout::Rect;
use std::fs;
use std::path::PathBuf;
use tuilibr::text_field::TextField;

#[test]
fn folders_form_prefix_with_own_kind() {
    let app = folder_app(&["..", "sub"], &["a.jpg", "b.jpg"]);
    assert_eq!(app.kind_at(0), Some(EntryKind::Folder));
    assert_eq!(app.kind_at(1), Some(EntryKind::Folder));
    assert_eq!(app.kind_at(2), Some(EntryKind::File));
    assert_eq!(app.kind_at(4), None);
    assert!(app.is_folder(1));
    assert!(!app.is_folder(2));
    assert_eq!(app.file_count(), 2);
    assert_eq!(app.first_file(), 2);
    assert!(app.selected_is_folder());

    let flat = app_with(&["a.jpg"]);
    assert_eq!(flat.folder_count, 0);
    assert!(!flat.selected_is_folder());
    assert!(flat.has_files());
}

#[test]
fn enter_opens_and_backspace_returns() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    for name in ["alpha", "beta"] {
        fs::create_dir(root.join(name)).unwrap();
    }
    fs::write(root.join("beta").join("photo.png"), b"synthetic").unwrap();

    let mut app = app_with(&[]);
    app.switch_to_dir(&root);
    wait_until(|| {
        app.refresh_from_scanner();
        !app.is_scanning()
    });
    assert_eq!(app.filenames, ["..", "alpha", "beta"]);
    assert_eq!(app.folder_count, 3);

    // Enter on a file does nothing here, and Enter on "beta" switches.
    app.gallery.cursor = 2;
    assert!(app.enter_selected_folder());
    assert_eq!(
        app.current_dir.as_deref(),
        Some(root.join("beta").as_path())
    );
    wait_until(|| {
        app.refresh_from_scanner();
        !app.is_scanning()
    });
    assert_eq!(app.filenames, ["..", "photo.png"]);
    assert_eq!(app.folder_count, 1);
    assert_eq!(app.gallery.cursor, 0);
    app.gallery.cursor = 1;
    assert!(!app.enter_selected_folder());

    // Backspace goes up, and the cursor lands on "beta".
    app.go_to_parent();
    assert_eq!(app.current_dir.as_deref(), Some(root.as_path()));
    wait_until(|| {
        app.refresh_from_scanner();
        !app.is_scanning()
    });
    assert_eq!(app.filenames, ["..", "alpha", "beta"]);
    assert_eq!(app.gallery.cursor, 2);
    assert_eq!(app.gallery.selected_index(), Some(2));

    // Enter on ".." does the same as Backspace.
    app.gallery.cursor = 2;
    assert!(app.enter_selected_folder());
    wait_until(|| {
        app.refresh_from_scanner();
        !app.is_scanning()
    });
    app.gallery.cursor = 0;
    assert!(app.enter_selected_folder());
    wait_until(|| {
        app.refresh_from_scanner();
        !app.is_scanning()
    });
    assert_eq!(app.current_dir.as_deref(), Some(root.as_path()));
    assert_eq!(app.gallery.cursor, 2, "the cursor is on beta");
}

#[test]
fn fullscreen_and_slideshow_skip_folders() {
    let mut app = folder_app(&["..", "sub"], &["a.jpg", "b.jpg"]);
    app.enter_fullscreen_at(1);
    assert_eq!(
        app.mode,
        ViewMode::Gallery,
        "a folder opens no fullscreen view"
    );
    app.enter_fullscreen_selected();
    assert_eq!(app.mode, ViewMode::Gallery);

    app.enter_fullscreen_at(3);
    assert_eq!(app.mode, ViewMode::Fullscreen);
    app.prev();
    assert_eq!(app.current, 2);
    app.prev();
    assert_eq!(app.current, 2, "the step back stops at the first file");
    app.first();
    assert_eq!(app.current, 2);

    // The slideshow loop wraps to the first file, not to the first folder.
    app.slideshow_loop = true;
    app.start_slideshow();
    app.loaded = Some(RgbaImage::new(1, 1).into());
    app.current = 3;
    app.slideshow_step();
    assert_eq!(app.current, 2);
    assert!(app.slideshow.is_some());

    let mut app = folder_app(&["..", "sub"], &["a.jpg"]);
    app.slideshow_autostart = true;
    app.maybe_autostart_slideshow();
    assert_eq!(app.mode, ViewMode::Fullscreen);
    assert_eq!(app.current, 2);

    // A grid without files starts no slideshow.
    let mut app = folder_app(&["..", "sub"], &[]);
    app.slideshow_autostart = true;
    app.maybe_autostart_slideshow();
    assert_eq!(app.mode, ViewMode::Gallery);
    app.start_slideshow();
    assert!(app.slideshow.is_none());
}

#[test]
fn folder_starts_no_background_jobs() {
    let mut app = folder_app(&["..", "sub"], &[]);
    app.scan_complete = true;
    app.spawn_thumb_decode(0, Rect::new(0, 0, 10, 5));
    app.spawn_thumb_decode(1, Rect::new(0, 0, 10, 5));
    app.pre_decode_hovered();
    app.request_cell_shape();
    assert!(
        !app.has_pending_work(),
        "no decode, no prefetch, no header read"
    );
    assert!(!app.cell_shape_pending());

    // With files, the shape sample starts after the folders.
    let mut app = folder_app(&["..", "sub"], &["a.jpg"]);
    app.scan_complete = true;
    app.request_cell_shape();
    assert!(app.cell_shape_pending());
    app.spawn_thumb_decode(2, Rect::new(0, 0, 10, 5));
    assert!(app.thumbnails_pending(), "the file decodes");
}

#[test]
fn marks_skip_folders() {
    let mut app = folder_app(&["..", "sub"], &["a.jpg", "b.jpg"]);
    app.toggle_selection_at_cursor();
    assert!(app.selection.is_empty());
    app.select_all_filtered();
    assert_eq!(app.selection.len(), 2);
    assert!(!app.is_marked(0));
    assert!(!app.is_marked(1));
    assert!(app.is_marked(2));
}

#[test]
fn search_matches_folder_names() {
    let mut app = folder_app(&["..", "summer"], &["winter.jpg"]);
    app.gallery.search_query = TextField::from("summ");
    app.update_filter();
    wait_until(|| {
        app.poll_filter();
        app.filter_rx.is_none()
    });
    assert_eq!(app.gallery.filtered_indices, vec![1]);
    assert!(app.selected_is_folder());
}

#[test]
fn scan_keeps_folders_in_front() {
    let list = crate::image_list::SharedImageList::new();
    let mut app = app_with(&[]);
    app.shared_list = list.clone();
    list.push_folders(vec![
        crate::image_list::FolderEntry {
            path: PathBuf::from("/x"),
            name: "..".into(),
        },
        crate::image_list::FolderEntry {
            path: PathBuf::from("/x/y/sub"),
            name: "sub".into(),
        },
    ]);
    list.push_batch(vec![
        PathBuf::from("/x/y/b.png"),
        PathBuf::from("/x/y/A.png"),
    ]);
    list.mark_complete();
    app.refresh_from_scanner();
    assert_eq!(app.filenames, ["..", "sub", "A.png", "b.png"]);
    assert_eq!(app.folder_count, 2);
    assert_eq!(
        app.mode,
        ViewMode::Gallery,
        "two entries, no fullscreen view"
    );

    // One file next to a folder opens directly, as in a flat folder.
    let list = crate::image_list::SharedImageList::new();
    let mut app = app_with(&[]);
    app.shared_list = list.clone();
    list.push_folders(vec![crate::image_list::FolderEntry {
        path: PathBuf::from("/x"),
        name: "..".into(),
    }]);
    list.push_batch(vec![PathBuf::from("/x/y/only.png")]);
    list.mark_complete();
    app.refresh_from_scanner();
    assert_eq!(
        app.mode,
        ViewMode::Gallery,
        "one image next to a folder stays in the grid"
    );
    assert_eq!(app.filenames, ["..", "only.png"]);
}

#[test]
fn parent_cell_reveals_gallery_folder() {
    let mut app = folder_app(&["..", "sub"], &["a.jpg"]);
    app.current_dir = Some(PathBuf::from("/photos/2026"));
    assert_eq!(app.reveal_path(), Some(PathBuf::from("/photos/2026")));
    app.gallery.cursor = 1;
    assert_eq!(app.reveal_path(), Some(PathBuf::from("/photos/sub")));
    app.gallery.cursor = 2;
    assert_eq!(app.reveal_path(), Some(PathBuf::from("/photos/a.jpg")));
}

#[test]
fn cursor_lands_on_start_file() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    for name in ["a.png", "b.png", "c.png"] {
        fs::write(root.join(name), b"synthetic").unwrap();
    }

    let mut app = app_with(&[]);
    app.switch_to_dir(&root);
    app.place_cursor_after_scan(root.join("b.png"));
    wait_until(|| {
        app.refresh_from_scanner();
        !app.is_scanning()
    });
    assert_eq!(app.filenames, ["..", "sub", "a.png", "b.png", "c.png"]);
    assert_eq!(app.gallery.selected_index(), Some(3));
    assert_eq!(
        app.mode,
        ViewMode::Gallery,
        "the grid opens, not the fullscreen view"
    );
}

#[test]
fn root_has_no_parent() {
    let mut app = app_with(&["/a.jpg"]);
    app.current_dir = Some("/".into());
    app.scan_complete = true;
    app.go_to_parent();
    assert_eq!(app.filenames, ["/a.jpg"], "nothing changed");
    assert!(!app.is_scanning(), "no new scan started");
}
