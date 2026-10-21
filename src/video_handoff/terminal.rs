//! mpv in this terminal: the hand-off of raw mode and the alternate screen, and the way back.

use crate::app::App;
use crate::mpv;
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

/// Run mpv in this terminal and wait for its exit code.
///
/// The outer error is a terminal error and ends viewr. The inner error is a failed mpv start.
pub(super) fn play_in_terminal(
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

#[cfg(test)]
mod tests {
    use crate::app::ViewMode;
    use crate::mpv;
    use crate::test_support::video::{fakes, terminal_state, video_app};
    use crate::video_handoff::hand_off_video;
    use std::io;

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
    fn a_resize_during_playback_marks_the_thumbnails_dirty() {
        let mut app = video_app();
        let mut state = terminal_state();
        let (_log, mut terminal, mut runner) = fakes(Ok(Some(0)));
        terminal.resized = true;

        hand_off_video(&mut app, &mut state, &mut terminal, &mut runner).unwrap();

        assert!(app.graphics_storage_dirty);
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
        assert_eq!(state.video.player, mpv::Player::Builtin);
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
        assert_eq!(state.video.player, terminal_state().video.player);
        assert_eq!(state.video.fallback_note, None);
    }
}
