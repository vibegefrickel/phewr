mod app;
mod config;
mod encoder;
mod gallery;
mod image_list;
mod info;
#[cfg(feature = "video")]
mod mpv;
mod picker;
mod prefetch;
mod scanner;
mod search;
mod theme;
mod ui;
#[cfg(feature = "video")]
mod video;

use app::{App, ViewMode};
use clap::Parser;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute, queue,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use encoder::GraphicsBackend;
use image_list::SharedImageList;
use ratatui::prelude::*;
use std::io::{self, Write, stdout};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "viewr",
    version,
    about = "Terminal image gallery: thumbnail grid, slideshow, EXIF, HEIC"
)]
struct Cli {
    /// Image file(s) or directory to display [default: .]
    files: Vec<PathBuf>,

    /// Explicit theme file path or configured catalog name
    #[arg(short, long)]
    theme: Option<String>,

    /// Number of threads for image decoding [default: all cores]
    #[arg(short = 'j', long)]
    threads: Option<usize>,

    /// Start a fullscreen slideshow right away
    #[arg(short, long)]
    slideshow: bool,

    /// Seconds between slides (overrides the configured interval)
    #[arg(short = 'n', long, value_name = "SECONDS")]
    interval: Option<f64>,

    /// Open the info panel on start
    #[arg(short, long)]
    info: bool,
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    restored: bool,
}

impl TerminalSession {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut output = stdout();
        if let Err(error) = execute!(output, EnterAlternateScreen, cursor::Hide) {
            leave_terminal(&mut output);
            return Err(error);
        }

        match Terminal::new(CrosstermBackend::new(output)) {
            Ok(terminal) => Ok(Self {
                terminal,
                restored: false,
            }),
            Err(error) => {
                leave_terminal(&mut stdout());
                Err(error)
            }
        }
    }

    fn terminal_mut(&mut self) -> &mut Terminal<CrosstermBackend<io::Stdout>> {
        &mut self.terminal
    }

    fn restore(&mut self) -> io::Result<()> {
        if self.restored {
            return Ok(());
        }
        self.restored = true;

        let mut first_error = encoder::KittyBackend.delete_all().err();
        if let Err(error) = disable_raw_mode()
            && first_error.is_none()
        {
            first_error = Some(error);
        }
        if let Err(error) = execute!(
            self.terminal.backend_mut(),
            cursor::Show,
            LeaveAlternateScreen
        ) && first_error.is_none()
        {
            first_error = Some(error);
        }

        first_error.map_or(Ok(()), Err)
    }
}

/// Undo raw mode and the alternate screen after a failed start.
#[expect(
    clippy::let_underscore_must_use,
    reason = "the start error matters more than a failed cleanup"
)]
fn leave_terminal(output: &mut io::Stdout) {
    let _ = disable_raw_mode();
    let _ = execute!(output, cursor::Show, LeaveAlternateScreen);
}

impl Drop for TerminalSession {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "a panic or an early return is already on its way; restore is best effort"
    )]
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn main() -> io::Result<()> {
    #[cfg(feature = "video")]
    ffmpeg::init()
        .map_err(|error| io::Error::other(format!("failed to initialize ffmpeg: {error}")))?;

    let cli = Cli::parse();
    let paths = if cli.files.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        cli.files
    };

    if let Some(n) = cli.threads {
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "--threads must be greater than zero",
            ));
        }
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global()
            .map_err(|error| {
                io::Error::other(format!("failed to configure thread pool: {error}"))
            })?;
    }

    #[expect(
        clippy::print_stderr,
        reason = "the CLI reports a bad path before the TUI starts"
    )]
    for p in &paths {
        if !p.is_dir() && !p.exists() {
            eprintln!("{}: file not found", p.display());
            std::process::exit(1);
        }
        if p.is_file() && !scanner::is_supported(p) {
            eprintln!("{}: unsupported media format", p.display());
            std::process::exit(1);
        }
    }

    let theme_set = theme::load_themes(cli.theme.as_deref())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let theme = theme_set.themes[theme_set.selected].theme.clone();

    let mut config = config::load();
    let mut state = LoopState::new(&config);
    if let Some(interval) = cli.interval {
        if !interval.is_finite() || interval <= 0.0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "--interval must be a positive number of seconds",
            ));
        }
        config.slideshow_interval = interval;
    }
    if cli.info {
        config.info_panel = true;
    }

    let initial_dir = initial_dir_from_paths(&paths);

    let shared_list = SharedImageList::new();
    scanner::spawn(paths, shared_list.clone());

    let mut session = TerminalSession::enter()?;

    let cell_px = query_cell_pixel_size();
    let mut app = App::with_config(theme, cell_px, shared_list, &config);
    app.install_themes(theme_set.themes, theme_set.selected);
    app.initial_dir = Some(initial_dir);
    app.slideshow_autostart = cli.slideshow;
    let run_result = run(session.terminal_mut(), &mut app, &mut state);
    let restore_result = session.restore();
    run_result.and(restore_result)
}

