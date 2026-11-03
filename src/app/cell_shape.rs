//! The cell shape of the grid, from the headers of the first files of a scan.

use super::{App, ViewMode};
use crate::decode;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// The files whose headers decide the cell shape: the first screen, or the first files of a
/// large folder.
pub(super) const SHAPE_SAMPLE: usize = 64;

impl App {
    /// Read the headers of the first `SHAPE_SAMPLE` files in the background, once per scan.
    ///
    /// The grid keeps the shape until the next scan, and the columns do not jump while the user
    /// browses.
    pub(crate) fn request_cell_shape(&mut self) {
        if self.shape_requested || !self.has_files() {
            return;
        }
        self.shape_requested = true;
        let paths: Vec<PathBuf> = self
            .images
            .iter()
            .skip(self.first_file())
            .take(SHAPE_SAMPLE)
            .cloned()
            .collect();
        let (tx, rx) = mpsc::channel();
        self.shape_rx = Some((rx, Instant::now()));
        // A thread of its own: after a directory switch, the rayon pool can be full of old decodes,
        // and the first screen waits for this read.
        thread::spawn(move || {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a closed receiver means that a new scan replaced this one"
            )]
            let _ = tx.send(decode::shape::folder_aspect(&paths));
        });
    }

    /// True while the header read runs. The thumbnail decodes wait: no cell decodes twice for two
    /// cell sizes.
    pub(crate) fn cell_shape_pending(&self) -> bool {
        self.shape_rx.is_some()
    }

    /// How long the running header read takes so far. A local read takes 2 ms to 4 ms.
    pub(crate) fn cell_shape_running_for(&self, now: Instant) -> Option<Duration> {
        let (_, started) = self.shape_rx.as_ref()?;
        Some(now.saturating_duration_since(*started))
    }

    /// Take the folder shape. A changed grid drops the thumbnails.
    pub(crate) fn poll_cell_shape(&mut self) {
        let Some(aspect) = self
            .shape_rx
            .as_ref()
            .and_then(|(rx, _)| rx.try_recv().ok())
        else {
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
    fn shape_reads_headers_once_per_scan() {
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

        assert_eq!(app.cell_shape_running_for(std::time::Instant::now()), None);
        app.request_cell_shape();
        assert!(app.cell_shape_pending());
        assert!(app.has_pending_work());
        let later = std::time::Instant::now() + std::time::Duration::from_millis(30);
        assert!(
            app.cell_shape_running_for(later)
                .is_some_and(|running| running >= std::time::Duration::from_millis(30))
        );
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

        app.request_cell_shape();
        assert!(!app.cell_shape_pending());

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
    fn scan_end_requests_shape() {
        let dir = test_dir();
        RgbaImage::new(8, 8).save(dir.path().join("a.png")).unwrap();
        let list = SharedImageList::new();
        crate::scanner::spawn(vec![dir.path().to_path_buf()], list.clone());
        let mut app = App::new((8, 16), list);
        wait_until(|| {
            app.refresh_from_scanner();
            app.scan_complete
        });
        assert_eq!(app.filenames, ["..", "a.png"]);
        assert_eq!(app.file_count(), 1);
        assert!(
            app.cell_shape_pending(),
            "the complete scan starts the header read"
        );
    }
}
