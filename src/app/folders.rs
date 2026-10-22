//! The folder entries of the grid: the kinds, the step into a folder and the step to the parent.

use super::{App, EntryKind};
use crate::picker;
use std::path::PathBuf;

impl App {
    /// The kind of the entry `index`. The folders are the first `folder_count` entries.
    pub(crate) fn kind_at(&self, index: usize) -> Option<EntryKind> {
        if index < self.folder_count {
            Some(EntryKind::Folder)
        } else if index < self.images.len() {
            Some(EntryKind::File)
        } else {
            None
        }
    }

    pub(crate) fn is_folder(&self, index: usize) -> bool {
        self.kind_at(index) == Some(EntryKind::Folder)
    }

    /// The count of the files, without the folders.
    pub(crate) fn file_count(&self) -> usize {
        self.images.len().saturating_sub(self.folder_count)
    }

    pub(crate) fn has_files(&self) -> bool {
        self.file_count() > 0
    }

    /// The index of the first file: the one after the folders.
    pub(crate) fn first_file(&self) -> usize {
        self.folder_count
    }

    pub(crate) fn selected_is_folder(&self) -> bool {
        self.gallery
            .selected_index()
            .is_some_and(|index| self.is_folder(index))
    }

    /// The folder of the grid: the known one, or the start folder of the command line.
    pub(crate) fn gallery_dir(&self) -> PathBuf {
        self.current_dir
            .clone()
            .or_else(|| {
                self.initial_dir
                    .as_deref()
                    .map(picker::canonicalize_or_self)
            })
            .unwrap_or_else(|| picker::initial_picker_dir(&self.images))
    }

    /// Enter on a folder cell: the grid switches to that folder. `false` on a file.
    pub(crate) fn enter_selected_folder(&mut self) -> bool {
        let Some(index) = self.gallery.selected_index() else {
            return false;
        };
        if !self.is_folder(index) {
            return false;
        }
        let target = self.images[index].clone();
        let from = self.gallery_dir();
        self.switch_to_dir(&target);
        // The parent lists the folder that the user came from, so the cursor lands on it.
        if from.parent() == Some(target.as_path()) {
            self.return_to = Some(from);
        }
        true
    }

    /// Backspace: the grid switches to the parent folder, with the cursor on the folder that the
    /// user came from. The root has no parent, so nothing changes there.
    pub(crate) fn go_to_parent(&mut self) {
        let from = self.gallery_dir();
        let Some(parent) = from.parent().map(PathBuf::from) else {
            return;
        };
        self.switch_to_dir(&parent);
        self.return_to = Some(from);
    }

    /// Put the cursor on the entry that `return_to` names, one time, after the scan ends.
    pub(super) fn place_cursor_on_return(&mut self) {
        let Some(path) = self.return_to.take() else {
            return;
        };
        let position = self
            .gallery
            .filtered_indices
            .iter()
            .position(|&index| self.images[index] == path);
        if let Some(position) = position {
            self.place_cursor(position);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::{DeleteMode, EntryKind, ViewMode};
    use crate::test_support::{app_with, folder_app, wait_until};
    use image::RgbaImage;
    use ratatui::layout::Rect;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn the_folders_are_a_prefix_with_their_own_kind() {
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
    fn enter_opens_the_folder_and_backspace_returns_to_it() {
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

        // Enter on a file does nothing here; Enter on "beta" switches.
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
    fn the_fullscreen_view_and_the_slideshow_skip_the_folders() {
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
        app.loaded = Some(RgbaImage::new(1, 1));
        app.current = 3;
        app.slideshow_step();
        assert_eq!(app.current, 2);
        assert!(app.slideshow.is_some());

        // The autostart opens the first file.
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
    fn no_thumbnail_prefetch_or_shape_job_starts_for_a_folder() {
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
    fn the_marks_and_the_delete_skip_the_folders() {
        let mut app = folder_app(&["..", "sub"], &["a.jpg", "b.jpg"]);
        app.toggle_selection_at_cursor();
        assert!(app.selection.is_empty());
        app.select_all_filtered();
        assert_eq!(app.selection.len(), 2);
        assert!(!app.is_marked(0));
        assert!(!app.is_marked(1));
        assert!(app.is_marked(2));

        app.clear_selection();
        app.gallery.cursor = 1;
        app.begin_delete(DeleteMode::Trash);
        assert!(
            app.pending_delete.is_none(),
            "a folder gets no delete question"
        );

        // A delete of the files keeps the folders and the cursor on a valid entry.
        let temp = tempfile::tempdir().unwrap();
        let photo = temp.path().join("a.png");
        fs::write(&photo, b"synthetic").unwrap();
        let mut app = folder_app(&["..", "sub"], &[]);
        app.images.push(photo.clone());
        app.filenames.push("a.png".to_owned());
        app.gallery.filtered_indices = vec![0, 1, 2];
        app.gallery.cursor = 2;
        app.begin_delete(DeleteMode::Permanent);
        assert_eq!(
            app.pending_delete
                .as_ref()
                .map(|pending| pending.paths.clone()),
            Some(vec![photo.clone()])
        );
        app.confirm_delete();
        assert!(!photo.exists());
        assert_eq!(app.filenames, ["..", "sub"]);
        assert_eq!(app.folder_count, 2);
        assert_eq!(app.gallery.cursor, 1);
        assert!(!app.has_files());
    }

    #[test]
    fn the_search_matches_the_folder_names() {
        let mut app = folder_app(&["..", "summer"], &["winter.jpg"]);
        app.gallery.search_query = "summ".to_owned();
        app.update_filter();
        wait_until(|| {
            app.poll_filter();
            app.filter_rx.is_none()
        });
        assert_eq!(app.gallery.filtered_indices, vec![1]);
        assert!(app.selected_is_folder());
    }

    #[test]
    fn the_scan_keeps_the_folders_in_front_of_the_sorted_files() {
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

        // One file next to a folder opens at once, as in a flat folder.
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
        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert_eq!(app.current, 1);
        assert_eq!(app.filenames, ["..", "only.png"]);
    }

    #[test]
    fn the_reveal_path_of_the_parent_cell_is_the_gallery_folder() {
        let mut app = folder_app(&["..", "sub"], &["a.jpg"]);
        app.current_dir = Some(PathBuf::from("/photos/2026"));
        assert_eq!(app.reveal_path(), Some(PathBuf::from("/photos/2026")));
        app.gallery.cursor = 1;
        assert_eq!(app.reveal_path(), Some(PathBuf::from("/photos/sub")));
        app.gallery.cursor = 2;
        assert_eq!(app.reveal_path(), Some(PathBuf::from("/photos/a.jpg")));
    }

    #[test]
    fn the_root_has_no_parent_to_go_to() {
        let mut app = app_with(&["/a.jpg"]);
        app.current_dir = Some("/".into());
        app.scan_complete = true;
        app.go_to_parent();
        assert_eq!(app.filenames, ["/a.jpg"], "nothing changed");
        assert!(!app.is_scanning(), "no new scan started");
    }
}
