//! Zoom and pan in the fullscreen view, with the full-resolution source behind them.

use super::App;
use crate::decode::resize::resize_to_exact;
use crate::decode::{EmbeddedThumbnail, decode_image_with_hint, foreground};
use image::RgbaImage;
use std::sync::mpsc;

/// Multiplier applied on each zoom step.
const ZOOM_STEP: f64 = 1.25;
/// Maximum zoom factor (relative to fit-to-window).
const MAX_ZOOM: f64 = 16.0;

impl App {
    /// Reset zoom/pan and drop the cached full-resolution source. Called when the
    /// current fullscreen image changes.
    pub(super) fn reset_zoom_state(&mut self) {
        self.zoom = 1.0;
        self.pan_x = 0.5;
        self.pan_y = 0.5;
        self.zoom_dirty = false;
        self.source = None;
        self.source_for = None;
        self.source_rx = None;
        self.source_target = None;
    }

    /// Return to fit-to-window, keeping the cached source for fast re-zoom.
    pub(crate) fn reset_zoom(&mut self) {
        if self.zoom <= 1.0 {
            return;
        }
        self.zoom = 1.0;
        self.pan_x = 0.5;
        self.pan_y = 0.5;
        self.zoom_dirty = true;
        self.needs_render = true;
    }

    pub(crate) fn zoom_in(&mut self) {
        if self.fullscreen_on_video() {
            return;
        }
        let new = (self.zoom * ZOOM_STEP).min(MAX_ZOOM);
        if new > self.zoom {
            self.zoom = new;
            self.clamp_pan();
            self.zoom_dirty = true;
            self.needs_render = true;
        }
    }

    pub(crate) fn zoom_out(&mut self) {
        let new = (self.zoom / ZOOM_STEP).max(1.0);
        if new < self.zoom {
            self.zoom = new;
            self.clamp_pan();
            self.zoom_dirty = true;
            self.needs_render = true;
        }
    }

    /// Pan the zoomed view. `dx`/`dy` are direction multipliers (usually -1, 0, or 1).
    pub(crate) fn pan(&mut self, dx: f64, dy: f64) {
        if self.zoom <= 1.0 {
            return;
        }
        let step = 0.15 / self.zoom;
        self.pan_x += dx * step;
        self.pan_y += dy * step;
        self.clamp_pan();
        self.zoom_dirty = true;
        self.needs_render = true;
    }

    fn clamp_pan(&mut self) {
        self.pan_x = self.pan_x.clamp(0.0, 1.0);
        self.pan_y = self.pan_y.clamp(0.0, 1.0);
    }

    pub(super) fn start_source_decode(&mut self, idx: usize) {
        let (tx, rx) = mpsc::channel();
        self.source_rx = Some(rx);
        self.source_target = Some(idx);
        let path = self.images[idx].clone();
        rayon::spawn(move || {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a closed receiver means that nobody waits for the result"
            )]
            let _ = tx.send(foreground(|| {
                decode_image_with_hint(&path, None, EmbeddedThumbnail::Deny)
            }));
        });
    }

    /// Receive an async full-resolution source decode for zooming.
    pub(crate) fn poll_source(&mut self) -> bool {
        let result = self.source_rx.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(result) = result {
            let idx = self.source_target.take();
            self.source_rx = None;
            if let (Some(idx), Ok(img)) = (idx, result)
                && idx == self.current
            {
                self.source = Some(img);
                self.source_for = Some(idx);
                self.zoom_dirty = true;
                self.needs_render = true;
            }
            return true;
        }
        false
    }

    /// Build the zoomed, panned crop of the current full-resolution source, sized
    /// to fill the viewport. Returns `None` when no source is available.
    pub(super) fn build_zoom_view(&self) -> Option<RgbaImage> {
        let src = self.source.as_ref()?;
        let (ow, oh) = (src.width(), src.height());
        if ow == 0 || oh == 0 {
            return None;
        }

        let vw = (u32::from(self.image_rect.width) * self.cell_px.0).max(1);
        let vh = (u32::from(self.image_rect.height) * self.cell_px.1).max(1);

        // Fit-to-window scale, never upscaling the base image (matches zoom == 1.0).
        let fit = f64::min(f64::from(vw) / f64::from(ow), f64::from(vh) / f64::from(oh));
        let base = fit.min(1.0);
        let eff = base * self.zoom;

        // Source-pixel crop window that maps onto the viewport at the effective scale.
        let crop_w = ((f64::from(vw) / eff).round() as u32).clamp(1, ow);
        let crop_h = ((f64::from(vh) / eff).round() as u32).clamp(1, oh);
        let max_x = ow.saturating_sub(crop_w);
        let max_y = oh.saturating_sub(crop_h);
        let x = (self.pan_x * f64::from(max_x)).round() as u32;
        let y = (self.pan_y * f64::from(max_y)).round() as u32;

        let cropped = src.crop_imm(x, y, crop_w, crop_h);
        let dst_w = ((f64::from(crop_w) * eff).round() as u32).max(1).min(vw);
        let dst_h = ((f64::from(crop_h) * eff).round() as u32).max(1).min(vh);
        Some(resize_to_exact(&cropped, dst_w, dst_h))
    }
}

