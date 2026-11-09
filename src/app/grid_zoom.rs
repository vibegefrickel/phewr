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
        if let Some(dir) = self.current_dir.clone() {
            self.state_writer
                .send(&self.state_file, config::StateChange::ClearFolder(dir));
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

    /// Queue the zoom level for this folder and as the global fallback. The writer thread saves it.
    fn remember_grid_zoom(&mut self) {
        let rows = u16::try_from(self.gallery.zoom_rows).unwrap_or(config::MAX_VISIBLE_ROWS);
        let change = config::StateChange::SetRows {
            folder: self.current_dir.clone(),
            rows,
        };
        self.state_writer.send(&self.state_file, change);
    }

    fn after_grid_zoom_change(&mut self) {
        self.mark_dirty();
    }
}

#[cfg(test)]
mod tests {

    use crate::config::{self, StateWriter};
    use crate::test_support::{app_with, char_key, gallery_app, test_dir};

    use std::fs;
    use std::time::Duration;

    /// A delay that no test reaches: only a flush or a drop writes.
    const HOLD: Duration = Duration::from_secs(60);

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
        app.state_writer.flush();
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
        app.state_writer.flush();
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
        assert!(!app.thumb_cache.contains(&0), "the taller picture is stale");
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
        assert!(
            app.thumb_cache.contains(&0),
            "the same size changes nothing"
        );
        assert!(!app.graphics_storage_dirty);

        // Twice as wide cells: a photo takes half the columns, and the grid gets more of them.
        app.set_cell_px((16, 16));
        assert_eq!(app.cell_px, (16, 16));
        assert!(!app.thumb_cache.contains(&0));
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
            assert!(app.thumb_cache.contains(&0), "{key}: the thumbnails stay");
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
        app.state_writer.flush();
        fs::remove_file(&app.state_file).unwrap();
        app.thumb_zoom_out();
        app.state_writer.flush();
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

    /// The ends of a fast run of steps: the file holds the last level, for the folder and global.
    #[test]
    fn fast_steps_end_in_the_last_level() {
        let dir = test_dir();
        let folder = dir.path().join("photos");
        fs::create_dir(&folder).unwrap();
        let mut app = app_with(&["a.png"]);
        app.state_file = dir.path().join("state.toml");
        app.state_writer = StateWriter::with_delay(HOLD);
        app.current_dir = Some(folder.clone());
        let start = app.gallery.zoom_rows;

        app.thumb_zoom_in();
        app.thumb_zoom_in();
        app.thumb_zoom_out();
        assert!(!app.state_file.exists(), "the batch waits for the delay");
        app.state_writer.flush();

        let expected = u16::try_from(start - 1).unwrap();
        assert_eq!(app.gallery.zoom_rows, start - 1);
        assert_eq!(
            config::load_folder_zoom(&app.state_file, &folder),
            Some(expected)
        );
        assert_eq!(config::load_global_zoom(&app.state_file), Some(expected));
    }

    #[test]
    fn reset_without_entry_writes_nothing() {
        let dir = test_dir();
        let folder = dir.path().join("photos");
        fs::create_dir(&folder).unwrap();
        let mut app = app_with(&["a.png"]);
        app.state_file = dir.path().join("state.toml");
        app.current_dir = Some(folder);

        app.thumb_zoom_reset();
        app.state_writer.flush();

        assert!(!app.state_file.exists(), "no entry, no write");
    }

    #[test]
    fn quit_flushes_queued_write() {
        let dir = test_dir();
        let folder = dir.path().join("photos");
        fs::create_dir(&folder).unwrap();
        let mut app = app_with(&["a.png"]);
        let state_file = dir.path().join("state.toml");
        app.state_file = state_file.clone();
        app.state_writer = StateWriter::with_delay(HOLD);
        app.current_dir = Some(folder.clone());

        app.thumb_zoom_in();
        let rows = u16::try_from(app.gallery.zoom_rows).unwrap();
        assert!(!state_file.exists(), "the write is still queued");
        drop(app);

        assert_eq!(config::load_folder_zoom(&state_file, &folder), Some(rows));
    }

    #[test]
    fn broken_state_file_stays_unchanged() {
        let dir = test_dir();
        let mut app = app_with(&["a.png"]);
        app.state_file = dir.path().join("state.toml");
        app.current_dir = Some(dir.path().to_path_buf());
        let broken = "visible_rows = 4\n[zoom\nbad line";
        fs::write(&app.state_file, broken).unwrap();

        app.thumb_zoom_in();
        app.thumb_zoom_reset();
        app.state_writer.flush();

        assert_eq!(fs::read_to_string(&app.state_file).unwrap(), broken);
    }

    /// A folder switch right after a key sees the queued level, not the old file.
    #[test]
    fn folder_switch_sees_queued_levels() {
        let dir = test_dir();
        let folder = dir.path().join("photos");
        let other = dir.path().join("other");
        let mut app = app_with(&["a.png"]);
        app.state_file = dir.path().join("state.toml");
        app.state_writer = StateWriter::with_delay(HOLD);
        app.current_dir = Some(folder.clone());
        config::save_visible_rows(&app.state_file, Some(&folder), 5);
        let start = app.gallery.zoom_rows;

        app.thumb_zoom_in();
        let zoomed = app.gallery.zoom_rows;
        assert_eq!(app.folder_zoom_rows(&folder), zoomed, "the folder level");
        assert_eq!(app.folder_zoom_rows(&other), zoomed, "the global level");

        app.thumb_zoom_reset();
        assert_ne!(zoomed, start);
        assert_eq!(
            app.folder_zoom_rows(&folder),
            zoomed,
            "cleared: the global level"
        );
        assert!(
            !app.state_file.exists()
                || config::load_folder_zoom(&app.state_file, &folder) == Some(5)
        );
    }
}
