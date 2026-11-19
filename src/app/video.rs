//! Videos in the view: the terminal player, the way back after a video and the mpv window.

use super::{App, ViewMode};
#[cfg(feature = "video")]
use std::path::{Path, PathBuf};
#[cfg(feature = "video")]
use std::sync::mpsc;

/// An mpv window that phewr follows: its file and the receiver of its exit code.
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
    /// Open the player view of the file `index`: a video, an animation or the motion of a Live
    /// Photo. `current` stays on a Live still, and the shown still goes to the prefetch cache.
    #[cfg(feature = "video")]
    pub(crate) fn enter_video(&mut self, index: usize) {
        if self.mode == ViewMode::Fullscreen && index == self.current {
            self.park_shown_image();
        }
        self.current = index;
        self.mode = ViewMode::Video;
        self.error = None;
        self.needs_render = true;
        self.loaded = None;
        self.video = None;
        self.motion = self.live_motion(index).map(Path::to_path_buf);
        self.video_window = None;
        self.clear_before_video = true;
    }

    /// True while the player plays the motion of a Live Photo.
    #[cfg(feature = "video")]
    pub(crate) fn plays_motion(&self) -> bool {
        self.motion.is_some()
    }

    /// The file of the player and the mpv window: the motion of a Live Photo, else the current
    /// file.
    #[cfg(feature = "video")]
    pub(crate) fn video_path(&self) -> &Path {
        self.motion
            .as_deref()
            .unwrap_or_else(|| self.current_path())
    }

    /// Ask the run loop to open the current video in the mpv window.
    #[cfg(feature = "video")]
    pub(crate) fn request_video_window(&mut self) {
        self.video = None;
        self.video_window_request = true;
        self.window_keeps_player = false;
    }

    /// Ask for the mpv window from the player view, which waits for the window.
    #[cfg(feature = "video")]
    pub(crate) fn request_video_window_from_player(&mut self) {
        self.request_video_window();
        self.window_keeps_player = true;
    }

    #[cfg(feature = "video")]
    pub(crate) fn take_video_window_request(&mut self) -> bool {
        std::mem::take(&mut self.video_window_request)
    }

    /// True when the player view asked for the window, see
    /// [`App::request_video_window_from_player`].
    #[cfg(feature = "video")]
    pub(crate) fn take_window_keeps_player(&mut self) -> bool {
        std::mem::take(&mut self.window_keeps_player)
    }

    #[cfg(feature = "video")]
    pub(crate) fn exit_video(&mut self) {
        self.video = None;
        self.motion = None;
        self.mode = ViewMode::Gallery;
        self.needs_render = true;
    }

    /// A failed open of the terminal player. The fullscreen view shows the error of a video, and
    /// the still of an animation or of a Live Photo. Enter does not retry the open.
    #[cfg(feature = "video")]
    pub(crate) fn leave_failed_player(&mut self, error: Option<String>) {
        self.video = None;
        self.motion = None;
        self.mode = ViewMode::Fullscreen;
        self.error = error;
        self.needs_render = true;
    }

    /// The view behind the mpv window of the current file. With `keep_player`, the player view
    /// keeps its last frame, and its player plays again after the window. A running slideshow
    /// shows the still of a Live Photo. Each other file shows the grid.
    #[cfg(feature = "video")]
    pub(crate) fn leave_for_window(&mut self, keep_player: bool) {
        if keep_player {
            // The decoder stops: the window is the only playback.
            self.video = None;
            self.needs_render = true;
        } else if !self.slideshow_shows_still(self.current) || !self.show_still() {
            self.exit_video();
        }
    }

    /// Show the still of an animation or of a Live Photo in the fullscreen view. `false` for a
    /// video, which has no still.
    #[cfg(feature = "video")]
    pub(crate) fn show_still(&mut self) -> bool {
        if self.is_video(self.current) {
            return false;
        }
        self.leave_failed_player(None);
        self.fullscreen_decode = None;
        self.reset_zoom_and_source();
        true
    }

    /// True while a slideshow video holds the slide. A slideshow window holds the grid or the
    /// player view until it closes.
    #[cfg(feature = "video")]
    pub(super) fn slideshow_video_holds(&mut self) -> bool {
        if self.mode == ViewMode::Video {
            if self
                .video
                .as_ref()
                .is_some_and(crate::player::Player::finished)
            {
                // At the end of the list, the slideshow stops and the player loops the clip.
                self.video = None;
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
        if window.index != self.current {
            self.video_window = None;
            return false;
        }
        self.slideshow_check_again_soon();
        true
    }

    /// Follow the mpv window of the current video until it exits. `holds_slideshow` holds a
    /// running slideshow until then.
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
    /// The end of a slideshow clip moves the slideshow on. Each other exit pauses the slideshow
    /// in the player view, and stops it in the grid. A step of `h` or `l` opens the neighbor
    /// video in a new window. The exit counts only while phewr shows the file of the window
    /// without an overlay.
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
        if window.holds_slideshow && crate::mpv::reached_the_end(code) {
            // A paused slideshow stays on the clip, and the player loops it.
            if self.slideshow_advancing() {
                self.slideshow_step();
            }
        } else {
            self.hold_slideshow_after_window();
            if let Some(step) = crate::mpv::step(code)
                && self.step_from_video(step)
                && self.mode == ViewMode::Video
            {
                self.request_video_window_from_player();
            }
        }
        Some(WindowExit { code, path })
    }

    /// True while phewr shows the file `index` of an mpv window and no overlay takes the keys.
    #[cfg(feature = "video")]
    fn follows_window(&self, index: usize) -> bool {
        index == self.current
            && index < self.images.len()
            && !self.help_visible
            && !self.gallery.search_active
    }

    /// Step to the neighbor after `l` or `h` in mpv. `false` at the end of the list.
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
        self.jump_to(target);
        self.slideshow_touch();
        true
    }

    /// A clip that a running slideshow played one time loops after a pause or a stop, like each
    /// clip outside a slideshow. A failed decode keeps its error on the screen.
    #[cfg(feature = "video")]
    pub(crate) fn replay_finished(&mut self) {
        if self.mode == ViewMode::Video
            && !self.slideshow_advancing()
            && self
                .video
                .as_ref()
                .is_some_and(|player| player.finished() && player.error.is_none())
        {
            self.video = None;
        }
    }

    /// A pause loops the clip of the player view, also the motion of a Live still that waits for
    /// it. A resume lets a run of the slideshow play on. A loop has no end, so a resume plays its
    /// clip one time from the start, and then the slideshow moves on.
    #[cfg(feature = "video")]
    pub(super) fn replay_after_toggle(&mut self) {
        if self.slideshow_advancing() {
            if self.mode == ViewMode::Video
                && self
                    .video
                    .as_ref()
                    .is_some_and(crate::player::Player::loops)
            {
                self.video = None;
            }
        } else if self.mode == ViewMode::Fullscreen && self.slideshow_motion_waits() {
            self.enter_video(self.current);
        }
    }

    /// A quit by the user returns to the slideshow, paused. The grid has no slideshow.
    #[cfg(feature = "video")]
    fn hold_slideshow_after_window(&mut self) {
        if self.mode == ViewMode::Gallery {
            self.stop_slideshow();
        } else if let Some(show) = self.slideshow.as_mut() {
            show.paused = true;
            self.needs_render = true;
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

    /// Only a failed open of the terminal player leaves the fullscreen view on a video, with its
    /// error. The image decode cannot read a video: the fullscreen view must not try it.
    pub(super) fn fullscreen_on_video(&self) -> bool {
        self.is_video(self.current) && self.mode == ViewMode::Fullscreen
    }

    #[cfg(feature = "video")]
    pub(crate) fn is_video(&self, index: usize) -> bool {
        self.images
            .get(index)
            .is_some_and(|path| tuilibr::pictures::motion::first_frame::is_video(path))
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
            .is_some_and(|path| tuilibr::pictures::motion::animation::is_animated(path))
    }

    /// True for each file that plays: a video, an animated image and a Live Photo still. Every
    /// view opens it in the terminal player.
    #[cfg(feature = "video")]
    pub(crate) fn opens_in_player(&self, index: usize) -> bool {
        self.is_video(index) || self.is_live(index) || self.reads_as_animated(index)
    }

    /// True when the gallery cursor is on a file that plays, for `p` in the gallery.
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
}

#[cfg(test)]
#[cfg(feature = "video")]
mod tests {
    use crate::app::ViewMode;
    use crate::test_support::{app_with, overdue};

    #[test]
    fn grid_video_returns_to_grid() {
        let mut app = app_with(&["clip.mp4", "b.png"]);
        app.enter_fullscreen_selected();
        assert_eq!(app.mode, ViewMode::Video);
        app.leave_for_window(false);
        assert_eq!(app.mode, ViewMode::Gallery);
    }

    #[test]
    fn fullscreen_steps_play_videos() {
        use crate::test_support::press;
        use crossterm::event::KeyCode;

        let mut app = app_with(&["a.png", "clip.mp4", "b.png", "last.mov"]);
        app.enter_fullscreen_at(0);
        for (key, index) in [
            (KeyCode::Char('l'), 1),
            (KeyCode::Right, 2),
            (KeyCode::End, 3),
            (KeyCode::Home, 0),
            (KeyCode::Char('k'), 1),
        ] {
            press(&mut app, key);
            assert_eq!(app.current, index, "{key:?}");
            let mode = if index % 2 == 1 {
                ViewMode::Video
            } else {
                ViewMode::Fullscreen
            };
            assert_eq!(app.mode, mode, "{key:?}");
        }
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.mode, ViewMode::Fullscreen, "a step onto a photo");
        app.prev();
        assert_eq!(app.mode, ViewMode::Video);

        // The way back from a video never lands the fullscreen view on it.
        app.leave_for_window(false);
        assert_eq!(app.mode, ViewMode::Gallery);
        assert!(!app.fullscreen_on_video());
    }

    #[test]
    fn scan_error_stays_after_video() {
        let mut app = app_with(&["a.png", "clip.mp4"]);
        app.scan_error = Some("denied".to_owned());
        app.enter_fullscreen_at(0);
        app.next();
        app.exit_video();
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
        assert!(app.take_video_window_request());
        assert!(!app.take_window_keeps_player(), "the grid stays the grid");

        app.exit_video();
        app.enter_fullscreen_at(0);
        app.next();
        char_key(&mut app, 'p');
        assert_eq!(app.mode, ViewMode::Video, "the player view waits");
        assert!(app.take_video_window_request());
        assert!(app.take_window_keeps_player());
    }

    #[test]
    fn animated_image_opens_in_player() {
        use crate::test_support::{char_key, module_fixture, press};
        use crossterm::event::KeyCode;
        let still = module_fixture("still.gif");
        let anim = module_fixture("anim.webp");
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
        assert!(!app.is_video(1), "the loading icon is for videos");

        app.enter_fullscreen_at(0);
        assert_eq!(app.mode, ViewMode::Fullscreen);
        app.next();
        assert_eq!(
            app.mode,
            ViewMode::Video,
            "a step plays the animation at once"
        );
        app.prev();
        assert_eq!(app.mode, ViewMode::Fullscreen, "the still GIF is a photo");
        press(&mut app, KeyCode::End);
        assert_eq!(app.mode, ViewMode::Video, "End plays the animation");

        // A failed open lands on the still first frame. The image decode reads it.
        app.leave_failed_player(None);
        assert!(!app.fullscreen_on_video());
        app.load_if_needed();
        assert_eq!(
            app.fullscreen_decode.as_ref().map(|decode| decode.index),
            Some(1)
        );

        press(&mut app, KeyCode::Enter);
        assert_eq!(app.mode, ViewMode::Fullscreen, "Enter does not retry");
        char_key(&mut app, 'p');
        assert_eq!(app.mode, ViewMode::Video);
        assert!(app.take_video_window_request(), "p opens the mpv window");

        char_key(&mut app, 'q');
        assert_eq!(app.mode, ViewMode::Gallery, "q goes to the grid");
        app.gallery.cursor = 1;
        assert!(app.selected_is_video());
        app.enter_fullscreen_selected();
        assert_eq!(app.mode, ViewMode::Video, "Enter in the grid plays it");
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

        // A paused slideshow does not wait for the end of the clip.
        app.slideshow_toggle_pause();
        assert!(!app.slideshow_advancing());
        app.slideshow_tick();
        assert_eq!(app.current, 0, "a paused slideshow waits");
    }

    #[test]
    fn slideshow_passes_failed_video() {
        let mut app = app_with(&["clip.mp4", "b.png"]);
        // A failed open of the player leaves its error in the fullscreen view.
        app.mode = ViewMode::Fullscreen;
        app.error = Some("no video stream".to_owned());
        app.start_slideshow();
        app.slideshow.as_mut().unwrap().next_at = overdue();
        app.load_if_needed();
        assert!(
            app.fullscreen_decode.is_none(),
            "no image decode of a video"
        );
        app.zoom_in();
        assert_eq!(app.zoom, 1.0);
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