fn initial_dir_from_paths(paths: &[PathBuf]) -> PathBuf {
    for p in paths {
        if p.is_dir() {
            return p.clone();
        }
    }
    if let Some(first) = paths.first()
        && let Some(parent) = first.parent()
        && !parent.as_os_str().is_empty()
    {
        return parent.to_path_buf();
    }
    PathBuf::from(".")
}

/// The state of the run loop that `App` does not hold.
struct LoopState {
    /// A one-line note for the status bar. The next key press clears it.
    status: Option<String>,
    /// The video player after the PATH lookup for mpv.
    #[cfg(feature = "video")]
    player: mpv::Player,
    /// The note that tells why the built-in player plays instead of mpv.
    #[cfg(feature = "video")]
    fallback_note: Option<String>,
}

impl LoopState {
    /// Without the video feature, the config holds nothing for the run loop.
    #[cfg(not(feature = "video"))]
    fn new(_config: &config::Config) -> Self {
        Self { status: None }
    }

    /// Look up mpv once at the start, so that a missing mpv does not cost a terminal hand-off.
    #[cfg(feature = "video")]
    fn new(config: &config::Config) -> Self {
        let player = mpv::resolve(
            config.video_player,
            mpv::find_on_path("mpv", std::env::var_os("PATH").as_deref()),
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

fn run(
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
        app.refresh_from_scanner();
        app.maybe_autostart_slideshow();
        app.poll_filter();
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

        // mpv takes the video before the draw, so that the gallery comes back in this pass.
        #[cfg(feature = "video")]
        if app.mode == ViewMode::Video && app.video.is_none() {
            hand_off_video(app, state, terminal, &mut mpv::SystemRunner)?;
        }

        if app.scan_complete
            && app.images.is_empty()
            && app.error.is_none()
            && app.mode != ViewMode::Picker
        {
            app.open_picker();
        }

        // 3. Draw UI chrome (always fast — ratatui only)
        terminal.draw(|frame| ui::draw(frame, app, state.status.as_deref()))?;

        // 4. Render Kitty images
        if app.theme_picker.is_some() || app.mode == ViewMode::Picker {
            app.needs_render = false;
            pending_emits.clear();
        } else if app.needs_render && !app.images.is_empty() {
            #[cfg(feature = "video")]
            let is_video = matches!(app.mode, ViewMode::Video);
            #[cfg(not(feature = "video"))]
            let is_video = false;

            match app.mode {
                ViewMode::Fullscreen => {
                    app.load_if_needed();
                    render_fullscreen_image(app)?;
                    app.prefetcher.set_target_hint(app.image_rect, app.cell_px);
                    app.prefetcher.kick(app.current, &app.images);
                }
                ViewMode::Gallery => {
                    render_gallery_images(app)?;
                }
                ViewMode::Picker => {}
                #[cfg(feature = "video")]
                ViewMode::Video => {
                    app.open_video_if_needed();
                    render_video_frame(app)?;
                }
            }
            app.needs_render = false;
            pending_emits.clear();
            if !is_video {
                terminal.draw(|frame| ui::draw(frame, app, state.status.as_deref()))?;
            }
        } else if !pending_emits.is_empty() && app.mode == ViewMode::Gallery {
            let count = pending_emits.len().min(4);
            let batch: Vec<usize> = pending_emits.drain(..count).collect();
            emit_new_thumbnails(app, &batch)?;
        }

        // 5. Wait for next event (shorter timeout when thumbnails are pending)
        let mut timeout = {
            #[cfg(feature = "video")]
            if let Some(v) = &app.video {
                v.time_until_next_frame()
            } else if pending_emits.is_empty() {
                Duration::from_millis(250)
            } else {
                Duration::from_millis(50)
            }
            #[cfg(not(feature = "video"))]
            if pending_emits.is_empty() {
                Duration::from_millis(250)
            } else {
                Duration::from_millis(50)
            }
        };
        if let Some(remaining) = app.slideshow_timeout() {
            timeout = timeout.min(remaining);
        }
        event::poll(timeout)?;
    }
}

/// Gives the terminal to mpv and takes it back. The tests use a fake.
///
/// viewr stays in raw mode and in its alternate screen during the hand-off. Raw mode turns
/// off ISIG, so `Ctrl+C`, `Ctrl+\` and `Ctrl+Z` reach mpv as keys and do not send signals to the
/// process group of viewr. mpv clears only ICANON and ECHO and restores the old mode at its
/// exit. The alternate screen stays, so the main screen of the shell never shows.
#[cfg(feature = "video")]
trait TerminalHandoff {
    /// The size for the command line of mpv, see [`mpv::fallback_geometry`].
    fn geometry(&self, cell_px: (u32, u32)) -> Option<mpv::Geometry>;
    /// Delete the Kitty images of viewr and clear the screen for mpv.
    fn suspend(&mut self, app: &mut App) -> io::Result<()>;
    /// Turn raw mode on if something turned it off, so that Ctrl+C cannot stop viewr.
    fn ensure_raw_mode(&mut self) -> io::Result<()>;
    /// Delete the last picture of mpv, stop mouse tracking, hide the cursor, clear the screen.
    fn resume(&mut self, app: &mut App) -> io::Result<()>;
    /// Drop the input that is in the queue after mpv. Returns true if it held a resize.
    fn discard_input(&mut self) -> io::Result<bool>;
}

#[cfg(feature = "video")]
impl TerminalHandoff for Terminal<CrosstermBackend<io::Stdout>> {
    fn geometry(&self, cell_px: (u32, u32)) -> Option<mpv::Geometry> {
        let size = crossterm::terminal::window_size().ok()?;
        mpv::fallback_geometry((size.columns, size.rows, size.width, size.height), cell_px)
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
        while event::poll(Duration::ZERO)? {
            resized |= matches!(event::read()?, Event::Resize(_, _));
        }
        Ok(resized)
    }
}

/// Play the current video in mpv when the configuration and PATH allow it.
///
/// After mpv, viewr goes back to the view that opened the video. The gallery has the cursor
/// on the video. The fullscreen view stays on the video and shows a play hint. If mpv does
/// not start, the built-in player takes over for the rest of the session.
#[cfg(feature = "video")]
fn hand_off_video(
    app: &mut App,
    state: &mut LoopState,
    handoff: &mut impl TerminalHandoff,
    runner: &mut impl mpv::Runner,
) -> io::Result<()> {
    let path = app.current_path().to_path_buf();
    let played = match &state.player {
        mpv::Player::Builtin => {
            state.status.clone_from(&state.fallback_note);
            return Ok(());
        }
        mpv::Player::Window(program) => runner
            .spawn(program, &mpv::window_args(&path))
            .map(|()| Some(format!("{} plays in an mpv window", mpv::file_name(&path)))),
        mpv::Player::Terminal(program) => play_in_terminal(app, handoff, runner, program, &path)?
            .map(|code| mpv::exit_note(code, &path)),
    };
    match played {
        Ok(note) => {
            state.status = note;
            app.stop_slideshow();
            app.return_from_video();
            focus_gallery_on_current(app);
        }
        Err(error) => {
            state.player = mpv::Player::Builtin;
            state.fallback_note = Some(format!("mpv did not start ({error}): built-in player"));
            state.status.clone_from(&state.fallback_note);
        }
    }
    Ok(())
}

/// Run mpv in this terminal and wait for its exit code.
///
/// The outer error is a terminal error and ends viewr. The inner error is a failed mpv start.
#[cfg(feature = "video")]
fn play_in_terminal(
    app: &mut App,
    handoff: &mut impl TerminalHandoff,
    runner: &mut impl mpv::Runner,
    program: &std::path::Path,
    path: &std::path::Path,
) -> io::Result<io::Result<Option<i32>>> {
    let args = mpv::terminal_args(path, handoff.geometry(app.cell_px));
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
    let played = runner.run(program, &args);
    handoff.resume(app)?;
    if handoff.discard_input()? {
        app.mark_dirty();
    }
    Ok(played)
}

/// Put the gallery cursor on the current file, so that the gallery shows where the user was.
#[cfg(feature = "video")]
fn focus_gallery_on_current(app: &mut App) {
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

fn handle_event(app: &mut App, event: &Event) -> io::Result<bool> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => handle_key(app, *key),
        Event::Resize(_, _) => {
            app.mark_dirty();
            Ok(false)
        }
        _ => Ok(false),
    }
}

/// Route a key press: overlays (help, delete question, theme picker) win over the view mode.
fn handle_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    if app.help_visible {
        app.help_visible = false;
        app.needs_render = true;
    } else if app.pending_delete.is_some() {
        match key.code {
            KeyCode::Char('y' | 'Y') => {
                app.graphics_delete_all()?;
                app.confirm_delete();
                if app.scan_complete && app.images.is_empty() {
                    return Ok(true);
                }
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                app.cancel_delete();
            }
            _ => {}
        }
    } else if app.theme_picker.is_some() {
        handle_theme_picker_key(app, key.code);
    } else if key.code == KeyCode::Char('t')
        && !(app.mode == ViewMode::Gallery && app.gallery.search_active)
        && !(app.mode == ViewMode::Picker
            && app
                .picker
                .as_ref()
                .is_some_and(|picker| picker.filter_active))
    {
        app.graphics_delete_all()?;
        app.open_theme_picker();
    } else {
        return match app.mode {
            ViewMode::Fullscreen => handle_fullscreen_key(app, key),
            #[cfg(feature = "video")]
            ViewMode::Video => handle_video_key(app, key),
            ViewMode::Gallery if app.gallery.search_active => {
                handle_search_key(app, key.code);
                Ok(false)
            }
            ViewMode::Gallery => handle_gallery_key(app, key),
            ViewMode::Picker => Ok(handle_picker_key(app, key.code)),
        };
    }
    Ok(false)
}

