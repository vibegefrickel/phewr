//! The fullscreen view: enter and leave, the steps through the list and the decode.

#[cfg(feature = "video")]
use super::VideoReturn;
use super::{App, ViewMode};
use crate::decode::screen::load_for_screen;
use crate::decode::{ScreenImage, spawn_foreground};
use crate::encoder::capped_box;
use crate::gallery::box_px;
use ratatui::layout::Rect;
use std::io;
use std::sync::mpsc;

/// The running fullscreen decode: the image, its decode box and the result receiver.
pub(super) struct FullscreenDecode {
    pub(super) index: usize,
    pub(super) rect: Rect,
    rx: mpsc::Receiver<io::Result<ScreenImage>>,
}

/// The pixel box of `rect`, below the payload cap.
fn decode_box(rect: Rect, cell_px: (u32, u32)) -> (u32, u32) {
    capped_box(box_px(rect, cell_px))
}

impl App {
    pub(crate) fn enter_fullscreen_selected(&mut self) {
        if let Some(index) = self.gallery.selected_index() {
            self.enter_fullscreen_at(index);
        }
    }

    /// Open the file `index`. A folder entry opens nothing.
    pub(crate) fn enter_fullscreen_at(&mut self, index: usize) {
        if index >= self.images.len() || self.is_folder(index) {
            return;
        }
        #[cfg(feature = "video")]
        if self.opens_in_player(index) {
            self.enter_video(index);
            return;
        }
        self.current = index;
        self.mode = ViewMode::Fullscreen;
        self.error = None;
        self.needs_render = true;
        self.fullscreen_decode = None;
        self.loaded = None;
        self.reset_zoom_and_source();
    }

    fn start_fullscreen_decode(&mut self, index: usize, rect: Rect) {
        let path = self.images[index].clone();
        let target = decode_box(rect, self.cell_px);
        let rx = spawn_foreground(move || load_for_screen(&path, target));
        self.fullscreen_decode = Some(FullscreenDecode { index, rect, rx });
    }

