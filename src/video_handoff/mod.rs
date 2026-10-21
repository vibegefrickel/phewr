//! The hand-off of a video to mpv, in the terminal or in a window, and the way back.

mod terminal;
mod window;

use crate::app::{App, ViewMode};
use crate::config::{self, Config};
use crate::mpv;
use crate::run_loop::LoopState;
use crate::yazi::find_on_path;
use std::io;
use std::path::Path;
pub(crate) use terminal::TerminalHandoff;
use terminal::play_in_terminal;
pub(crate) use window::poll_mpv_window;

/// The video player of the session, after the PATH lookup for mpv.
pub(crate) struct VideoState {
    pub(crate) player: mpv::Player,
    /// The note that tells why the built-in player plays instead of mpv.
    pub(crate) fallback_note: Option<String>,
}

impl VideoState {
    /// Look up mpv once at the start, so that a missing mpv does not cost a terminal hand-off.
    pub(crate) fn new(config: &Config) -> Self {
        let player = mpv::resolve(
            config.video_player,
            find_on_path("mpv", std::env::var_os("PATH").as_deref()),
        );
        Self {
            fallback_note: (player == mpv::Player::Builtin
                && config.video_player != config::VideoPlayer::Builtin)
                .then(|| mpv::MISSING_NOTE.to_owned()),
            player,
        }
    }
}

/// The result of an mpv start: a window that plays on, or the exit code of mpv in the terminal.
enum Played {
    Window(std::sync::mpsc::Receiver<Option<i32>>),
    Terminal(Option<i32>),
}

/// Give the current video to mpv. After `n` or `N` in mpv, the next video goes to mpv at once,
/// without a draw of viewr in between.
pub(crate) fn hand_off_videos(
    app: &mut App,
    state: &mut LoopState,
    handoff: &mut impl TerminalHandoff,
    runner: &mut impl mpv::Runner,
) -> io::Result<()> {
    while app.mode == ViewMode::Video
        && app.video.is_none()
        && hand_off_video(app, state, handoff, runner)?
    {}
    Ok(())
}

/// Play the current video in mpv when the configuration and PATH allow it.
///
/// After mpv, viewr goes back to the view that opened the video. The gallery has the cursor
/// on the video. The fullscreen view stays on the video and shows a play hint. If mpv does
/// not start, the built-in player takes over for the rest of the session.
///
/// A running slideshow that is not paused plays the video one time, see [`after_mpv`].
/// Returns true when mpv took the video, so that the run loop can start the next video.
fn hand_off_video(
    app: &mut App,
    state: &mut LoopState,
    handoff: &mut impl TerminalHandoff,
    runner: &mut impl mpv::Runner,
) -> io::Result<bool> {
    let path = app.current_path().to_path_buf();
    let playback = if app.slideshow_advancing() {
        mpv::Playback::Once
    } else {
        mpv::Playback::Loop
    };
    let played = match &state.video.player {
        mpv::Player::Builtin => {
            state.status.clone_from(&state.video.fallback_note);
            return Ok(false);
        }
        mpv::Player::Window(program) => runner
            .spawn(program, &mpv::window_args(&path, playback))
            .map(Played::Window),
        mpv::Player::Terminal(program) => {
            let picture = mpv::Picture {
                longest_side: handoff.longest_side(app.cell_px),
                tone_map: mpv::hdr_of(&path).and_then(|hdr| {
                    let temp = std::env::temp_dir().join("viewr");
                    mpv::tone_map_file(&[&app.disk_cache_dir, &temp], hdr)
                }),
            };
            let args = mpv::terminal_args(&path, handoff.geometry(app.cell_px), playback, &picture);
            play_in_terminal(app, handoff, runner, program, &args)?.map(Played::Terminal)
        }
    };
    match played {
        Ok(played) => {
            state.status = after_mpv(app, played, playback, &path);
            Ok(true)
        }
        Err(error) => {
            state.video.player = mpv::Player::Builtin;
            state.video.fallback_note =
                Some(format!("mpv did not start ({error}): built-in player"));
            state.status.clone_from(&state.video.fallback_note);
            Ok(false)
        }
    }
}