#[cfg(test)]
mod tests {

    use crate::app::ViewMode;
    use crate::test_support::{app, app_with, app_with_files, test_dir, wait_until};
    use image::RgbaImage;
    use ratatui::layout::Rect;

    use image::DynamicImage;

    #[test]
    fn zoom_view_crops_and_fills_the_viewport() {
        use super::App;
        use crate::image_list::SharedImageList;
        use ratatui::layout::Rect;

        let mut app = App::new((1, 1), SharedImageList::new());
        app.image_rect = Rect::new(0, 0, 100, 100);
        // 400x400 source, viewport 100x100 -> fit scale 0.25 (base image is 100x100).
        app.source = Some(DynamicImage::new_rgba8(400, 400));
        app.source_for = Some(0);
        app.current = 0;

        // At 2x zoom the effective scale is 0.5, so the crop is 200x200 source px
        // upscaled to exactly the 100x100 viewport.
        app.zoom = 2.0;
        let view = app.build_zoom_view().expect("zoom view");
        assert_eq!(view.dimensions(), (100, 100));
    }

    #[test]
    fn zoom_and_pan_stay_inside_the_limits() {
        let mut app = app_with(&["a.png"]);
        app.mode = ViewMode::Fullscreen;
        app.reset_zoom();
        app.pan(1.0, 1.0);
        assert_eq!((app.pan_x, app.pan_y), (0.0, 0.0), "no pan at fit size");

        app.zoom_out();
        assert_eq!(app.zoom, 1.0);
        app.zoom_in();
        assert_eq!(app.zoom, 1.25);
        assert!(app.zoom_dirty);
        for _ in 0..30 {
            app.zoom_in();
        }
        assert_eq!(app.zoom, 16.0);
        app.needs_render = false;
        app.zoom_in();
        assert!(!app.needs_render, "the maximum stops the zoom");

        app.pan_x = 0.5;
        app.pan_y = 0.5;
        app.pan(-1.0, 1.0);
        assert!(app.pan_x < 0.5 && app.pan_y > 0.5);
        for _ in 0..200 {
            app.pan(-1.0, 1.0);
        }
        assert_eq!((app.pan_x, app.pan_y), (0.0, 1.0));

        app.zoom_out();
        assert_eq!(app.zoom, 12.8);
        app.reset_zoom();
        assert_eq!(app.zoom, 1.0);
        assert_eq!((app.pan_x, app.pan_y), (0.5, 0.5));
    }

    #[test]
    fn zoom_view_follows_the_pan_point() {
        let mut app = app();
        app.cell_px = (1, 1);
        app.image_rect = Rect::new(0, 0, 10, 10);
        // The left half is black, the right half is white.
        let source = RgbaImage::from_fn(40, 40, |x, _| {
            image::Rgba(if x < 20 { [0, 0, 0, 255] } else { [255; 4] })
        });
        app.source = Some(DynamicImage::ImageRgba8(source));
        app.zoom = 4.0;
        app.pan_x = 0.0;
        let left = app.build_zoom_view().unwrap();
        app.pan_x = 1.0;
        let right = app.build_zoom_view().unwrap();
        assert_eq!(left.dimensions(), (10, 10));
        assert_eq!(left.get_pixel(5, 5).0, [0, 0, 0, 255]);
        assert_eq!(right.get_pixel(5, 5).0, [255; 4]);

        app.source = Some(DynamicImage::new_rgba8(0, 0));
        assert!(app.build_zoom_view().is_none());
        app.source = None;
        assert!(app.build_zoom_view().is_none());
    }

    #[test]
    fn zoom_decodes_the_source_and_then_crops_it() {
        let dir = test_dir();
        let mut app = app_with_files(dir.path(), &["a.png", "b.png"]);
        app.image_rect = Rect::new(0, 0, 2, 2);
        app.enter_fullscreen_at(0);
        app.loaded = Some(RgbaImage::new(1, 1));
        app.zoom_in();
        assert!(!app.poll_source(), "nothing requested yet");

        app.load_if_needed();
        assert_eq!(app.source_target, Some(0));
        assert_eq!(
            app.loaded.as_ref().unwrap().dimensions(),
            (1, 1),
            "keeps the fit image"
        );
        wait_until(|| app.poll_source());
        assert_eq!(app.source_for, Some(0));
        assert!(app.zoom_dirty);

        app.load_if_needed();
        assert!(!app.zoom_dirty);
        // The 8x8 source at zoom 1.25 fills 10x10 of the 16x32 viewport.
        assert_eq!(app.loaded.as_ref().unwrap().dimensions(), (10, 10));

        // A source that arrives for an old image is dropped.
        app.source_for = None;
        app.source = None;
        app.load_if_needed();
        app.current = 1;
        wait_until(|| app.poll_source());
        assert!(app.source.is_none());

        // Zoom without a fit image: request the source and decode the fit image too.
        app.loaded = None;
        app.load_if_needed();
        assert_eq!(app.source_target, Some(1));
        assert!(app.fullscreen_target.is_some());
    }
}
