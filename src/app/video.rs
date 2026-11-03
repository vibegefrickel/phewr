//! Videos in the view: the terminal player, the way back after a video and the mpv window.

use super::{App, ViewMode};
#[cfg(feature = "video")]
use std::path::PathBuf;
#[cfg(feature = "video")]
use std::sync::mpsc;

/// The view that opened a video: the view that comes back after mpv.
#[cfg(feature = "video")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VideoReturn {
    Gallery,
    Fullscreen,
}

/// An mpv window that viewr follows: its file and the receiver of its exit code.
#[cfg(feature = "video")]
pub(super) struct VideoWindow {
    index: usize,
    exit: mpsc::Receiver<Option<i32>>,
    /// True when a slideshow plays the video once and waits for the window.
    holds_slideshow: bool,
}

/// The exit code and the file of a followed mpv window, for the status note.
#[cfg(feature = "video")]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct WindowExit {
    pub(crate) code: Option<i32>,
    pub(crate) path: PathBuf,
}

impl App {
    #[cfg(feature = "video")]
    pub(crate) fn enter_video(&mut self, index: usize) {
        self.video_return = if self.mode == ViewMode::Fullscreen {
            VideoReturn::Fullscreen
        } else {
            VideoReturn::Gallery
        };
        self.current = index;
        self.mode = ViewMode::Video;
        self.error = None;
        self.needs_render = true;
        self.loaded = None;
        self.video = None;
        self.clear_before_video = true;
    }

    /// Ask the run loop to open the current video in the mpv window.
    #[cfg(feature = "video")]
    pub(crate) fn request_video_window(&mut self) {
        self.video = None;
        self.video_window_request = true;
    }

    #[cfg(feature = "video")]
    pub(crate) fn take_video_window_request(&mut self) -> bool {
        std::mem::take(&mut self.video_window_request)
    }

    #[cfg(feature = "video")]
    pub(crate) fn exit_video(&mut self) {
        self.video = None;
        self.mode = ViewMode::Gallery;
        self.needs_render = true;
    }

    /// Leave the video for the view that opened it. The fullscreen view stays on the video with a
    /// play hint: the image decode cannot read a video.
    #[cfg(feature = "video")]
    pub(crate) fn return_from_video(&mut self) {
        if self.video_return == VideoReturn::Gallery {
            self.exit_video();
            return;
        }
        self.video = None;
        self.mode = ViewMode::Fullscreen;
        self.loaded = None;
        self.fullscreen_decode = None;
        self.reset_zoom_and_source();
        self.needs_render = true;
    }

    /// True while a slideshow video holds the slide. An mpv window holds the placeholder until it
    /// closes.
    #[cfg(feature = "video")]
    pub(super) fn slideshow_video_holds(&mut self) -> bool {
        if self.mode == ViewMode::Video {
            if self
                .video
                .as_ref()
                .is_some_and(crate::player::Player::finished)
            {
                self.return_from_video();
                self.slideshow_step();
            }
            return true;
        }
        let Some(window) = self.video_window.as_ref() else {
            return false;
        };
        if !window.holds_slideshow {
            return false;
        }
        if window.index != self.current || self.mode != ViewMode::Fullscreen {
            self.video_window = None;
            return false;
        }
        self.slideshow_check_again_soon();
        true
    }

    /// Follow the mpv window of the current video until it exits. `holds_slideshow` holds a
    /// running slideshow on the placeholder until then.
    #[cfg(feature = "video")]
    pub(crate) fn watch_video_window(
        &mut self,
        exit: mpsc::Receiver<Option<i32>>,
        holds_slideshow: bool,
    ) {
        self.video_window = Some(VideoWindow {
            index: self.current,
            exit,
            holds_slideshow,
        });
    }

    #[cfg(feature = "video")]
    pub(crate) fn watches_video_window(&self) -> bool {
        self.video_window.is_some()
    }

