//! The size of the grid cells: `+`, `-` and `0` in the gallery, and the remembered size.

use super::App;
use crate::config;

impl App {
    pub(crate) fn thumb_zoom_in(&mut self) {
        if self.gallery.thumb_zoom_in() {
            self.remember_grid_zoom();
            self.after_grid_zoom_change();
        }
    }

    pub(crate) fn thumb_zoom_out(&mut self) {
        if self.gallery.thumb_zoom_out() {
            self.remember_grid_zoom();
            self.after_grid_zoom_change();
        }
    }

    /// Go back to the configured zoom level, and forget the remembered level of this folder.
    pub(crate) fn thumb_zoom_reset(&mut self) {
        if let Some(dir) = &self.current_dir {
            config::clear_folder_zoom(&self.state_file, dir);
        }
        if self.gallery.reset_zoom() {
            self.after_grid_zoom_change();
        }
    }

    /// The label row makes the thumbnails one row smaller: every cached one is stale.
    pub(crate) fn toggle_filenames(&mut self) {
        self.gallery.toggle_filenames();
        self.mark_dirty();
    }

    /// The cell shape and every thumbnail depend on the pixel size of a terminal cell.
    pub(crate) fn set_cell_px(&mut self, cell_px: (u32, u32)) {
        if cell_px == self.cell_px {
            return;
        }
        self.cell_px = cell_px;
        self.gallery
            .set_cell_shape(self.gallery.image_aspect(), cell_px);
        self.mark_dirty();
    }

    /// Save the zoom level for this folder and as the global fallback.
    fn remember_grid_zoom(&self) {
        let rows = u16::try_from(self.gallery.zoom_rows).unwrap_or(config::MAX_VISIBLE_ROWS);
        config::save_visible_rows(&self.state_file, self.current_dir.as_deref(), rows);
    }

    fn after_grid_zoom_change(&mut self) {
        self.mark_dirty();
    }
}

#[cfg(test)]
mod tests {

    use crate::config;
    use crate::test_support::{app_with, char_key, gallery_app, test_dir};

    use std::fs;

    #[test]
    fn zoom_saves_and_reset_clears() {
        let dir = test_dir();
        let folder = dir.path().join("photos");
        fs::create_dir(&folder).unwrap();
        let mut app = app_with(&["a.png"]);
        app.state_file = dir.path().join("state/state.toml");
        app.current_dir = Some(folder.clone());
        let start = app.gallery.zoom_rows;

        app.thumb_zoom_in();
        assert_eq!(app.gallery.zoom_rows, start - 1);
        assert!(app.graphics_storage_dirty);
        let saved = fs::read_to_string(&app.state_file).unwrap();
        assert!(
            saved.contains(&format!("visible_rows = {}", app.gallery.zoom_rows)),
            "{saved}"
        );
        assert!(
            saved.contains(&format!(
                "\"{}\"",
                folder.to_string_lossy().replace('\\', "\\\\")
            )),
            "{saved}"
        );

        app.thumb_zoom_out();
        app.thumb_zoom_out();
        assert_eq!(app.gallery.zoom_rows, start + 1);

        app.thumb_zoom_reset();
        assert_eq!(app.gallery.zoom_rows, start);
        let saved = fs::read_to_string(&app.state_file).unwrap();
        assert!(
            !saved.contains(&format!(
                "\"{}\"",
                folder.to_string_lossy().replace('\\', "\\\\")
            )),
            "{saved}"
        );
        assert!(saved.contains("visible_rows"), "the global fallback stays");

        app.thumb_zoom_reset();
        assert_eq!(app.gallery.zoom_rows, start);
    }

    #[test]
    fn filename_toggle_forgets_thumbnails() {
        let mut app = gallery_app(3);
        app.thumb_cache.insert(0, image::RgbaImage::new(1, 1));
        let rows_before = app.gallery.thumb_rows;

        char_key(&mut app, 'n');

        assert!(app.gallery.show_filenames);
        assert_eq!(app.gallery.thumb_rows, rows_before - 1);
        assert!(!app.thumb_cache.contains(0), "the taller picture is stale");
        assert!(app.graphics_storage_dirty);
        assert!(app.needs_render);
    }

    #[test]
    fn new_cell_size_forgets_thumbnails() {
        let mut app = gallery_app(3);
        app.thumb_cache.insert(0, image::RgbaImage::new(1, 1));
        app.graphics_storage_dirty = false;
        let cols_before = app.gallery.grid_cols;

        app.set_cell_px((8, 16));
        assert!(app.thumb_cache.contains(0), "the same size changes nothing");
        assert!(!app.graphics_storage_dirty);

        // Twice as wide cells: a photo takes half the columns, and the grid gets more of them.
        app.set_cell_px((16, 16));
        assert_eq!(app.cell_px, (16, 16));
        assert!(!app.thumb_cache.contains(0));
        assert!(app.graphics_storage_dirty);
        assert!(app.gallery.grid_cols > cols_before);
    }

    /// A `+` at the largest cells and a `-` at the most rows change nothing: the thumbnails stay,
    /// no picture goes, and nothing draws again.
    #[test]
    fn zoom_at_limit_keeps_pictures() {
        let dir = test_dir();
        let mut app = gallery_app(3);
        app.state_file = dir.path().join("state.toml");
        for (key, limit) in [('+', 2), ('-', 5)] {
            for _ in 0..40 {
                char_key(&mut app, key);
            }
            assert_eq!(app.gallery.zoom_rows, limit);
            app.thumb_cache.insert(0, image::RgbaImage::new(1, 1));
            app.graphics_storage_dirty = false;
            app.needs_render = false;
            app.graphics_out.take_bytes();

            char_key(&mut app, key);

            assert_eq!(app.gallery.zoom_rows, limit);
            assert!(app.thumb_cache.contains(0), "{key}: the thumbnails stay");
            assert!(
                !app.graphics_storage_dirty,
                "{key}: no redraw of the pictures"
            );
            assert!(!app.needs_render, "{key}: no draw");
            assert!(
                app.graphics_out.take_bytes().is_empty(),
                "{key}: no delete in the terminal"
            );
        }
    }

    #[test]
    fn zoom_stops_at_limits() {
        let dir = test_dir();
        let folder = dir.path().join("photos");
        fs::create_dir(&folder).unwrap();
        let mut app = app_with(&["a.png"]);
        app.state_file = dir.path().join("state.toml");
        app.current_dir = Some(folder);
        for _ in 0..40 {
            app.thumb_zoom_out();
        }
        let most = app.gallery.zoom_rows;
        fs::remove_file(&app.state_file).unwrap();
        app.thumb_zoom_out();
        assert_eq!(app.gallery.zoom_rows, most);
        assert!(!app.state_file.exists(), "no change, no write");
    }

    #[test]
    fn unsaved_folder_uses_global_level() {
        let dir = test_dir();
        let folder = dir.path().join("photos");
        fs::create_dir(&folder).unwrap();
        let mut app = app_with(&["a.png"]);
        app.state_file = dir.path().join("state.toml");
        app.current_dir = Some(folder.clone());

        // The state file has a global level but no entry for this folder.
        config::save_visible_rows(&app.state_file, None, 3);
        app.gallery.set_zoom_rows(4);
        assert_eq!(app.gallery.zoom_rows, 4);

        app.apply_folder_zoom(&folder);
        assert_eq!(
            app.gallery.zoom_rows, 3,
            "a folder without an entry uses the global level"
        );
    }
}
