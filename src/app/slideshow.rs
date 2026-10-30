//! The slideshow: start, pause, the interval, the clock and the autostart from the CLI.

#[cfg(feature = "video")]
use super::VideoReturn;
use super::{App, ViewMode};
use std::time::{Duration, Instant};

/// Step for `[` / `]` while a slideshow runs.
const SLIDESHOW_INTERVAL_STEP: f64 = 0.5;
const MIN_SLIDESHOW_INTERVAL: f64 = 0.5;
const MAX_SLIDESHOW_INTERVAL: f64 = 600.0;

/// A running slideshow in fullscreen mode.
pub(crate) struct Slideshow {
    pub(crate) interval: Duration,
    pub(crate) paused: bool,
    /// When the next advance is due. Pushed back while an image is still decoding.
    pub(super) next_at: Instant,
}

impl App {
    pub(crate) fn slideshow_running(&self) -> bool {
        self.slideshow.is_some()
    }

    pub(crate) fn toggle_slideshow(&mut self) {
        if self.slideshow.is_some() {
            self.stop_slideshow();
        } else {
            self.start_slideshow();
        }
    }

    pub(crate) fn start_slideshow(&mut self) {
        if !self.has_files() {
            return;
        }
        let interval = self.slideshow_interval;
        self.slideshow = Some(Slideshow {
            interval,
            paused: false,
            next_at: Instant::now() + interval,
        });
        // A slideshow lives in the fullscreen view, also when it starts on a video.
        #[cfg(feature = "video")]
        if self.mode == ViewMode::Video {
            self.video_return = VideoReturn::Fullscreen;
        }
        self.needs_render = true;
    }

    /// A running slideshow that is not paused. It plays a video one time and then moves on.
    /// Outside of it, a video plays in a loop until the user stops it.
    pub(crate) fn slideshow_advancing(&self) -> bool {
        self.slideshow.as_ref().is_some_and(|show| !show.paused)
    }

    /// The interval of a running slideshow that is not paused, for the hold of an animation.
    #[cfg(feature = "video")]
    pub(crate) fn slideshow_interval_running(&self) -> Option<Duration> {
        self.slideshow
            .as_ref()
            .filter(|show| !show.paused)
            .map(|show| show.interval)
    }

    pub(crate) fn stop_slideshow(&mut self) {
        if self.slideshow.take().is_some() {
            self.needs_render = true;
        }
    }

    pub(crate) fn slideshow_toggle_pause(&mut self) {
        if let Some(show) = self.slideshow.as_mut() {
            show.paused = !show.paused;
            if !show.paused {
                show.next_at = Instant::now() + show.interval;
            }
            self.needs_render = true;
        }
    }

    /// Lengthen or shorten the interval by one step; also becomes the default for
    /// the next slideshow this session.
    pub(crate) fn slideshow_adjust_interval(&mut self, steps: f64) {
        let current = self
            .slideshow
            .as_ref()
            .map_or(self.slideshow_interval, |show| show.interval)
            .as_secs_f64();
        let next = (current + steps * SLIDESHOW_INTERVAL_STEP)
            .clamp(MIN_SLIDESHOW_INTERVAL, MAX_SLIDESHOW_INTERVAL);
        let interval = Duration::from_secs_f64(next);
        self.slideshow_interval = interval;
        if let Some(show) = self.slideshow.as_mut() {
            show.interval = interval;
            show.next_at = Instant::now() + interval;
        }
        self.needs_render = true;
    }

    /// Restart the countdown, for example after a step of the user.
    pub(crate) fn slideshow_touch(&mut self) {
        if let Some(show) = self.slideshow.as_mut() {
            show.next_at = Instant::now() + show.interval;
        }
    }

    /// How long the event loop may sleep before the slideshow needs attention.
    pub(crate) fn slideshow_timeout(&self) -> Option<Duration> {
        let show = self.slideshow.as_ref()?;
        // In the video view, the end of the clip moves the slideshow on, not the clock.
        if show.paused || self.in_video_view() {
            return None;
        }
        Some(show.next_at.saturating_duration_since(Instant::now()))
    }

    /// Advance when the interval elapsed. Waits for the current image to finish
    /// decoding so every slide is actually shown for the full interval.
    pub(crate) fn slideshow_tick(&mut self) {
        if !self.slideshow_advancing() {
            return;
        }
        #[cfg(feature = "video")]
        if self.slideshow_video_holds() {
            return;
        }
        let Some(show) = self.slideshow.as_ref() else {
            return;
        };
        if self.mode != ViewMode::Fullscreen || Instant::now() < show.next_at {
            return;
        }
        if self.loaded.is_none() && !self.fullscreen_on_video() {
            // Still decoding: check again shortly instead of skipping the slide.
            self.slideshow_check_again_soon();
            return;
        }
        self.slideshow_step();
    }

