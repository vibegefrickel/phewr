//! The results of the directory scan, the sort at its end and the filename filter.

use super::App;
use super::cell_shape::SHAPE_SAMPLE;
use std::path::PathBuf;

/// `scanner::name_key`, the key of the scanner batches: the end of the scan keeps the order of
/// the first screen. The folder prefetch sorts the same way.
pub(super) fn sort_by_name(media: &mut [(PathBuf, String)]) {
    media.sort_by_cached_key(|(path, _)| crate::scanner::name_key(path));
}

impl App {
    pub(crate) fn refresh_from_scanner(&mut self) {
        let scan_errors = self.shared_list.drain_errors();
        if !scan_errors.is_empty() {
            self.scan_error = Some(scan_errors.join("; "));
            self.needs_render = true;
        }

        if self.shared_list.len() > self.known_len {
            let batch = self.shared_list.batch_since(self.known_len);
            let old_len = self.images.len();
            self.images.extend(batch.paths);
            self.filenames.extend(batch.filenames);
            self.folder_count = self.shared_list.folder_count();

            if self.gallery.search_query.is_empty() {
                self.gallery
                    .filtered_indices
                    .extend(old_len..self.images.len());
            } else {
                self.update_filter();
            }

            self.known_len = batch.len;
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
            // phewr always starts in the grid, even with one image.
            self.place_cursor_on_return();
        }
    }

    /// Sort the files by name. The folders stay in front. A sorted folder scan keeps the list: no
    /// sort, no copy, no redraw.
    fn finalize_scan_order(&mut self) {
        if self.file_count() < 2
            || crate::scanner::is_sorted_by_name(&self.images[self.first_file()..])
        {
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
        self.mark_dirty();
    }

    pub(crate) fn is_scanning(&self) -> bool {
        !self.scan_complete
    }

    pub(crate) fn update_filter(&mut self) {
        if self.gallery.search_query.is_empty() {
            self.gallery.filtered_indices = (0..self.images.len()).collect();
            self.gallery.reset_cursor();
            self.needs_render = true;
            self.filter_rx = None;
            return;
        }

        self.filter_rx = Some(
            self.filter_worker
                .submit(&self.gallery.search_query, &self.filenames),
        );
    }

    pub(crate) fn poll_filter(&mut self) {
        let results = self.filter_rx.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(results) = results {
            self.gallery.filtered_indices = results;
            self.gallery.reset_cursor();
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
    fn results_extend_then_sort() {
        let list = SharedImageList::new();
        let mut app = App::new((8, 16), list.clone());
        assert!(app.is_scanning());

        list.push_error("synthetic: denied".to_owned());
        list.push_error("other: denied".to_owned());
        list.push_batch(vec![PathBuf::from("/x/b.png"), PathBuf::from("/x/C.png")]);
        app.refresh_from_scanner();
        assert_eq!(
            app.scan_error.as_deref(),
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
        let paths = list.batch_since(0).paths;
        assert_eq!(paths, app.images);
    }

    #[test]
    fn sorted_scan_keeps_list_and_screen() {
        let list = SharedImageList::new();
        let mut app = App::new((8, 16), list.clone());
        list.push_batch(vec![PathBuf::from("/x/a.png"), PathBuf::from("/x/B.png")]);
        list.push_batch(vec![PathBuf::from("/x/c.png")]);
        app.refresh_from_scanner();
        app.gallery.cursor = 1;
        app.graphics_storage_dirty = false;

        list.mark_complete();
        app.refresh_from_scanner();

        assert!(!app.is_scanning());
        assert_eq!(app.filenames, ["a.png", "B.png", "c.png"]);
        assert_eq!(app.gallery.cursor, 1);
        assert_eq!(app.gallery.filtered_indices, vec![0, 1, 2]);
        assert!(
            !app.graphics_storage_dirty,
            "the sorted list needs no redraw"
        );
    }

    #[test]
    fn single_image_stays_in_grid() {
        let list = SharedImageList::new();
        let mut app = App::new((8, 16), list.clone());
        list.push_batch(vec![PathBuf::from("/x/only.png")]);
        list.mark_complete();
        app.refresh_from_scanner();
        assert_eq!(app.mode, ViewMode::Gallery);
        app.refresh_from_scanner();
        assert_eq!(app.images.len(), 1);
    }

    #[test]
    fn results_respect_active_filter() {
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
    fn filter_runs_in_background_and_resets() {
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

    #[test]
    fn old_query_never_wins() {
        let mut app = app_with(&["apple.png", "berry.png", "cherry.png"]);
        app.gallery.search_query = "app".to_owned();
        app.update_filter();
        app.gallery.search_query = "che".to_owned();
        app.update_filter();
        wait_until(|| {
            app.poll_filter();
            app.filter_rx.is_none()
        });
        assert_eq!(app.gallery.filtered_indices, vec![2]);
    }
}
