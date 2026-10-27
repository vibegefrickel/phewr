//! The results of the directory scan, the sort at its end and the filename filter.

use super::App;
use super::cell_shape::SHAPE_SAMPLE;
use crate::search;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

/// The order of the files after a scan: by the lowercase name, then by the path. The folder
/// prefetch sorts a folder the same way.
pub(super) fn sort_by_name(media: &mut [(PathBuf, String)]) {
    media.sort_by_cached_key(|(path, filename)| (filename.to_lowercase(), path.clone()));
}

impl App {
    pub(crate) fn refresh_from_scanner(&mut self) {
        let scan_errors = self.shared_list.drain_errors();
        if !scan_errors.is_empty() {
            self.error = Some(scan_errors.join("; "));
            self.needs_render = true;
        }

        let new_len = self.shared_list.len();
        if new_len > self.known_len {
            let (new_paths, new_filenames) = self.shared_list.drain_since(self.known_len);
            let old_len = self.images.len();
            self.images.extend(new_paths);
            self.filenames.extend(new_filenames);
            self.folder_count = self.shared_list.folder_count();

            if self.gallery.search_query.is_empty() {
                self.gallery
                    .filtered_indices
                    .extend(old_len..self.images.len());
            } else {
                self.update_filter();
            }

            self.known_len = new_len;
            if old_len == 0 {
                self.needs_render = true;
            }
            if self.file_count() >= SHAPE_SAMPLE {
                self.request_cell_shape();
            }
        }

        if !self.scan_complete && self.shared_list.is_complete() {
            self.scan_complete = true;
            self.finalize_scan_order();
            self.request_cell_shape();
            // viewr always starts in the grid, also with one image: it never opens the
            // fullscreen view by itself.
            self.place_cursor_on_return();
        }
    }

    /// Sort the files by name. The folders stay in front, in the order of the scanner.
    fn finalize_scan_order(&mut self) {
        if self.file_count() < 2 {
            return;
        }

        let selected_path = self
            .gallery
            .selected_index()
            .and_then(|index| self.images.get(index))
            .cloned();
        let current_path = self.images.get(self.current).cloned();
        let first_file = self.first_file();
        let mut media: Vec<(PathBuf, String)> = self
            .images
            .drain(first_file..)
            .zip(self.filenames.drain(first_file..))
            .collect();
        sort_by_name(&mut media);
        let (files, names): (Vec<PathBuf>, Vec<String>) = media.into_iter().unzip();
        self.images.extend(files);
        self.filenames.extend(names);

        self.current = current_path
            .as_ref()
            .and_then(|path| self.images.iter().position(|candidate| candidate == path))
            .unwrap_or(0);
        if self.gallery.search_query.is_empty() {
            self.gallery.filtered_indices = (0..self.images.len()).collect();
            self.gallery.cursor = selected_path
                .as_ref()
                .and_then(|path| self.images.iter().position(|candidate| candidate == path))
                .unwrap_or(0);
            self.gallery.ensure_cursor_visible();
        } else {
            self.update_filter();
        }

        self.shared_list.replace_all(
            self.images.clone(),
            self.filenames.clone(),
            self.folder_count,
        );
        self.known_len = self.images.len();
        self.forget_thumbnails();
        self.prefetcher.invalidate();
        self.graphics_storage_dirty = true;
        self.loaded = None;
        self.needs_render = true;
    }

    pub(crate) fn is_scanning(&self) -> bool {
        !self.scan_complete
    }

