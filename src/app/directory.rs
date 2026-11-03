//! The switch of the gallery to another directory.

use super::App;
use crate::config;
use crate::image_list::SharedImageList;
use crate::scanner::{self, canonicalize_or_self};
use std::path::Path;

impl App {
    /// Show the folder `dir` and start the view state again. A new scanner thread fills a new list,
    /// and the old scanner ends without a reader.
    pub(crate) fn switch_to_dir(&mut self, dir: &Path) {
        // The park key is the size that this folder decoded for. The park comes before the zoom of
        // the new folder.
        self.park_thumbnails();
        let canonical = canonicalize_or_self(dir);
        self.current_dir = Some(canonical.clone());
        self.apply_folder_zoom(&canonical);

        let new_list = SharedImageList::new();
        scanner::spawn(vec![canonical.clone()], new_list.clone());
        self.shared_list = new_list;

        self.images.clear();
        self.filenames.clear();
        self.folder_count = 0;
        self.return_to = None;
        self.folder_prefetch.disarm_parent();
        self.known_len = 0;
        self.scan_complete = false;
        self.selection.clear();
        self.pending_delete = None;
        self.delete_error = None;
        self.slideshow = None;
        self.reset_zoom_and_source();

        self.gallery.search_active = false;
        self.gallery.search_query.clear();
        self.gallery.filtered_indices.clear();
        self.gallery.reset_cursor();
        self.filter_rx = None;
        self.shape_rx = None;
        self.shape_requested = false;

        self.current = 0;
        self.mark_dirty();
        self.fullscreen_decode = None;
        self.error = None;
        self.scan_error = None;
    }

    /// Apply the saved zoom level for `dir`. A folder without its own entry falls back to the
    /// global level, then to the configured default.
    pub(crate) fn apply_folder_zoom(&mut self, dir: &Path) {
        let rows = self.folder_zoom_rows(dir);
        self.gallery.set_zoom_rows(rows);
    }

    /// The zoom level that a switch to `dir` applies.
    pub(super) fn folder_zoom_rows(&self, dir: &Path) -> usize {
        let rows = config::load_folder_zoom(&self.state_file, dir)
            .or_else(|| config::load_global_zoom(&self.state_file))
            .unwrap_or_else(|| {
                u16::try_from(self.gallery.default_zoom_rows).unwrap_or(config::MAX_VISIBLE_ROWS)
            });
        usize::from(rows)
    }
}

#[cfg(test)]
mod tests {

    use crate::config;
    use crate::test_support::{app_with, test_dir};
    use image::RgbaImage;

    use std::fs;

    #[test]
    fn switch_applies_saved_zoom() {
        let dir = test_dir();
        let root = fs::canonicalize(dir.path()).unwrap();
        let photos = root.join("photos");
        let other = root.join("other");
        fs::create_dir(&photos).unwrap();
        fs::create_dir(&other).unwrap();
        RgbaImage::new(4, 4).save(photos.join("a.png")).unwrap();
        RgbaImage::new(4, 4).save(other.join("b.png")).unwrap();

        let mut app = app_with(&["/nowhere/x.png"]);
        app.state_file = dir.path().join("state.toml");
        config::save_visible_rows(&app.state_file, Some(&photos), 3);

        app.switch_to_dir(&other);
        assert_eq!(
            app.gallery.zoom_rows, 3,
            "a folder without an entry falls back to the global level"
        );

        app.switch_to_dir(&photos);
        assert_eq!(
            app.gallery.zoom_rows, 3,
            "the saved level of the folder is applied"
        );
    }
}
