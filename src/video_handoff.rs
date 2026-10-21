//! The hand-off of a video to mpv, in the terminal or in a window, and the way back.

use crate::app::{App, ViewMode};
use crate::mpv;
use crate::run_loop::LoopState;
use crossterm::{
    cursor,
    event::{self, Event},
    execute,
    terminal::enable_raw_mode,
};
use ratatui::prelude::*;
use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::time::Duration;

/// Gives the terminal to mpv and takes it back. The tests use a fake.
///
/// viewr stays in raw mode and in its alternate screen during the hand-off. Raw mode turns
/// off ISIG, so `Ctrl+C`, `Ctrl+\` and `Ctrl+Z` reach mpv as keys and do not send signals to the
/// process group of viewr. mpv clears only ICANON and ECHO and restores the old mode at its
/// exit. The alternate screen stays, so the main screen of the shell never shows.
pub(crate) trait TerminalHandoff {
    /// The size for the command line of mpv, see [`mpv::fallback_geometry`].
    fn geometry(&self, cell_px: (u32, u32)) -> Option<mpv::Geometry>;
    /// The longest side of the terminal in pixels, see [`mpv::longest_side`].
    fn longest_side(&self, cell_px: (u32, u32)) -> u32;
    /// Delete the Kitty images of viewr and clear the screen for mpv.
    fn suspend(&mut self, app: &mut App) -> io::Result<()>;
    /// Turn raw mode on if something turned it off, so that Ctrl+C cannot stop viewr.
    fn ensure_raw_mode(&mut self) -> io::Result<()>;
    /// Delete the last picture of mpv, stop mouse tracking, hide the cursor, clear the screen.
    fn resume(&mut self, app: &mut App) -> io::Result<()>;
    /// Drop the input that is in the queue after mpv. Returns true if it held a resize.
    fn discard_input(&mut self) -> io::Result<bool>;
}

impl TerminalHandoff for Terminal<CrosstermBackend<io::Stdout>> {
    fn geometry(&self, cell_px: (u32, u32)) -> Option<mpv::Geometry> {
        let size = crossterm::terminal::window_size().ok()?;
        mpv::fallback_geometry((size.columns, size.rows, size.width, size.height), cell_px)
    }

    fn longest_side(&self, cell_px: (u32, u32)) -> u32 {
        crossterm::terminal::window_size().map_or(FALLBACK_SIDE, |size| {
            mpv::longest_side((size.columns, size.rows, size.width, size.height), cell_px)
        })
    }

    fn suspend(&mut self, app: &mut App) -> io::Result<()> {
        app.graphics_delete_all_to(self.backend_mut())?;
        self.clear()
    }

    fn ensure_raw_mode(&mut self) -> io::Result<()> {
        if crossterm::terminal::is_raw_mode_enabled()? {
            return Ok(());
        }
        enable_raw_mode()
    }

    fn resume(&mut self, app: &mut App) -> io::Result<()> {
        app.graphics_delete_all_to(self.backend_mut())?;
        execute!(self.backend_mut(), event::DisableMouseCapture, cursor::Hide)?;
        self.clear()
    }

    fn discard_input(&mut self) -> io::Result<bool> {
        let mut resized = false;
        while event::poll(LATE_INPUT)? {
            resized |= matches!(event::read()?, Event::Resize(_, _));
        }
        Ok(resized)
    }
}

/// The size of the picture for mpv when the terminal does not tell its size.
const FALLBACK_SIDE: u32 = 1920;