/// Keys of the fullscreen view: navigation, zoom and pan, slideshow, info, delete.
fn handle_fullscreen_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    match key.code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Char('?') => {
            app.graphics_delete_all()?;
            app.help_visible = true;
        }
        KeyCode::Esc if app.slideshow_running() => app.stop_slideshow(),
        KeyCode::Esc => {
            app.graphics_delete_all()?;
            app.enter_gallery();
        }
        KeyCode::Char('i') => {
            app.graphics_delete_all()?;
            app.toggle_info();
        }
        KeyCode::Enter if app.shows_video_placeholder() => {
            app.stop_slideshow();
            app.enter_fullscreen_at(app.current);
        }
        KeyCode::Char('s') => app.toggle_slideshow(),
        KeyCode::Char(' ') => app.slideshow_toggle_pause(),
        KeyCode::Char('[') => app.slideshow_adjust_interval(-1.0),
        KeyCode::Char(']') => app.slideshow_adjust_interval(1.0),
        KeyCode::Char('+' | '=') => app.zoom_in(),
        KeyCode::Char('-' | '_') => app.zoom_out(),
        KeyCode::Char('0') => app.reset_zoom(),
        KeyCode::Left | KeyCode::Char('h') if app.zoom > 1.0 => app.pan(-1.0, 0.0),
        KeyCode::Right | KeyCode::Char('l') if app.zoom > 1.0 => app.pan(1.0, 0.0),
        KeyCode::Up | KeyCode::Char('k') if app.zoom > 1.0 => app.pan(0.0, -1.0),
        KeyCode::Down | KeyCode::Char('j') if app.zoom > 1.0 => app.pan(0.0, 1.0),
        KeyCode::Left | KeyCode::Char('h') => {
            app.prev();
            app.slideshow_touch();
        }
        KeyCode::Right | KeyCode::Char('l') => {
            app.next();
            app.slideshow_touch();
        }
        KeyCode::Home => {
            app.graphics_delete_all()?;
            app.first();
            app.slideshow_touch();
        }
        KeyCode::End => {
            app.graphics_delete_all()?;
            app.last();
            app.slideshow_touch();
        }
        KeyCode::Char('d' | 'D') => {
            app.begin_delete(matches!(key.code, KeyCode::Char('D')));
        }
        _ => {}
    }
    Ok(false)
}

