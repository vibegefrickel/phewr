//! Videos in the view: the built-in player, the way back after a video and the mpv window.

use super::{App, ViewMode};
#[cfg(feature = "video")]
use std::path::PathBuf;
#[cfg(feature = "video")]
use std::sync::mpsc;

/// The view that opened a video, and so the view that comes back after mpv.
#[cfg(feature = "video")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VideoReturn {
    Gallery,
    Fullscreen,
}

/// An mpv window that viewr follows: the index of its file and the receiver of its exit code.
#[cfg(feature = "video")]
pub(super) struct VideoWindow {
    index: usize,
    exit: mpsc::Receiver<Option<i32>>,
    /// True when a slideshow plays the video one time and waits for the window.
    holds_slideshow: bool,
}

/// The exit of a followed mpv window: the exit code and the file, for the status note.
#[cfg(feature = "video")]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct WindowExit {
    pub code: Option<i32>,
    pub path: PathBuf,
}

impl App {
    #[cfg(feature = "video")]
    pub(crate) fn enter_video(&mut self, idx: usize) {
        self.video_return = if self.mode == ViewMode::Fullscreen {
            VideoReturn::Fullscreen
        } else {
            VideoReturn::Gallery
        };
        self.current = idx;
        self.mode = ViewMode::Video;
        self.error = None;
        self.needs_render = true;
        self.loaded = None;
        self.video = None;
    }

    #[cfg(feature = "video")]
    pub(crate) fn open_video_if_needed(&mut self) {
        if self.video.is_some() {
            return;
        }
        let path = &self.images[self.current];
        let repeat = !self.slideshow_advancing();
        match crate::video::VideoPlayback::open(path, self.image_rect, self.cell_px, repeat) {
            Ok(v) => self.video = Some(v),
            Err(e) => {
                self.error = Some(e.to_string());
                self.mode = ViewMode::Fullscreen;
            }
        }
    }

    #[cfg(feature = "video")]
    pub(crate) fn exit_video(&mut self) {
        if let Some(v) = self.video.take() {
            v.stop();
        }
        self.mode = ViewMode::Gallery;
        self.needs_render = true;
    }

    /// Leave the video for the view that opened it.
    ///
    /// The fullscreen view stays on the video and shows a play hint, because the image
    /// decode cannot read a video. The arrow keys then step on from the video.
    #[cfg(feature = "video")]
    pub(crate) fn return_from_video(&mut self) {
        if self.video_return == VideoReturn::Gallery {
            self.exit_video();
            return;
        }
        if let Some(v) = self.video.take() {
            v.stop();
        }
        self.mode = ViewMode::Fullscreen;
        self.loaded = None;
        self.fullscreen_rx = None;
        self.fullscreen_target = None;
        self.reset_zoom_state();
        self.needs_render = true;
    }

