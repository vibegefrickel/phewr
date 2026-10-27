//! The fullscreen view: enter and leave, the steps through the list and the decode.

#[cfg(feature = "video")]
use super::VideoReturn;
use super::{App, ViewMode};
use crate::decode::resize::load_and_resize_px;
use crate::decode::{EmbeddedThumbnail, spawn_foreground};
use ratatui::layout::Rect;

impl App {
    pub(crate) fn enter_fullscreen_selected(&mut self) {
        if let Some(idx) = self.gallery.selected_index() {
            self.enter_fullscreen_at(idx);
        }
    }

    /// Open the file `idx`. A folder entry opens nothing.
    pub(crate) fn enter_fullscreen_at(&mut self, idx: usize) {
        if idx >= self.images.len() || self.is_folder(idx) {
            return;
        }
        #[cfg(feature = "video")]
        if self.plays_at(idx) {
            self.enter_video(idx);
            return;
        }
        self.current = idx;
        self.mode = ViewMode::Fullscreen;
        self.error = None;
        self.needs_render = true;
        self.fullscreen_rx = None;
        self.fullscreen_target = None;
        self.loaded = None;
        self.reset_zoom_state();
    }

    fn start_fullscreen_decode(&mut self, idx: usize, rect: Rect) {
        self.fullscreen_target = Some((idx, rect));
        let path = self.images[idx].clone();
        let target = (
            u32::from(rect.width) * self.cell_px.0,
            u32::from(rect.height) * self.cell_px.1,
        );
        self.fullscreen_rx = Some(spawn_foreground(move || {
            load_and_resize_px(&path, target, EmbeddedThumbnail::Deny)
        }));
    }

