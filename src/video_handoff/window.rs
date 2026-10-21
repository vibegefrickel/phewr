//! mpv in its own window: viewr follows the window until it exits.

use super::mpv_note;
use crate::app::App;
use crate::run_loop::LoopState;

/// Act on the exit of an mpv window, and show the note of a failed playback.
pub(crate) fn poll_mpv_window(app: &mut App, state: &mut LoopState) {
    if let Some(exit) = app.poll_video_window()
        && let Some(note) = mpv_note(app, exit.code, &exit.path)
    {
        state.status = Some(note);
    }
}

#[cfg(test)]
mod tests {
    use super::poll_mpv_window;
    use crate::app::{VideoReturn, ViewMode};
    use crate::mpv;
    use crate::test_support::video::{
        fakes, fullscreen_app_on_video, fullscreen_video_app, gallery_app_on_video, last_start,
        slideshow_video_app, state, video_app, window_state,
    };
    use crate::video_handoff::{hand_off_video, hand_off_videos};
    use std::path::PathBuf;

    #[test]
    fn a_window_holds_the_slideshow_until_the_end_of_the_clip() {
        let mut app = slideshow_video_app();
        let mut state = window_state();
        let (log, mut terminal, mut runner) = fakes(Ok(None));

        hand_off_video(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        let start = last_start(&log);
        assert!(start.contains(" --loop-file=no "), "{start}");
        assert!(start.contains(r#"keybind CLOSE_WIN "quit 4""#), "{start}");
        assert_eq!(
            state.status.as_deref(),
            Some("clip.mp4 plays in an mpv window, the slideshow waits for its end")
        );
        assert!(app.shows_video_placeholder());
        app.slideshow_tick();
        assert_eq!(app.current, 1, "the open window holds the slide");
        assert!(app.slideshow_timeout().is_some(), "the loop checks again");

        runner.window.take().unwrap().send(Some(0)).unwrap();
        poll_mpv_window(&mut app, &mut state);
        assert_eq!(app.current, 2);
        assert!(app.slideshow_running());
    }

    #[test]
    fn a_closed_or_quit_window_stops_the_slideshow() {
        for close in [Some(Some(mpv::USER_QUIT)), Some(None), None] {
            let mut app = slideshow_video_app();
            let mut state = window_state();
            let (_log, mut terminal, mut runner) = fakes(Ok(None));
            hand_off_video(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

            let window = runner.window.take().unwrap();
            match close {
                Some(code) => window.send(code).unwrap(),
                // A wait thread that ends without a code.
                None => drop(window),
            }
            poll_mpv_window(&mut app, &mut state);
            app.slideshow_tick();

            assert_eq!(app.current, 1, "{close:?}");
            assert!(!app.slideshow_running(), "{close:?}");
            assert!(app.shows_video_placeholder());
        }
    }

    #[test]
    fn a_step_away_from_the_window_forgets_it() {
        let mut app = slideshow_video_app();
        let mut state = window_state();
        let (_log, mut terminal, mut runner) = fakes(Ok(None));
        hand_off_video(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        app.next();
        let late_quit = runner.window.take().unwrap().send(Some(mpv::USER_QUIT));
        assert!(late_quit.is_err(), "viewr dropped the exit of the window");
        app.prev();
        assert_eq!(app.mode, ViewMode::Video, "prev steps onto the video");
        app.return_from_video();
        app.slideshow_tick();

        assert!(app.slideshow_running(), "the late quit does not count");
        assert!(app.shows_video_placeholder());
    }

    #[test]
    fn window_player_starts_mpv_without_the_terminal() {
        let mut app = video_app();
        let mut state = state(mpv::Player::Window(PathBuf::from("/bin/mpv")));
        let (log, mut terminal, mut runner) = fakes(Ok(None));

        hand_off_video(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        assert_eq!(
            *log.borrow(),
            [
                "spawn (cooked) /bin/mpv --no-terminal --force-window=immediate \
                 --fs --native-fs=no --hwdec=auto-safe --loop-file=inf \
                 --input-commands=keybind n \"quit 10\",keybind N \"quit 11\" \
                 -- /videos/clip.mp4"
            ]
        );
        assert_eq!(app.mode, ViewMode::Gallery);
        assert_eq!(
            state.status.as_deref(),
            Some("clip.mp4 plays in an mpv window")
        );
    }

    #[test]
    fn window_player_keeps_the_fullscreen_view_on_the_video() {
        let mut app = fullscreen_video_app();
        let mut state = window_state();
        let (_log, mut terminal, mut runner) = fakes(Ok(None));

        hand_off_video(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert_eq!(app.current, 1);
        assert!(app.shows_video_placeholder());
        assert_eq!(
            state.status.as_deref(),
            Some("clip.mp4 plays in an mpv window")
        );
    }

    #[test]
    fn n_and_shift_n_in_a_window_open_the_neighbor_video_in_a_new_window() {
        let mut app = gallery_app_on_video(&["clip.mp4", "next.mov", "b.jpg"], 0);
        let mut state = window_state();
        let (log, mut terminal, mut runner) = fakes(Ok(None));
        hand_off_videos(&mut app, &mut state, &mut terminal, &mut runner).unwrap();
        assert_eq!(app.mode, ViewMode::Gallery);
        assert!(app.watches_video_window());

        runner
            .window
            .take()
            .unwrap()
            .send(Some(mpv::NEXT_FILE))
            .unwrap();
        poll_mpv_window(&mut app, &mut state);
        assert_eq!(app.mode, ViewMode::Video);
        assert_eq!(app.video_return, VideoReturn::Gallery);
        hand_off_videos(&mut app, &mut state, &mut terminal, &mut runner).unwrap();
        assert!(last_start(&log).ends_with(" -- /videos/next.mov"));
        assert_eq!(app.mode, ViewMode::Gallery);
        assert_eq!(app.gallery.cursor, 1);

        runner
            .window
            .take()
            .unwrap()
            .send(Some(mpv::PREVIOUS_FILE))
            .unwrap();
        poll_mpv_window(&mut app, &mut state);
        assert_eq!(app.mode, ViewMode::Video);
        assert_eq!(app.current, 0);
        assert_eq!(
            state.status.as_deref(),
            Some("next.mov plays in an mpv window")
        );
    }

    #[test]
    fn a_step_in_a_window_onto_an_image_opens_the_fullscreen_view() {
        let mut app = fullscreen_app_on_video(&["a.jpg", "clip.mp4", "b.jpg"], false);
        let mut state = window_state();
        let (_log, mut terminal, mut runner) = fakes(Ok(None));
        hand_off_videos(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        runner
            .window
            .take()
            .unwrap()
            .send(Some(mpv::NEXT_FILE))
            .unwrap();
        poll_mpv_window(&mut app, &mut state);

        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert_eq!(app.current, 2);
        assert!(!app.watches_video_window());
    }

    #[test]
    fn a_window_that_fails_shows_the_exit_code() {
        let mut app = video_app();
        let mut state = window_state();
        let (_log, mut terminal, mut runner) = fakes(Ok(None));
        hand_off_videos(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        runner.window.take().unwrap().send(Some(2)).unwrap();
        poll_mpv_window(&mut app, &mut state);

        assert_eq!(
            state.status.as_deref(),
            Some("mpv could not play clip.mp4 (exit code 2)")
        );
        assert_eq!(app.mode, ViewMode::Gallery);
        assert!(!app.watches_video_window());
    }

    #[test]
    fn a_window_step_behind_an_overlay_does_nothing() {
        let mut app = fullscreen_app_on_video(&["a.jpg", "clip.mp4", "b.jpg"], false);
        let mut state = window_state();
        let (_log, mut terminal, mut runner) = fakes(Ok(None));
        hand_off_videos(&mut app, &mut state, &mut terminal, &mut runner).unwrap();
        app.help_visible = true;

        runner
            .window
            .take()
            .unwrap()
            .send(Some(mpv::NEXT_FILE))
            .unwrap();
        poll_mpv_window(&mut app, &mut state);

        assert_eq!(app.current, 1, "the help popup keeps the view");
        assert!(app.shows_video_placeholder());
        assert!(!app.watches_video_window(), "viewr forgets the window");
    }

    #[test]
    fn the_loop_looks_for_the_window_exit_every_100_ms() {
        let mut app = fullscreen_app_on_video(&["a.jpg", "clip.mp4"], false);
        let mut state = window_state();
        let (_log, mut terminal, mut runner) = fakes(Ok(None));
        hand_off_videos(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        assert!(crate::run_loop::next_timeout(&app, &[]) <= crate::run_loop::WINDOW_POLL);
    }
}