    /// Push the next check of the slideshow 100 ms out, while a decode or a video holds the slide.
    pub(super) fn slideshow_check_again_soon(&mut self) {
        if let Some(show) = self.slideshow.as_mut() {
            show.next_at = Instant::now() + Duration::from_millis(100);
        }
    }

    /// Go to the next file, or wrap to the first with the loop setting, or stop at the end.
    /// The next file gets the full interval.
    pub(super) fn slideshow_step(&mut self) {
        let last = self.images.len().saturating_sub(1);
        if self.current < last {
            self.next();
        } else if self.slideshow_loop && self.has_files() {
            self.jump_to(self.first_file());
        } else {
            self.stop_slideshow();
            return;
        }
        if let Some(show) = self.slideshow.as_mut() {
            show.next_at = Instant::now() + show.interval;
        }
    }

    /// Honour `--slideshow` once the directory scan is done, so the show starts
    /// at the first image in sorted order.
    pub(crate) fn maybe_autostart_slideshow(&mut self) {
        if !self.slideshow_autostart || !self.scan_complete {
            return;
        }
        self.slideshow_autostart = false;
        if !self.has_files() {
            return;
        }
        if self.mode == ViewMode::Gallery {
            self.enter_fullscreen_at(self.first_file());
        }
        if self.in_slideshow_view() {
            self.start_slideshow();
        }
    }
}

#[cfg(test)]
mod tests {

    use crate::app::ViewMode;
    use crate::test_support::{app, app_with, overdue};
    use image::RgbaImage;

    use super::Slideshow;
    use std::time::{Duration, Instant};

    #[test]
    fn slideshow_advances_wraps_and_stops_without_loop() {
        use super::{App, Slideshow};
        use crate::image_list::SharedImageList;
        use std::path::PathBuf;
        use std::time::{Duration, Instant};

        let mut app = App::new((8, 16), SharedImageList::new());
        app.images = vec![PathBuf::from("a.png"), PathBuf::from("b.png")];
        app.filenames = vec!["a.png".into(), "b.png".into()];
        app.mode = ViewMode::Fullscreen;
        app.current = 0;
        app.loaded = Some(RgbaImage::new(1, 1));

        app.start_slideshow();
        app.slideshow = Some(Slideshow {
            interval: Duration::from_millis(10),
            paused: false,
            next_at: Instant::now()
                .checked_sub(Duration::from_millis(1))
                .unwrap(),
        });
        app.slideshow_tick();
        assert_eq!(app.current, 1);

        // Wrap to the first image when looping.
        app.loaded = Some(RgbaImage::new(1, 1));
        app.slideshow.as_mut().unwrap().next_at = Instant::now()
            .checked_sub(Duration::from_millis(1))
            .unwrap();
        app.slideshow_tick();
        assert_eq!(app.current, 0);
        assert!(app.slideshow_running());

        // Without looping the show ends on the last image.
        app.slideshow_loop = false;
        app.current = 1;
        app.loaded = Some(RgbaImage::new(1, 1));
        app.slideshow.as_mut().unwrap().next_at = Instant::now()
            .checked_sub(Duration::from_millis(1))
            .unwrap();
        app.slideshow_tick();
        assert_eq!(app.current, 1);
        assert!(!app.slideshow_running());
    }

    #[test]
    fn slideshow_waits_for_a_pending_decode() {
        use super::{App, Slideshow};
        use crate::image_list::SharedImageList;
        use std::path::PathBuf;
        use std::time::{Duration, Instant};

        let mut app = App::new((8, 16), SharedImageList::new());
        app.images = vec![PathBuf::from("a.png"), PathBuf::from("b.png")];
        app.filenames = vec!["a.png".into(), "b.png".into()];
        app.mode = ViewMode::Fullscreen;
        app.loaded = None;
        app.slideshow = Some(Slideshow {
            interval: Duration::from_millis(10),
            paused: false,
            next_at: Instant::now()
                .checked_sub(Duration::from_millis(1))
                .unwrap(),
        });
        app.slideshow_tick();
        assert_eq!(app.current, 0, "must not advance while decoding");
        assert!(app.slideshow_timeout().is_some());
    }