    pub(crate) fn poll_fullscreen(&mut self) -> bool {
        let result = self
            .fullscreen_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok());
        if let Some(result) = result {
            if let Some((idx, rect)) = self.fullscreen_target
                && idx == self.current
                && rect == self.image_rect
                && self.mode == ViewMode::Fullscreen
            {
                match result {
                    Ok(img) => {
                        self.loaded = Some(img);
                        self.error = None;
                        self.loaded_for_rect = rect;
                        self.needs_render = true;
                    }
                    Err(e) => {
                        self.error = Some(e.to_string());
                    }
                }
            }
            self.fullscreen_rx = None;
            self.fullscreen_target = None;
            return true;
        }
        false
    }

    pub(crate) fn enter_gallery(&mut self) {
        self.slideshow = None;
        self.mode = ViewMode::Gallery;
        self.loaded = None;
        self.needs_render = true;
    }

    pub(crate) fn next(&mut self) {
        if self.current + 1 < self.images.len() {
            self.jump_to(self.current + 1);
        }
    }

    /// The step back stops at the first file, because the folders come before it.
    pub(crate) fn prev(&mut self) {
        if self.current > self.first_file() {
            self.jump_to(self.current - 1);
        }
    }

    pub(crate) fn first(&mut self) {
        let first = self.first_file();
        if self.has_files() && self.current != first {
            self.jump_to(first);
        }
    }

    pub(crate) fn last(&mut self) {
        let last = self.images.len().saturating_sub(1);
        if self.has_files() && self.current != last {
            self.jump_to(last);
        }
    }

    pub(super) fn jump_to(&mut self, target: usize) {
        // A step away from the placeholder stops the player and forgets the mpv window.
        #[cfg(feature = "video")]
        {
            self.video = None;
            self.video_window = None;
        }

        // The image on the screen goes back to the prefetch cache, so that a step back is free.
        if self.zoom <= 1.0
            && self.loaded_for_rect == self.image_rect
            && let Some(shown) = self.loaded.take()
        {
            self.prefetcher
                .store(self.current, shown, self.image_rect, self.cell_px);
        }
        self.current = target;
        self.error = None;
        self.needs_render = true;
        self.fullscreen_rx = None;
        self.fullscreen_target = None;
        self.reset_zoom_state();

        #[cfg(feature = "video")]
        if self.plays_at(self.current) {
            self.mode = ViewMode::Video;
            self.video_return = VideoReturn::Fullscreen;
            self.loaded = None;
            self.clear_before_video = true;
            return;
        }

        self.mode = ViewMode::Fullscreen;
        // A prefetch result that arrived since the last pass of the loop counts as a hit.
        self.prefetcher.poll();
        if let Some(img) = self
            .prefetcher
            .take_resized(self.current, self.image_rect, self.cell_px)
        {
            self.loaded = Some(img);
            self.loaded_for_rect = self.image_rect;
        } else {
            self.loaded = None;
            self.start_fullscreen_decode(self.current, self.image_rect);
        }
    }

    /// Decode the hovered cell at the size of the fullscreen box, so that Enter shows it at
    /// once. Without the hint, the decode came back at full size and `take_resized` scaled a
    /// 24 MP frame on the UI thread (50 ms to 70 ms).
    pub(crate) fn pre_decode_hovered(&mut self) {
        if let Some(idx) = self.gallery.selected_index()
            && !self.is_folder(idx)
        {
            self.prefetcher
                .set_target_hint(self.fullscreen_box, self.cell_px);
            self.prefetcher.kick_gallery(idx, &self.images);
        }
    }

    /// Decode the neighbors of the fullscreen file, the files only.
    pub(crate) fn prefetch_neighbors(&mut self) {
        self.prefetcher
            .set_target_hint(self.image_rect, self.cell_px);
        self.prefetcher
            .kick(self.current, self.first_file(), &self.images);
    }

    pub(crate) fn load_if_needed(&mut self) {
        if self.fullscreen_on_video() {
            return;
        }
        // Zoomed view: crop the full-resolution source around the pan point and
        // resize that crop to fill the viewport.
        if self.zoom > 1.0 {
            if self.source_for == Some(self.current) {
                if (self.zoom_dirty
                    || self.loaded.is_none()
                    || self.loaded_for_rect != self.image_rect)
                    && let Some(view) = self.build_zoom_view()
                {
                    self.loaded = Some(view);
                    self.error = None;
                    self.loaded_for_rect = self.image_rect;
                    self.zoom_dirty = false;
                }
                return;
            }
            // Source not decoded yet: request it and keep showing the fit image
            // until it arrives (poll_source will trigger the rebuild).
            if self.source_target != Some(self.current) {
                self.start_source_decode(self.current);
            }
            self.zoom_dirty = false;
            if self.loaded.is_some() {
                return;
            }
        }

        if self.loaded.is_some() && self.loaded_for_rect == self.image_rect && !self.zoom_dirty {
            return;
        }
        self.zoom_dirty = false;

        if let Some(img) = self
            .prefetcher
            .take_resized(self.current, self.image_rect, self.cell_px)
        {
            self.loaded = Some(img);
            self.error = None;
            self.loaded_for_rect = self.image_rect;
            return;
        }

        if let Some((_, rect)) = self.fullscreen_target
            && rect == self.image_rect
        {
            return;
        }

        self.start_fullscreen_decode(self.current, self.image_rect);
    }
}

#[cfg(test)]
mod tests {

    use crate::app::ViewMode;
    use crate::test_support::{app, app_with, app_with_files, test_dir, wait_until};

    use ratatui::layout::Rect;
    use std::fs;

    use image::DynamicImage;

    #[test]
    fn fullscreen_decode_arrives_and_stale_results_are_dropped() {
        let dir = test_dir();
        let mut app = app_with_files(dir.path(), &["a.png", "b.png"]);
        app.image_rect = Rect::new(0, 0, 4, 4);
        app.enter_fullscreen_selected();
        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert!(!app.poll_fullscreen(), "nothing requested yet");

        app.load_if_needed();
        wait_until(|| app.poll_fullscreen());
        assert!(app.loaded.is_some());
        assert!(app.error.is_none());

        // Loaded for this rect: nothing to do.
        app.load_if_needed();
        assert!(app.fullscreen_target.is_none());

        // A result for an image that is no longer current is dropped.
        app.loaded = None;
        app.load_if_needed();
        // The same request is in flight, so a second call starts nothing new.
        app.load_if_needed();
        app.current = 1;
        wait_until(|| app.poll_fullscreen());
        assert!(app.loaded.is_none());
    }

