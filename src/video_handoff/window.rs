//! mpv in its own window: phewr follows the window until it exits.

use super::mpv_note;
use crate::app::App;
use crate::run_loop::LoopState;

/// Show the note of a failed playback after an mpv window exits.
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
        fake_runner, fullscreen_app_on_video, gallery_app_on_video, last_start,
        slideshow_video_app, video_app, window_state,
    };
    use crate::video_handoff::play_videos;

    #[test]
    fn window_holds_slideshow() {
        let mut app = slideshow_video_app();
        let mut state = window_state();
        let (log, mut runner) = fake_runner();

        play_videos(&mut app, &mut state, &mut runner);

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
    fn quit_window_stops_slideshow() {
        for close in [Some(Some(mpv::USER_QUIT)), Some(None), None] {
            let mut app = slideshow_video_app();
            let mut state = window_state();
            let (_log, mut runner) = fake_runner();
            play_videos(&mut app, &mut state, &mut runner);

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
    fn slideshow_window_gets_socket() {
        let mut app = slideshow_video_app();
        let mut state = window_state();
        let (log, mut runner) = fake_runner();
        play_videos(&mut app, &mut state, &mut runner);
        assert!(last_start(&log).contains(" --input-ipc-server="));

        let mut app = video_app();
        let (log, mut runner) = fake_runner();
        play_videos(&mut app, &mut window_state(), &mut runner);
        assert!(!last_start(&log).contains("--input-ipc-server"));
    }

    #[test]
    fn app_quit_reason_stops_slideshow() {
        let mut app = slideshow_video_app();
        let mut state = window_state();
        let (_log, mut runner) = fake_runner();
        play_videos(&mut app, &mut state, &mut runner);

        // The runner folds the reason `quit` of an exit code 0 into the quit code.
        let code = mpv::exit_with_reason(Some(0), Some("quit"));
        runner.window.take().unwrap().send(code).unwrap();
        poll_mpv_window(&mut app, &mut state);
        app.slideshow_tick();

        assert_eq!(app.current, 1);
        assert!(!app.slideshow_running());
    }

    #[test]
    fn step_away_forgets_window() {
        let mut app = slideshow_video_app();
        let mut state = window_state();
        let (_log, mut runner) = fake_runner();
        play_videos(&mut app, &mut state, &mut runner);

        app.next();
        let late_quit = runner.window.take().unwrap().send(Some(mpv::USER_QUIT));
        assert!(late_quit.is_err(), "phewr dropped the exit of the window");
        app.prev();
        assert_eq!(app.mode, ViewMode::Video, "prev steps onto the video");
        app.return_from_video();
        app.slideshow_tick();

        assert!(app.slideshow_running(), "the late quit does not count");
        assert!(app.shows_video_placeholder());
    }

    #[test]
    fn window_player_skips_terminal() {
        let mut app = video_app();
        let mut state = window_state();
        let (log, mut runner) = fake_runner();

        play_videos(&mut app, &mut state, &mut runner);

        assert_eq!(
            *log.borrow(),
            ["spawn /bin/mpv --no-terminal --force-window=immediate \
                 --fs --native-fs=no --hwdec=auto-safe --loop-file=inf \
                 --input-commands=keybind l \"quit 10\",keybind h \"quit 11\" \
                 -- /videos/clip.mp4"]
        );
        assert_eq!(app.mode, ViewMode::Gallery);
        assert_eq!(
            state.status.as_deref(),
            Some("clip.mp4 plays in an mpv window")
        );
    }

    #[test]
    fn n_steps_to_neighbor_video() {
        let mut app = gallery_app_on_video(&["clip.mp4", "next.mov", "b.jpg"], 0);
        let mut state = window_state();
        let (log, mut runner) = fake_runner();
        play_videos(&mut app, &mut state, &mut runner);
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
        play_videos(&mut app, &mut state, &mut runner);
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
    fn step_onto_image_opens_fullscreen() {
        let mut app = fullscreen_app_on_video(&["a.jpg", "clip.mp4", "b.jpg"], false);
        let mut state = window_state();
        let (_log, mut runner) = fake_runner();
        play_videos(&mut app, &mut state, &mut runner);

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
    fn failed_window_shows_exit_code() {
        let mut app = video_app();
        let mut state = window_state();
        let (_log, mut runner) = fake_runner();
        play_videos(&mut app, &mut state, &mut runner);

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
    fn step_behind_overlay_is_ignored() {
        let mut app = fullscreen_app_on_video(&["a.jpg", "clip.mp4", "b.jpg"], false);
        let mut state = window_state();
        let (_log, mut runner) = fake_runner();
        play_videos(&mut app, &mut state, &mut runner);
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
        assert!(!app.watches_video_window(), "phewr forgets the window");
    }

    #[test]
    fn loop_polls_window_every_100_ms() {
        let mut app = fullscreen_app_on_video(&["a.jpg", "clip.mp4"], false);
        let mut state = window_state();
        let (_log, mut runner) = fake_runner();
        play_videos(&mut app, &mut state, &mut runner);

        assert!(crate::run_loop::next_timeout(&app, &[]) <= crate::run_loop::WINDOW_POLL);
    }
}
