//! The marks of the grid, and the delete question with its answer.

use super::{App, ViewMode};
use std::collections::HashSet;
use std::path::PathBuf;

/// `d` asks for the trash, `D` for the permanent delete.
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

    /// The grid asks per cell per frame. An empty selection answers without a path hash.
    pub(crate) fn is_marked(&self, index: usize) -> bool {
        !self.selection.is_empty()
            && self
                .images
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

        let mut new_images = Vec::with_capacity(self.images.len().saturating_sub(removed.len()));
        let mut new_filenames =
            Vec::with_capacity(self.filenames.len().saturating_sub(removed.len()));
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

    /// The grid position for the cursor after a delete: the count of the kept images before the
    /// cursor.
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
mod tests;
