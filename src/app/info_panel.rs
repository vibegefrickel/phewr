//! The info panel: its toggle, the file that it shows and the cached EXIF data.

use super::{App, ViewMode};
use crate::info::{self, ImageInfo};
use std::path::{Path, PathBuf};

impl App {
    pub(crate) fn toggle_info(&mut self) {
        self.info_visible = !self.info_visible;
        // The panel changes the image area and the grid area. Every picture needs a new placement.
        self.needs_render = true;
    }

    /// The fullscreen file, or the gallery cursor.
    fn view_index(&self) -> Option<usize> {
        match self.mode {
            ViewMode::Gallery => self.gallery.selected_index(),
            ViewMode::Fullscreen => Some(self.current),
            #[cfg(feature = "video")]
            ViewMode::Video => Some(self.current),
        }
    }

    /// The file that yazi reveals when viewr quits: the fullscreen file, or the gallery cursor.
    /// The `..` cell reveals the folder of the grid: yazi stays where viewr was.
    pub(crate) fn reveal_path(&self) -> Option<PathBuf> {
        let index = self.view_index()?;
        if self.is_parent_entry(index) {
            return Some(self.gallery_dir());
        }
        self.images.get(index).cloned()
    }

    /// The file that the info panel and the status row describe. A folder is no file.
    pub(crate) fn view_file(&self) -> Option<PathBuf> {
        let index = self.view_index()?;
        if self.is_folder(index) {
            return None;
        }
        self.images.get(index).cloned()
    }

    /// The metadata of `path`, read at the first use and kept for the session.
    pub(crate) fn info_for(&mut self, path: &Path) -> ImageInfo {
        if let Some(cached) = self.info_cache.get(path) {
            return cached.clone();
        }
        let loaded = info::load(path);
        self.info_cache.put(path.to_path_buf(), loaded.clone());
        loaded
    }
}

#[cfg(test)]
mod tests {

    use crate::app::ViewMode;
    use crate::test_support::{app_with_files, test_dir};

    use std::fs;

    #[test]
    fn view_file_follows_view() {
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
        // The cache answers: the removed file still has its first rows.
        assert_eq!(app.info_for(&path).rows, first.rows);
    }
}