    /// Take the exit code of the followed mpv window, and act on it.
    ///
    /// In a slideshow, the end of the clip moves on and each other exit stops the slideshow. The
    /// exit counts only while viewr shows the file of the window without an overlay.
    #[cfg(feature = "video")]
    pub(crate) fn poll_video_window(&mut self) -> Option<WindowExit> {
        let window = self.video_window.as_ref()?;
        let code = match window.exit.try_recv() {
            Ok(code) => code,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => None,
        };
        let window = self.video_window.take()?;
        if !self.follows_window(window.index) {
            return None;
        }
        let path = self.images[window.index].clone();
        if let Some(step) = crate::mpv::step(code) {
            self.step_from_video(step);
        } else if window.holds_slideshow && self.slideshow.is_some() {
            let reached_end = crate::mpv::reached_the_end(code);
            // A paused slideshow stays on the placeholder after the end of the clip.
            if !reached_end || self.slideshow_advancing() {
                self.after_slideshow_video(reached_end);
            }
        }
        Some(WindowExit { code, path })
    }

    /// True while viewr shows the file `index` of an mpv window and no overlay takes the keys.
    #[cfg(feature = "video")]
    fn follows_window(&self, index: usize) -> bool {
        index == self.current
            && index < self.images.len()
            && matches!(self.mode, ViewMode::Fullscreen | ViewMode::Gallery)
            && !self.help_visible
            && !self.gallery.search_active
    }

    /// Step to the neighbor after `n` or `N` in mpv. `false` at the end of the list.
    ///
    /// A video keeps the view that the first video returns to: a run of videos from the gallery
    /// ends in the gallery.
    #[cfg(feature = "video")]
    pub(crate) fn step_from_video(&mut self, step: crate::mpv::Step) -> bool {
        let target = match step {
            crate::mpv::Step::Next => {
                Some(self.current + 1).filter(|&next| next < self.images.len())
            }
            crate::mpv::Step::Previous => self
                .current
                .checked_sub(1)
                .filter(|&previous| previous >= self.first_file()),
        };
        let Some(target) = target else {
            return false;
        };
        let return_to = self.video_return;
        self.jump_to(target);
        if self.mode == ViewMode::Video {
            self.video_return = return_to;
        }
        self.slideshow_touch();
        true
    }

    /// A quit by the user stops the slideshow and keeps the placeholder.
    #[cfg(feature = "video")]
    fn after_slideshow_video(&mut self, reached_end: bool) {
        if reached_end {
            self.slideshow_step();
        } else {
            self.stop_slideshow();
        }
    }

    pub(crate) fn in_slideshow_view(&self) -> bool {
        self.mode == ViewMode::Fullscreen || self.in_video_view()
    }

    /// True in the view of a video player. The clip and not the clock drives the slideshow there.
    #[cfg_attr(
        not(feature = "video"),
        expect(
            clippy::unused_self,
            reason = "without the video feature, no view plays a video"
        )
    )]
    pub(super) fn in_video_view(&self) -> bool {
        #[cfg(feature = "video")]
        let in_video = self.mode == ViewMode::Video;
        #[cfg(not(feature = "video"))]
        let in_video = false;
        in_video
    }

    /// The image decode cannot read a video: the fullscreen view must not try it.
    pub(super) fn fullscreen_on_video(&self) -> bool {
        self.is_video(self.current) && self.mode == ViewMode::Fullscreen
    }

    #[cfg(feature = "video")]
    pub(crate) fn is_video(&self, index: usize) -> bool {
        self.images
            .get(index)
            .is_some_and(|path| crate::video::is_video(path))
    }

    /// True when a thumbnail job found an animated GIF, WebP, APNG or AVIS. The draw reads no file.
    pub(crate) fn is_animated(&self, index: usize) -> bool {
        self.stored_animation_flag(index).unwrap_or(false)
    }

    fn stored_animation_flag(&self, index: usize) -> Option<bool> {
        let path = self.images.get(index)?;
        self.animated.get(path).copied()
    }

    /// The header check reads the file after a key press: a file that an export tool rewrote in
    /// place takes the right route. The flag of the grid marker can be older.
    #[cfg(feature = "video")]
    fn reads_as_animated(&self, index: usize) -> bool {
        self.images
            .get(index)
            .is_some_and(|path| crate::player::is_animated(path))
    }

    #[cfg(feature = "video")]
    pub(crate) fn opens_in_player(&self, index: usize) -> bool {
        self.is_video(index) || self.reads_as_animated(index)
    }

    /// True on the still frame of an animated image: Enter plays it, `p` opens the mpv window.
    #[cfg(feature = "video")]
    pub(crate) fn fullscreen_on_animation(&self) -> bool {
        self.mode == ViewMode::Fullscreen && self.reads_as_animated(self.current)
    }

    /// True when the gallery cursor is on a video or an animated image, for `p` in the gallery.
    #[cfg(feature = "video")]
    pub(crate) fn selected_is_video(&self) -> bool {
        self.gallery
            .filtered_indices
            .get(self.gallery.cursor)
            .is_some_and(|&index| self.opens_in_player(index))
    }

    /// Without the `video` feature, the scan lists no videos.
    #[cfg(not(feature = "video"))]
    #[expect(clippy::unused_self, reason = "the same signature as the video build")]
    pub(crate) fn is_video(&self, _index: usize) -> bool {
        false
    }

    /// The play hint for a video in fullscreen. A failed open shows its error instead.
    pub(crate) fn shows_video_placeholder(&self) -> bool {
        self.fullscreen_on_video() && self.error.is_none()
    }
}

