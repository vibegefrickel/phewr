//! The cell shape of the grid, from the headers of the first files of a scan.

use super::{App, ViewMode};
use crate::decode;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

/// The files whose headers decide the cell shape of the grid: the first screen, or the first
/// files of a large folder.
pub(super) const SHAPE_SAMPLE: usize = 64;

impl App {
    /// Read the headers of the first `SHAPE_SAMPLE` files in the background, one time per scan.
    /// The grid keeps the shape until the next scan, so the columns do not jump while the user
    /// browses. A later call does nothing.
    pub(crate) fn request_cell_shape(&mut self) {
        if self.shape_requested || self.images.is_empty() {
            return;
        }
        self.shape_requested = true;
        let paths: Vec<PathBuf> = self.images.iter().take(SHAPE_SAMPLE).cloned().collect();
        let (tx, rx) = mpsc::channel();
        self.shape_rx = Some(rx);
        // A thread of its own: the rayon pool can be full of decodes of the old folder after a
        // directory switch, and the first screen of the new folder waits for this read.
        thread::spawn(move || {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a closed receiver means that a new scan replaced this one"
            )]
            let _ = tx.send(decode::shape::folder_aspect(&paths));
        });
    }

    /// True while the header read runs. The thumbnail decodes wait, so that no cell decodes
    /// twice for two cell sizes.
    pub(crate) fn cell_shape_pending(&self) -> bool {
        self.shape_rx.is_some()
    }

    /// Take the folder shape. A changed grid drops the thumbnails. The gallery draws again,
    /// because its first screen waited for the shape. A fullscreen image stays as it is.
    pub(crate) fn poll_cell_shape(&mut self) {
        let Some(aspect) = self.shape_rx.as_ref().and_then(|rx| rx.try_recv().ok()) else {
            return;
        };
        self.shape_rx = None;
        if self.gallery.set_cell_shape(aspect, self.cell_px) {
            self.forget_thumbnails();
            self.graphics_storage_dirty = true;
        }
        if self.mode == ViewMode::Gallery {
            self.needs_render = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::test_support::{app, test_dir, wait_until};

    use image::RgbaImage;
    use ratatui::layout::Rect;

    use crate::image_list::SharedImageList;

    #[test]
    fn the_folder_shape_comes_from_the_headers_once_per_scan() {
        let dir = test_dir();
        let mut app = app();
        for index in 0..3 {
            let path = dir.path().join(format!("{index}.png"));
            RgbaImage::new(30, 40).save(&path).unwrap();
            app.images.push(path);
            app.filenames.push(format!("{index}.png"));
        }
        app.gallery.filtered_indices = (0..3).collect();
        app.gallery.update_grid(Rect::new(0, 0, 120, 20));
        let landscape_cols = app.gallery.grid_cols;
        assert!(!app.cell_shape_pending());

        app.request_cell_shape();
        assert!(app.cell_shape_pending());
        assert!(app.has_pending_work());
        wait_until(|| {
            app.poll_cell_shape();
            !app.cell_shape_pending()
        });
        assert!(
            app.gallery.grid_cols > landscape_cols,
            "portraits: more columns"
        );
        assert!(app.needs_render);
        let portrait_cols = app.gallery.grid_cols;

        // A second request in the same scan does nothing.
        app.request_cell_shape();
        assert!(!app.cell_shape_pending());

        // A new directory forgets the shape; the next scan reads it again.
        app.switch_to_dir(dir.path());
        app.images = vec![dir.path().join("0.png")];
        app.gallery.filtered_indices = vec![0];
        app.gallery.update_grid(Rect::new(0, 0, 120, 20));
        assert_eq!(
            app.gallery.grid_cols, portrait_cols,
            "the old shape stays until the read"
        );
        app.request_cell_shape();
        assert!(app.cell_shape_pending());
    }

    #[test]
    fn the_scan_requests_the_shape_when_it_completes() {
        let dir = test_dir();
        RgbaImage::new(8, 8).save(dir.path().join("a.png")).unwrap();
        let list = SharedImageList::new();
        crate::scanner::spawn(vec![dir.path().to_path_buf()], list.clone());
        let mut app = App::new((8, 16), list);
        wait_until(|| {
            app.refresh_from_scanner();
            app.scan_complete
        });
        assert_eq!(app.images.len(), 1);
        assert!(
            app.cell_shape_pending(),
            "the complete scan starts the header read"
        );
    }
}
