//! The event loop: input, the background polls, the draw, the Kitty writes and the wait.

use crate::app::{App, ViewMode};
use crate::config::Config;
use crate::keys::{LateAnswerFilter, handle_event};
#[cfg(feature = "video")]
use crate::kitty_output::render_video_frame;
use crate::kitty_output::{
    emit_new_thumbnails, play_grid_loops, render_fullscreen_image, render_gallery_images,
    sync_grid_icons,
};
#[cfg(feature = "video")]
use crate::mpv;
use crate::pictures::kitty::answers::AnswerKeys;
use crate::pictures::kitty::query::query_cell_pixel_size;
use crate::terminal::{disable_mouse, enable_mouse};
use crate::ui;
#[cfg(feature = "video")]
use crate::video_handoff::{VideoState, play_videos, poll_mpv_window};
use crossterm::event::{self, Event, KeyEventKind, MouseEventKind};
use ratatui::prelude::*;
use std::io::{self, Write};
use std::time::{Duration, Instant};

/// The wait while a background result is on its way.
const BUSY_POLL: Duration = Duration::from_millis(10);
/// The wait during the first `SHAPE_POLL_WINDOW` of the cell shape read. A local read takes
/// 2 ms to 4 ms, and the first screen waits for it.
const SHAPE_POLL: Duration = Duration::from_millis(1);
/// After this time, the shape read is slow (a network share), and the loop waits `BUSY_POLL`.
const SHAPE_POLL_WINDOW: Duration = Duration::from_millis(20);
/// The wait while a disk cache read of the grid runs. A read takes about 1 ms.
const CACHE_READ_POLL: Duration = Duration::from_millis(2);
/// The wait until the first grid picture. A cold grid shows it after 15 ms to 25 ms.
const FIRST_PICTURE_POLL: Duration = Duration::from_millis(2);
const IDLE_POLL: Duration = Duration::from_millis(250);
/// The wait while an mpv window plays: the latency of `n` and `N` in the window.
#[cfg(feature = "video")]
pub(crate) const WINDOW_POLL: Duration = Duration::from_millis(100);

/// The state of the run loop that `App` does not hold.
pub(crate) struct LoopState {
    /// A one-line note for the status row. The next key press clears it.
    pub(crate) status: Option<String>,
    pub(crate) late_answers: LateAnswerFilter,
    /// The answers of the terminal for the self check of the screen, read from the keys.
    pub(crate) answers: AnswerKeys,
    /// `[gallery] mouse`.
    pub(crate) mouse: bool,
    /// True while the terminal reports the mouse.
    pub(crate) mouse_captured: bool,
    /// True when the terminal answered the cell size query at start. A resize then asks again.
    pub(crate) cell_size_from_terminal: bool,
    #[cfg(feature = "video")]
    pub(crate) video: VideoState,
}

