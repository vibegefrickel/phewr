//! The marks of the grid, and the delete question with its answer.

use super::{App, ViewMode};
use std::collections::HashSet;
use std::path::PathBuf;

/// Where a deleted file goes: `d` asks for the trash, `D` for the permanent delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeleteMode {
    Trash,
    Permanent,
}

pub(crate) struct PendingDelete {
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) from_fullscreen: bool,
    pub(crate) mode: DeleteMode,
}

impl App {
    /// Mark or unmark the file under the cursor. A folder takes no mark.
    pub(crate) fn toggle_selection_at_cursor(&mut self) {
        let Some(index) = self.gallery.selected_index() else {
            return;
        };
        if self.is_folder(index) {
            return;
        }
        let path = &self.images[index];
        if !self.selection.remove(path) {
            self.selection.insert(path.clone());
        }
    }

    /// Mark every file of the view. The folders stay unmarked.
    pub(crate) fn select_all_filtered(&mut self) {
        for &index in &self.gallery.filtered_indices {
            if index >= self.folder_count {
                self.selection.insert(self.images[index].clone());
            }
        }
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selection.clear();
    }

    pub(crate) fn is_marked(&self, index: usize) -> bool {
        self.images
            .get(index)
            .is_some_and(|p| self.selection.contains(p))
    }

    pub(crate) fn begin_delete(&mut self, mode: DeleteMode) {
        let from_fullscreen = matches!(self.mode, ViewMode::Fullscreen);
        #[cfg(feature = "video")]
        let from_fullscreen = from_fullscreen || matches!(self.mode, ViewMode::Video);

        let paths: Vec<PathBuf> = if from_fullscreen {
            if self.current >= self.images.len() {
                return;
            }
            vec![self.images[self.current].clone()]
        } else if !self.selection.is_empty() {
            let mut v: Vec<PathBuf> = self
                .gallery
                .filtered_indices
                .iter()
                .filter_map(|&i| {
                    let p = self.images.get(i)?;
                    if self.selection.contains(p) {
                        Some(p.clone())
                    } else {
                        None
                    }
                })
                .collect();
            if v.is_empty() {
                v = self.selection.iter().cloned().collect();
            }
            v
        } else if let Some(index) = self.gallery.selected_index()
            && !self.is_folder(index)
        {
            vec![self.images[index].clone()]
        } else {
            return;
        };

        if paths.is_empty() {
            return;
        }

        self.pending_delete = Some(PendingDelete {
            paths,
            from_fullscreen,
            mode,
        });
        self.delete_error = None;
    }

    pub(crate) fn cancel_delete(&mut self) {
        self.pending_delete.take();
    }

    pub(crate) fn confirm_delete(&mut self) {
        let Some(pending) = self.pending_delete.take() else {
            return;
        };

        let mut removed: HashSet<PathBuf> = HashSet::new();
        let mut errors: Vec<String> = Vec::new();
        for p in &pending.paths {
            let result = match pending.mode {
                DeleteMode::Permanent => std::fs::remove_file(p).map_err(|error| error.to_string()),
                DeleteMode::Trash => trash::delete(p).map_err(|error| error.to_string()),
            };
            match result {
                Ok(()) => {
                    removed.insert(p.clone());
                }
                Err(error) => {
                    errors.push(format!("{}: {error}", p.display()));
                }
            }
        }

        if removed.is_empty() {
            self.delete_error = Some(errors.join("; "));
            self.needs_render = true;
            return;
        }

        let current_path = self.images.get(self.current).cloned();
        let cursor_target = self.cursor_after_delete(&removed);

        let mut new_images = Vec::with_capacity(self.images.len() - removed.len());
        let mut new_filenames = Vec::with_capacity(self.filenames.len() - removed.len());
        for (i, p) in self.images.iter().enumerate() {
            if !removed.contains(p) {
                new_images.push(p.clone());
                new_filenames.push(self.filenames[i].clone());
            }
        }

        self.images = new_images;
        self.filenames = new_filenames;
        self.selection.retain(|p| !removed.contains(p));
        self.forget_info(&removed);
        self.known_len = self.images.len();
        self.shared_list.replace_all(
            self.images.clone(),
            self.filenames.clone(),
            self.folder_count,
        );

        if self.gallery.search_query.is_empty() {
            self.filter_rx = None;
            self.gallery.filtered_indices = (0..self.images.len()).collect();
            self.place_cursor(cursor_target);
        } else {
            self.update_filter();
            self.filter_cursor = Some(cursor_target);
        }

        self.mark_dirty();
        self.fullscreen_decode = None;

        self.delete_error = if errors.is_empty() {
            None
        } else {
            Some(errors.join("; "))
        };

        if pending.from_fullscreen {
            if self.has_files() {
                let old_current = current_path
                    .as_ref()
                    .and_then(|p| self.images.iter().position(|q| q == p));
                let target = old_current.unwrap_or_else(|| self.current.min(self.images.len() - 1));
                self.jump_to(target);
            } else {
                self.mode = ViewMode::Gallery;
                self.slideshow = None;
            }
        } else if !self.has_files() {
            self.current = self.first_file();
        } else if self.current >= self.images.len() {
            self.current = self.images.len() - 1;
        }

        self.needs_render = true;
    }

    /// The grid position for the cursor after a delete of `removed`: the image that moves up
    /// into the place of the cursor. The count of the kept images before the cursor is that place.
    fn cursor_after_delete(&self, removed: &HashSet<PathBuf>) -> usize {
        self.gallery
            .filtered_indices
            .iter()
            .take(self.gallery.cursor)
            .filter(|&&index| {
                self.images
                    .get(index)
                    .is_some_and(|path| !removed.contains(path))
            })
            .count()
    }

    /// Put the grid cursor on `position`, or on the last image of a shorter list.
    pub(super) fn place_cursor(&mut self, position: usize) {
        let last = self.gallery.filtered_indices.len().saturating_sub(1);
        self.gallery.cursor = position.min(last);
        self.gallery.ensure_cursor_visible();
    }
}

#[cfg(test)]
mod tests {

    use super::{DeleteMode, PendingDelete};
    use crate::app::ViewMode;
    use crate::test_support::{app_with, app_with_files, test_dir, wait_until};
    use std::path::{Path, PathBuf};

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
    fn begin_delete_picks_the_paths_of_the_view() {
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
    fn confirm_delete_removes_files_from_the_grid() {
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
    fn confirm_delete_in_the_grid_keeps_the_cursor_in_place() {
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
    fn confirm_delete_reports_the_files_that_stay() {
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
    fn confirm_delete_in_fullscreen_moves_on_and_ends_in_the_gallery() {
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
    fn confirm_delete_in_the_grid_keeps_the_current_index_valid() {
        let dir = test_dir();
        let mut app = app_with_files(dir.path(), &["a.png"]);
        app.begin_delete(DeleteMode::Permanent);
        app.confirm_delete();
        assert!(app.images.is_empty());
        assert_eq!(app.current, 0);
        assert_eq!(app.mode, ViewMode::Gallery);
    }

    #[test]
    fn confirm_delete_keeps_an_active_filter() {
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
    fn confirm_delete_with_a_filter_keeps_the_cursor_place() {
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
}
