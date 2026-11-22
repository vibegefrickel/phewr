//! The info pane: its toggle, the toggle of the pane layout, the file that it shows and the
//! cached EXIF data.

use super::{App, ViewMode};
use crate::info::{self, ImageInfo};
use std::io;
use std::path::{Path, PathBuf};

impl App {
    /// Tab, `Ctrl-B` and the right click: show or hide the info pane. Without the info pane, no
    /// layout draws a box, so `Ctrl-B` needs no change of the layout. A slideshow shows no panes, so the
    /// toggle does nothing there.
    pub(crate) fn toggle_info(&mut self) -> io::Result<()> {
        if self.slideshow_fills_screen() {
            return Ok(());
        }
        self.info_visible = !self.info_visible;
        self.change_panes()
    }

    /// `B`: the other pane layout for this session. The config file keeps its own value. A
    /// slideshow shows no panes, so the toggle does nothing there. Without the info pane, both
    /// layouts draw the same screen, so the pictures stay.
    pub(crate) fn toggle_layout(&mut self) -> io::Result<()> {
        if self.slideshow_fills_screen() {
            return Ok(());
        }
        self.pane_layout = self.pane_layout.toggled();
        if !self.info_visible {
            return Ok(());
        }
        self.change_panes()
    }

    /// A running or paused slideshow outside the grid: the picture takes the whole screen,
    /// without the panes and without the status row.
    pub(crate) fn slideshow_fills_screen(&self) -> bool {
        self.slideshow_running() && self.mode != ViewMode::Gallery
    }

    /// A pane change moves the main pane. Kitty draws the pictures above the new frame lines, so
    /// they go first. The grid takes new thumbnail sizes, so it is dirty. The fullscreen picture
    /// keeps its decode and its prefetched neighbors: `load_if_needed` fits it to the new rect.
    fn change_panes(&mut self) -> io::Result<()> {
        self.graphics_delete_all()?;
        if self.mode == ViewMode::Gallery {
            self.mark_dirty();
        } else {
            self.needs_render = true;
        }
        Ok(())
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

    /// The file that yazi reveals when phewr quits: the fullscreen file, or the gallery cursor.
    /// The `..` cell reveals the folder of the grid: yazi stays where phewr was.
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
    use crate::test_support::{app_with_files, fullscreen_app, gallery_app, test_dir, text};
    use tuilibr::frame::Layout;

    use std::fs;

    #[test]
    fn view_file_follows_view() {
        let dir = test_dir();
        let mut app = app_with_files(dir.path(), &["a.png", "b.png"]);
        app.toggle_info().unwrap();
        assert!(app.info_visible);
        assert!(text(&app.graphics_out.take_bytes()).contains("a=d"));
        app.toggle_info().unwrap();
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

    /// `B` switches the frame for the session: with the info pane, the pictures go, and the grid
    /// draws again. Without it, the screen stays the same, and so do the pictures.
    #[test]
    fn toggle_layout_deletes_the_pictures() {
        let dir = test_dir();
        let mut app = app_with_files(dir.path(), &["a.png"]);
        assert_eq!(app.pane_layout, Layout::Maximal);
        app.needs_render = false;
        app.toggle_layout().unwrap();
        assert_eq!(app.pane_layout, Layout::Minimal);
        assert!(!app.needs_render);
        assert!(app.graphics_out.take_bytes().is_empty());

        app.info_visible = true;
        app.toggle_layout().unwrap();
        assert_eq!(app.pane_layout, Layout::Maximal);
        assert!(app.needs_render);
        assert!(text(&app.graphics_out.take_bytes()).contains("a=d"));
    }

    /// The fullscreen view keeps its decode after a pane change: the draw fits it to the new
    /// rect. The grid takes new thumbnail sizes, so it is dirty.
    #[test]
    fn fullscreen_pane_change_keeps_the_decode() {
        let mut app = fullscreen_app(3, 1);
        app.loaded = Some(image::RgbaImage::new(4, 4).into());
        app.graphics_storage_dirty = false;
        app.needs_render = false;
        app.toggle_info().unwrap();
        assert!(app.info_visible);
        assert!(app.loaded.is_some());
        assert!(!app.graphics_storage_dirty);
        assert!(app.needs_render);
        assert!(text(&app.graphics_out.take_bytes()).contains("a=d"));

        let mut app = gallery_app(3);
        app.info_visible = true;
        app.graphics_storage_dirty = false;
        app.toggle_layout().unwrap();
        assert!(app.graphics_storage_dirty);
    }

    /// A slideshow shows no panes: Tab, `B` and the right click change nothing and delete no
    /// picture.
    #[test]
    fn slideshow_ignores_pane_toggles() {
        let mut app = fullscreen_app(3, 1);
        app.start_slideshow();
        app.loaded = Some(image::RgbaImage::new(4, 4).into());
        app.graphics_out.take_bytes();
        app.toggle_info().unwrap();
        app.toggle_layout().unwrap();
        assert!(!app.info_visible);
        assert_eq!(app.pane_layout, Layout::Maximal);
        assert!(app.loaded.is_some());
        assert!(app.graphics_out.take_bytes().is_empty());
    }
}