/// Lead the view and the slideshow on after mpv, and return the status note.
///
/// `n` and `N` in mpv step to the neighbor in the list, see [`App::step_from_video`].
/// Outside a slideshow, or in a paused one, mpv played in a loop until the user quit it, so a
/// slideshow stops. In [`mpv::Playback::Once`], exit code 0 in the terminal is the end of the
/// file and the slideshow moves on. Each other exit stops the slideshow on the placeholder:
/// `q`, `Q` and `Ctrl+C` (code 4), an error or a signal. viewr follows a window until it
/// closes, see [`App::poll_video_window`]. A window in a slideshow holds the slideshow on the
/// placeholder until then.
fn after_mpv(
    app: &mut App,
    played: Played,
    playback: mpv::Playback,
    path: &Path,
) -> Option<String> {
    let once = playback == mpv::Playback::Once;
    let code = match played {
        Played::Window(exit) => {
            back_from_video(app);
            if !once {
                app.stop_slideshow();
            }
            app.watch_video_window(exit, once);
            let name = mpv::file_name(path);
            return Some(if once {
                format!("{name} plays in an mpv window, the slideshow waits for its end")
            } else {
                format!("{name} plays in an mpv window")
            });
        }
        Played::Terminal(code) => code,
    };
    if let Some(step) = mpv::step(code) {
        if app.step_from_video(step) {
            focus_gallery_on_current(app);
        } else {
            back_from_video(app);
        }
    } else {
        back_from_video(app);
        if once {
            app.after_slideshow_video(mpv::reached_the_end(code));
        } else {
            app.stop_slideshow();
        }
    }
    mpv_note(app, code, path)
}

/// Return to the view that opened the video, with the gallery cursor on it.
fn back_from_video(app: &mut App) {
    app.return_from_video();
    focus_gallery_on_current(app);
}

/// The status note after mpv played `path`: a failed playback, or a step past the list end.
fn mpv_note(app: &App, code: Option<i32>, path: &Path) -> Option<String> {
    let Some(step) = mpv::step(code) else {
        return mpv::exit_note(code, path);
    };
    // A step that moved nothing hit an end of the list.
    (app.current_path() == path).then(|| {
        let end = match step {
            mpv::Step::Next => "last",
            mpv::Step::Previous => "first",
        };
        format!("{} is the {end} file", mpv::file_name(path))
    })
}

/// Put the gallery cursor on the current file, so that the gallery shows where the user was.
pub(crate) fn focus_gallery_on_current(app: &mut App) {
    if let Some(position) = app
        .gallery
        .filtered_indices
        .iter()
        .position(|&index| index == app.current)
    {
        app.gallery.cursor = position;
        app.gallery.ensure_cursor_visible();
    }
}

