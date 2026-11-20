//! Zoom and pan in the fullscreen view, with the full-resolution source behind them.

use super::App;
use std::io;
use std::sync::mpsc;
use tuilibr::cells::round_to_u32;
use tuilibr::pictures::decode::{
    Decoded, EmbeddedThumbnail, ScreenImage, decode_image, spawn_foreground,
};
use tuilibr::pictures::kitty::placement::box_px;

const ZOOM_STEP: f64 = 1.25;
/// Relative to the fitted image.
const MAX_ZOOM: f64 = 16.0;

/// The pan position of the zoomed view and the full-resolution source behind it.
#[derive(Default)]
pub(super) struct ZoomState {
    pan_x: f64,
    pan_y: f64,
    /// True when the zoomed view needs a new crop.
    pub(super) dirty: bool,
    /// The full-resolution source of the fullscreen image. It is RGB when every pixel is opaque,
    /// and each crop is then RGB too.
    pub(super) source: Option<ScreenImage>,
    pub(super) source_for: Option<usize>,
    pub(super) source_rx: Option<mpsc::Receiver<io::Result<ScreenImage>>>,
    pub(super) source_target: Option<usize>,
    /// The image whose full-resolution decode failed. No later request tries again.
    pub(super) source_failed: Option<usize>,
}

impl App {
    /// Reset zoom and pan and drop the source after a file change. `reset_zoom` keeps the source.
    pub(super) fn reset_zoom_and_source(&mut self) {
        self.zoom = 1.0;
        self.zoom_state = ZoomState {
            pan_x: 0.5,
            pan_y: 0.5,
            ..ZoomState::default()
        };
    }