/// How long viewr waits for more input after mpv. Mouse reports and key presses can still be on
/// their way when mpv exits, and must not reach the gallery as keys.
const LATE_INPUT: Duration = Duration::from_millis(20);

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
    let played = match &state.player {
        mpv::Player::Builtin => {
            state.status.clone_from(&state.fallback_note);
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
            state.player = mpv::Player::Builtin;
            state.fallback_note = Some(format!("mpv did not start ({error}): built-in player"));
            state.status.clone_from(&state.fallback_note);
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

/// Act on the exit of an mpv window, and show the note of a failed playback.
pub(crate) fn poll_mpv_window(app: &mut App, state: &mut LoopState) {
    if let Some(exit) = app.poll_video_window()
        && let Some(note) = mpv_note(app, exit.code, &exit.path)
    {
        state.status = Some(note);
    }
}

/// Run mpv in this terminal and wait for its exit code.
///
/// The outer error is a terminal error and ends viewr. The inner error is a failed mpv start.
fn play_in_terminal(
    app: &mut App,
    handoff: &mut impl TerminalHandoff,
    runner: &mut impl mpv::Runner,
    program: &Path,
    args: &[OsString],
) -> io::Result<io::Result<Option<i32>>> {
    if let Err(error) = handoff
        .suspend(app)
        .and_then(|()| handoff.ensure_raw_mode())
    {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "the suspend error matters more than a failed cleanup"
        )]
        let _ = handoff.resume(app);
        return Err(error);
    }
    let played = runner.run(program, args);
    handoff.resume(app)?;
    if handoff.discard_input()? {
        app.mark_dirty();
    }
    Ok(played)
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
    use super::{LoopState, TerminalHandoff, hand_off_video, hand_off_videos, poll_mpv_window};
    use crate::app::{App, VideoReturn, ViewMode};
    use crate::image_list::SharedImageList;
    use crate::keys::fullscreen::handle_fullscreen_key;
    use crate::keys::video::handle_video_key;
    use crate::mpv;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::cell::{Cell, RefCell};
    use std::ffi::OsString;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::mpsc;

    type Log = Rc<RefCell<Vec<String>>>;

    /// A terminal that records each step. `raw` is the raw mode that the runner sees.
    struct FakeTerminal {
        log: Log,
        raw: Rc<Cell<bool>>,
        fail_suspend: bool,
        resized: bool,
    }

    impl TerminalHandoff for FakeTerminal {
        fn geometry(&self, _cell_px: (u32, u32)) -> Option<mpv::Geometry> {
            None
        }

        fn longest_side(&self, _cell_px: (u32, u32)) -> u32 {
            1600
        }

        fn suspend(&mut self, _app: &mut App) -> io::Result<()> {
            self.log.borrow_mut().push("suspend".to_owned());
            if self.fail_suspend {
                return Err(io::Error::other("broken pipe"));
            }
            Ok(())
        }

        fn ensure_raw_mode(&mut self) -> io::Result<()> {
            self.log.borrow_mut().push("raw mode".to_owned());
            self.raw.set(true);
            Ok(())
        }

        fn resume(&mut self, _app: &mut App) -> io::Result<()> {
            self.log.borrow_mut().push("resume".to_owned());
            Ok(())
        }

        fn discard_input(&mut self) -> io::Result<bool> {
            self.log.borrow_mut().push("discard input".to_owned());
            Ok(self.resized)
        }
    }

    /// Records each start; `exit` is the exit code of mpv or the error of the start.
    /// `window` sends the exit code of the last window, so that the test closes it.
    /// `exits` holds the exit codes of the next runs in the terminal; `exit` comes after.
    struct FakeRunner {
        log: Log,
        raw: Rc<Cell<bool>>,
        exit: Result<Option<i32>, io::ErrorKind>,
        exits: Vec<Option<i32>>,
        window: Option<mpsc::Sender<Option<i32>>>,
    }

    impl FakeRunner {
        fn record(&self, verb: &str, program: &Path, args: &[OsString]) -> io::Result<()> {
            let args: Vec<String> = args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            let mode = if self.raw.get() { "raw" } else { "cooked" };
            self.log.borrow_mut().push(format!(
                "{verb} ({mode}) {} {}",
                program.display(),
                args.join(" ")
            ));
            self.exit.map(drop).map_err(io::Error::from)
        }
    }

    impl mpv::Runner for FakeRunner {
        fn run(&mut self, program: &Path, args: &[OsString]) -> io::Result<Option<i32>> {
            self.record("run", program, args)?;
            if !self.exits.is_empty() {
                return Ok(self.exits.remove(0));
            }
            Ok(self.exit?)
        }

        fn spawn(
            &mut self,
            program: &Path,
            args: &[OsString],
        ) -> io::Result<mpsc::Receiver<Option<i32>>> {
            self.record("spawn", program, args)?;
            let (exit_tx, exit_rx) = mpsc::channel();
            self.window = Some(exit_tx);
            Ok(exit_rx)
        }
    }

    /// An app with a photo and a video, in video mode for the video.
    fn video_app() -> App {
        let mut app = App::new((8, 16), SharedImageList::new());
        app.images = vec![
            PathBuf::from("/videos/a.jpg"),
            PathBuf::from("/videos/clip.mp4"),
        ];
        app.filenames = vec!["a.jpg".to_owned(), "clip.mp4".to_owned()];
        app.gallery.filtered_indices = vec![0, 1];
        app.enter_video(1);
        app
    }

    /// An app in the fullscreen view that steps from a photo onto a video.
    fn fullscreen_video_app() -> App {
        fullscreen_app_on_video(&["a.jpg", "clip.mp4", "b.jpg"], false)
    }

    /// An app in a running slideshow that steps from a photo onto a video.
    fn slideshow_video_app() -> App {
        fullscreen_app_on_video(&["a.jpg", "clip.mp4", "b.jpg"], true)
    }

    /// An app with `names` below `/videos` that steps from the first file onto the second,
    /// a video, in the fullscreen view. `slideshow` starts a slideshow on the first file.
    fn fullscreen_app_on_video(names: &[&str], slideshow: bool) -> App {
        let mut app = App::new((8, 16), SharedImageList::new());
        app.images = names
            .iter()
            .map(|name| Path::new("/videos").join(name))
            .collect();
        app.filenames = names.iter().map(|name| (*name).to_owned()).collect();
        app.gallery.filtered_indices = (0..names.len()).collect();
        app.enter_fullscreen_at(0);
        if slideshow {
            app.start_slideshow();
        }
        app.next();
        assert_eq!(app.mode, ViewMode::Video);
        app
    }

    fn state(player: mpv::Player) -> LoopState {
        LoopState {
            status: None,
            player,
            fallback_note: None,
        }
    }

    /// The fakes start in cooked mode, so the log shows that the hand-off turns raw mode on.
    fn fakes(exit: Result<Option<i32>, io::ErrorKind>) -> (Log, FakeTerminal, FakeRunner) {
        let log = Log::default();
        let raw = Rc::new(Cell::new(false));
        let terminal = FakeTerminal {
            log: Rc::clone(&log),
            raw: Rc::clone(&raw),
            fail_suspend: false,
            resized: false,
        };
        let runner = FakeRunner {
            log: Rc::clone(&log),
            raw,
            exit,
            exits: Vec::new(),
            window: None,
        };
        (log, terminal, runner)
    }

    fn terminal_state() -> LoopState {
        state(mpv::Player::Terminal(PathBuf::from("/bin/mpv")))
    }

    #[test]
    fn terminal_player_runs_mpv_in_raw_mode_and_returns_to_the_gallery() {
        let mut app = video_app();
        let mut state = terminal_state();
        let (log, mut terminal, mut runner) = fakes(Ok(Some(0)));

        hand_off_video(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        assert_eq!(
            *log.borrow(),
            [
                "suspend",
                "raw mode",
                "run (raw) /bin/mpv --vo=kitty --vo-kitty-alt-screen=no --profile=sw-fast \
                 --hwdec=auto-copy-safe --really-quiet --terminal=yes --input-terminal=yes \
                 --vf-pre=lavfi=[scale=w=min(iw\\,1600):h=min(ih\\,1600)\
                 :force_original_aspect_ratio=decrease:force_divisible_by=2:flags=bilinear] \
                 --loop-file=inf --input-commands=keybind n \"quit 10\",keybind N \"quit 11\" \
                 -- /videos/clip.mp4",
                "resume",
                "discard input",
            ]
        );
        assert_eq!(app.mode, ViewMode::Gallery);
        assert_eq!(app.gallery.cursor, 1, "the cursor is on the video");
        assert!(app.needs_render);
        assert!(!app.graphics_storage_dirty, "no resize, no full redraw");
        assert_eq!(state.status, None);
    }

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

    fn window_state() -> LoopState {
        state(mpv::Player::Window(PathBuf::from("/bin/mpv")))
    }

    /// The arguments of the last mpv start in `log`.
    fn last_start(log: &Log) -> String {
        log.borrow()
            .iter()
            .rev()
            .find(|entry| entry.starts_with("run ") || entry.starts_with("spawn "))
            .cloned()
            .unwrap()
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
    fn a_resize_during_playback_marks_the_thumbnails_dirty() {
        let mut app = video_app();
        let mut state = terminal_state();
        let (_log, mut terminal, mut runner) = fakes(Ok(Some(0)));
        terminal.resized = true;

        hand_off_video(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        assert!(app.graphics_storage_dirty);
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
    fn an_mpv_that_does_not_start_gives_the_terminal_back_and_falls_back() {
        let mut app = video_app();
        let mut state = terminal_state();
        let (log, mut terminal, mut runner) = fakes(Err(io::ErrorKind::NotFound));

        hand_off_video(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        assert_eq!(
            log.borrow()[log.borrow().len() - 2..],
            ["resume", "discard input"]
        );
        assert_eq!(app.mode, ViewMode::Video);
        assert_eq!(state.player, mpv::Player::Builtin);
        assert!(
            state
                .status
                .as_deref()
                .is_some_and(|status| status.starts_with("mpv did not start"))
        );
    }

    #[test]
    fn a_failed_suspend_starts_no_mpv_and_keeps_the_player() {
        let mut app = video_app();
        let mut state = terminal_state();
        let (log, mut terminal, mut runner) = fakes(Ok(Some(0)));
        terminal.fail_suspend = true;

        let error = hand_off_video(&mut app, &mut state, &mut terminal, &mut runner)
            .expect_err("a terminal error ends the run loop");

        assert_eq!(error.to_string(), "broken pipe");
        assert_eq!(*log.borrow(), ["suspend", "resume"]);
        assert_eq!(state.player, terminal_state().player);
        assert_eq!(state.fallback_note, None);
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
    fn builtin_player_starts_nothing_and_shows_the_fallback_note() {
        let mut app = video_app();
        let mut state = state(mpv::Player::Builtin);
        state.fallback_note = Some(mpv::MISSING_NOTE.to_owned());
        let (log, mut terminal, mut runner) = fakes(Ok(Some(0)));

        hand_off_video(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        assert!(log.borrow().is_empty());
        assert_eq!(app.mode, ViewMode::Video);
        assert_eq!(state.status.as_deref(), Some(mpv::MISSING_NOTE));
    }

    /// An app with `names` below `/videos` in the gallery, in the video view of `index`.
    fn gallery_app_on_video(names: &[&str], index: usize) -> App {
        let mut app = App::new((8, 16), SharedImageList::new());
        app.images = names
            .iter()
            .map(|name| Path::new("/videos").join(name))
            .collect();
        app.filenames = names.iter().map(|name| (*name).to_owned()).collect();
        app.gallery.filtered_indices = (0..names.len()).collect();
        app.enter_video(index);
        app
    }

    /// The mpv starts in `log`.
    fn starts(log: &Log) -> Vec<String> {
        log.borrow()
            .iter()
            .filter(|entry| entry.starts_with("run ") || entry.starts_with("spawn "))
            .cloned()
            .collect()
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