impl LoopState {
    pub(crate) fn new(config: &Config) -> Self {
        Self {
            status: None,
            late_answers: LateAnswerFilter::default(),
            answers: AnswerKeys::default(),
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
        // Drain the buffered input first. Navigation never blocks.
        while event::poll(Duration::ZERO)? {
            let event = event::read()?;
            if handle_input(app, state, &event)? {
                return Ok(());
            }
        }
        if release_held(app, state, Instant::now())? {
            return Ok(());
        }

        #[cfg(feature = "video")]
        poll_mpv_window(app, state);
        sync_mouse_capture(terminal.backend_mut(), state, window_open(app))?;
        poll_background(app, &mut pending_emits);

        // The player starts before the draw. A fallback view then comes back in this pass.
        #[cfg(feature = "video")]
        play_videos(app, state, &mut mpv::SystemRunner);

        terminal.draw(|frame| draw_ui(frame, app, state))?;

        if render_images(app, &mut pending_emits)? {
            terminal.draw(|frame| draw_ui(frame, app, state))?;
        }
        // After the render: the grid jobs of this pass pause the folder prefetch.
        app.tick_folder_prefetch(Instant::now());

        let timeout = next_timeout(app, &pending_emits);
        let wait = state.answers.wait(Instant::now());
        event::poll(wait.map_or(timeout, |wait| timeout.min(wait)))?;
    }
}

/// Take one input event. Returns `true` when the event quits phewr.
///
/// The Kitty answers go first, also a late probe answer. The answers of the self check go to
/// the screen, and the next grid pass sends a lost picture again. An event that can belong to
/// an answer waits for the next event or for [`release_held`].
fn handle_input(app: &mut App, state: &mut LoopState, event: &Event) -> io::Result<bool> {
    let read = state.answers.read(event.clone(), Instant::now());
    state.late_answers.kitty_answers_came(read.ends);
    for answer in &read.answers {
        app.screen.answer(answer);
    }
    handle_events(app, state, &read.events)
}

/// Ends a hold of key events that waited `ANSWER_WAIT` for the end of an answer, after the
/// input went quiet. Returns `true` when an event that comes back quits phewr.
fn release_held(app: &mut App, state: &mut LoopState, now: Instant) -> io::Result<bool> {
    let events = state.answers.release_stale(now);
    handle_events(app, state, &events)
}

/// The events that belong to no Kitty answer: a late OSC answer goes, the rest goes to the app.
fn handle_events(app: &mut App, state: &mut LoopState, events: &[Event]) -> io::Result<bool> {
    for event in events {
        if !state.late_answers.swallows(event) && handle_app_event(app, state, event)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Take one event of the user or the terminal. Returns `true` when the event quits phewr.
fn handle_app_event(app: &mut App, state: &mut LoopState, event: &Event) -> io::Result<bool> {
    if clears_status(event) {
        state.status = None;
        app.scan_error = None;
    }
    if matches!(event, Event::Resize(..)) && state.cell_size_from_terminal {
        // A new font gives new cells. A terminal that answered at start answers again without a
        // timeout.
        if let Some(cell_px) = query_cell_pixel_size() {
            app.set_cell_px(cell_px);
        }
    }
    handle_event(app, event)
}

/// True for a key press and a mouse button press. A key release or repeat keeps the note.
fn clears_status(event: &Event) -> bool {
    match event {
        Event::Key(key) => key.kind == KeyEventKind::Press,
        Event::Mouse(mouse) => matches!(mouse.kind, MouseEventKind::Down(_)),
        _ => false,
    }
}

/// True while an mpv window plays. The mouse then belongs to the window.
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

fn mouse_capture_wanted(enabled: bool, window_open: bool) -> bool {
    enabled && !window_open
}

/// Capture or release the mouse when the wanted state changed.
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

/// Draw the chrome and remember the fullscreen box for the gallery pre-decode.
fn draw_ui(frame: &mut Frame<'_>, app: &mut App, state: &LoopState) {
    ui::draw(frame, app, state.status.as_deref());
    app.fullscreen_box = fullscreen_box(frame.area(), app);
}

fn fullscreen_box(area: Rect, app: &App) -> Rect {
    let (main, _info) = ui::split_info(area, app);
    ui::content_area(main, app)
}

/// Take the results of the background work.
pub(crate) fn poll_background(app: &mut App, pending_emits: &mut Vec<usize>) {
    app.refresh_from_scanner();
    app.maybe_autostart_slideshow();
    app.poll_filter();
    app.poll_meta();
    app.request_meta();
    app.poll_cell_shape();
    app.poll_fullscreen();
    app.poll_source();
    app.prefetcher.poll();
    pending_emits.extend(app.poll_thumbnails());
    app.poll_grid_loops();
    app.slideshow_tick();

    // No frame moves while the help popup is open above the pictures.
    #[cfg(feature = "video")]
    if !app.help_visible
        && let Some(ref mut v) = app.video
        && v.poll()
    {
        app.needs_render = true;
    }
}

/// Write the Kitty images of the view. `true` asks for a second draw: the pictures of a full
/// render cover the text.
pub(crate) fn render_images(app: &mut App, pending_emits: &mut Vec<usize>) -> io::Result<bool> {
    // The key that closes the help renders the view again. No picture goes out while the help is
    // open, not even a late thumbnail.
    if app.help_visible {
        pending_emits.clear();
        return Ok(false);
    }
    // The grid writes all thumbnails again when it returns.
    if app.mode != ViewMode::Gallery {
        pending_emits.clear();
    }
    let draws = app.needs_render && !app.images.is_empty();
    let chrome_again = match app.mode {
        ViewMode::Fullscreen if draws => {
            app.load_if_needed();
            render_fullscreen_image(app)?;
            // The neighbors wait for the image on the screen. Their decodes must not take the cores from
            // the image that the user waits for.
            if app.loaded.is_some() {
                app.prefetch_neighbors();
            }
            finish_render(app, pending_emits);
            true
        }
        ViewMode::Gallery if draws => {
            render_gallery_images(app)?;
            finish_render(app, pending_emits);
            true
        }
        #[cfg(feature = "video")]
        ViewMode::Video if draws => {
            render_video_frame(app)?;
            finish_render(app, pending_emits);
            false
        }
        ViewMode::Gallery if !pending_emits.is_empty() => {
            emit_new_thumbnails(app, pending_emits)?;
            false
        }
        // The loops write in a pass without new thumbnails.
        ViewMode::Gallery => play_grid_loops(app, Instant::now(), pending_emits)?,
        _ => false,
    };
    // A pass without the icons of the last draw writes them. A cell that got its picture loses
    // its icon in the same pass.
    sync_grid_icons(app, pending_emits)?;
    Ok(chrome_again)
}

fn finish_render(app: &mut App, pending_emits: &mut Vec<usize>) {
    app.needs_render = false;
    pending_emits.clear();
}

/// How long the loop waits for the next event.
///
/// No event wakes the loop when a decode finishes. The wait is the latency of every background
/// result. A running grid loop job alone gives no busy poll. While the player runs, the frame
/// clock sets the wait, and background results wait for the next frame.
pub(crate) fn next_timeout(app: &App, pending_emits: &[usize]) -> Duration {
    if app.mode == ViewMode::Gallery && (!pending_emits.is_empty() || app.loops_to_send()) {
        return Duration::ZERO;
    }
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
    if waits_for_a_fast_shape(app.cell_shape_running_for(Instant::now())) {
        timeout = timeout.min(SHAPE_POLL);
    }
    if app.cache_reads_pending() {
        timeout = timeout.min(CACHE_READ_POLL);
    }
    if app.waits_for_the_first_picture() {
        timeout = timeout.min(FIRST_PICTURE_POLL);
    }
    if let Some(remaining) = app.slideshow_timeout() {
        timeout = timeout.min(remaining);
    }
    if let Some(remaining) = app.folder_prefetch_timeout(Instant::now()) {
        timeout = timeout.min(remaining);
    }
    if let Some(remaining) = app.grid_loop_timeout(Instant::now()) {
        timeout = timeout.min(remaining);
    }
    #[cfg(feature = "video")]
    if app.watches_video_window() {
        timeout = timeout.min(WINDOW_POLL);
    }
    #[cfg(feature = "video")]
    if let Some(remaining) = crate::video_handoff::prestart_timeout(app, Instant::now()) {
        timeout = timeout.min(remaining);
    }
    timeout
}

fn waits_for_a_fast_shape(running: Option<Duration>) -> bool {
    running.is_some_and(|running| running < SHAPE_POLL_WINDOW)
}

#[cfg(test)]
mod tests;