#[cfg(test)]
#[cfg(feature = "video")]
mod tests {
    use crate::app::{VideoReturn, ViewMode};
    use crate::test_support::{app_with, overdue};

    #[test]
    fn grid_video_returns_to_grid() {
        let mut app = app_with(&["clip.mp4", "b.png"]);
        app.enter_fullscreen_selected();
        assert_eq!(app.mode, ViewMode::Video);
        assert_eq!(app.video_return, VideoReturn::Gallery);
        app.return_from_video();
        assert_eq!(app.mode, ViewMode::Gallery);
    }

    #[test]
    fn fullscreen_video_returns_to_placeholder() {
        let mut app = app_with(&["a.png", "clip.mp4"]);
        app.enter_fullscreen_at(0);
        app.next();
        assert_eq!(app.mode, ViewMode::Video);
        assert_eq!(app.video_return, VideoReturn::Fullscreen);

        app.return_from_video();
        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert!(app.shows_video_placeholder());

        // The placeholder needs no decode and no zoom.
        app.load_if_needed();
        assert!(app.fullscreen_decode.is_none());
        app.zoom_in();
        assert_eq!(app.zoom, 1.0);

        app.enter_video(1);
        assert_eq!(app.video_return, VideoReturn::Fullscreen);
        app.exit_video();
        assert_eq!(app.mode, ViewMode::Gallery);
    }

    #[test]
    fn scan_error_keeps_placeholder() {
        let mut app = app_with(&["a.png", "clip.mp4"]);
        app.scan_error = Some("denied".to_owned());
        app.enter_fullscreen_at(0);
        app.next();
        app.return_from_video();
        assert!(app.shows_video_placeholder());
        assert_eq!(app.scan_error.as_deref(), Some("denied"));
        app.enter_fullscreen_at(0);
        assert_eq!(app.scan_error.as_deref(), Some("denied"));
    }

    #[test]
    fn p_asks_for_mpv_window() {
        use crate::test_support::char_key;

        let mut app = app_with(&["a.png", "clip.mp4"]);
        app.gallery.cursor = 0;
        assert!(!app.selected_is_video());
        char_key(&mut app, 'p');
        assert_eq!(app.mode, ViewMode::Gallery, "p on a photo does nothing");
        assert!(!app.take_video_window_request());

        app.gallery.cursor = 1;
        assert!(app.selected_is_video());
        char_key(&mut app, 'p');
        assert_eq!(app.mode, ViewMode::Video);
        assert_eq!(app.current, 1);
        assert_eq!(app.video_return, VideoReturn::Gallery);
        assert!(app.take_video_window_request());

        app.return_from_video();
        app.enter_fullscreen_at(0);
        app.next();
        app.return_from_video();
        assert!(app.shows_video_placeholder());
        char_key(&mut app, 'p');
        assert_eq!(app.mode, ViewMode::Video);
        assert_eq!(app.video_return, VideoReturn::Fullscreen);
        assert!(app.take_video_window_request());
    }