/// Keys of the video player: pause, navigation, delete.
#[cfg(feature = "video")]
fn handle_video_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    match key.code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Char(' ') => {
            if let Some(ref mut v) = app.video {
                v.toggle_pause();
                app.needs_render = true;
            }
        }
        KeyCode::Esc => {
            app.graphics_delete_all()?;
            app.return_from_video();
            focus_gallery_on_current(app);
        }
        KeyCode::Char('?') => {
            app.graphics_delete_all()?;
            app.help_visible = true;
        }
        KeyCode::Left | KeyCode::Char('h') => {
            app.graphics_delete_all()?;
            app.prev();
        }
        KeyCode::Right | KeyCode::Char('l') => {
            app.graphics_delete_all()?;
            app.next();
        }
        KeyCode::Home => {
            app.graphics_delete_all()?;
            app.first();
        }
        KeyCode::End => {
            app.graphics_delete_all()?;
            app.last();
        }
        KeyCode::Char('d' | 'D') => {
            app.begin_delete(matches!(key.code, KeyCode::Char('D')));
        }
        _ => {}
    }
    Ok(false)
}

/// Keys while the gallery search line takes input.
fn handle_search_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            app.gallery.search_active = false;
            if !app.gallery.search_query.is_empty() {
                app.gallery.search_query.clear();
                app.update_filter();
            }
        }
        KeyCode::Enter => {
            app.gallery.search_active = false;
        }
        KeyCode::Backspace if app.gallery.search_query.pop().is_some() => {
            app.update_filter();
        }
        KeyCode::Char(c) => {
            app.gallery.search_query.push(c);
            app.update_filter();
        }
        _ => {}
    }
}