    #[test]
    fn a_broken_file_reports_the_decode_error() {
        let dir = test_dir();
        let path = dir.path().join("broken.png");
        fs::write(&path, b"not a png").unwrap();
        let mut app = app_with(&[path.to_str().unwrap()]);
        app.image_rect = Rect::new(0, 0, 4, 4);
        app.enter_fullscreen_at(0);
        app.load_if_needed();
        wait_until(|| app.poll_fullscreen());
        assert!(app.loaded.is_none());
        assert!(app.error.is_some());

        app.enter_fullscreen_at(9);
        assert_eq!(app.current, 0, "an index past the end is ignored");
    }

    #[test]
    fn navigation_moves_inside_the_list() {
        let dir = test_dir();
        let mut app = app_with_files(dir.path(), &["a.png", "b.png", "c.png"]);
        app.enter_fullscreen_at(0);
        app.prev();
        assert_eq!(app.current, 0);
        app.first();
        assert_eq!(app.current, 0);
        app.next();
        assert_eq!(app.current, 1);
        assert!(app.fullscreen_target.is_some(), "the move starts a decode");
        app.last();
        assert_eq!(app.current, 2);
        app.next();
        assert_eq!(app.current, 2);
        app.last();
        app.prev();
        assert_eq!(app.current, 1);
        app.first();
        assert_eq!(app.current, 0);

        // A prefetched neighbor shows at once.
        app.prefetcher
            .set_target_hint(Rect::new(0, 0, 1, 1), (8, 16));
        app.prefetcher.kick(0, 0, &app.images);
        wait_until(|| {
            app.prefetcher.poll();
            app.prefetcher.pending_count() == 0
        });
        app.next();
        assert!(app.loaded.is_some());
        assert!(app.fullscreen_target.is_none());

        app.start_slideshow();
        app.enter_gallery();
        assert_eq!(app.mode, ViewMode::Gallery);
        assert!(!app.slideshow_running());
        assert!(app.loaded.is_none());
    }

    #[test]
    fn load_if_needed_takes_a_prefetched_image() {
        let dir = test_dir();
        let mut app = app_with_files(dir.path(), &["a.png", "b.png"]);
        app.image_rect = Rect::new(0, 0, 1, 1);
        app.enter_fullscreen_at(0);
        app.gallery.cursor = 1;
        app.pre_decode_hovered();
        wait_until(|| {
            app.prefetcher.poll();
            app.prefetcher.pending_count() == 0
        });
        app.current = 1;
        app.load_if_needed();
        assert!(app.loaded.is_some());
        assert!(app.fullscreen_target.is_none());
    }

    /// Regression: a gallery pre-decode before the first fullscreen view has no size hint.
    /// A large photo (an iPhone HEIC, and also a JPEG or PNG) then showed at its full size,
    /// larger than the screen.
    #[test]
    fn a_pre_decode_without_a_hint_fits_the_fullscreen_box() {
        let dir = test_dir();
        for name in ["large.png", "large.jpg"] {
            let path = dir.path().join(name);
            DynamicImage::new_rgb8(640, 480).save(&path).unwrap();
            let mut app = app();
            app.images.push(path);
            app.filenames.push(name.to_owned());
            app.gallery.filtered_indices = vec![0];
            app.pre_decode_hovered();
            wait_until(|| {
                app.prefetcher.poll();
                app.prefetcher.pending_count() == 0
            });

            app.enter_fullscreen_selected();
            // 10x6 cells of 8x16 px: a box of 80x96 px.
            app.image_rect = Rect::new(0, 0, 10, 6);
            app.load_if_needed();
            let loaded = app
                .loaded
                .as_ref()
                .expect("the prefetched image shows at once");
            assert_eq!(loaded.dimensions(), (80, 60), "{name}");
        }
    }
}