    #[test]
    fn animated_image_opens_in_player() {
        use crate::test_support::{char_key, fixture, press};
        use crossterm::event::KeyCode;
        let still = fixture("still.gif");
        let anim = fixture("anim.webp");
        let mut app = app_with(&[still.to_str().unwrap(), anim.to_str().unwrap()]);
        assert!(!app.opens_in_player(0));
        assert!(
            app.opens_in_player(1),
            "a key press reads the header of the one file"
        );
        assert!(
            !app.is_animated(1),
            "the draw reads no header, so the grid marker waits for the thumbnail job"
        );
        assert!(
            !app.is_video(1),
            "the loading icon and the placeholder are for videos"
        );

        app.enter_fullscreen_at(0);
        assert_eq!(app.mode, ViewMode::Fullscreen);
        app.next();
        assert_eq!(
            app.mode,
            ViewMode::Video,
            "a step plays the animation at once"
        );
        assert_eq!(app.video_return, VideoReturn::Fullscreen);

        // Esc lands on the still first frame. The image decode reads it.
        app.return_from_video();
        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert!(!app.shows_video_placeholder());
        assert!(app.fullscreen_on_animation());
        app.load_if_needed();
        assert_eq!(
            app.fullscreen_decode.as_ref().map(|decode| decode.index),
            Some(1)
        );

        press(&mut app, KeyCode::Enter);
        assert_eq!(app.mode, ViewMode::Video, "Enter plays it again");
        app.return_from_video();
        char_key(&mut app, 'p');
        assert_eq!(app.mode, ViewMode::Video);
        assert!(app.take_video_window_request(), "p opens the mpv window");

        app.exit_video();
        app.gallery.cursor = 1;
        assert!(app.selected_is_video());
        app.enter_fullscreen_selected();
        assert_eq!(app.mode, ViewMode::Video);
        assert_eq!(app.video_return, VideoReturn::Gallery);
    }

    #[test]
    fn clock_leaves_video_slideshow() {
        let mut app = app_with(&["a.png", "clip.mp4", "b.png"]);
        app.enter_fullscreen_at(0);
        app.start_slideshow();
        app.next();
        assert_eq!(app.mode, ViewMode::Video);
        assert!(app.slideshow_advancing());
        assert_eq!(app.slideshow_timeout(), None, "no wake-up for the clock");

        app.slideshow.as_mut().unwrap().next_at = overdue();
        app.slideshow_tick();
        assert_eq!(app.current, 1, "the clip, not the clock, ends the slide");
        assert_eq!(app.mode, ViewMode::Video);
    }

    #[test]
    fn slideshow_starts_on_video() {
        let mut app = app_with(&["clip.mp4", "b.png"]);
        app.slideshow_autostart = true;
        app.scan_complete = true;
        app.maybe_autostart_slideshow();
        assert_eq!(app.mode, ViewMode::Video);
        assert!(app.slideshow_running());
        assert_eq!(app.video_return, VideoReturn::Fullscreen);

        // A paused slideshow does not wait for the end of the clip.
        app.slideshow_toggle_pause();
        assert!(!app.slideshow_advancing());
        app.slideshow_tick();
        assert_eq!(app.current, 0, "a paused slideshow waits");
    }

    #[test]
    fn slideshow_passes_video_placeholder() {
        let mut app = app_with(&["clip.mp4", "b.png"]);
        app.mode = ViewMode::Fullscreen;
        app.start_slideshow();
        app.slideshow.as_mut().unwrap().next_at = overdue();
        app.slideshow_tick();
        assert_eq!(app.current, 1);
        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert!(app.loaded.is_none());
        assert_eq!(
            app.fullscreen_decode.as_ref().map(|decode| decode.index),
            Some(1)
        );
    }
}