/// Keys of the thumbnail grid.
fn handle_gallery_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    let prev_offset = app.gallery.scroll_offset;
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
        KeyCode::Char('?') => {
            app.graphics_delete_all()?;
            app.help_visible = true;
        }
        KeyCode::Char('/') => {
            app.gallery.search_active = true;
        }
        KeyCode::Enter => {
            app.graphics_delete_all()?;
            app.enter_fullscreen_selected();
        }
        KeyCode::Char('s') => {
            app.graphics_delete_all()?;
            app.enter_fullscreen_selected();
            if app.mode == ViewMode::Fullscreen {
                app.start_slideshow();
            }
        }
        KeyCode::Char('i') => {
            app.graphics_delete_all()?;
            app.toggle_info();
        }
        KeyCode::Char('+' | '=') => {
            app.graphics_delete_all()?;
            app.thumb_zoom_in();
        }
        KeyCode::Char('-' | '_') => {
            app.graphics_delete_all()?;
            app.thumb_zoom_out();
        }
        KeyCode::Char('0') => {
            app.graphics_delete_all()?;
            app.thumb_zoom_reset();
        }
        KeyCode::Left | KeyCode::Char('h') => {
            app.gallery.move_left();
            app.pre_decode_hovered();
        }
        KeyCode::Right | KeyCode::Char('l') => {
            app.gallery.move_right();
            app.pre_decode_hovered();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.gallery.move_up();
            app.pre_decode_hovered();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.gallery.move_down();
            app.pre_decode_hovered();
        }
        KeyCode::PageUp | KeyCode::Char('b') if ctrl || matches!(key.code, KeyCode::PageUp) => {
            app.gallery.move_page_up();
            app.pre_decode_hovered();
        }
        KeyCode::PageDown | KeyCode::Char('f') if ctrl || matches!(key.code, KeyCode::PageDown) => {
            app.gallery.move_page_down();
            app.pre_decode_hovered();
        }
        KeyCode::Char('g') | KeyCode::Home => {
            app.gallery.move_to_first();
            app.pre_decode_hovered();
        }
        KeyCode::Char('G') | KeyCode::End => {
            app.gallery.move_to_last();
            app.pre_decode_hovered();
        }
        KeyCode::Char(' ') => {
            app.toggle_selection_at_cursor();
        }
        KeyCode::Char('a') => {
            app.select_all_filtered();
        }
        KeyCode::Char('A') => {
            app.clear_selection();
        }
        KeyCode::Char('d' | 'D') => {
            app.begin_delete(matches!(key.code, KeyCode::Char('D')));
        }
        KeyCode::Char('o') => {
            app.graphics_delete_all()?;
            app.open_picker();
        }
        _ => {}
    }
    if app.gallery.scroll_offset != prev_offset {
        app.needs_render = true;
    }
    Ok(false)
}

fn handle_theme_picker_key(app: &mut App, code: KeyCode) {
    let Some(picker) = app.theme_picker.as_ref() else {
        return;
    };
    let selected = picker.selected;
    let visible_height = picker.visible_height.max(1);
    let last = app.themes.len().saturating_sub(1);
    match code {
        KeyCode::Down | KeyCode::Char('j') => {
            app.theme_picker_select(if selected == last { 0 } else { selected + 1 });
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.theme_picker_select(if selected == 0 { last } else { selected - 1 });
        }
        KeyCode::Home | KeyCode::Char('g') => app.theme_picker_select(0),
        KeyCode::End | KeyCode::Char('G') => app.theme_picker_select(last),
        KeyCode::PageDown => {
            app.theme_picker_select((selected + visible_height).min(last));
        }
        KeyCode::PageUp => app.theme_picker_select(selected.saturating_sub(visible_height)),
        KeyCode::Enter => app.theme_picker_confirm(),
        KeyCode::Esc | KeyCode::Char('q' | 't') => app.theme_picker_cancel(),
        _ => {}
    }
}