#[cfg(test)]
mod tests {
    use super::{hand_off_video, hand_off_videos};
    use crate::app::ViewMode;
    use crate::keys::fullscreen::handle_fullscreen_key;
    use crate::keys::video::handle_video_key;
    use crate::mpv;
    use crate::test_support::video::{
        fakes, fullscreen_app_on_video, fullscreen_video_app, gallery_app_on_video, last_start,
        slideshow_video_app, starts, state, terminal_state, video_app,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn terminal_player_returns_to_the_fullscreen_view_that_opened_the_video() {
        let mut app = fullscreen_video_app();
        let mut state = terminal_state();
        let (_log, mut terminal, mut runner) = fakes(Ok(Some(0)));

        hand_off_video(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert_eq!(app.current, 1, "fullscreen stays on the video");
        assert!(app.shows_video_placeholder());
        assert!(app.loaded.is_none());
        assert!(app.needs_render);
        assert_eq!(state.status, None);
    }

    #[test]
    fn a_slideshow_plays_the_video_once_in_the_terminal_and_moves_on() {
        let mut app = slideshow_video_app();
        let mut state = terminal_state();
        let (log, mut terminal, mut runner) = fakes(Ok(Some(0)));

        hand_off_video(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        let start = last_start(&log);
        assert!(start.contains(" --loop-file=no "), "{start}");
        assert!(
            start.contains(
                r#" --input-commands=keybind n "quit 10",keybind N "quit 11",keybind q "quit 4",keybind Q "quit 4" "#
            ),
            "{start}"
        );
        assert_eq!(app.current, 2, "the end of the clip moves on");
        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert!(app.slideshow_running());
        let interval = app.slideshow.as_ref().unwrap().interval;
        assert!(
            app.slideshow_timeout().unwrap() > interval / 2,
            "the next slide gets the full interval"
        );
        assert_eq!(state.status, None);
    }

    #[test]
    fn a_quit_in_mpv_stops_the_slideshow_on_the_placeholder() {
        for (code, note) in [
            (Some(mpv::USER_QUIT), None),
            (None, None),
            (Some(2), Some("mpv could not play clip.mp4 (exit code 2)")),
        ] {
            let mut app = slideshow_video_app();
            let mut state = terminal_state();
            let (_log, mut terminal, mut runner) = fakes(Ok(code));

            hand_off_video(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

            assert_eq!(app.current, 1, "{code:?}");
            assert!(app.shows_video_placeholder(), "{code:?}");
            assert!(!app.slideshow_running(), "{code:?} stops the slideshow");
            assert_eq!(state.status.as_deref(), note);
        }
    }

    #[test]
    fn the_last_video_ends_the_slideshow_or_wraps_with_the_loop() {
        for wrap in [false, true] {
            let mut app = fullscreen_app_on_video(&["a.jpg", "clip.mp4"], true);
            app.slideshow_loop = wrap;
            let mut state = terminal_state();
            let (_log, mut terminal, mut runner) = fakes(Ok(Some(0)));

            hand_off_video(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

            assert_eq!(app.slideshow_running(), wrap);
            assert_eq!(
                app.current,
                usize::from(!wrap),
                "the loop wraps to the first"
            );
            assert_eq!(app.mode, ViewMode::Fullscreen);
        }
    }

    #[test]
    fn a_paused_slideshow_loops_the_video_and_stops_after_mpv() {
        let mut app = slideshow_video_app();
        app.slideshow_toggle_pause();
        let mut state = terminal_state();
        let (log, mut terminal, mut runner) = fakes(Ok(Some(0)));

        hand_off_video(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        assert!(last_start(&log).contains(" --loop-file=inf "));
        assert!(!last_start(&log).contains("quit 4"));
        assert_eq!(app.current, 1);
        assert!(app.shows_video_placeholder());
        assert!(!app.slideshow_running(), "the user quit the loop");
    }

    #[test]
    fn the_terminal_player_outside_a_slideshow_loops_as_before() {
        let mut app = fullscreen_video_app();
        let mut state = terminal_state();
        let (log, mut terminal, mut runner) = fakes(Ok(Some(0)));

        hand_off_video(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        let start = last_start(&log);
        assert!(start.contains(" --loop-file=inf "), "{start}");
        assert!(!start.contains("quit 4"), "{start}");
        assert_eq!(app.current, 1);
        assert!(!app.slideshow_running());
    }

    #[test]
    fn enter_on_the_fullscreen_placeholder_plays_the_video_again() {
        let mut app = fullscreen_video_app();
        let mut state = terminal_state();
        let (log, mut terminal, mut runner) = fakes(Ok(Some(0)));
        hand_off_video(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert!(!handle_fullscreen_key(&mut app, enter).unwrap());
        assert_eq!(app.mode, ViewMode::Video);
        hand_off_video(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        let runs = log
            .borrow()
            .iter()
            .filter(|entry| entry.starts_with("run "))
            .count();
        assert_eq!(runs, 2);
        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert_eq!(app.current, 1);
    }

    #[test]
    fn a_video_from_the_gallery_returns_to_the_gallery_after_a_fullscreen_return() {
        let mut app = fullscreen_video_app();
        let mut state = terminal_state();
        let (_log, mut terminal, mut runner) = fakes(Ok(Some(0)));
        hand_off_video(&mut app, &mut state, &mut terminal, &mut runner).unwrap();
        assert_eq!(app.mode, ViewMode::Fullscreen);

        app.enter_gallery();
        app.gallery.cursor = 1;
        app.enter_fullscreen_selected();
        assert_eq!(app.mode, ViewMode::Video);
        hand_off_video(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        assert_eq!(app.mode, ViewMode::Gallery, "the gallery opened this video");
        assert_eq!(app.gallery.cursor, 1);
    }

    #[test]
    fn esc_in_the_builtin_player_returns_to_the_fullscreen_view() {
        let mut app = fullscreen_video_app();

        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(!handle_video_key(&mut app, esc).unwrap());

        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert_eq!(app.current, 1);
        assert!(app.shows_video_placeholder());
    }

    #[test]
    fn esc_in_the_builtin_player_stops_a_running_slideshow() {
        let mut app = slideshow_video_app();
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);

        assert!(!handle_video_key(&mut app, esc).unwrap());
        app.slideshow_tick();

        assert!(!app.slideshow_running(), "Esc is a quit, like q in mpv");
        assert_eq!(app.current, 1);
        assert!(app.shows_video_placeholder());
    }

    #[test]
    fn a_step_in_the_builtin_player_gives_the_next_file_the_full_interval() {
        let mut app = slideshow_video_app();
        let right = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);

        assert!(!handle_video_key(&mut app, right).unwrap());

        assert_eq!(app.current, 2);
        assert!(app.slideshow_running());
        let interval = app.slideshow.as_ref().unwrap().interval;
        assert!(app.slideshow_timeout().unwrap() > interval / 2);
    }

    #[test]
    fn a_failed_builtin_open_shows_the_error_and_no_play_hint() {
        let mut app = fullscreen_video_app();
        app.open_video_if_needed();
        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert!(app.error.is_some(), "the video file does not exist");

        assert!(!app.shows_video_placeholder());
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert!(!handle_fullscreen_key(&mut app, enter).unwrap());
        assert_eq!(
            app.mode,
            ViewMode::Fullscreen,
            "Enter does not retry the open"
        );
    }

    #[test]
    fn a_failed_playback_shows_the_exit_code() {
        let mut app = video_app();
        let mut state = terminal_state();
        let (_log, mut terminal, mut runner) = fakes(Ok(Some(2)));

        hand_off_video(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        assert_eq!(app.mode, ViewMode::Gallery);
        assert_eq!(
            state.status.as_deref(),
            Some("mpv could not play clip.mp4 (exit code 2)")
        );
    }

    #[test]
    fn builtin_player_starts_nothing_and_shows_the_fallback_note() {
        let mut app = video_app();
        let mut state = state(mpv::Player::Builtin);
        state.video.fallback_note = Some(mpv::MISSING_NOTE.to_owned());
        let (log, mut terminal, mut runner) = fakes(Ok(Some(0)));

        hand_off_video(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        assert!(log.borrow().is_empty());
        assert_eq!(app.mode, ViewMode::Video);
        assert_eq!(state.status.as_deref(), Some(mpv::MISSING_NOTE));
    }

    #[test]
    fn n_in_the_terminal_plays_the_next_video_at_once_and_q_returns_to_the_gallery() {
        let mut app = gallery_app_on_video(&["clip.mp4", "next.mov", "b.jpg"], 0);
        let mut state = terminal_state();
        let (log, mut terminal, mut runner) = fakes(Ok(Some(0)));
        runner.exits = vec![Some(mpv::NEXT_FILE)];

        hand_off_videos(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        let starts = starts(&log);
        assert_eq!(starts.len(), 2);
        assert!(starts[0].ends_with(" -- /videos/clip.mp4"), "{}", starts[0]);
        assert!(starts[1].ends_with(" -- /videos/next.mov"), "{}", starts[1]);
        assert_eq!(
            app.mode,
            ViewMode::Gallery,
            "a run of videos ends in the gallery"
        );
        assert_eq!(app.current, 1);
        assert_eq!(app.gallery.cursor, 1, "the cursor is on the last video");
        assert_eq!(state.status, None);
    }

    #[test]
    fn a_step_in_the_terminal_onto_an_image_shows_it_in_the_fullscreen_view() {
        for (code, target) in [(mpv::NEXT_FILE, 2), (mpv::PREVIOUS_FILE, 0)] {
            let mut app = gallery_app_on_video(&["a.jpg", "clip.mp4", "b.jpg"], 1);
            let mut state = terminal_state();
            let (log, mut terminal, mut runner) = fakes(Ok(Some(code)));

            hand_off_videos(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

            assert_eq!(starts(&log).len(), 1, "{code}");
            assert_eq!(app.mode, ViewMode::Fullscreen, "{code}");
            assert_eq!(app.current, target, "{code}");
            assert_eq!(app.gallery.cursor, target, "{code}");
            assert_eq!(state.status, None);
        }
    }

    #[test]
    fn a_step_past_an_end_of_the_list_returns_with_a_note() {
        for (code, note) in [
            (mpv::NEXT_FILE, "clip.mp4 is the last file"),
            (mpv::PREVIOUS_FILE, "clip.mp4 is the first file"),
        ] {
            let mut app = gallery_app_on_video(&["clip.mp4"], 0);
            let mut state = terminal_state();
            let (log, mut terminal, mut runner) = fakes(Ok(Some(code)));

            hand_off_videos(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

            assert_eq!(starts(&log).len(), 1);
            assert_eq!(app.mode, ViewMode::Gallery);
            assert_eq!(app.current, 0);
            assert_eq!(state.status.as_deref(), Some(note));
        }
    }

    #[test]
    fn a_run_of_videos_from_the_fullscreen_view_ends_on_the_placeholder() {
        let mut app = fullscreen_app_on_video(&["a.jpg", "clip.mp4", "next.mov"], false);
        let mut state = terminal_state();
        let (log, mut terminal, mut runner) = fakes(Ok(Some(0)));
        runner.exits = vec![Some(mpv::NEXT_FILE)];

        hand_off_videos(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        assert_eq!(starts(&log).len(), 2);
        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert_eq!(app.current, 2);
        assert!(app.shows_video_placeholder());
    }

    #[test]
    fn a_step_in_a_slideshow_keeps_the_slideshow_and_its_rules() {
        let mut app = fullscreen_app_on_video(&["a.jpg", "clip.mp4", "next.mov", "b.jpg"], true);
        let mut state = terminal_state();
        let (log, mut terminal, mut runner) = fakes(Ok(Some(0)));
        runner.exits = vec![Some(mpv::NEXT_FILE)];

        hand_off_videos(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        let starts = starts(&log);
        assert_eq!(starts.len(), 2);
        assert!(
            starts[1].contains(" --loop-file=no "),
            "the next video plays once"
        );
        assert_eq!(app.current, 3, "the end of the second clip moves on");
        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert!(app.slideshow_running());
    }
}