    pub(crate) fn update_filter(&mut self) {
        self.filter_cursor = None;
        if self.gallery.search_query.is_empty() {
            self.gallery.filtered_indices = (0..self.images.len()).collect();
            self.gallery.reset_cursor();
            self.needs_render = true;
            self.filter_rx = None;
            return;
        }

        let query = self.gallery.search_query.clone();
        let filenames = self.filenames.clone();
        let (tx, rx) = mpsc::channel();
        self.filter_rx = Some(rx);

        thread::spawn(move || {
            let results = search::filter(&query, &filenames);
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a closed receiver means that nobody waits for the result"
            )]
            let _ = tx.send(results);
        });
    }

    pub(crate) fn poll_filter(&mut self) {
        let results = self.filter_rx.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(results) = results {
            self.gallery.filtered_indices = results;
            match self.filter_cursor.take() {
                Some(position) => self.place_cursor(position),
                None => self.gallery.reset_cursor(),
            }
            self.needs_render = true;
            self.filter_rx = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::app::ViewMode;
    use crate::test_support::{app_with, wait_until};

    use std::path::{Path, PathBuf};

    use crate::image_list::SharedImageList;

    #[test]
    fn scanner_results_extend_the_list_and_sort_at_the_end() {
        let list = SharedImageList::new();
        let mut app = App::new((8, 16), list.clone());
        assert!(app.is_scanning());

        list.push_error("synthetic: denied".to_owned());
        list.push_error("other: denied".to_owned());
        list.push_batch(vec![PathBuf::from("/x/b.png"), PathBuf::from("/x/C.png")]);
        app.refresh_from_scanner();
        assert_eq!(
            app.error.as_deref(),
            Some("synthetic: denied; other: denied")
        );
        assert_eq!(app.gallery.filtered_indices, vec![0, 1]);

        list.push_batch(vec![PathBuf::from("/x/a.png")]);
        app.needs_render = false;
        app.refresh_from_scanner();
        assert_eq!(app.images.len(), 3);
        assert!(!app.needs_render, "only the first batch forces a redraw");

        // Hover and show C.png, then let the scan end: the order sorts, both stay on C.png.
        app.gallery.cursor = 1;
        app.current = 1;
        list.mark_complete();
        app.refresh_from_scanner();
        assert!(!app.is_scanning());
        assert_eq!(app.filenames, ["a.png", "b.png", "C.png"]);
        assert_eq!(app.gallery.cursor, 2);
        assert_eq!(app.current, 2);
        assert_eq!(app.current_path(), Path::new("/x/C.png"));
        assert!(app.graphics_storage_dirty);
        let (paths, _) = list.drain_since(0);
        assert_eq!(paths, app.images);
    }

    #[test]
    fn a_single_scanned_image_stays_in_the_grid() {
        let list = SharedImageList::new();
        let mut app = App::new((8, 16), list.clone());
        list.push_batch(vec![PathBuf::from("/x/only.png")]);
        list.mark_complete();
        app.refresh_from_scanner();
        assert_eq!(app.mode, ViewMode::Gallery);
        // A second refresh after the end changes nothing.
        app.refresh_from_scanner();
        assert_eq!(app.images.len(), 1);
    }

    #[test]
    fn scanner_results_respect_an_active_filter() {
        let list = SharedImageList::new();
        let mut app = App::new((8, 16), list.clone());
        app.gallery.search_query = "be".to_owned();
        list.push_batch(vec![
            PathBuf::from("/x/zebra.png"),
            PathBuf::from("/x/berry.png"),
        ]);
        app.refresh_from_scanner();
        wait_until(|| {
            app.poll_filter();
            app.filter_rx.is_none()
        });
        assert!(app.gallery.filtered_indices.contains(&1));
        assert!(!app.gallery.filtered_indices.contains(&0));

        list.mark_complete();
        app.refresh_from_scanner();
        assert_eq!(app.filenames, ["berry.png", "zebra.png"]);
        wait_until(|| {
            app.poll_filter();
            app.filter_rx.is_none()
        });
        assert_eq!(app.gallery.filtered_indices, vec![0]);
    }

    #[test]
    fn update_filter_runs_in_the_background_and_an_empty_query_resets() {
        let mut app = app_with(&["apple.png", "berry.png", "cherry.png"]);
        app.poll_filter();
        app.gallery.search_query = "rry".to_owned();
        app.gallery.cursor = 1;
        app.update_filter();
        wait_until(|| {
            app.poll_filter();
            app.filter_rx.is_none()
        });
        let mut found = app.gallery.filtered_indices.clone();
        found.sort_unstable();
        assert_eq!(found, vec![1, 2]);
        assert_eq!(app.gallery.cursor, 0);

        app.gallery.search_query.clear();
        app.update_filter();
        assert_eq!(app.gallery.filtered_indices, vec![0, 1, 2]);
        assert!(app.filter_rx.is_none());
    }
}