fn handle_picker_key(app: &mut App, code: KeyCode) -> bool {
    let filter_active = app.picker.as_ref().is_some_and(|p| p.filter_active);

    if filter_active {
        let Some(picker) = app.picker.as_mut() else {
            return false;
        };
        match code {
            KeyCode::Esc => {
                picker.filter_active = false;
                picker.filter.clear();
                picker.rebuild_filter();
                app.needs_render = true;
            }
            KeyCode::Enter => {
                picker.filter_active = false;
                app.needs_render = true;
            }
            KeyCode::Backspace => {
                picker.filter.pop();
                picker.rebuild_filter();
                app.needs_render = true;
            }
            KeyCode::Char(c) => {
                picker.filter.push(c);
                picker.rebuild_filter();
                app.needs_render = true;
            }
            _ => {}
        }
        return false;
    }

    match code {
        KeyCode::Char('q') => return true,
        KeyCode::Esc if !app.images.is_empty() => {
            app.close_picker();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let Some(p) = app.picker.as_mut() {
                p.move_up();
                app.needs_render = true;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let Some(p) = app.picker.as_mut() {
                p.move_down();
                app.needs_render = true;
            }
        }
        KeyCode::Home | KeyCode::Char('g') => {
            if let Some(p) = app.picker.as_mut() {
                p.move_first();
                app.needs_render = true;
            }
        }
        KeyCode::End | KeyCode::Char('G') => {
            if let Some(p) = app.picker.as_mut() {
                p.move_last();
                app.needs_render = true;
            }
        }
        KeyCode::PageUp => {
            if let Some(p) = app.picker.as_mut() {
                p.move_page_up(10);
                app.needs_render = true;
            }
        }
        KeyCode::PageDown => {
            if let Some(p) = app.picker.as_mut() {
                p.move_page_down(10);
                app.needs_render = true;
            }
        }
        KeyCode::Left | KeyCode::Char('h') => {
            let target = app.picker.as_mut().and_then(picker::PickerState::ascend);
            if let Some(t) = target {
                app.switch_to_dir(&t);
            }
        }
        KeyCode::Right | KeyCode::Char('l') => {
            let target = app.picker.as_mut().and_then(|p| {
                let sel = p.selected()?;
                if sel.is_parent {
                    return p.ascend();
                }
                p.enter_selected()
            });
            if let Some(t) = target {
                app.switch_to_dir(&t);
            }
        }
        KeyCode::Enter => {
            let target = app
                .picker
                .as_ref()
                .and_then(|picker| picker.selected())
                .map(|entry| entry.path.clone());
            if let Some(target) = target {
                app.switch_to_dir(&target);
                app.close_picker();
            }
        }
        KeyCode::Char('/') => {
            if let Some(p) = app.picker.as_mut() {
                p.filter_active = true;
                p.filter.clear();
                p.rebuild_filter();
                app.needs_render = true;
            }
        }
        KeyCode::Char('?') => {
            app.help_visible = true;
            app.needs_render = true;
        }
        _ => {}
    }
    false
}

fn query_cell_pixel_size() -> (u32, u32) {
    crossterm::terminal::window_size().map_or((8, 16), |ws| {
        // A terminal that does not report pixel dimensions (common under
        // terminal multiplexers such as rift/tmux, and some SSH setups)
        // returns ws.width/ws.height == 0 even though columns/rows are
        // valid. Treat a zero pixel report as "unknown" and fall back to
        // conventional cell metrics, otherwise the division below collapses
        // to 1x1 and thumbnails render only a few pixels tall.
        let w = if ws.columns > 0 && ws.width > 0 {
            u32::from(ws.width) / u32::from(ws.columns)
        } else {
            8
        };
        let h = if ws.rows > 0 && ws.height > 0 {
            u32::from(ws.height) / u32::from(ws.rows)
        } else {
            16
        };
        (w.max(1), h.max(1))
    })
}

fn render_fullscreen_image(app: &mut App) -> io::Result<()> {
    use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};

    let mut out = stdout().lock();
    // Wrap the delete + retransmit in a synchronized update so the terminal swaps
    // atomically instead of briefly showing a blank viewport (visible as flicker
    // when zooming or panning, which redraw rapidly).
    queue!(out, BeginSynchronizedUpdate)?;
    app.graphics_delete_all_to(&mut out)?;

    if let Some(img) = &app.loaded {
        let (cpw, cph) = app.cell_px;
        let img_cols = img.width().div_ceil(cpw);
        let img_rows = img.height().div_ceil(cph);
        let offset_x =
            app.image_rect.x + (app.image_rect.width.saturating_sub(img_cols as u16)) / 2;
        let offset_y =
            app.image_rect.y + (app.image_rect.height.saturating_sub(img_rows as u16)) / 2;

        queue!(out, cursor::MoveTo(offset_x, offset_y))?;
        app.graphics.transmit(
            &mut out,
            img,
            &encoder::DisplayOptions {
                id: None,
                cols: None,
                rows: None,
            },
        )?;
    }

    queue!(out, EndSynchronizedUpdate)?;
    out.flush()
}

#[cfg(feature = "video")]
const VIDEO_IMAGE_ID: u32 = 900;

#[cfg(feature = "video")]
fn render_video_frame(app: &App) -> io::Result<()> {
    use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};

    let mut out = stdout().lock();

    if let Some(v) = &app.video
        && let Some(img) = &v.current_frame
    {
        let (cpw, cph) = app.cell_px;
        let img_cols = img.width().div_ceil(cpw);
        let img_rows = img.height().div_ceil(cph);
        let offset_x =
            app.image_rect.x + (app.image_rect.width.saturating_sub(img_cols as u16)) / 2;
        let offset_y =
            app.image_rect.y + (app.image_rect.height.saturating_sub(img_rows as u16)) / 2;

        queue!(out, BeginSynchronizedUpdate)?;
        queue!(out, cursor::MoveTo(offset_x, offset_y))?;
        app.graphics.transmit(
            &mut out,
            img,
            &encoder::DisplayOptions {
                id: Some(VIDEO_IMAGE_ID),
                cols: None,
                rows: None,
            },
        )?;
        queue!(out, EndSynchronizedUpdate)?;
    }

    out.flush()
}

