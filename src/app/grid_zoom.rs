//! The size of the grid cells: `+`, `-` and `0` in the gallery, and the remembered size.

use super::App;
use crate::config;

impl App {
    pub(crate) fn thumb_zoom_in(&mut self) {
        if self.gallery.thumb_zoom_in() {
            self.after_grid_zoom_change(true);
        }
    }

    pub(crate) fn thumb_zoom_out(&mut self) {
        if self.gallery.thumb_zoom_out() {
            self.after_grid_zoom_change(true);
        }
    }

    /// Back to the configured zoom level; also forgets the remembered level.
    pub(crate) fn thumb_zoom_reset(&mut self) {
        config::clear_visible_rows(&self.state_file);
        if self.gallery.reset_zoom() {
            self.after_grid_zoom_change(false);
        }
    }

    /// Show or hide the filename labels. The label row makes the thumbnails one row smaller, so
    /// every cached one is stale, as after a zoom change.
    pub(crate) fn toggle_filenames(&mut self) {
        self.gallery.toggle_filenames();
        self.mark_dirty();
    }

    /// The pixel size of a terminal cell changed, after a resize or a font change. The cell
    /// shape and every thumbnail depend on it.
    pub(crate) fn set_cell_px(&mut self, cell_px: (u32, u32)) {
        if cell_px == self.cell_px {
            return;
        }
        self.cell_px = cell_px;
        self.gallery
            .set_cell_shape(self.gallery.image_aspect(), cell_px);
        self.mark_dirty();
    }

    fn after_grid_zoom_change(&mut self, remember: bool) {
        if remember {
            let rows = u16::try_from(self.gallery.zoom_rows).unwrap_or(config::MAX_VISIBLE_ROWS);
            config::save_visible_rows(&self.state_file, rows);
        }
        // Thumbnails are decoded for a pixel size, so every cached one is stale.
        self.mark_dirty();
    }
}

#[cfg(test)]
mod tests {

    use crate::test_support::{app_with, char_key, gallery_app, test_dir};

    use std::fs;

    #[test]
    fn thumb_zoom_writes_and_reset_clears_the_state_file() {
        let dir = test_dir();
        let mut app = app_with(&["a.png"]);
        app.state_file = dir.path().join("state/state.toml");
        let start = app.gallery.zoom_rows;

        app.thumb_zoom_in();
        assert_eq!(app.gallery.zoom_rows, start - 1);
        assert!(app.graphics_storage_dirty);
        let saved = fs::read_to_string(&app.state_file).unwrap();
        assert!(
            saved.contains(&format!("visible_rows = {}", app.gallery.zoom_rows)),
            "{saved}"
        );

        app.thumb_zoom_out();
        app.thumb_zoom_out();
        assert_eq!(app.gallery.zoom_rows, start + 1);

        app.thumb_zoom_reset();
        assert_eq!(app.gallery.zoom_rows, start);
        assert!(!app.state_file.exists());

        // A reset at the default level only removes the state file.
        app.thumb_zoom_reset();
        assert!(!app.state_file.exists());
    }

    #[test]
    fn the_filename_toggle_forgets_the_thumbnails_of_the_taller_box() {
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
    fn a_new_cell_size_forgets_the_thumbnails_and_the_same_one_changes_nothing() {
        let mut app = gallery_app(3);
        app.thumb_cache.insert(0, image::RgbaImage::new(1, 1));
        app.graphics_storage_dirty = false;
        let cols_before = app.gallery.grid_cols;

        app.set_cell_px((8, 16));
        assert!(app.thumb_cache.contains(0), "the same size changes nothing");
        assert!(!app.graphics_storage_dirty);

        // Twice as wide cells: a photo takes half the columns, so the grid gets more of them.
        app.set_cell_px((16, 16));
        assert_eq!(app.cell_px, (16, 16));
        assert!(!app.thumb_cache.contains(0));
        assert!(app.graphics_storage_dirty);
        assert!(app.gallery.grid_cols > cols_before);
    }

    #[test]
    fn thumb_zoom_stops_at_the_limits() {
        let dir = test_dir();
        let mut app = app_with(&["a.png"]);
        app.state_file = dir.path().join("state.toml");
        for _ in 0..40 {
            app.thumb_zoom_out();
        }
        let most = app.gallery.zoom_rows;
        fs::remove_file(&app.state_file).unwrap();
        app.thumb_zoom_out();
        assert_eq!(app.gallery.zoom_rows, most);
        assert!(!app.state_file.exists(), "no change, no write");
    }
}
