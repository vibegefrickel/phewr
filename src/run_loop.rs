//! The event loop: input, the background polls, the draw, the Kitty writes and the wait.

use crate::app::{App, ViewMode};
use crate::config::Config;
use crate::keys::{LateAnswerFilter, handle_event};
#[cfg(feature = "video")]
use crate::kitty_output::render_video_frame;
use crate::kitty_output::{
    emit_new_thumbnails, play_grid_loops, render_fullscreen_image, render_gallery_images,
};
#[cfg(feature = "video")]
use crate::mpv;
use crate::terminal::{disable_mouse, enable_mouse, query_cell_pixel_size};
use crate::ui;
#[cfg(feature = "video")]
use crate::video_handoff::{VideoState, play_videos, poll_mpv_window};
use crossterm::event::{self, Event, MouseEventKind};
use ratatui::prelude::*;
use std::io::{self, Write};
use std::time::{Duration, Instant};

/// The wait of the loop while a background result is on its way.
const BUSY_POLL: Duration = Duration::from_millis(10);
/// The wait of the loop during the first `SHAPE_POLL_WINDOW` of the header read for the cell
/// shape. A local read takes 2 ms to 4 ms, and the first screen of a folder waits for it.
const SHAPE_POLL: Duration = Duration::from_millis(1);
/// After this time, the header read is slow (a network share, an external disk), and the loop
/// waits `BUSY_POLL` again, so that it does not draw 1000 times per second.
const SHAPE_POLL_WINDOW: Duration = Duration::from_millis(20);
/// The wait of the loop while a disk cache read of the grid runs or waits. A read takes about
/// 1 ms, so a warm grid has its pictures after 2 ms to 4 ms, not after `BUSY_POLL`.
const CACHE_READ_POLL: Duration = Duration::from_millis(2);
/// The wait of the loop while the grid decodes its first picture. A cold grid shows its first
/// thumbnail after 15 ms to 25 ms, so a wait of `BUSY_POLL` would add up to 10 ms to it.
const FIRST_PICTURE_POLL: Duration = Duration::from_millis(2);
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

        // 3. Draw UI chrome (always fast — ratatui only)
        terminal.draw(|frame| draw_ui(frame, app, state))?;

        // 4. Render Kitty images
        if render_images(app, &mut pending_emits)? {
            terminal.draw(|frame| draw_ui(frame, app, state))?;
        }
        // After the render, so that the grid jobs of this pass pause the folder prefetch.
        app.tick_folder_prefetch(Instant::now());

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
    app.poll_meta();
    app.request_meta();
    app.poll_cell_shape();
    app.poll_fullscreen();
    app.poll_source();
    app.prefetcher.poll();
    pending_emits.extend(app.poll_thumbnails());
    app.poll_grid_loops();
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
        ViewMode::Gallery if !pending_emits.is_empty() => emit_new_thumbnails(app, pending_emits)?,
        // The loops wait for the stills: they write in a pass without new thumbnails.
        ViewMode::Gallery => return play_grid_loops(app, Instant::now()),
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
/// loop looks every 10 ms, every 2 ms while a disk cache read of the grid is on its way, and
/// every 1 ms during the first 20 ms of the cell shape read. Otherwise it looks every 250 ms.
/// The thumbnails that one pass of the grid left for the next pass go out without a wait,
/// after the loop read the input, and so do the frames of new grid loops. A playing grid loop
/// wakes the loop at its next frame, not on a short poll.
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
    timeout
}

/// True during the first `SHAPE_POLL_WINDOW` of a header read that runs for `running`.
fn waits_for_a_fast_shape(running: Option<Duration>) -> bool {
    running.is_some_and(|running| running < SHAPE_POLL_WINDOW)
}

#[cfg(test)]
mod tests {
    use super::{
        CACHE_READ_POLL, FIRST_PICTURE_POLL, LoopState, SHAPE_POLL, SHAPE_POLL_WINDOW,
        mouse_capture_wanted, next_timeout, sync_mouse_capture, waits_for_a_fast_shape,
    };
    use crate::app::ViewMode;
    use crate::config::Config;
    use crate::test_support::{app, app_with, app_with_files, test_dir, wait_until};
    use ratatui::layout::Rect;
    use std::time::Duration;

    #[test]
    fn the_emits_that_a_pass_left_go_out_without_a_wait() {
        let mut app = app_with(&["a.png"]);
        app.scan_complete = true;
        assert_eq!(next_timeout(&app, &[0]), Duration::ZERO);
        app.mode = ViewMode::Fullscreen;
        assert!(
            next_timeout(&app, &[0]) > Duration::ZERO,
            "the fullscreen view writes no grid pictures"
        );
    }

    #[test]
    fn the_loop_looks_every_two_milliseconds_while_the_grid_reads_the_disk_cache() {
        let dir = test_dir();
        let names = ["a.png", "b.png", "c.png", "d.png", "e.png"];
        let mut app = app_with_files(dir.path(), &names);
        app.scan_complete = true;
        for index in 0..names.len() {
            app.spawn_thumb_decode(index, Rect::new(0, 0, 1, 1));
        }
        assert!(app.cache_reads_pending(), "the fifth cell waits and reads");
        assert_eq!(next_timeout(&app, &[]), CACHE_READ_POLL);
    }

    #[test]
    fn the_loop_looks_every_two_milliseconds_until_the_first_picture_of_the_grid() {
        let dir = test_dir();
        let mut app = app_with_files(dir.path(), &["a.png", "b.png"]);
        app.scan_complete = true;
        app.gallery.update_grid(Rect::new(0, 0, 80, 24));
        app.spawn_thumb_decode(0, Rect::new(0, 0, 1, 1));
        assert!(app.waits_for_the_first_picture());
        assert_eq!(next_timeout(&app, &[]), FIRST_PICTURE_POLL);

        app.thumb_cache.insert(1, image::RgbaImage::new(1, 1));
        assert!(!app.waits_for_the_first_picture(), "one picture shows");
    }

    #[test]
    fn the_loop_looks_every_millisecond_while_the_cell_shape_is_on_its_way() {
        let dir = test_dir();
        let mut app = app();
        let path = dir.path().join("a.png");
        image::RgbaImage::new(30, 40).save(&path).unwrap();
        app.images.push(path);
        app.filenames.push("a.png".to_owned());

        app.request_cell_shape();
        assert_eq!(next_timeout(&app, &[]), SHAPE_POLL);
        wait_until(|| {
            app.poll_cell_shape();
            !app.cell_shape_pending()
        });
        assert!(next_timeout(&app, &[]) > SHAPE_POLL);
    }

    #[test]
    fn a_slow_shape_read_gets_the_normal_wait_after_the_window() {
        assert!(!waits_for_a_fast_shape(None), "no read runs");
        assert!(waits_for_a_fast_shape(Some(Duration::ZERO)));
        assert!(!waits_for_a_fast_shape(Some(SHAPE_POLL_WINDOW)));
    }

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
        app.image_rect = Rect::new(0, 0, 100, 40);
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