fn emit_new_thumbnails(app: &mut App, new_indices: &[usize]) -> io::Result<()> {
    let mut out = stdout().lock();
    let visible: Vec<(usize, usize)> = app.gallery.visible_items().collect();
    let mut newly_transmitted: Vec<u32> = Vec::new();
    for (vis_idx, img_idx) in visible {
        if !new_indices.contains(&img_idx) {
            continue;
        }
        let cell_rect = app.gallery.cell_rect(vis_idx);
        if let Some((img, id)) = app.thumb_cache.peek(img_idx) {
            queue!(out, cursor::MoveTo(cell_rect.x + 1, cell_rect.y + 1))?;
            app.graphics.transmit(
                &mut out,
                img,
                &encoder::DisplayOptions {
                    id: Some(id),
                    cols: None,
                    rows: None,
                },
            )?;
            newly_transmitted.push(id);
        }
    }
    out.flush()?;
    for id in newly_transmitted {
        app.transmitted_image_ids.insert(id);
    }
    Ok(())
}

fn render_gallery_images(app: &mut App) -> io::Result<()> {
    let visible: Vec<(usize, usize)> = app.gallery.visible_items().collect();

    for &(vis_idx, img_idx) in &visible {
        let cell_rect = app.gallery.cell_rect(vis_idx);
        let inner = Rect {
            x: cell_rect.x + 1,
            y: cell_rect.y + 1,
            width: cell_rect.width.saturating_sub(2),
            height: cell_rect.height.saturating_sub(2),
        };
        let path = app.images[img_idx].clone();
        app.spawn_thumb_decode(img_idx, path, inner);
    }

    let mut out = stdout().lock();
    if app.graphics_storage_dirty {
        app.graphics_delete_all_to(&mut out)?;
        app.graphics_storage_dirty = false;
    } else {
        // Drop visible placements only; keep stored image data so already-transmitted thumbs
        // can be re-placed with a cheap `a=p` instead of a full PNG retransmit.
        app.graphics.clear_placements_to(&mut out)?;
    }

    let mut newly_transmitted: Vec<u32> = Vec::new();
    for &(vis_idx, img_idx) in &visible {
        let cell_rect = app.gallery.cell_rect(vis_idx);
        if let Some((img, id)) = app.thumb_cache.peek(img_idx) {
            let inner_x = cell_rect.x + 1;
            let inner_y = cell_rect.y + 1;
            queue!(out, cursor::MoveTo(inner_x, inner_y))?;
            if app.transmitted_image_ids.contains(&id) {
                app.graphics.place_by_id_to(&mut out, id)?;
            } else {
                app.graphics.transmit(
                    &mut out,
                    img,
                    &encoder::DisplayOptions {
                        id: Some(id),
                        cols: None,
                        rows: None,
                    },
                )?;
                newly_transmitted.push(id);
            }
        }
    }

    out.flush()?;
    for id in newly_transmitted {
        app.transmitted_image_ids.insert(id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        App, KeyCode, SharedImageList, ViewMode, handle_picker_key, handle_theme_picker_key,
    };
    use crate::theme::{NamedTheme, Theme};
    use ratatui::style::{Color, Style};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    fn picker_app(root: &Path) -> App {
        let list = SharedImageList::new();
        let mut app = App::new(Theme::default(), (8, 16), list);
        app.initial_dir = Some(root.to_path_buf());
        app.open_picker();
        app
    }

    fn synthetic_tree(test_name: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "viewr-synthetic-{test_name}-{}",
            std::process::id()
        ));
        let child = root.join("synthetic-images");
        if root.exists() {
            fs::remove_dir_all(&root).unwrap();
        }
        fs::create_dir_all(&child).unwrap();
        (
            fs::canonicalize(root).unwrap(),
            fs::canonicalize(child).unwrap(),
        )
    }

    fn select_path(app: &mut App, target: &Path) {
        let picker = app.picker.as_mut().unwrap();
        let entry_index = picker
            .entries
            .iter()
            .position(|entry| entry.path == target)
            .unwrap();
        picker.cursor = picker
            .filtered_indices
            .iter()
            .position(|index| *index == entry_index)
            .unwrap();
    }

    #[test]
    fn enter_chooses_directory_and_closes_picker() {
        let (root, child) = synthetic_tree("choose-directory");
        let mut app = picker_app(&root);
        select_path(&mut app, &child);

        assert!(!handle_picker_key(&mut app, KeyCode::Enter));
        assert_eq!(app.mode, ViewMode::Gallery);
        assert!(app.picker.is_none());
        assert_eq!(app.current_dir, Some(fs::canonicalize(&child).unwrap()));

        std::thread::sleep(Duration::from_millis(20));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn right_descends_without_closing_picker() {
        let (root, child) = synthetic_tree("descend-directory");
        let mut app = picker_app(&root);
        select_path(&mut app, &child);

        assert!(!handle_picker_key(&mut app, KeyCode::Right));
        assert_eq!(app.mode, ViewMode::Picker);
        assert_eq!(
            app.picker.as_ref().map(|picker| &picker.current_dir),
            Some(&fs::canonicalize(&child).unwrap())
        );

        std::thread::sleep(Duration::from_millis(20));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn theme_picker_applies_for_the_session_and_cancel_restores() {
        let list = SharedImageList::new();
        let first = Theme::fallback();
        let mut second = Theme::fallback();
        second.title = Style::default().fg(Color::Red);
        let mut app = App::new(first.clone(), (8, 16), list);
        app.install_themes(
            vec![
                NamedTheme {
                    name: "first".to_owned(),
                    path: None,
                    theme: first.clone(),
                },
                NamedTheme {
                    name: "second".to_owned(),
                    path: None,
                    theme: second.clone(),
                },
            ],
            0,
        );

        app.open_theme_picker();
        handle_theme_picker_key(&mut app, KeyCode::Down);
        assert_eq!(app.theme, second);
        app.theme_picker_confirm();
        assert_eq!(app.theme_index, 1);
        assert!(app.theme_picker.is_none());

        app.open_theme_picker();
        handle_theme_picker_key(&mut app, KeyCode::Up);
        assert_eq!(app.theme, first);
        app.theme_picker_cancel();
        assert_eq!(app.theme, second);
        assert_eq!(app.theme_index, 1);
    }

    #[test]
    fn theme_picker_pages_by_the_visible_rows() {
        let list = SharedImageList::new();
        let theme = Theme::fallback();
        let mut app = App::new(theme.clone(), (8, 16), list);
        let themes = (0..12)
            .map(|index| NamedTheme {
                name: format!("theme-{index}"),
                path: None,
                theme: theme.clone(),
            })
            .collect();
        app.install_themes(themes, 0);
        app.open_theme_picker();
        app.theme_picker.as_mut().unwrap().visible_height = 5;

        handle_theme_picker_key(&mut app, KeyCode::PageDown);
        assert_eq!(app.theme_picker.as_ref().unwrap().selected, 5);
        handle_theme_picker_key(&mut app, KeyCode::PageDown);
        assert_eq!(app.theme_picker.as_ref().unwrap().selected, 10);
        handle_theme_picker_key(&mut app, KeyCode::PageUp);
        assert_eq!(app.theme_picker.as_ref().unwrap().selected, 5);
    }

    #[cfg(feature = "video")]
    mod video_handoff {
        use super::super::{App, LoopState, SharedImageList, TerminalHandoff, ViewMode};
        use super::super::{hand_off_video, handle_fullscreen_key, handle_video_key, mpv};
        use crate::theme::Theme;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use std::cell::{Cell, RefCell};
        use std::ffi::OsString;
        use std::io;
        use std::path::{Path, PathBuf};
        use std::rc::Rc;

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
        struct FakeRunner {
            log: Log,
            raw: Rc<Cell<bool>>,
            exit: Result<Option<i32>, io::ErrorKind>,
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
                Ok(self.exit?)
            }

            fn spawn(&mut self, program: &Path, args: &[OsString]) -> io::Result<()> {
                self.record("spawn", program, args)
            }
        }

        /// An app with a photo and a video, in video mode for the video.
        fn video_app() -> App {
            let mut app = App::new(Theme::default(), (8, 16), SharedImageList::new());
            app.images = vec![
                PathBuf::from("/videos/a.jpg"),
                PathBuf::from("/videos/clip.mp4"),
            ];
            app.filenames = vec!["a.jpg".to_owned(), "clip.mp4".to_owned()];
            app.gallery.filtered_indices = vec![0, 1];
            app.enter_video(1);
            app
        }

        /// An app in a running slideshow that steps from a photo onto a video.
        fn fullscreen_video_app() -> App {
            let mut app = App::new(Theme::default(), (8, 16), SharedImageList::new());
            app.images = vec![
                PathBuf::from("/videos/a.jpg"),
                PathBuf::from("/videos/clip.mp4"),
                PathBuf::from("/videos/b.jpg"),
            ];
            app.filenames = vec![
                "a.jpg".to_owned(),
                "clip.mp4".to_owned(),
                "b.jpg".to_owned(),
            ];
            app.gallery.filtered_indices = vec![0, 1, 2];
            app.enter_fullscreen_at(0);
            app.start_slideshow();
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
                     --really-quiet --terminal=yes --input-terminal=yes --loop-file=inf \
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
            assert!(!app.slideshow_running(), "the slideshow stops at the video");
            assert!(app.needs_render);
            assert_eq!(state.status, None);
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
                     --loop-file=inf -- /videos/clip.mp4"
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
            let mut state = state(mpv::Player::Window(PathBuf::from("/bin/mpv")));
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
    }
}
