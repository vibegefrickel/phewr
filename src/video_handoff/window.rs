//! mpv in its own window: phewr follows the window until it exits.

use super::mpv_note;
use crate::app::App;
use crate::run_loop::LoopState;

/// Replace the window note with the note of a failed playback when an mpv window exits. A clean
/// exit leaves no note.
pub(crate) fn poll_mpv_window(app: &mut App, state: &mut LoopState) {
    if let Some(exit) = app.poll_video_window() {
        state.status = mpv_note(app, exit.code, &exit.path);
    }
}

#[cfg(test)]
mod tests {
    use super::poll_mpv_window;
    use crate::app::{App, VideoReturn, ViewMode};
    use crate::config::VideoPlayer;
    use crate::mpv;
    use crate::run_loop::LoopState;
    use crate::test_support::char_key;
    use crate::test_support::video::{
        FakeDecoder, FakeRunner, clip_info, fake_decoder, fake_runner, fullscreen_app_on_video,
        gallery_app_on_video, last_start, slideshow_video_app, state_with, video_app, window_state,
    };
    use crate::video_handoff::play_videos;
    use std::rc::Rc;

    /// A slideshow in the terminal player that opened its video in the mpv window with `p`.
    fn slideshow_in_window() -> (App, LoopState, Rc<FakeDecoder>, FakeRunner) {
        let mut app = slideshow_video_app();
        let decoder = fake_decoder(clip_info());
        let mut state = state_with(VideoPlayer::Terminal, Rc::clone(&decoder));
        let (_log, mut runner) = fake_runner();
        play_videos(&mut app, &mut state, &mut runner);
        assert!(app.video.is_some());
        char_key(&mut app, 'p');
        play_videos(&mut app, &mut state, &mut runner);
        assert!(app.watches_video_window());
        (app, state, decoder, runner)
    }

    #[test]
    fn slideshow_window_holds_player_view() {
        let (mut app, mut state, decoder, mut runner) = slideshow_in_window();
        assert_eq!(app.mode, ViewMode::Video, "the last frame stays");
        assert!(app.video.is_none(), "the decoder stops");
        app.slideshow_tick();
        play_videos(&mut app, &mut state, &mut runner);
        assert_eq!(app.current, 1, "the open window holds the slide");
        assert_eq!(decoder.starts().len(), 1, "no second ffmpeg");

        runner.window.take().unwrap().send(Some(0)).unwrap();
        poll_mpv_window(&mut app, &mut state);
        assert_eq!(app.current, 2, "the end of the clip moves on");
        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert!(app.slideshow_advancing());
    }

    #[test]
    fn quit_exit_clears_window_note() {
        let (mut app, mut state, _decoder, mut runner) = slideshow_in_window();
        assert!(
            state
                .status
                .as_ref()
                .is_some_and(|note| note.contains("waits for its end"))
        );
        let window = runner.window.take().unwrap();
        window
            .send(mpv::exit_with_reason(Some(0), Some("quit")))
            .unwrap();
        poll_mpv_window(&mut app, &mut state);
        assert_eq!(state.status, None);
        assert!(app.slideshow_running());
        assert!(!app.slideshow_advancing(), "the slideshow is paused");
    }

    #[test]
    fn clip_end_clears_window_note() {
        let (mut app, mut state, _decoder, mut runner) = slideshow_in_window();
        assert!(state.status.is_some());
        runner.window.take().unwrap().send(Some(0)).unwrap();
        poll_mpv_window(&mut app, &mut state);
        assert_eq!(state.status, None);
        assert_eq!(app.current, 2, "the end of the clip moves on");
        assert!(app.slideshow_advancing());
    }

    #[test]
    fn quit_window_pauses_slideshow() {
        let closes = [
            Some(Some(mpv::USER_QUIT)),
            Some(mpv::exit_with_reason(Some(0), Some("quit"))),
            Some(None),
            // A wait thread that ends without a code.
            None,
        ];
        for close in closes {
            let (mut app, mut state, decoder, mut runner) = slideshow_in_window();
            let window = runner.window.take().unwrap();
            match close {
                Some(code) => window.send(code).unwrap(),
                None => drop(window),
            }
            poll_mpv_window(&mut app, &mut state);
            app.slideshow_tick();
            assert_eq!(app.current, 1, "{close:?}");
            assert_eq!(app.mode, ViewMode::Video, "{close:?}");
            assert!(app.slideshow_running(), "{close:?}");
            assert!(!app.slideshow_advancing(), "{close:?}");

            play_videos(&mut app, &mut state, &mut runner);
            assert!(app.video.is_some(), "{close:?}");
            assert_eq!(decoder.starts().len(), 2, "{close:?}");
            decoder.feed(vec![0; 640 * 480 * 4]);
            assert!(app.video.as_mut().unwrap().poll());
            decoder.end();
            assert!(!app.video.as_mut().unwrap().poll());
            assert_eq!(
                decoder.starts().len(),
                3,
                "a paused slideshow loops the clip: {close:?}"
            );
        }
    }

    #[test]
    fn step_in_slideshow_window_pauses() {
        let (mut app, mut state, _decoder, mut runner) = slideshow_in_window();
        let window = runner.window.take().unwrap();
        window.send(Some(mpv::PREVIOUS_FILE)).unwrap();
        poll_mpv_window(&mut app, &mut state);
        assert_eq!(app.current, 0);
        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert!(app.slideshow_running());
        assert!(!app.slideshow_advancing());
    }

    #[test]
    fn slideshow_window_without_player_shows_grid() {
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
        assert_eq!(app.mode, ViewMode::Gallery);
        assert_eq!(app.gallery.cursor, 1);
        app.slideshow_tick();
        assert_eq!(app.current, 1, "the open window holds the slide");
        assert!(app.slideshow_timeout().is_some(), "the loop checks again");

        runner.window.take().unwrap().send(Some(0)).unwrap();
        poll_mpv_window(&mut app, &mut state);
        assert_eq!(app.current, 2);
        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert!(app.slideshow_running());
    }

    #[test]
    fn quit_window_without_player_goes_to_grid() {
        for close in [Some(Some(mpv::USER_QUIT)), Some(None), None] {
            let mut app = slideshow_video_app();
            let mut state = window_state();
            let (_log, mut runner) = fake_runner();
            play_videos(&mut app, &mut state, &mut runner);

            let window = runner.window.take().unwrap();
            match close {
                Some(code) => window.send(code).unwrap(),
                None => drop(window),
            }
            poll_mpv_window(&mut app, &mut state);
            app.slideshow_tick();

            assert_eq!(app.current, 1, "{close:?}");
            assert!(!app.slideshow_running(), "{close:?}");
            assert_eq!(app.mode, ViewMode::Gallery, "{close:?}");
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
        app.slideshow_tick();

        assert!(app.slideshow_advancing(), "the late quit does not count");
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
        assert_eq!(state.status, None, "the exit drops the old window note");
        play_videos(&mut app, &mut state, &mut runner);
        assert_eq!(
            state.status.as_deref(),
            Some("clip.mp4 plays in an mpv window")
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
        assert_eq!(app.mode, ViewMode::Gallery, "no terminal player");
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
