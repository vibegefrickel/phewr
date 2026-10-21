//! The event loop: input, the background polls, the draw, the Kitty writes and the wait.

use crate::app::{App, ViewMode};
use crate::config;
use crate::keys::handle_event;
#[cfg(feature = "video")]
use crate::kitty_output::render_video_frame;
use crate::kitty_output::{emit_new_thumbnails, render_fullscreen_image, render_gallery_images};
#[cfg(feature = "video")]
use crate::mpv;
use crate::ui;
#[cfg(feature = "video")]
use crate::video_handoff::{hand_off_videos, poll_mpv_window};
#[cfg(feature = "video")]
use crate::yazi::find_on_path;
use crossterm::event::{self, Event};
use ratatui::prelude::*;
use std::io;
use std::time::Duration;

/// The wait of the loop while a background result is on its way.
const BUSY_POLL: Duration = Duration::from_millis(10);
/// The wait of the loop when nothing runs in the background.
const IDLE_POLL: Duration = Duration::from_millis(250);
/// The wait of the loop while an mpv window plays: the latency of `n` and `N` in the window.
#[cfg(feature = "video")]
pub(crate) const WINDOW_POLL: Duration = Duration::from_millis(100);

/// The state of the run loop that `App` does not hold.
pub(crate) struct LoopState {
    /// A one-line note for the status bar. The next key press clears it.
    pub(crate) status: Option<String>,
    /// The video player after the PATH lookup for mpv.
    #[cfg(feature = "video")]
    pub(crate) player: mpv::Player,
    /// The note that tells why the built-in player plays instead of mpv.
    #[cfg(feature = "video")]
    pub(crate) fallback_note: Option<String>,
}

impl LoopState {
    /// Without the video feature, the config holds nothing for the run loop.
    #[cfg(not(feature = "video"))]
    pub(crate) fn new(_config: &config::Config) -> Self {
        Self { status: None }
    }

    /// Look up mpv once at the start, so that a missing mpv does not cost a terminal hand-off.
    #[cfg(feature = "video")]
    pub(crate) fn new(config: &config::Config) -> Self {
        let player = mpv::resolve(
            config.video_player,
            find_on_path("mpv", std::env::var_os("PATH").as_deref()),
        );
        Self {
            status: None,
            fallback_note: (player == mpv::Player::Builtin
                && config.video_player != config::VideoPlayer::Builtin)
                .then(|| mpv::MISSING_NOTE.to_owned()),
            player,
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
            if matches!(event, Event::Key(_)) {
                state.status = None;
            }
            if handle_event(app, &event)? {
                return Ok(());
            }
        }

        // 2. Poll background tasks
        #[cfg(feature = "video")]
        poll_mpv_window(app, state);
        poll_background(app, &mut pending_emits);

        // mpv takes the video before the draw, so that the gallery comes back in this pass.
        #[cfg(feature = "video")]
        hand_off_videos(app, state, terminal, &mut mpv::SystemRunner)?;

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

    #[cfg(feature = "video")]
    if let Some(ref mut v) = app.video
        && v.poll_frame()
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
                app.prefetcher.set_target_hint(app.image_rect, app.cell_px);
                app.prefetcher.kick(app.current, &app.images);
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
            app.open_video_if_needed();
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
    use super::LoopState;
    use crate::config::Config;

    #[test]
    fn the_loop_state_starts_without_a_status() {
        let config = Config {
            video_player: crate::config::VideoPlayer::Builtin,
            ..Config::default()
        };

        let state = LoopState::new(&config);

        assert_eq!(state.status, None);
        #[cfg(feature = "video")]
        {
            assert_eq!(state.player, crate::mpv::Player::Builtin);
            assert_eq!(
                state.fallback_note, None,
                "the user chose the built-in player"
            );
        }
    }
}