    /// Go back to the fitted image. The source stays for the next zoom.
    pub(crate) fn reset_zoom(&mut self) {
        if self.zoom <= 1.0 {
            return;
        }
        self.zoom = 1.0;
        self.zoom_state.pan_x = 0.5;
        self.zoom_state.pan_y = 0.5;
        self.zoom_state.dirty = true;
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
            self.zoom_state.dirty = true;
            self.needs_render = true;
        }
    }

    pub(crate) fn zoom_out(&mut self) {
        let new = (self.zoom / ZOOM_STEP).max(1.0);
        if new < self.zoom {
            self.zoom = new;
            self.clamp_pan();
            self.zoom_state.dirty = true;
            self.needs_render = true;
        }
    }

    /// Pan the zoomed view by one step. The keys pass -1, 0 or 1 for `dx` and `dy`.
    pub(crate) fn pan(&mut self, dx: f64, dy: f64) {
        if self.zoom <= 1.0 {
            return;
        }
        let step = 0.15 / self.zoom;
        self.zoom_state.pan_x += dx * step;
        self.zoom_state.pan_y += dy * step;
        self.clamp_pan();
        self.zoom_state.dirty = true;
        self.needs_render = true;
    }

    /// Move the zoomed view by a fraction of the pan range, as a mouse drag does.
    pub(crate) fn pan_fraction(&mut self, dx: f64, dy: f64) {
        if self.zoom <= 1.0 || (dx == 0.0 && dy == 0.0) {
            return;
        }
        self.zoom_state.pan_x += dx;
        self.zoom_state.pan_y += dy;
        self.clamp_pan();
        self.zoom_state.dirty = true;
        self.needs_render = true;
    }

    /// The drawn size at this zoom minus the box, per axis, as `build_zoom_view` crops. A drag
    /// divides by it: the picture follows the pointer even for a letterboxed image.
    pub(crate) fn pan_range_px(&self) -> (f64, f64) {
        let (ow, oh) = match (&self.zoom_state.source, &self.loaded) {
            (Some(source), _) => source.dimensions(),
            (None, Some(loaded)) => loaded.dimensions(),
            (None, None) => return (0.0, 0.0),
        };
        if ow == 0 || oh == 0 {
            return (0.0, 0.0);
        }
        let (vw, vh) = box_px(self.image_rect, self.cell_px);
        let (vw, vh) = (f64::from(vw), f64::from(vh));
        let (ow, oh) = (f64::from(ow), f64::from(oh));
        let eff = f64::min(vw / ow, vh / oh).min(1.0) * self.zoom;
        ((eff * ow - vw).max(0.0), (eff * oh - vh).max(0.0))
    }

    /// The pan position, for the mouse drag tests outside this module.
    #[cfg(test)]
    pub(crate) const fn pan_position(&self) -> (f64, f64) {
        (self.zoom_state.pan_x, self.zoom_state.pan_y)
    }

    const fn clamp_pan(&mut self) {
        self.zoom_state.pan_x = self.zoom_state.pan_x.clamp(0.0, 1.0);
        self.zoom_state.pan_y = self.zoom_state.pan_y.clamp(0.0, 1.0);
    }

    pub(super) fn start_source_decode(&mut self, index: usize) {
        self.zoom_state.source_target = Some(index);
        let path = self.images[index].clone();
        self.zoom_state.source_rx = Some(spawn_foreground(move || {
            decode_image(&path, None, EmbeddedThumbnail::Deny).map(Decoded::into_screen)
        }));
    }

    /// Take the finished decode of the full-resolution source.
    pub(crate) fn poll_source(&mut self) -> bool {
        let result = self
            .zoom_state
            .source_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok());
        if let Some(result) = result {
            let index = self.zoom_state.source_target.take();
            self.zoom_state.source_rx = None;
            match (index, result) {
                (Some(index), Ok(img)) if index == self.current => {
                    self.zoom_state.source = Some(img);
                    self.zoom_state.source_for = Some(index);
                    self.zoom_state.dirty = true;
                    self.needs_render = true;
                }
                (Some(index), Err(e)) if index == self.current => {
                    self.zoom_state.source_failed = Some(index);
                    self.error = Some(e.to_string());
                    self.needs_render = true;
                }
                _ => {}
            }
            return true;
        }
        false
    }

    /// The zoomed and panned crop of the source, sized to fill the viewport. `None` without a
    /// source.
    pub(super) fn build_zoom_view(&self) -> Option<ScreenImage> {
        let src = self.zoom_state.source.as_ref()?;
        let (ow, oh) = (src.width(), src.height());
        if ow == 0 || oh == 0 {
            return None;
        }

        let (vw, vh) = box_px(self.image_rect, self.cell_px);
        let (vw, vh) = (vw.max(1), vh.max(1));

        let fit = f64::min(f64::from(vw) / f64::from(ow), f64::from(vh) / f64::from(oh));
        let base = fit.min(1.0);
        let eff = base * self.zoom;

        let crop_w = round_to_u32(f64::from(vw) / eff).clamp(1, ow);
        let crop_h = round_to_u32(f64::from(vh) / eff).clamp(1, oh);
        let max_x = ow.saturating_sub(crop_w);
        let max_y = oh.saturating_sub(crop_h);
        let x = round_to_u32(self.zoom_state.pan_x * f64::from(max_x));
        let y = round_to_u32(self.zoom_state.pan_y * f64::from(max_y));

        let dst_w = round_to_u32(f64::from(crop_w) * eff).max(1).min(vw);
        let dst_h = round_to_u32(f64::from(crop_h) * eff).max(1).min(vh);
        Some(src.crop_resized((x, y, crop_w, crop_h), (dst_w, dst_h)))
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn pan_fraction_clamps_at_edges() {
        let mut app = app_with(&["a.jpg"]);
        app.zoom_state.pan_x = 0.5;
        app.zoom_state.pan_y = 0.5;
        app.pan_fraction(0.3, 0.0);
        assert!(
            (app.zoom_state.pan_x - 0.5).abs() < f64::EPSILON,
            "no pan without zoom"
        );

        app.zoom = 2.0;
        app.needs_render = false;
        app.pan_fraction(0.3, -0.2);
        assert!((app.zoom_state.pan_x - 0.8).abs() < 1e-9);
        assert!((app.zoom_state.pan_y - 0.3).abs() < 1e-9);
        assert!(app.needs_render);
        app.pan_fraction(0.5, -0.5);
        assert!((app.zoom_state.pan_x - 1.0).abs() < f64::EPSILON);
        assert!(app.zoom_state.pan_y.abs() < f64::EPSILON);
    }

    #[test]
    fn pan_range_is_size_minus_box() {
        let mut app = app_with(&["a.jpg"]);
        app.cell_px = (10, 20);
        app.image_rect = Rect::new(0, 0, 40, 10);
        assert_eq!(app.pan_range_px(), (0.0, 0.0), "no image, no range");

        // A 200 x 200 picture in a 400 x 200 box: letterboxed, 200 px wide on the screen.
        app.loaded = Some(RgbaImage::new(200, 200).into());
        assert_eq!(app.pan_range_px(), (0.0, 0.0), "fit view");
        app.zoom = 2.0;
        assert_eq!(
            app.pan_range_px(),
            (0.0, 200.0),
            "400 px wide fits the 400 px box"
        );
        app.zoom = 4.0;
        assert_eq!(app.pan_range_px(), (400.0, 600.0));
    }

    use crate::app::ViewMode;
    use crate::test_support::{app, app_with, app_with_files, test_dir, wait_until};
    use image::RgbaImage;
    use ratatui::layout::Rect;

    #[test]
    fn rgb_source_gives_rgb_crop() {
        use tuilibr::pictures::decode::ScreenImage;
        let mut app = app();
        app.cell_px = (1, 1);
        app.image_rect = Rect::new(0, 0, 10, 10);
        let source = image::RgbImage::from_pixel(40, 40, image::Rgb([7, 8, 9]));
        app.zoom_state.source = Some(source.into());
        app.zoom = 4.0;
        let crop = app.build_zoom_view().unwrap();
        assert_eq!(
            crop,
            ScreenImage::from(image::RgbImage::from_pixel(10, 10, image::Rgb([7, 8, 9])))
        );
    }

    #[test]
    fn view_crops_and_fills_viewport() {
        use super::App;
        use crate::image_list::SharedImageList;
        use ratatui::layout::Rect;

        let mut app = App::new((1, 1), SharedImageList::new());
        app.image_rect = Rect::new(0, 0, 100, 100);
        // 400x400 source, viewport 100x100: fit scale 0.25 (base image is 100x100).
        app.zoom_state.source = Some(RgbaImage::new(400, 400).into());
        app.zoom_state.source_for = Some(0);
        app.current = 0;

        // At 2x zoom the effective scale is 0.5: the crop is 200x200 source px, upscaled to the
        // 100x100 viewport.
        app.zoom = 2.0;
        let view = app.build_zoom_view().expect("zoom view");
        assert_eq!(view.dimensions(), (100, 100));
    }

    #[test]
    fn zoom_and_pan_stay_in_limits() {
        let mut app = app_with(&["a.png"]);
        app.mode = ViewMode::Fullscreen;
        app.reset_zoom();
        app.pan(1.0, 1.0);
        assert_eq!(
            (app.zoom_state.pan_x, app.zoom_state.pan_y),
            (0.0, 0.0),
            "no pan at fit size"
        );

        app.zoom_out();
        assert_eq!(app.zoom, 1.0);
        app.zoom_in();
        assert_eq!(app.zoom, 1.25);
        assert!(app.zoom_state.dirty);
        for _ in 0..30 {
            app.zoom_in();
        }
        assert_eq!(app.zoom, 16.0);
        app.needs_render = false;
        app.zoom_in();
        assert!(!app.needs_render, "the maximum stops the zoom");

        app.zoom_state.pan_x = 0.5;
        app.zoom_state.pan_y = 0.5;
        app.pan(-1.0, 1.0);
        assert!(app.zoom_state.pan_x < 0.5 && app.zoom_state.pan_y > 0.5);
        for _ in 0..200 {
            app.pan(-1.0, 1.0);
        }
        assert_eq!((app.zoom_state.pan_x, app.zoom_state.pan_y), (0.0, 1.0));

        app.zoom_out();
        assert_eq!(app.zoom, 12.8);
        app.reset_zoom();
        assert_eq!(app.zoom, 1.0);
        assert_eq!((app.zoom_state.pan_x, app.zoom_state.pan_y), (0.5, 0.5));
    }

    #[test]
    fn view_follows_pan_point() {
        let mut app = app();
        app.cell_px = (1, 1);
        app.image_rect = Rect::new(0, 0, 10, 10);
        let source = RgbaImage::from_fn(40, 40, |x, _| {
            image::Rgba(if x < 20 { [0, 0, 0, 255] } else { [255; 4] })
        });
        app.zoom_state.source = Some(source.into());
        app.zoom = 4.0;
        app.zoom_state.pan_x = 0.0;
        let left = app.build_zoom_view().unwrap().into_rgba();
        app.zoom_state.pan_x = 1.0;
        let right = app.build_zoom_view().unwrap().into_rgba();
        assert_eq!(left.dimensions(), (10, 10));
        assert_eq!(left.get_pixel(5, 5).0, [0, 0, 0, 255]);
        assert_eq!(right.get_pixel(5, 5).0, [255; 4]);

        app.zoom_state.source = Some(RgbaImage::new(0, 0).into());
        assert!(app.build_zoom_view().is_none());
        app.zoom_state.source = None;
        assert!(app.build_zoom_view().is_none());
    }

    #[test]
    fn decodes_source_then_crops() {
        let dir = test_dir();
        let mut app = app_with_files(dir.path(), &["a.png", "b.png"]);
        app.image_rect = Rect::new(0, 0, 2, 2);
        app.enter_fullscreen_at(0);
        app.loaded = Some(RgbaImage::new(1, 1).into());
        app.zoom_in();
        assert!(!app.poll_source(), "nothing requested yet");

        app.load_if_needed();
        assert_eq!(app.zoom_state.source_target, Some(0));
        assert_eq!(
            app.loaded.as_ref().unwrap().dimensions(),
            (1, 1),
            "keeps the fit image"
        );
        wait_until(|| app.poll_source());
        assert_eq!(app.zoom_state.source_for, Some(0));
        assert!(app.zoom_state.dirty);

        app.load_if_needed();
        assert!(!app.zoom_state.dirty);
        // The 8x8 source at zoom 1.25 fills 10x10 of the 16x32 viewport.
        assert_eq!(app.loaded.as_ref().unwrap().dimensions(), (10, 10));

        // A source that arrives for an old image goes away.
        app.zoom_state.source_for = None;
        app.zoom_state.source = None;
        app.load_if_needed();
        app.current = 1;
        wait_until(|| app.poll_source());
        assert!(app.zoom_state.source.is_none());

        // Zoom without a fit image: request the source and decode the fit image too.
        app.loaded = None;
        app.load_if_needed();
        assert_eq!(app.zoom_state.source_target, Some(1));
        assert!(app.fullscreen_decode.is_some());
    }

    #[test]
    fn failed_source_decode_is_not_repeated() {
        let dir = test_dir();
        let mut app = app_with_files(dir.path(), &["a.png"]);
        app.image_rect = Rect::new(0, 0, 2, 2);
        app.enter_fullscreen_at(0);
        app.loaded = Some(RgbaImage::new(1, 1).into());
        app.zoom_in();
        std::fs::remove_file(&app.images[0]).unwrap();

        app.load_if_needed();
        assert_eq!(app.zoom_state.source_target, Some(0));
        wait_until(|| app.poll_source());
        assert!(app.error.is_some());
        assert_eq!(app.zoom_state.source_failed, Some(0));

        app.load_if_needed();
        assert!(app.zoom_state.source_target.is_none(), "no second decode");
        assert!(app.zoom_state.source_rx.is_none());
    }
}