    /// A slideshow video holds the slide until its end. Returns true while it holds.
    ///
    /// The built-in player ends the clip and the slideshow moves on. An mpv window holds the
    /// placeholder until the window closes, see [`App::poll_video_window`]. If the user leaves
    /// the placeholder, the slideshow forgets the window.
    #[cfg(feature = "video")]
    pub(super) fn slideshow_video_holds(&mut self) -> bool {
        if self.mode == ViewMode::Video {
            if self.video.as_ref().is_some_and(|video| video.finished) {
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

    /// Follow the mpv window of the current video until it exits.
    ///
    /// `holds_slideshow` holds a running slideshow on the placeholder until the window closes.
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

    /// True while an mpv window plays and viewr waits for its exit code.
    #[cfg(feature = "video")]
    pub(crate) fn watches_video_window(&self) -> bool {
        self.video_window.is_some()
    }

    /// Take the exit code of the followed mpv window, and act on it.
    ///
    /// `n` and `N` in the window step to the neighbor, see [`App::step_from_video`]. In a
    /// slideshow, the end of the clip moves on and each other exit stops the slideshow. The
    /// exit counts only while viewr shows the file of the window in the fullscreen view or in
    /// the gallery, without an overlay. Otherwise viewr forgets the window.
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
            && self.pending_delete.is_none()
            && !self.gallery.search_active
    }

    /// Step from the current video to its neighbor in the list, after `n` or `N` in mpv.
    ///
    /// An image opens in the fullscreen view. A video opens in the video view and keeps the
    /// view that the first video returns to, so that a run of videos from the gallery ends in
    /// the gallery. Returns false at the end of the list, and then nothing moves.
    #[cfg(feature = "video")]
    pub(crate) fn step_from_video(&mut self, step: crate::mpv::Step) -> bool {
        let target = match step {
            crate::mpv::Step::Next => {
                Some(self.current + 1).filter(|&next| next < self.images.len())
            }
            crate::mpv::Step::Previous => self.current.checked_sub(1),
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

    /// After a video that a slideshow played one time: the end of the clip moves the slideshow
    /// on. A quit by the user stops the slideshow and keeps the placeholder.
    #[cfg(feature = "video")]
    pub(crate) fn after_slideshow_video(&mut self, reached_end: bool) {
        if reached_end {
            self.slideshow_step();
        } else {
            self.stop_slideshow();
        }
    }

    /// True in the fullscreen view or in a video player: the views of a slideshow.
    pub(crate) fn in_slideshow_view(&self) -> bool {
        self.mode == ViewMode::Fullscreen || self.in_video_view()
    }

    /// True in the view of a video player, where the clip and not the clock drives the slideshow.
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

    /// The image decode cannot read a video, so the fullscreen view must not try it.
    pub(super) fn fullscreen_on_video(&self) -> bool {
        self.is_video_at(self.current) && self.mode == ViewMode::Fullscreen
    }

    /// True when the image `index` is a video, for the placeholder and the loading icon.
    #[cfg(feature = "video")]
    pub(crate) fn is_video_at(&self, index: usize) -> bool {
        self.images
            .get(index)
            .is_some_and(|path| crate::video::is_video(path))
    }

    /// Without the `video` feature, the scan lists no videos.
    #[cfg(not(feature = "video"))]
    #[expect(clippy::unused_self, reason = "the same signature as the video build")]
    pub(crate) fn is_video_at(&self, _index: usize) -> bool {
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
    fn a_video_from_the_grid_returns_to_the_grid() {
        let mut app = app_with(&["clip.mp4", "b.png"]);
        app.enter_fullscreen_selected();
        assert_eq!(app.mode, ViewMode::Video);
        assert_eq!(app.video_return, VideoReturn::Gallery);
        app.return_from_video();
        assert_eq!(app.mode, ViewMode::Gallery);
    }

    #[test]
    fn a_video_from_fullscreen_returns_to_its_placeholder() {
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
        assert!(app.fullscreen_target.is_none());
        app.zoom_in();
        assert_eq!(app.zoom, 1.0);

        app.enter_video(1);
        assert_eq!(app.video_return, VideoReturn::Fullscreen);
        app.exit_video();
        assert_eq!(app.mode, ViewMode::Gallery);
    }

    #[test]
    fn a_video_that_does_not_open_shows_its_error() {
        let mut app = app_with(&["/nowhere/clip.mp4"]);
        app.enter_fullscreen_at(0);
        app.open_video_if_needed();
        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert!(app.error.is_some());
        assert!(!app.shows_video_placeholder());
    }

    #[test]
    fn the_clock_does_not_move_the_slideshow_in_the_video_view() {
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
    fn a_slideshow_can_start_on_a_video() {
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
    fn the_slideshow_moves_past_a_video_placeholder() {
        let mut app = app_with(&["clip.mp4", "b.png"]);
        app.mode = ViewMode::Fullscreen;
        app.start_slideshow();
        app.slideshow.as_mut().unwrap().next_at = overdue();
        app.slideshow_tick();
        assert_eq!(app.current, 1);
        assert_eq!(app.mode, ViewMode::Fullscreen);
        // b.png is not prefetched, so its decode starts.
        assert!(app.loaded.is_none());
        assert_eq!(app.fullscreen_target.map(|(index, _)| index), Some(1));
    }
}
