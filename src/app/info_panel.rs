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

    /// The file that yazi reveals when viewr quits: the fullscreen file, or the gallery cursor.
    pub(crate) fn reveal_path(&self) -> Option<&Path> {
        let index = match self.mode {
            ViewMode::Gallery | ViewMode::Picker => self.gallery.selected_index()?,
            ViewMode::Fullscreen => self.current,
            #[cfg(feature = "video")]
            ViewMode::Video => self.current,
        };
        self.images.get(index).map(PathBuf::as_path)
    }

    /// Path the info panel describes: the fullscreen image, or the hovered thumbnail.
    #[cfg_attr(
        not(feature = "video"),
        expect(
            clippy::match_wildcard_for_single_variants,
            reason = "the wildcard also covers ViewMode::Video when the feature is on"
        )
    )]
    pub(crate) fn info_path(&self) -> Option<PathBuf> {
        match self.mode {
            ViewMode::Gallery => self
                .gallery
                .selected_index()
                .and_then(|idx| self.images.get(idx))
                .cloned(),
            ViewMode::Picker => None,
            _ => self.images.get(self.current).cloned(),
        }
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
    fn info_path_follows_the_view_and_info_is_cached() {
        let dir = test_dir();
        let mut app = app_with_files(dir.path(), &["a.png", "b.png"]);
        app.toggle_info();
        assert!(app.info_visible);
        app.toggle_info();
        assert!(!app.info_visible);

        app.gallery.cursor = 1;
        assert_eq!(app.info_path(), Some(app.images[1].clone()));
        app.mode = ViewMode::Fullscreen;
        app.current = 0;
        assert_eq!(app.info_path(), Some(app.images[0].clone()));
        app.mode = ViewMode::Picker;
        assert_eq!(app.info_path(), None);

        let path = app.images[0].clone();
        let first = app.info_for(&path);
        fs::remove_file(&path).unwrap();
        // The cache answers, so the removed file still has its first rows.
        assert_eq!(app.info_for(&path).rows, first.rows);
    }
}
