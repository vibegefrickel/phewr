//! The directory picker, and the switch of the gallery to another directory.

use super::{App, ViewMode};
use crate::config;
use crate::image_list::SharedImageList;
use crate::picker::{self, PickerState};
use crate::scanner;
use std::path::Path;

impl App {
    pub(crate) fn open_picker(&mut self) {
        let start = self.gallery_dir();
        self.current_dir = Some(start.clone());
        self.picker = Some(PickerState::new(start));
        self.mode = ViewMode::Picker;
        self.needs_render = true;
    }

    pub(crate) fn close_picker(&mut self) {
        self.picker = None;
        self.mode = ViewMode::Gallery;
        self.needs_render = true;
    }

    /// Replace image list with the contents of `dir` and reset all view state.
    /// Spawns a fresh scanner thread; the previous scanner (if any) is orphaned
    /// against an unused `SharedImageList` clone and will be dropped as it completes.
    pub(crate) fn switch_to_dir(&mut self, dir: &Path) {
        let canonical = picker::canonicalize_or_self(dir);
        self.current_dir = Some(canonical.clone());
        self.apply_folder_zoom(&canonical);

        let new_list = SharedImageList::new();
        scanner::spawn(vec![canonical.clone()], new_list.clone());
        self.shared_list = new_list;

        self.images.clear();
        self.filenames.clear();
        self.folder_count = 0;
        self.return_to = None;
        self.known_len = 0;
        self.scan_complete = false;
        self.selection.clear();
        self.pending_delete = None;
        self.delete_error = None;
        self.slideshow = None;

        self.gallery.search_active = false;
        self.gallery.search_query.clear();
        self.gallery.filtered_indices.clear();
        self.gallery.reset_cursor();
        self.filter_rx = None;
        self.shape_rx = None;
        self.shape_requested = false;

        self.current = 0;
        self.forget_thumbnails();
        self.prefetcher.invalidate();
        self.graphics_storage_dirty = true;
        self.fullscreen_rx = None;
        self.fullscreen_target = None;
        self.loaded = None;
        self.error = None;

        if let Some(ref mut p) = self.picker {
            p.current_dir = canonical;
            p.load_dir();
            p.adjust_scroll(p.filtered_indices.len().max(1));
        }

        self.needs_render = true;
    }

    /// Apply the saved zoom level for `dir`. This runs when viewr opens a folder at start and
    /// on every folder switch. A folder without its own entry falls back to the global saved
    /// level, then to the configured default.
    pub(crate) fn apply_folder_zoom(&mut self, dir: &Path) {
        let rows = config::load_folder_zoom(&self.state_file, dir)
            .or_else(|| config::load_global_zoom(&self.state_file))
            .unwrap_or_else(|| {
                u16::try_from(self.gallery.default_zoom_rows).unwrap_or(config::MAX_VISIBLE_ROWS)
            });
        self.gallery.set_zoom_rows(usize::from(rows));
    }
}

#[cfg(test)]
mod tests {

    use crate::app::ViewMode;
    use crate::config;
    use crate::test_support::{app_with, test_dir, wait_until};
    use image::RgbaImage;

    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn picker_opens_at_the_known_directory_and_switches() {
        let dir = test_dir();
        let root = fs::canonicalize(dir.path()).unwrap();
        let photos = root.join("photos");
        fs::create_dir(&photos).unwrap();
        RgbaImage::new(4, 4).save(photos.join("b.png")).unwrap();
        RgbaImage::new(4, 4).save(photos.join("a.png")).unwrap();

        let mut app = app_with(&["/nowhere/x.png"]);
        app.initial_dir = Some(root.clone());
        app.open_picker();
        assert_eq!(app.mode, ViewMode::Picker);
        assert_eq!(app.current_dir.as_deref(), Some(root.as_path()));
        app.close_picker();
        assert_eq!(app.mode, ViewMode::Gallery);
        assert!(app.picker.is_none());

        app.open_picker();
        app.selection.insert(PathBuf::from("/nowhere/x.png"));
        app.gallery.search_query = "x".to_owned();
        app.switch_to_dir(&photos);
        assert_eq!(app.current_dir.as_deref(), Some(photos.as_path()));
        assert_eq!(app.picker.as_ref().unwrap().current_dir, photos);
        assert!(app.images.is_empty());
        assert!(app.selection.is_empty());
        assert!(app.gallery.search_query.is_empty());

        wait_until(|| {
            app.refresh_from_scanner();
            !app.is_scanning()
        });
        assert_eq!(app.filenames, ["..", "a.png", "b.png"]);
        assert_eq!(app.folder_count, 1);

        // Without a known directory, the picker starts at the folder of the first image.
        let mut app = app_with(&[photos.join("a.png").to_str().unwrap()]);
        app.open_picker();
        assert_eq!(app.current_dir.as_deref(), Some(photos.as_path()));
    }

    #[test]
    fn switching_to_a_folder_applies_its_saved_zoom_level() {
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
