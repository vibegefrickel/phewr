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

    use crate::test_support::{app_with, test_dir};

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