    #[test]
    fn slideshow_interval_adjusts_in_steps_and_clamps() {
        use super::App;
        use crate::image_list::SharedImageList;

        let mut app = App::new((8, 16), SharedImageList::new());
        assert_eq!(app.slideshow_interval.as_secs_f64(), 3.0);
        app.slideshow_adjust_interval(1.0);
        assert_eq!(app.slideshow_interval.as_secs_f64(), 3.5);
        app.slideshow_adjust_interval(-10.0);
        assert_eq!(app.slideshow_interval.as_secs_f64(), 0.5);
    }

    #[test]
    fn slideshow_starts_pauses_and_stops() {
        let mut app = app();
        app.start_slideshow();
        assert!(!app.slideshow_running(), "no images, no slideshow");
        app.toggle_slideshow();
        assert!(!app.slideshow_running());

        let mut app = app_with(&["a.png", "b.png"]);
        app.toggle_slideshow();
        assert!(app.slideshow_running());
        assert!(app.slideshow_timeout().unwrap() <= app.slideshow_interval);

        app.slideshow_toggle_pause();
        assert!(app.slideshow.as_ref().unwrap().paused);
        assert_eq!(app.slideshow_timeout(), None);
        app.slideshow_toggle_pause();
        assert!(!app.slideshow.as_ref().unwrap().paused);
        assert!(app.slideshow_timeout().is_some());

        app.slideshow.as_mut().unwrap().next_at = overdue();
        app.slideshow_touch();
        assert!(app.slideshow_timeout().unwrap() > Duration::from_secs(1));

        app.slideshow_adjust_interval(2.0);
        assert_eq!(
            app.slideshow.as_ref().unwrap().interval,
            Duration::from_secs(4)
        );
        assert_eq!(app.slideshow_interval, Duration::from_secs(4));

        app.toggle_slideshow();
        assert!(!app.slideshow_running());
        app.needs_render = false;
        app.stop_slideshow();
        assert!(!app.needs_render, "a second stop changes nothing");

        // Without a slideshow, pause and touch do nothing.
        app.slideshow_toggle_pause();
        app.slideshow_touch();
        assert_eq!(app.slideshow_timeout(), None);
    }

    #[test]
    fn slideshow_tick_waits_for_its_turn() {
        let mut app = app_with(&["a.png", "b.png"]);
        app.slideshow_tick();
        assert_eq!(app.current, 0, "no slideshow");

        app.loaded = Some(RgbaImage::new(1, 1));
        app.start_slideshow();
        app.slideshow.as_mut().unwrap().next_at = overdue();
        app.slideshow_tick();
        assert_eq!(app.current, 0, "the gallery does not advance");

        app.mode = ViewMode::Fullscreen;
        app.slideshow_toggle_pause();
        app.slideshow.as_mut().unwrap().next_at = overdue();
        app.slideshow_tick();
        assert_eq!(app.current, 0, "a paused show does not advance");

        app.slideshow_toggle_pause();
        app.slideshow_tick();
        assert_eq!(app.current, 0, "the interval did not elapse");

        app.slideshow.as_mut().unwrap().next_at = overdue();
        app.slideshow_tick();
        assert_eq!(app.current, 1);
        assert!(app.slideshow_timeout().unwrap() > Duration::from_secs(1));
    }

    #[test]
    fn autostart_waits_for_the_scan_and_opens_the_first_image() {
        let mut app = app_with(&["a.png", "b.png"]);
        app.slideshow_autostart = true;
        app.maybe_autostart_slideshow();
        assert!(!app.slideshow_running(), "the scan is still running");

        app.scan_complete = true;
        app.gallery.cursor = 1;
        app.maybe_autostart_slideshow();
        assert!(!app.slideshow_autostart);
        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert_eq!(app.current, 0);
        assert!(app.slideshow_running());

        let mut empty = app_with(&[]);
        empty.slideshow_autostart = true;
        empty.scan_complete = true;
        empty.maybe_autostart_slideshow();
        assert!(!empty.slideshow_autostart);
        assert!(!empty.slideshow_running());
    }

    #[test]
    fn slideshow_struct_reports_its_interval() {
        let show = Slideshow {
            interval: Duration::from_secs(2),
            paused: true,
            next_at: Instant::now(),
        };
        let mut app = app_with(&["a.png"]);
        app.slideshow = Some(show);
        assert_eq!(app.slideshow_timeout(), None);
        app.slideshow_adjust_interval(-1.0);
        assert_eq!(
            app.slideshow.as_ref().unwrap().interval,
            Duration::from_millis(1500)
        );
    }
}
