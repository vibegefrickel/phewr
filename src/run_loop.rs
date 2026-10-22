//! The event loop: input, the background polls, the draw, the Kitty writes and the wait.

use crate::app::{App, ViewMode};
use crate::config::Config;
use crate::keys::{LateAnswerFilter, handle_event};
#[cfg(feature = "video")]
use crate::kitty_output::render_video_frame;
use crate::kitty_output::{emit_new_thumbnails, render_fullscreen_image, render_gallery_images};
#[cfg(feature = "video")]
use crate::mpv;
use crate::terminal::{disable_mouse, enable_mouse, query_cell_pixel_size};
use crate::ui;
#[cfg(feature = "video")]
use crate::video_handoff::{VideoState, play_videos, poll_mpv_window};
use crossterm::event::{self, Event, MouseEventKind};
use ratatui::prelude::*;
use std::io::{self, Write};
use std::time::Duration;

/// The wait of the loop while a background result is on its way.
const BUSY_POLL: Duration = Duration::from_millis(10);
/// The wait of the loop when nothing runs in the background.
const IDLE_POLL: Duration = Duration::from_millis(250);
/// The wait of the loop while an mpv window plays: the latency of `n` and `N` in the window.
/// The terminal player sets its own wait, see [`crate::player::Player::time_until_next_frame`].
#[cfg(feature = "video")]
pub(crate) const WINDOW_POLL: Duration = Duration::from_millis(100);

/// The state of the run loop that `App` does not hold.
pub(crate) struct LoopState {
    /// A one-line note for the status bar. The next key press clears it.
    pub(crate) status: Option<String>,
    /// Drops the answer of a terminal that answered the Kitty probe too late.
    pub(crate) late_answers: LateAnswerFilter,
    /// `[gallery] mouse`: the terminal reports the mouse while nothing else needs it.
    pub(crate) mouse: bool,
    /// True while the terminal reports the mouse. The session captured it at the start.
    pub(crate) mouse_captured: bool,
    /// True when the terminal answered the cell size query at start, so a resize asks again.
    pub(crate) cell_size_from_terminal: bool,
    #[cfg(feature = "video")]
    pub(crate) video: VideoState,
}

impl LoopState {
    /// The video player comes from the config. Without the video feature, nothing reads it.
    pub(crate) fn new(config: &Config) -> Self {
        Self {
            status: None,
            late_answers: LateAnswerFilter::default(),
            cell_size_from_terminal: false,
            mouse: config.mouse,
            mouse_captured: config.mouse,
            #[cfg(feature = "video")]
            video: VideoState::new(config),
        }
    }
}

pub(crate) fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    state: &mut LoopState,
) -> io::Result<()> {
    let mut pending_emits: Vec<usize> = Vec::new();

    loop {
        // 1. Drain all buffered input first so navigation is never blocked
        while event::poll(Duration::ZERO)? {
            let event = event::read()?;
            if state.late_answers.swallows(&event) {
                continue;
            }
            if matches!(event, Event::Key(_))
                || matches!(event, Event::Mouse(mouse) if matches!(mouse.kind, MouseEventKind::Down(_)))
            {
                state.status = None;
            }
            if matches!(event, Event::Resize(..)) && state.cell_size_from_terminal {
                // A new font gives new cells. The thumbnails follow. A terminal that answered
                // at start answers again at once, so the loop never waits for the timeout.
                if let Some(cell_px) = query_cell_pixel_size() {
                    app.set_cell_px(cell_px);
                }
            }
            if handle_event(app, &event)? {
                return Ok(());
            }
        }

        // 2. Poll background tasks
        #[cfg(feature = "video")]
        poll_mpv_window(app, state);
        sync_mouse_capture(terminal.backend_mut(), state, window_open(app))?;
        poll_background(app, &mut pending_emits);

        // The player starts before the draw, so that a fallback view comes back in this pass.
        #[cfg(feature = "video")]
        play_videos(app, state, &mut mpv::SystemRunner);

        if app.scan_complete
            && app.images.is_empty()
            && app.error.is_none()
            && app.mode != ViewMode::Picker
        {
            app.open_picker();
        }

        // 3. Draw UI chrome (always fast — ratatui only)
        terminal.draw(|frame| draw_ui(frame, app, state))?;

        // 4. Render Kitty images
        if render_images(app, &mut pending_emits)? {
            terminal.draw(|frame| draw_ui(frame, app, state))?;
        }

        // 5. Wait for next event (shorter timeout when thumbnails are pending)
        event::poll(next_timeout(app, &pending_emits))?;
    }
}

