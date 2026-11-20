//! The folder entries of the grid: the kinds, the step into a folder and the step to the
//! parent.

use super::{App, EntryKind};
use crate::scanner::canonicalize_or_self;
use std::path::{Path, PathBuf};

impl App {
    const fn kind_at(&self, index: usize) -> Option<EntryKind> {
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

    /// No folder can carry the name `..`.
    pub(crate) fn is_parent_entry(&self, index: usize) -> bool {
        self.is_folder(index) && self.filenames.get(index).is_some_and(|name| name == "..")
    }

    pub(crate) const fn file_count(&self) -> usize {
        self.images.len().saturating_sub(self.folder_count)
    }

    pub(crate) const fn has_files(&self) -> bool {
        self.file_count() > 0
    }

    /// The entry after the folders.
    pub(crate) const fn first_file(&self) -> usize {
        self.folder_count
    }

    pub(crate) fn selected_is_folder(&self) -> bool {
        self.gallery
            .selected_index()
            .is_some_and(|index| self.is_folder(index))
    }

    /// The known folder of the grid, or the start folder of the command line.
    pub(crate) fn gallery_dir(&self) -> PathBuf {
        self.current_dir
            .clone()
            .or_else(|| self.initial_dir.as_deref().map(canonicalize_or_self))
            .unwrap_or_else(|| {
                let first_parent = self.images.first().and_then(|path| path.parent());
                canonicalize_or_self(first_parent.unwrap_or_else(|| Path::new(".")))
            })
    }

    /// Enter on a folder cell switches to that folder. `false` on a file.
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
        // The parent lists the folder that the user came from, and the cursor lands on it.
        if from.parent() == Some(target.as_path()) {
            self.return_to = Some(from);
        }
        true
    }

    /// Backspace: switch to the parent folder, with the cursor on the folder before. The root has
    /// no parent.
    pub(crate) fn go_to_parent(&mut self) {
        let from = self.gallery_dir();
        let Some(parent) = from.parent().map(PathBuf::from) else {
            return;
        };
        self.switch_to_dir(&parent);
        self.return_to = Some(from);
    }

    /// The start with one file argument: the cursor goes to that file after the scan.
    pub(crate) fn place_cursor_after_scan(&mut self, path: PathBuf) {
        self.return_to = Some(path);
    }

    /// Put the cursor on the entry that `return_to` names, once, after the scan ends.
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

    /// Put the grid cursor on `position`, or on the last image of a shorter list.
    pub(super) fn place_cursor(&mut self, position: usize) {
        let last = self.gallery.filtered_indices.len().saturating_sub(1);
        self.gallery.cursor = position.min(last);
        self.gallery.ensure_cursor_visible();
    }
}

#[cfg(test)]
mod tests;
