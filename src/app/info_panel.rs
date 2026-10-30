//! The info panel: its toggle, the file that it shows and the cached EXIF data.

use super::{App, ViewMode};
use crate::info::{self, ImageInfo};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

impl App {
    pub(crate) fn toggle_info(&mut self) {
        self.info_visible = !self.info_visible;
        // The panel changes the image/grid area, so placements must be redone.
        self.needs_render = true;
    }

    /// The image that the view is on: the fullscreen file, or the gallery cursor.
    fn view_index(&self) -> Option<usize> {
        match self.mode {
            ViewMode::Gallery => self.gallery.selected_index(),
            ViewMode::Fullscreen => Some(self.current),
            #[cfg(feature = "video")]
            ViewMode::Video => Some(self.current),
        }
    }

    /// The file that yazi reveals when viewr quits: the fullscreen file, or the gallery cursor.
    /// A subfolder cell reveals that folder. The `..` cell reveals the folder of the grid, so
    /// that yazi stays where viewr was.
    pub(crate) fn reveal_path(&self) -> Option<PathBuf> {
        let index = self.view_index()?;
        if self.is_parent_entry(index) {
            return Some(self.gallery_dir());
        }
        self.images.get(index).cloned()
    }

    /// The file that the info panel and the status row describe: the open file, or the gallery
    /// cursor. A folder is no file.
    pub(crate) fn view_file(&self) -> Option<PathBuf> {
        let index = self.view_index()?;
        if self.is_folder(index) {
            return None;
        }
        self.images.get(index).cloned()
    }

    /// The file of the info panel, for `ui.rs`. It goes when `ui.rs` calls `view_file`.
    pub(crate) fn info_path(&self) -> Option<PathBuf> {
        self.view_file()
    }

    /// Metadata for `path`, read on first use and cached for the session.
    pub(crate) fn info_for(&mut self, path: &Path) -> ImageInfo {
        if let Some(cached) = self.info_cache.get(path) {
            return cached.clone();
        }
        let loaded = info::load(path);
        self.info_cache.put(path.to_path_buf(), loaded.clone());
        loaded
    }

    #[expect(
        clippy::iter_over_hash_type,
        reason = "each entry goes on its own; order does not matter"
    )]
    pub(super) fn forget_info(&mut self, paths: &HashSet<PathBuf>) {
        for path in paths {
            self.info_cache.pop(path);
        }
    }
}

#[cfg(test)]
mod tests {

    use crate::app::ViewMode;
    use crate::test_support::{app_with_files, test_dir};

    use std::fs;

    #[test]
    fn the_view_file_follows_the_view_and_info_is_cached() {
        let dir = test_dir();
        let mut app = app_with_files(dir.path(), &["a.png", "b.png"]);
        app.toggle_info();
        assert!(app.info_visible);
        app.toggle_info();
        assert!(!app.info_visible);

        app.gallery.cursor = 1;
        assert_eq!(app.view_file(), Some(app.images[1].clone()));
        app.mode = ViewMode::Fullscreen;
        app.current = 0;
        assert_eq!(app.view_file(), Some(app.images[0].clone()));

        let path = app.images[0].clone();
        let first = app.info_for(&path);
        fs::remove_file(&path).unwrap();
        // The cache answers, so the removed file still has its first rows.
        assert_eq!(app.info_for(&path).rows, first.rows);
    }
}