/// True while an mpv window plays. The mouse then belongs to the window, not to viewr.
fn window_open(app: &App) -> bool {
    #[cfg(feature = "video")]
    {
        app.watches_video_window()
    }
    #[cfg(not(feature = "video"))]
    {
        let _ = app;
        false
    }
}

/// Whether the terminal must report the mouse: the config allows it and no mpv window plays.
pub(crate) fn mouse_capture_wanted(enabled: bool, window_open: bool) -> bool {
    enabled && !window_open
}

/// Capture or release the mouse when the wanted state changed since the last loop.
pub(crate) fn sync_mouse_capture(
    out: &mut impl Write,
    state: &mut LoopState,
    window_open: bool,
) -> io::Result<()> {
    let wanted = mouse_capture_wanted(state.mouse, window_open);
    if wanted == state.mouse_captured {
        return Ok(());
    }
    if wanted {
        enable_mouse(out)?;
    } else {
        disable_mouse(out)?;
    }
    state.mouse_captured = wanted;
    Ok(())
}

/// Draw the chrome and remember the fullscreen box, so that the gallery pre-decode knows the size.
fn draw_ui(frame: &mut Frame<'_>, app: &mut App, state: &LoopState) {
    ui::draw(frame, app, state.status.as_deref());
    app.fullscreen_box = fullscreen_box(frame.area(), app);
}

/// The box of the fullscreen image in `area`: next to the info panel, above the status row.
fn fullscreen_box(area: Rect, app: &App) -> Rect {
    let (main, _info) = ui::split_info(area, app);
    ui::content_area(main)
}

/// Take the results of the background work: the scan, the filter, the decodes and the clock.
pub(crate) fn poll_background(app: &mut App, pending_emits: &mut Vec<usize>) {
    app.refresh_from_scanner();
    app.maybe_autostart_slideshow();
    app.poll_filter();
    app.poll_cell_shape();
    app.poll_fullscreen();
    app.poll_source();
    app.prefetcher.poll();
    pending_emits.extend(app.poll_thumbnails());
    app.slideshow_tick();

    // The help popup sits above the pictures, so no frame moves while it is open.
    #[cfg(feature = "video")]
    if !app.help_visible
        && let Some(ref mut v) = app.video
        && v.poll()
    {
        app.needs_render = true;
    }
}

/// Write the Kitty images of the view. `true` means that the chrome needs a second draw,
/// because the pictures of a full render cover the text.
pub(crate) fn render_images(app: &mut App, pending_emits: &mut Vec<usize>) -> io::Result<bool> {
    let draws = app.needs_render && !app.images.is_empty();
    match app.mode {
        ViewMode::Picker => finish_render(app, pending_emits),
        ViewMode::Fullscreen if draws => {
            app.load_if_needed();
            render_fullscreen_image(app)?;
            // The neighbors wait until the image is on the screen, so that their decodes
            // do not take the cores from the one that the user waits for.
            if app.loaded.is_some() {
                app.prefetch_neighbors();
            }
            finish_render(app, pending_emits);
            return Ok(true);
        }
        ViewMode::Gallery if draws => {
            render_gallery_images(app)?;
            finish_render(app, pending_emits);
            return Ok(true);
        }
        #[cfg(feature = "video")]
        ViewMode::Video if draws => {
            render_video_frame(app)?;
            finish_render(app, pending_emits);
        }
        ViewMode::Gallery if !pending_emits.is_empty() => {
            let count = pending_emits.len().min(4);
            let batch: Vec<usize> = pending_emits.drain(..count).collect();
            emit_new_thumbnails(app, &batch)?;
        }
        _ => {}
    }
    Ok(false)
}

/// The pictures of the view are out: nothing waits for a render or an emit.
fn finish_render(app: &mut App, pending_emits: &mut Vec<usize>) {
    app.needs_render = false;
    pending_emits.clear();
}

/// How long the loop waits for the next event.
///
/// No event wakes the loop when a decode finishes, so the wait is the latency of every
/// background result. While a result is on its way, or thumbnails wait for their emit, the
/// loop looks every 10 ms. Otherwise it looks every 250 ms.
pub(crate) fn next_timeout(app: &App, pending_emits: &[usize]) -> Duration {
    let busy = app.has_pending_work() || !pending_emits.is_empty();
    let mut timeout = {
        #[cfg(feature = "video")]
        if let Some(v) = &app.video {
            v.time_until_next_frame()
        } else if busy {
            BUSY_POLL
        } else {
            IDLE_POLL
        }
        #[cfg(not(feature = "video"))]
        if busy { BUSY_POLL } else { IDLE_POLL }
    };
    if let Some(remaining) = app.slideshow_timeout() {
        timeout = timeout.min(remaining);
    }
    #[cfg(feature = "video")]
    if app.watches_video_window() {
        timeout = timeout.min(WINDOW_POLL);
    }
    timeout
}