    pub(crate) fn poll_fullscreen(&mut self) -> bool {
        let Some(decode) = &self.fullscreen_decode else {
            return false;
        };
        let Ok(result) = decode.rx.try_recv() else {
            return false;
        };
        let (index, rect) = (decode.index, decode.rect);
        self.fullscreen_decode = None;
        if index == self.current && rect == self.image_rect && self.mode == ViewMode::Fullscreen {
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
        true
    }

    pub(crate) fn enter_gallery(&mut self) {
        self.slideshow = None;
        self.mode = ViewMode::Gallery;
        self.loaded = None;
        self.reset_zoom_and_source();
        self.needs_render = true;
    }

    pub(crate) fn next(&mut self) {
        if self.current + 1 < self.images.len() {
            self.jump_to(self.current + 1);
        }
    }

    /// The step back stops at the first file. The folders come before it.
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

        // The image on the screen goes back to the prefetch cache: a step back is free.
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
        self.fullscreen_decode = None;
        self.reset_zoom_and_source();

        #[cfg(feature = "video")]
        if self.opens_in_player(self.current) {
            self.mode = ViewMode::Video;
            self.video_return = VideoReturn::Fullscreen;
            self.loaded = None;
            self.clear_before_video = true;
            return;
        }

        self.mode = ViewMode::Fullscreen;
        // A prefetch result that arrived since the last pass counts as a hit.
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

    /// Decode the hovered cell at the size of the fullscreen box. A decode at full size makes
    /// `take_resized` scale the frame on the UI thread.
    pub(crate) fn pre_decode_hovered(&mut self) {
        if let Some(index) = self.gallery.selected_index()
            && !self.is_folder(index)
        {
            self.prefetcher
                .set_target_hint(self.fullscreen_box, self.cell_px);
            self.prefetcher.kick_gallery(index, &self.images);
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
        if self.zoom > 1.0 {
            if self.zoom_state.source_for == Some(self.current) {
                if (self.zoom_state.dirty
                    || self.loaded.is_none()
                    || self.loaded_for_rect != self.image_rect)
                    && let Some(view) = self.build_zoom_view()
                {
                    self.loaded = Some(view);
                    self.error = None;
                    self.loaded_for_rect = self.image_rect;
                    self.zoom_state.dirty = false;
                }
                return;
            }
            if self.zoom_state.source_target != Some(self.current)
                && self.zoom_state.source_failed != Some(self.current)
            {
                self.start_source_decode(self.current);
            }
            self.zoom_state.dirty = false;
            if self.loaded.is_some() {
                return;
            }
        }

        if self.loaded.is_some()
            && self.loaded_for_rect == self.image_rect
            && !self.zoom_state.dirty
        {
            return;
        }
        self.zoom_state.dirty = false;

        if let Some(img) = self
            .prefetcher
            .take_resized(self.current, self.image_rect, self.cell_px)
        {
            self.loaded = Some(img);
            self.error = None;
            self.loaded_for_rect = self.image_rect;
            return;
        }

        if self
            .fullscreen_decode
            .as_ref()
            .is_some_and(|decode| decode.rect == self.image_rect)
        {
            return;
        }

        self.start_fullscreen_decode(self.current, self.image_rect);
    }
}

#[cfg(test)]
mod tests {
    use super::decode_box;
    use crate::app::ViewMode;
    use crate::test_support::{app, app_with, app_with_files, test_dir, wait_until};

    use ratatui::layout::Rect;
    use std::fs;

    use image::DynamicImage;

    #[test]
    fn drops_stale_decode_results() {
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

        app.load_if_needed();
        assert!(app.fullscreen_decode.is_none());

        // A result for an image that is no longer current goes away.
        app.loaded = None;
        app.load_if_needed();
        // The same request is in flight. A second call starts nothing.
        app.load_if_needed();
        app.current = 1;
        wait_until(|| app.poll_fullscreen());
        assert!(app.loaded.is_none());
    }

    #[test]
    fn broken_file_reports_decode_error() {
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
    fn navigation_stays_inside_list() {
        let dir = test_dir();
        let mut app = app_with_files(dir.path(), &["a.png", "b.png", "c.png"]);
        app.enter_fullscreen_at(0);
        app.prev();
        assert_eq!(app.current, 0);
        app.first();
        assert_eq!(app.current, 0);
        app.next();
        assert_eq!(app.current, 1);
        assert!(app.fullscreen_decode.is_some(), "the move starts a decode");
        app.last();
        assert_eq!(app.current, 2);
        app.next();
        assert_eq!(app.current, 2);
        app.last();
        app.prev();
        assert_eq!(app.current, 1);
        app.first();
        assert_eq!(app.current, 0);

        // A prefetched neighbor shows now.
        app.prefetcher
            .set_target_hint(Rect::new(0, 0, 1, 1), (8, 16));
        app.prefetcher.kick(0, 0, &app.images);
        wait_until(|| {
            app.prefetcher.poll();
            app.prefetcher.pending_count() == 0
        });
        app.next();
        assert!(app.loaded.is_some());
        assert!(app.fullscreen_decode.is_none());

        app.start_slideshow();
        app.enter_gallery();
        assert_eq!(app.mode, ViewMode::Gallery);
        assert!(!app.slideshow_running());
        assert!(app.loaded.is_none());
    }

    #[test]
    fn enter_gallery_drops_zoom_source() {
        let dir = test_dir();
        let mut app = app_with_files(dir.path(), &["a.png"]);
        app.image_rect = Rect::new(0, 0, 2, 2);
        app.enter_fullscreen_at(0);
        app.loaded = Some(image::RgbaImage::new(1, 1).into());
        app.zoom_in();
        app.load_if_needed();
        wait_until(|| app.poll_source());
        assert!(app.zoom_state.source.is_some());

        app.enter_gallery();
        assert!(app.zoom_state.source.is_none());
        assert!(app.zoom_state.source_for.is_none());
    }

    #[test]
    fn load_takes_prefetched_image() {
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
        assert!(app.fullscreen_decode.is_none());
    }

    #[test]
    fn decode_box_stays_below_payload_cap() {
        assert_eq!(
            decode_box(Rect::new(0, 0, 240, 135), (16, 16)),
            (3840, 2160)
        );
        let (width, _) = decode_box(Rect::new(0, 0, 480, 270), (16, 16));
        assert!(width < 7680, "{width}");
    }

    /// A gallery pre-decode before the first fullscreen view has no size hint. A large photo once
    /// showed at its full size, larger than the screen.
    #[test]
    fn pre_decode_fits_fullscreen_box() {
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
