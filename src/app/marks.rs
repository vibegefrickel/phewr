//! The marks of the grid.

use super::App;

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
}

#[cfg(test)]
mod tests;