#[cfg(test)]
mod tests {
    use super::{LoopState, mouse_capture_wanted, sync_mouse_capture};
    use crate::config::Config;

    #[test]
    fn the_loop_releases_the_mouse_while_a_window_plays_and_takes_it_back() {
        let mut state = LoopState::new(&Config::default());
        assert!(state.mouse_captured);
        let mut out = Vec::new();

        sync_mouse_capture(&mut out, &mut state, false).unwrap();
        assert!(out.is_empty(), "nothing changed");
        sync_mouse_capture(&mut out, &mut state, true).unwrap();
        assert!(!state.mouse_captured);
        assert!(
            String::from_utf8_lossy(&out).contains("\x1b[?1000l"),
            "{out:?}"
        );
        out.clear();
        sync_mouse_capture(&mut out, &mut state, true).unwrap();
        assert!(out.is_empty(), "released once");
        sync_mouse_capture(&mut out, &mut state, false).unwrap();
        assert!(state.mouse_captured);
        assert!(
            String::from_utf8_lossy(&out).contains("\x1b[?1000h"),
            "{out:?}"
        );
    }

    #[test]
    fn without_the_mouse_key_the_loop_never_writes_a_capture() {
        let config = Config {
            mouse: false,
            ..Config::default()
        };
        let mut state = LoopState::new(&config);
        let mut out = Vec::new();
        for window in [false, true, false] {
            sync_mouse_capture(&mut out, &mut state, window).unwrap();
        }
        assert!(out.is_empty());
        assert!(!mouse_capture_wanted(false, false));
        assert!(mouse_capture_wanted(true, false));
        assert!(!mouse_capture_wanted(true, true));
    }

    #[cfg(feature = "video")]
    #[test]
    fn the_help_popup_holds_the_player_and_the_frame_write() {
        use crate::app::ViewMode;
        use crate::player::Player;
        use crate::test_support::video::{clip_info, fake_decoder, shared, video_app};
        use std::path::Path;

        let mut app = video_app();
        app.image_rect = ratatui::layout::Rect::new(0, 0, 100, 40);
        let decoder = fake_decoder(clip_info());
        app.video = Some(
            Player::open(
                shared(&decoder),
                Path::new("/bin/ffmpeg"),
                Path::new("/videos/clip.mp4"),
                (64, 48),
                &[],
                true,
            )
            .unwrap(),
        );
        assert!(!crate::test_support::char_key(&mut app, '?'));
        assert!(app.help_visible);
        assert!(
            crate::test_support::text(&app.graphics_out.take_bytes()).contains("a=d"),
            "the help deletes the pictures first"
        );
        decoder.feed(vec![1; 64 * 48 * 4]);
        app.needs_render = false;
        let mut pending = Vec::new();

        super::poll_background(&mut app, &mut pending);
        assert!(!app.needs_render, "no frame while the help is open");
        assert!(app.video.as_ref().unwrap().current_frame.is_none());
        app.needs_render = true;
        super::render_images(&mut app, &mut pending).unwrap();
        assert!(app.graphics_out.take_bytes().is_empty());

        assert!(!crate::test_support::char_key(&mut app, 'x'));
        assert!(!app.help_visible);
        assert_eq!(app.mode, ViewMode::Video);
        super::poll_background(&mut app, &mut pending);
        assert!(app.needs_render, "the frame comes after the help");
        super::render_images(&mut app, &mut pending).unwrap();
        let out = app.graphics_out.take_bytes();
        for path in crate::encoder::kitty::temp_file_paths(&out) {
            std::fs::remove_file(path).unwrap();
        }
        assert!(crate::test_support::text(&out).contains("a=T,"));
    }

    #[test]
    fn the_loop_state_starts_without_a_status() {
        let config = Config {
            video_player: crate::config::VideoPlayer::Window,
            ..Config::default()
        };

        let state = LoopState::new(&config);

        assert_eq!(state.status, None);
        #[cfg(feature = "video")]
        assert_eq!(state.video.player, crate::config::VideoPlayer::Window);
    }
}
