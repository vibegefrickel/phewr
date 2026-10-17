mod app;
mod config;
mod decode;
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

        let backend = self.terminal.backend_mut();
        let mut first_error = encoder::KittyBackend
            .delete_all_to(backend)
            .and_then(|()| Write::flush(backend))
            .err();
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

/// The result of an mpv start: a window that plays on, or the exit code of mpv in the terminal.
#[cfg(feature = "video")]
enum Played {
    Window(std::sync::mpsc::Receiver<Option<i32>>),
    Terminal(Option<i32>),
}

/// Play the current video in mpv when the configuration and PATH allow it.
///
/// After mpv, viewr goes back to the view that opened the video. The gallery has the cursor
/// on the video. The fullscreen view stays on the video and shows a play hint. If mpv does
/// not start, the built-in player takes over for the rest of the session.
///
/// A running slideshow that is not paused plays the video one time, see [`after_mpv`].
#[cfg(feature = "video")]
fn hand_off_video(
    app: &mut App,
    state: &mut LoopState,
    handoff: &mut impl TerminalHandoff,
    runner: &mut impl mpv::Runner,
) -> io::Result<()> {
    let path = app.current_path().to_path_buf();
    let playback = if app.slideshow_advancing() {
        mpv::Playback::Once
    } else {
        mpv::Playback::Loop
    };
    let played = match &state.player {
        mpv::Player::Builtin => {
            state.status.clone_from(&state.fallback_note);
            return Ok(());
        }
        mpv::Player::Window(program) => runner
            .spawn(program, &mpv::window_args(&path, playback))
            .map(Played::Window),
        mpv::Player::Terminal(program) => {
            play_in_terminal(app, handoff, runner, program, &path, playback)?.map(Played::Terminal)
        }
    };
    match played {
        Ok(played) => {
            app.return_from_video();
            focus_gallery_on_current(app);
            state.status = after_mpv(app, played, playback, &path);
        }
        Err(error) => {
            state.player = mpv::Player::Builtin;
            state.fallback_note = Some(format!("mpv did not start ({error}): built-in player"));
            state.status.clone_from(&state.fallback_note);
        }
    }
    Ok(())
}

/// Lead the slideshow on after mpv, and return the status note.
///
/// Outside a slideshow, or in a paused one, mpv played in a loop until the user quit it, so a
/// slideshow stops. In [`mpv::Playback::Once`], exit code 0 in the terminal is the end of the
/// file and the slideshow moves on. Each other exit stops the slideshow on the placeholder:
/// `q`, `Q` and `Ctrl+C` (code 4), an error or a signal. A window holds the slideshow on the
/// placeholder until the window closes, see [`App::hold_slideshow_for_window`].
#[cfg(feature = "video")]
fn after_mpv(
    app: &mut App,
    played: Played,
    playback: mpv::Playback,
    path: &std::path::Path,
) -> Option<String> {
    let name = mpv::file_name(path);
    match (played, playback) {
        (Played::Window(_), mpv::Playback::Loop) => {
            app.stop_slideshow();
            Some(format!("{name} plays in an mpv window"))
        }
        (Played::Window(exit), mpv::Playback::Once) => {
            app.hold_slideshow_for_window(exit);
            Some(format!(
                "{name} plays in an mpv window, the slideshow waits for its end"
            ))
        }
        (Played::Terminal(code), mpv::Playback::Loop) => {
            app.stop_slideshow();
            mpv::exit_note(code, path)
        }
        (Played::Terminal(code), mpv::Playback::Once) => {
            app.after_slideshow_video(mpv::reached_the_end(code));
            mpv::exit_note(code, path)
        }
    }
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
    playback: mpv::Playback,
) -> io::Result<io::Result<Option<i32>>> {
    let args = mpv::terminal_args(path, handoff.geometry(app.cell_px), playback);
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
///
/// Esc stops a running slideshow, like `q` in mpv: the user wants out. A step to another file
/// keeps the slideshow and gives that file the full interval, like in the fullscreen view.
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
            app.stop_slideshow();
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
            app.slideshow_touch();
        }
        KeyCode::Right | KeyCode::Char('l') => {
            app.graphics_delete_all()?;
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
            if app.in_slideshow_view() {
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
    crossterm::terminal::window_size().map_or((8, 16), |ws| cell_pixel_size(&ws))
}

/// The pixel size of one cell from a window size report, so that the tests need no terminal.
fn cell_pixel_size(ws: &crossterm::terminal::WindowSize) -> (u32, u32) {
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
}

fn render_fullscreen_image(app: &mut App) -> io::Result<()> {
    let mut out = app.graphics_out.clone();
    render_fullscreen_image_to(app, &mut out)
}

/// Write the fullscreen image to `out`, so that the tests can read the escape sequences.
fn render_fullscreen_image_to(app: &mut App, out: &mut impl Write) -> io::Result<()> {
    use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};

    // Wrap the delete + retransmit in a synchronized update so the terminal swaps
    // atomically instead of briefly showing a blank viewport (visible as flicker
    // when zooming or panning, which redraw rapidly).
    queue!(out, BeginSynchronizedUpdate)?;
    app.graphics_delete_all_to(out)?;

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
            out,
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
    let mut out = app.graphics_out.clone();
    render_video_frame_to(app, &mut out)
}

/// Write the current video frame to `out`, so that the tests can read the escape sequences.
#[cfg(feature = "video")]
fn render_video_frame_to(app: &App, out: &mut impl Write) -> io::Result<()> {
    use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};

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
            out,
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
    let mut out = app.graphics_out.clone();
    emit_new_thumbnails_to(app, new_indices, &mut out)
}

/// Write the new thumbnails to `out`, so that the tests can read the escape sequences.
fn emit_new_thumbnails_to(
    app: &mut App,
    new_indices: &[usize],
    out: &mut impl Write,
) -> io::Result<()> {
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
                out,
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

    let mut out = app.graphics_out.clone();
    place_gallery_images_to(app, &visible, &mut out)
}

/// Write the `visible` thumbnails to `out`, so that the tests can read the escape sequences.
fn place_gallery_images_to(
    app: &mut App,
    visible: &[(usize, usize)],
    out: &mut impl Write,
) -> io::Result<()> {
    if app.graphics_storage_dirty {
        app.graphics_delete_all_to(out)?;
        app.graphics_storage_dirty = false;
    } else {
        // Drop visible placements only; keep stored image data so already-transmitted thumbs
        // can be re-placed with a cheap `a=p` instead of a full PNG retransmit.
        app.graphics.clear_placements_to(out)?;
    }

    let mut newly_transmitted: Vec<u32> = Vec::new();
    for &(vis_idx, img_idx) in visible {
        let cell_rect = app.gallery.cell_rect(vis_idx);
        if let Some((img, id)) = app.thumb_cache.peek(img_idx) {
            let inner_x = cell_rect.x + 1;
            let inner_y = cell_rect.y + 1;
            queue!(out, cursor::MoveTo(inner_x, inner_y))?;
            if app.transmitted_image_ids.contains(&id) {
                app.graphics.place_by_id_to(out, id)?;
            } else {
                app.graphics.transmit(
                    out,
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
    use tempfile::TempDir;

    fn picker_app(root: &Path) -> App {
        let list = SharedImageList::new();
        let mut app = App::new(Theme::default(), (8, 16), list);
        app.initial_dir = Some(root.to_path_buf());
        app.open_picker();
        app
    }

    /// A temp directory with the subdirectory `synthetic-images`. The tree goes away on drop.
    /// Both paths are canonical, because the picker canonicalizes its directories.
    fn synthetic_tree() -> (TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        let child = root.join("synthetic-images");
        fs::create_dir(&child).unwrap();
        (temp, root, child)
    }

    /// Drop the tree after a short wait: `switch_to_dir` starts a scanner thread in it.
    fn cleanup(temp: TempDir) {
        std::thread::sleep(Duration::from_millis(20));
        drop(temp);
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
        let (temp, root, child) = synthetic_tree();
        let mut app = picker_app(&root);
        select_path(&mut app, &child);

        assert!(!handle_picker_key(&mut app, KeyCode::Enter));
        assert_eq!(app.mode, ViewMode::Gallery);
        assert!(app.picker.is_none());
        assert_eq!(app.current_dir, Some(fs::canonicalize(&child).unwrap()));

        cleanup(temp);
    }

    #[test]
    fn right_descends_without_closing_picker() {
        let (temp, root, child) = synthetic_tree();
        let mut app = picker_app(&root);
        select_path(&mut app, &child);

        assert!(!handle_picker_key(&mut app, KeyCode::Right));
        assert_eq!(app.mode, ViewMode::Picker);
        assert_eq!(
            app.picker.as_ref().map(|picker| &picker.current_dir),
            Some(&fs::canonicalize(&child).unwrap())
        );

        cleanup(temp);
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
        struct FakeRunner {
            log: Log,
            raw: Rc<Cell<bool>>,
            exit: Result<Option<i32>, io::ErrorKind>,
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
            let mut app = App::new(Theme::default(), (8, 16), SharedImageList::new());
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
                start.contains(r#" --input-commands=keybind q "quit 4",keybind Q "quit 4" "#),
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

            assert!(last_start(&log).contains(" --loop-file=inf -- "));
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
            assert!(start.contains(" --loop-file=inf -- "), "{start}");
            assert!(!start.contains("--input-commands"), "{start}");
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
            app.slideshow_tick();
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
    }

    /// The CLI and the start directory, before the TUI starts.
    mod cli {
        use super::super::{Cli, initial_dir_from_paths};
        use super::synthetic_tree;
        use clap::Parser;
        use std::path::PathBuf;

        #[test]
        fn parses_files_and_all_options() {
            let cli = Cli::try_parse_from([
                "viewr", "-t", "nord", "-j", "4", "-s", "-n", "2.5", "-i", "a.jpg", "photos",
            ])
            .unwrap();

            assert_eq!(cli.files, [PathBuf::from("a.jpg"), PathBuf::from("photos")]);
            assert_eq!(cli.theme.as_deref(), Some("nord"));
            assert_eq!(cli.threads, Some(4));
            assert!(cli.slideshow);
            assert_eq!(cli.interval, Some(2.5));
            assert!(cli.info);
        }

        #[test]
        fn parses_the_long_options() {
            let cli = Cli::try_parse_from([
                "viewr",
                "--theme",
                "/themes/dark.toml",
                "--threads",
                "2",
                "--slideshow",
                "--interval",
                "7",
                "--info",
            ])
            .unwrap();

            assert!(cli.files.is_empty());
            assert_eq!(cli.theme.as_deref(), Some("/themes/dark.toml"));
            assert_eq!(cli.threads, Some(2));
            assert!(cli.slideshow && cli.info);
            assert_eq!(cli.interval, Some(7.0));
        }

        #[test]
        fn defaults_are_empty() {
            let cli = Cli::try_parse_from(["viewr"]).unwrap();

            assert!(cli.files.is_empty());
            assert_eq!(cli.theme, None);
            assert_eq!(cli.threads, None);
            assert_eq!(cli.interval, None);
            assert!(!cli.slideshow && !cli.info);
        }

        #[test]
        fn rejects_bad_values_and_unknown_flags() {
            assert!(Cli::try_parse_from(["viewr", "--interval", "soon"]).is_err());
            assert!(Cli::try_parse_from(["viewr", "--threads", "-1"]).is_err());
            assert!(Cli::try_parse_from(["viewr", "--fullscreen"]).is_err());
        }

        #[test]
        fn a_directory_argument_is_the_start_directory() {
            let (_temp, root, child) = synthetic_tree();

            let paths = [root.join("missing.jpg"), child.clone()];
            assert_eq!(initial_dir_from_paths(&paths), child);
        }

        #[test]
        fn a_file_argument_starts_in_its_parent_directory() {
            assert_eq!(
                initial_dir_from_paths(&[PathBuf::from("/photos/a.jpg")]),
                PathBuf::from("/photos")
            );
            assert_eq!(
                initial_dir_from_paths(&[PathBuf::from("a.jpg")]),
                PathBuf::from(".")
            );
            assert_eq!(initial_dir_from_paths(&[]), PathBuf::from("."));
        }
    }

    /// The key handlers, driven through `handle_event` like the run loop does.
    mod keys {
        use super::super::{App, SharedImageList, ViewMode, handle_event, handle_theme_picker_key};
        use super::{cleanup, picker_app, select_path, synthetic_tree};
        use crate::config::{
            Config, MAX_THUMB_COLS, MAX_THUMB_ROWS, MIN_THUMB_COLS, MIN_THUMB_ROWS,
        };
        use crate::theme::{NamedTheme, Theme};
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
        use ratatui::layout::Rect;
        use std::fs;
        use std::path::PathBuf;
        use tempfile::TempDir;

        fn press(app: &mut App, code: KeyCode) -> bool {
            press_with(app, code, KeyModifiers::NONE)
        }

        fn press_with(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> bool {
            handle_event(app, &Event::Key(KeyEvent::new(code, modifiers))).unwrap()
        }

        fn char_key(app: &mut App, c: char) -> bool {
            press(app, KeyCode::Char(c))
        }

        /// Paths that do not exist: a decode fails in the background and writes no cache.
        fn photo_paths(count: usize) -> Vec<PathBuf> {
            (0..count)
                .map(|index| PathBuf::from(format!("/photos/photo-{index:02}.jpg")))
                .collect()
        }

        /// A scanned gallery of `count` photos in a grid of 3 columns and 2 visible rows.
        fn gallery_app_with(count: usize, config: &Config) -> App {
            let mut app =
                App::new_with_config(Theme::default(), (8, 16), SharedImageList::new(), config);
            app.images = photo_paths(count);
            app.filenames = app
                .images
                .iter()
                .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
                .collect();
            app.gallery.filtered_indices = (0..count).collect();
            let width = app.gallery.thumb_cols * 3;
            let height = app.gallery.cell_height_total() * 2;
            app.gallery.update_grid(Rect::new(0, 0, width, height));
            app.scan_complete = true;
            app.needs_render = false;
            app
        }

        fn gallery_app(count: usize) -> App {
            gallery_app_with(count, &Config::default())
        }

        fn fullscreen_app(count: usize, index: usize) -> App {
            let mut app = gallery_app(count);
            app.enter_fullscreen_at(index);
            app
        }

        fn themed_app() -> App {
            let mut app = gallery_app(2);
            let themes = ["first", "second", "third"]
                .into_iter()
                .map(|name| NamedTheme {
                    name: name.to_owned(),
                    path: None,
                    theme: Theme::fallback(),
                })
                .collect();
            app.install_themes(themes, 0);
            app
        }

        fn theme_selected(app: &App) -> usize {
            app.theme_picker.as_ref().unwrap().selected
        }

        fn zoom_is_one(app: &App) -> bool {
            (app.zoom - 1.0).abs() < f64::EPSILON
        }

        #[test]
        fn resize_marks_the_pictures_dirty() {
            let mut app = gallery_app(3);
            app.graphics_storage_dirty = false;

            assert!(!handle_event(&mut app, &Event::Resize(80, 24)).unwrap());

            assert!(app.graphics_storage_dirty);
            assert!(app.needs_render);
        }

        #[test]
        fn key_releases_and_other_events_change_nothing() {
            let mut app = gallery_app(3);
            let release = KeyEvent::new_with_kind(
                KeyCode::Char('q'),
                KeyModifiers::NONE,
                KeyEventKind::Release,
            );

            assert!(!handle_event(&mut app, &Event::Key(release)).unwrap());
            assert!(!handle_event(&mut app, &Event::FocusGained).unwrap());
            assert!(!app.needs_render);
        }

        #[test]
        fn key_handlers_write_the_kitty_sequences_to_the_app_output() {
            const DELETE_ALL: &[u8] = b"\x1b_Ga=d,d=A,q=2\x1b\\";
            let mut app = gallery_app(3);
            app.transmitted_image_ids.insert(5);

            char_key(&mut app, '?');
            assert_eq!(app.graphics_out.take_bytes(), DELETE_ALL);
            assert!(app.transmitted_image_ids.is_empty());
            char_key(&mut app, 'x');
            assert!(
                app.graphics_out.take_bytes().is_empty(),
                "no picture change"
            );
            press(&mut app, KeyCode::Enter);
            assert_eq!(app.graphics_out.take_bytes(), DELETE_ALL);
            assert_eq!(app.mode, ViewMode::Fullscreen);
        }

        #[test]
        fn any_key_closes_the_help_and_does_nothing_else() {
            let mut app = gallery_app(3);

            assert!(!char_key(&mut app, '?'));
            assert!(app.help_visible);
            assert!(!char_key(&mut app, 'q'), "q only closes the help");
            assert!(!app.help_visible);
            assert!(app.needs_render);
            assert_eq!(app.mode, ViewMode::Gallery);
        }

        #[test]
        fn t_opens_the_theme_picker_and_its_keys_move_the_selection() {
            let mut app = themed_app();

            assert!(!char_key(&mut app, 't'));
            assert_eq!(theme_selected(&app), 0);
            press(&mut app, KeyCode::Down);
            assert_eq!(theme_selected(&app), 1);
            char_key(&mut app, 'j');
            char_key(&mut app, 'j');
            assert_eq!(theme_selected(&app), 0, "Down wraps to the first theme");
            char_key(&mut app, 'k');
            assert_eq!(theme_selected(&app), 2, "Up wraps to the last theme");
            press(&mut app, KeyCode::Up);
            assert_eq!(theme_selected(&app), 1);
            char_key(&mut app, 'g');
            assert_eq!(theme_selected(&app), 0);
            char_key(&mut app, 'G');
            assert_eq!(theme_selected(&app), 2);
            press(&mut app, KeyCode::Home);
            assert_eq!(theme_selected(&app), 0);
            press(&mut app, KeyCode::End);
            assert_eq!(theme_selected(&app), 2);
            char_key(&mut app, 'x');
            assert_eq!(theme_selected(&app), 2, "other keys do nothing");

            press(&mut app, KeyCode::Enter);
            assert!(app.theme_picker.is_none());
            assert_eq!(app.theme_index, 2);
        }

        #[test]
        fn esc_q_and_t_cancel_the_theme_picker() {
            for code in [KeyCode::Esc, KeyCode::Char('q'), KeyCode::Char('t')] {
                let mut app = themed_app();
                char_key(&mut app, 't');
                press(&mut app, KeyCode::Down);

                assert!(!press(&mut app, code), "{code:?} does not quit");
                assert!(app.theme_picker.is_none());
                assert_eq!(app.theme_index, 0);
            }
        }

        #[test]
        fn the_theme_picker_keys_do_nothing_without_a_picker() {
            let mut app = themed_app();

            handle_theme_picker_key(&mut app, KeyCode::Down);

            assert!(app.theme_picker.is_none());
            assert_eq!(app.theme_index, 0);
        }

        #[test]
        fn t_is_text_in_the_gallery_search() {
            let mut app = themed_app();
            char_key(&mut app, '/');

            char_key(&mut app, 't');

            assert!(app.theme_picker.is_none());
            assert_eq!(app.gallery.search_query, "t");
        }

        #[test]
        fn fullscreen_arrows_step_through_the_photos() {
            let mut app = fullscreen_app(4, 1);

            press(&mut app, KeyCode::Right);
            assert_eq!(app.current, 2);
            char_key(&mut app, 'l');
            assert_eq!(app.current, 3);
            char_key(&mut app, 'l');
            assert_eq!(app.current, 3, "the last photo stays");
            press(&mut app, KeyCode::Left);
            assert_eq!(app.current, 2);
            char_key(&mut app, 'h');
            assert_eq!(app.current, 1);
            press(&mut app, KeyCode::End);
            assert_eq!(app.current, 3);
            press(&mut app, KeyCode::Home);
            assert_eq!(app.current, 0);
            assert_eq!(app.mode, ViewMode::Fullscreen);
        }

        #[test]
        fn fullscreen_zoom_keys_and_pan_keys() {
            let mut app = fullscreen_app(3, 1);

            char_key(&mut app, '+');
            let zoom = app.zoom;
            assert!(zoom > 1.0);
            char_key(&mut app, '=');
            assert!(app.zoom > zoom);

            for code in [
                KeyCode::Left,
                KeyCode::Char('h'),
                KeyCode::Right,
                KeyCode::Char('l'),
                KeyCode::Up,
                KeyCode::Char('k'),
                KeyCode::Down,
                KeyCode::Char('j'),
            ] {
                app.needs_render = false;
                press(&mut app, code);
                assert_eq!(app.current, 1, "{code:?} pans and does not step");
                assert!(app.needs_render, "{code:?} pans");
            }

            let zoomed = app.zoom;
            char_key(&mut app, '-');
            assert!(app.zoom < zoomed, "- zooms out");
            char_key(&mut app, '_');
            char_key(&mut app, '_');
            char_key(&mut app, '_');
            assert!(zoom_is_one(&app), "zoom out stops at 1");

            char_key(&mut app, '+');
            char_key(&mut app, '0');
            assert!(zoom_is_one(&app));
        }

        #[test]
        fn fullscreen_slideshow_keys() {
            let mut app = fullscreen_app(3, 0);

            char_key(&mut app, 's');
            assert!(app.slideshow_running());
            char_key(&mut app, ' ');
            assert!(app.slideshow.as_ref().unwrap().paused);
            char_key(&mut app, ' ');
            assert!(!app.slideshow.as_ref().unwrap().paused);

            let interval = app.slideshow.as_ref().unwrap().interval;
            char_key(&mut app, ']');
            assert!(app.slideshow.as_ref().unwrap().interval > interval);
            char_key(&mut app, '[');
            assert_eq!(app.slideshow.as_ref().unwrap().interval, interval);

            press(&mut app, KeyCode::Right);
            assert_eq!(app.current, 1);
            assert!(app.slideshow_running(), "a step keeps the slideshow");

            press(&mut app, KeyCode::Esc);
            assert!(!app.slideshow_running());
            assert_eq!(
                app.mode,
                ViewMode::Fullscreen,
                "Esc stops the slideshow first"
            );

            char_key(&mut app, 's');
            char_key(&mut app, 's');
            assert!(!app.slideshow_running(), "s toggles");

            press(&mut app, KeyCode::Esc);
            assert_eq!(app.mode, ViewMode::Gallery);
        }

        #[test]
        fn fullscreen_info_help_quit_and_other_keys() {
            let mut app = fullscreen_app(2, 0);
            let info = app.info_visible;

            char_key(&mut app, 'i');
            assert_ne!(app.info_visible, info);
            char_key(&mut app, '?');
            assert!(app.help_visible);
            char_key(&mut app, 'x');
            assert!(!app.help_visible);

            press(&mut app, KeyCode::Enter);
            char_key(&mut app, 'x');
            assert_eq!(app.mode, ViewMode::Fullscreen);
            assert_eq!(app.current, 0);
            assert!(char_key(&mut app, 'q'));
        }

        #[test]
        fn fullscreen_delete_asks_and_n_or_esc_cancel() {
            let mut app = fullscreen_app(3, 1);

            char_key(&mut app, 'd');
            let pending = app.pending_delete.as_ref().unwrap();
            assert_eq!(pending.paths, [PathBuf::from("/photos/photo-01.jpg")]);
            assert!(pending.from_fullscreen);
            assert!(!pending.permanent);

            char_key(&mut app, 'x');
            assert!(app.pending_delete.is_some(), "other keys keep the question");
            char_key(&mut app, 'n');
            assert!(app.pending_delete.is_none());

            char_key(&mut app, 'D');
            assert!(app.pending_delete.as_ref().unwrap().permanent);
            char_key(&mut app, 'N');
            assert!(app.pending_delete.is_none());

            char_key(&mut app, 'D');
            press(&mut app, KeyCode::Esc);
            assert!(app.pending_delete.is_none());
            assert_eq!(
                app.mode,
                ViewMode::Fullscreen,
                "Esc only cancels the question"
            );
            assert_eq!(app.images.len(), 3);
        }

        /// A temp directory with small files that the test can delete.
        fn deletable_files(names: &[&str]) -> (TempDir, Vec<PathBuf>) {
            let dir = tempfile::tempdir().unwrap();
            let paths = names
                .iter()
                .map(|name| {
                    let path = dir.path().join(name);
                    fs::write(&path, b"not an image").unwrap();
                    path
                })
                .collect();
            (dir, paths)
        }

        #[test]
        fn y_deletes_permanently_and_the_last_delete_quits() {
            let (_dir, paths) = deletable_files(&["a.jpg", "b.jpg"]);
            let mut app = gallery_app(0);
            app.images.clone_from(&paths);
            app.filenames = vec!["a.jpg".to_owned(), "b.jpg".to_owned()];
            app.gallery.filtered_indices = vec![0, 1];

            char_key(&mut app, 'D');
            assert!(!char_key(&mut app, 'y'));
            assert!(!paths[0].exists());
            assert_eq!(app.images, [paths[1].clone()]);
            assert!(app.pending_delete.is_none());

            char_key(&mut app, 'D');
            assert!(char_key(&mut app, 'Y'), "an empty scanned list ends viewr");
            assert!(!paths[1].exists());
            assert!(app.images.is_empty());
        }

        #[test]
        fn gallery_arrows_move_the_cursor_in_the_grid() {
            let mut app = gallery_app(9);

            press(&mut app, KeyCode::Right);
            char_key(&mut app, 'l');
            assert_eq!(app.gallery.cursor, 2);
            press(&mut app, KeyCode::Down);
            assert_eq!(app.gallery.cursor, 5);
            char_key(&mut app, 'j');
            assert_eq!(app.gallery.cursor, 8);
            assert_eq!(app.gallery.scroll_offset, 1);
            assert!(app.needs_render, "a scroll redraws the pictures");
            press(&mut app, KeyCode::Left);
            char_key(&mut app, 'h');
            assert_eq!(app.gallery.cursor, 6);
            press(&mut app, KeyCode::Up);
            assert_eq!(app.gallery.cursor, 3);
            char_key(&mut app, 'k');
            assert_eq!(app.gallery.cursor, 0);
            assert_eq!(app.gallery.scroll_offset, 0);
        }

        #[test]
        fn gallery_jumps_by_page_and_to_the_ends() {
            let mut app = gallery_app(9);

            press(&mut app, KeyCode::PageDown);
            assert_eq!(app.gallery.cursor, 6);
            press(&mut app, KeyCode::PageUp);
            assert_eq!(app.gallery.cursor, 0);
            press_with(&mut app, KeyCode::Char('f'), KeyModifiers::CONTROL);
            assert_eq!(app.gallery.cursor, 6);
            press_with(&mut app, KeyCode::Char('b'), KeyModifiers::CONTROL);
            assert_eq!(app.gallery.cursor, 0);
            char_key(&mut app, 'f');
            assert_eq!(app.gallery.cursor, 0, "f without Ctrl does nothing");

            char_key(&mut app, 'G');
            assert_eq!(app.gallery.cursor, 8);
            char_key(&mut app, 'b');
            assert_eq!(app.gallery.cursor, 8, "b without Ctrl does nothing");
            char_key(&mut app, 'g');
            assert_eq!(app.gallery.cursor, 0);
            press(&mut app, KeyCode::End);
            assert_eq!(app.gallery.cursor, 8);
            press(&mut app, KeyCode::Home);
            assert_eq!(app.gallery.cursor, 0);
        }

        #[test]
        fn gallery_selection_keys_and_a_delete_of_the_selection() {
            let mut app = gallery_app(4);

            char_key(&mut app, ' ');
            assert!(app.is_marked(0));
            char_key(&mut app, ' ');
            assert!(!app.is_marked(0));
            char_key(&mut app, 'a');
            assert_eq!(app.selection.len(), 4);
            char_key(&mut app, 'A');
            assert!(app.selection.is_empty());

            press(&mut app, KeyCode::Right);
            char_key(&mut app, ' ');
            press(&mut app, KeyCode::Right);
            char_key(&mut app, ' ');
            char_key(&mut app, 'd');
            let pending = app.pending_delete.as_ref().unwrap();
            assert_eq!(pending.paths, photo_paths(3)[1..]);
            assert!(!pending.from_fullscreen);
            char_key(&mut app, 'n');
        }

        #[test]
        fn gallery_enter_and_s_open_the_fullscreen_view() {
            let mut app = gallery_app(4);
            press(&mut app, KeyCode::Right);

            press(&mut app, KeyCode::Enter);
            assert_eq!(app.mode, ViewMode::Fullscreen);
            assert_eq!(app.current, 1);
            assert!(!app.slideshow_running());

            press(&mut app, KeyCode::Esc);
            assert_eq!(app.mode, ViewMode::Gallery);
            char_key(&mut app, 's');
            assert_eq!(app.mode, ViewMode::Fullscreen);
            assert!(app.slideshow_running());
        }

        #[test]
        fn gallery_s_without_a_photo_starts_no_slideshow() {
            let mut app = gallery_app(0);

            char_key(&mut app, 's');

            assert_eq!(app.mode, ViewMode::Gallery);
            assert!(!app.slideshow_running());
        }

        #[test]
        fn gallery_info_help_quit_and_other_keys() {
            let mut app = gallery_app(2);
            let info = app.info_visible;

            char_key(&mut app, 'i');
            assert_ne!(app.info_visible, info);
            char_key(&mut app, '?');
            assert!(app.help_visible);
            press(&mut app, KeyCode::Tab);
            assert!(!app.help_visible);
            press(&mut app, KeyCode::Tab);
            assert_eq!(app.mode, ViewMode::Gallery);

            assert!(char_key(&mut app, 'q'));
            assert!(press(&mut app, KeyCode::Esc));
        }

        /// The thumbnail keys save the size in the state file of the user, so these tests only
        /// press them at the size limits, where the size does not change.
        #[test]
        fn gallery_thumbnail_size_keys_stop_at_the_limits() {
            let largest = Config {
                thumb_cols: MAX_THUMB_COLS,
                thumb_rows: MAX_THUMB_ROWS,
                ..Config::default()
            };
            let mut app = gallery_app_with(2, &largest);
            assert!(
                !app.gallery.thumb_zoom_in(),
                "the guard: no state file write"
            );
            char_key(&mut app, '+');
            char_key(&mut app, '=');
            assert_eq!(app.gallery.thumb_cols, MAX_THUMB_COLS);

            let smallest = Config {
                thumb_cols: MIN_THUMB_COLS,
                thumb_rows: MIN_THUMB_ROWS,
                ..Config::default()
            };
            let mut app = gallery_app_with(2, &smallest);
            assert!(
                !app.gallery.thumb_zoom_out(),
                "the guard: no state file write"
            );
            char_key(&mut app, '-');
            char_key(&mut app, '_');
            assert_eq!(app.gallery.thumb_cols, MIN_THUMB_COLS);
        }

        #[test]
        fn gallery_search_keys_edit_the_query() {
            let mut app = gallery_app(3);

            char_key(&mut app, '/');
            assert!(app.gallery.search_active);
            char_key(&mut app, 'q');
            char_key(&mut app, '7');
            assert_eq!(app.gallery.search_query, "q7", "q is text in the search");
            press(&mut app, KeyCode::Backspace);
            assert_eq!(app.gallery.search_query, "q");
            press(&mut app, KeyCode::Tab);
            assert_eq!(app.gallery.search_query, "q");
            press(&mut app, KeyCode::Enter);
            assert!(!app.gallery.search_active);
            assert_eq!(app.gallery.search_query, "q", "Enter keeps the filter");

            char_key(&mut app, '/');
            press(&mut app, KeyCode::Esc);
            assert!(!app.gallery.search_active);
            assert!(app.gallery.search_query.is_empty(), "Esc clears the filter");
            assert_eq!(app.gallery.filtered_indices, [0, 1, 2]);

            char_key(&mut app, '/');
            press(&mut app, KeyCode::Backspace);
            press(&mut app, KeyCode::Esc);
            assert!(!app.gallery.search_active);
            assert_eq!(app.mode, ViewMode::Gallery);
        }

        #[test]
        fn gallery_o_opens_the_directory_picker() {
            let (_temp, root, _child) = synthetic_tree();
            let mut app = gallery_app(2);
            app.initial_dir = Some(root.clone());

            char_key(&mut app, 'o');

            assert_eq!(app.mode, ViewMode::Picker);
            assert_eq!(app.picker.as_ref().unwrap().current_dir, root);
            press(&mut app, KeyCode::Esc);
            assert_eq!(
                app.mode,
                ViewMode::Gallery,
                "Esc closes the picker over photos"
            );
        }

        /// A picker in a temp directory with the directories a, b and c.
        fn picker_tree() -> (TempDir, App) {
            let (temp, root, child) = synthetic_tree();
            fs::remove_dir(child).unwrap();
            for name in ["a", "b", "c"] {
                fs::create_dir(root.join(name)).unwrap();
            }
            let app = picker_app(&root);
            (temp, app)
        }

        fn picker_cursor(app: &App) -> usize {
            app.picker.as_ref().unwrap().cursor
        }

        #[test]
        fn picker_keys_move_the_cursor() {
            let (temp, mut app) = picker_tree();

            press(&mut app, KeyCode::Down);
            char_key(&mut app, 'j');
            assert_eq!(picker_cursor(&app), 2);
            press(&mut app, KeyCode::Up);
            assert_eq!(picker_cursor(&app), 1);
            char_key(&mut app, 'k');
            assert_eq!(picker_cursor(&app), 0);
            char_key(&mut app, 'G');
            assert_eq!(picker_cursor(&app), 3, "the parent entry and a, b, c");
            char_key(&mut app, 'g');
            assert_eq!(picker_cursor(&app), 0);
            press(&mut app, KeyCode::End);
            assert_eq!(picker_cursor(&app), 3);
            press(&mut app, KeyCode::Home);
            assert_eq!(picker_cursor(&app), 0);
            press(&mut app, KeyCode::PageDown);
            assert_eq!(picker_cursor(&app), 3);
            press(&mut app, KeyCode::PageUp);
            assert_eq!(picker_cursor(&app), 0);

            press(&mut app, KeyCode::Tab);
            press(&mut app, KeyCode::Esc);
            assert_eq!(app.mode, ViewMode::Picker, "Esc needs photos to go back to");
            char_key(&mut app, '?');
            assert!(app.help_visible);
            char_key(&mut app, 'x');
            assert!(char_key(&mut app, 'q'));

            cleanup(temp);
        }

        #[test]
        fn picker_filter_keys() {
            let (temp, mut app) = picker_tree();

            char_key(&mut app, '/');
            assert!(app.picker.as_ref().unwrap().filter_active);
            char_key(&mut app, 'b');
            char_key(&mut app, 't');
            assert!(app.theme_picker.is_none(), "t is text in the filter");
            assert_eq!(app.picker.as_ref().unwrap().filter, "bt");
            press(&mut app, KeyCode::Backspace);
            press(&mut app, KeyCode::Tab);
            let picker = app.picker.as_ref().unwrap();
            assert_eq!(picker.filter, "b");
            assert_eq!(picker.selected().unwrap().name, "b");

            press(&mut app, KeyCode::Enter);
            let picker = app.picker.as_ref().unwrap();
            assert!(!picker.filter_active);
            assert_eq!(picker.filter, "b", "Enter keeps the filter");
            assert_eq!(app.mode, ViewMode::Picker);

            char_key(&mut app, '/');
            assert!(
                app.picker.as_ref().unwrap().filter.is_empty(),
                "/ starts anew"
            );
            char_key(&mut app, 'c');
            press(&mut app, KeyCode::Esc);
            let picker = app.picker.as_ref().unwrap();
            assert!(!picker.filter_active);
            assert!(picker.filter.is_empty());
            assert_eq!(picker.filtered_indices.len(), 4);

            cleanup(temp);
        }

        #[test]
        fn picker_left_and_the_parent_entry_go_up() {
            let (temp, root, child) = synthetic_tree();
            let grandchild = child.join("grandchild");
            fs::create_dir(&grandchild).unwrap();
            let mut app = picker_app(&grandchild);

            assert_eq!(picker_cursor(&app), 0, "the cursor is on the parent entry");
            press(&mut app, KeyCode::Right);
            assert_eq!(app.picker.as_ref().unwrap().current_dir, child);
            assert_eq!(app.current_dir.as_ref(), Some(&child));

            char_key(&mut app, 'l');
            assert_eq!(app.picker.as_ref().unwrap().current_dir, root);

            select_path(&mut app, &child);
            char_key(&mut app, 'l');
            assert_eq!(app.picker.as_ref().unwrap().current_dir, child);
            char_key(&mut app, 'h');
            assert_eq!(app.picker.as_ref().unwrap().current_dir, root);
            assert_eq!(app.mode, ViewMode::Picker);

            cleanup(temp);
        }

        #[test]
        fn t_opens_the_theme_picker_over_the_directory_picker() {
            let (temp, mut app) = picker_tree();

            char_key(&mut app, 't');

            assert!(app.theme_picker.is_some());
            assert_eq!(app.mode, ViewMode::Picker);

            cleanup(temp);
        }

        #[cfg(feature = "video")]
        fn video_app() -> App {
            let mut app = gallery_app(0);
            app.images = ["a.jpg", "clip.mp4", "other.mp4", "b.jpg"]
                .into_iter()
                .map(|name| PathBuf::from("/videos").join(name))
                .collect();
            app.filenames = ["a.jpg", "clip.mp4", "other.mp4", "b.jpg"]
                .into_iter()
                .map(str::to_owned)
                .collect();
            app.gallery.filtered_indices = vec![0, 1, 2, 3];
            app.enter_video(1);
            app
        }

        #[cfg(feature = "video")]
        #[test]
        fn video_keys_step_between_the_videos() {
            let mut app = video_app();

            char_key(&mut app, ' ');
            assert!(app.video.is_none(), "space without a player does nothing");
            press(&mut app, KeyCode::Right);
            assert_eq!(app.current, 2);
            assert_eq!(app.mode, ViewMode::Video);
            press(&mut app, KeyCode::Left);
            assert_eq!(app.current, 1);
            char_key(&mut app, 'l');
            assert_eq!(app.current, 2);
            char_key(&mut app, 'h');
            assert_eq!(app.current, 1);
            press(&mut app, KeyCode::Tab);
            assert_eq!(app.current, 1);

            char_key(&mut app, '?');
            assert!(app.help_visible);
            char_key(&mut app, 'x');
            assert!(!app.help_visible);

            press(&mut app, KeyCode::Home);
            assert_eq!(app.current, 0);
            assert_eq!(app.mode, ViewMode::Fullscreen, "a.jpg is a photo");

            let mut app = video_app();
            press(&mut app, KeyCode::End);
            assert_eq!(app.current, 3);
            assert_eq!(app.mode, ViewMode::Fullscreen, "b.jpg is a photo");
        }

        #[cfg(feature = "video")]
        #[test]
        fn gallery_s_on_a_video_starts_the_slideshow_with_the_video() {
            let mut app = video_app();
            app.enter_gallery();
            app.gallery.cursor = 1;

            char_key(&mut app, 's');

            assert_eq!(app.mode, ViewMode::Video);
            assert!(app.slideshow_running());
            assert_eq!(app.video_return, crate::app::VideoReturn::Fullscreen);
        }

        #[cfg(feature = "video")]
        #[test]
        fn video_delete_and_quit_keys() {
            let mut app = video_app();

            char_key(&mut app, 'd');
            let pending = app.pending_delete.as_ref().unwrap();
            assert_eq!(pending.paths, [PathBuf::from("/videos/clip.mp4")]);
            assert!(pending.from_fullscreen && !pending.permanent);
            press(&mut app, KeyCode::Esc);
            assert!(app.pending_delete.is_none());
            assert_eq!(app.mode, ViewMode::Video, "Esc only cancels the question");

            char_key(&mut app, 'D');
            assert!(app.pending_delete.as_ref().unwrap().permanent);
            char_key(&mut app, 'n');

            assert!(char_key(&mut app, 'q'));
        }
    }

    /// The Kitty output, written to a buffer instead of the terminal.
    mod output {
        use super::super::{
            App, LoopState, SharedImageList, cell_pixel_size, emit_new_thumbnails_to,
            place_gallery_images_to, query_cell_pixel_size, render_fullscreen_image_to,
        };
        use crate::config::Config;
        use crate::theme::Theme;
        use crossterm::terminal::WindowSize;
        use image::RgbaImage;
        use ratatui::layout::Rect;
        use std::path::PathBuf;

        const TRANSMIT: &str = "\x1b_Ga=T,";
        const PLACE: &str = "\x1b_Ga=p,";
        const DELETE_ALL: &str = "\x1b_Ga=d,d=A,";
        const CLEAR_PLACEMENTS: &str = "\x1b_Ga=d,d=a,";

        fn text(out: &[u8]) -> String {
            String::from_utf8_lossy(out).into_owned()
        }

        /// A gallery of `count` photos with a decoded thumbnail for each, so no decode starts.
        fn thumbnail_app(count: usize) -> App {
            let mut app = App::new(Theme::default(), (8, 16), SharedImageList::new());
            app.images = (0..count)
                .map(|index| PathBuf::from(format!("/photos/{index}.jpg")))
                .collect();
            app.gallery.filtered_indices = (0..count).collect();
            let width = app.gallery.thumb_cols * 3;
            let height = app.gallery.cell_height_total();
            app.gallery.update_grid(Rect::new(0, 0, width, height));
            for index in 0..count {
                app.thumb_cache.insert(index, RgbaImage::new(4, 4));
            }
            app
        }

        /// The write half of `render_gallery_images`, for the visible cells of the grid.
        fn place_visible(app: &mut App, out: &mut Vec<u8>) {
            let visible: Vec<(usize, usize)> = app.gallery.visible_items().collect();
            place_gallery_images_to(app, &visible, out).unwrap();
        }

        #[test]
        fn fullscreen_centers_the_image_in_a_synchronized_update() {
            let mut app = App::new(Theme::default(), (8, 16), SharedImageList::new());
            app.loaded = Some(RgbaImage::new(16, 32));
            app.image_rect = Rect::new(2, 1, 10, 10);
            app.transmitted_image_ids.insert(7);
            let mut out = Vec::new();

            render_fullscreen_image_to(&mut app, &mut out).unwrap();

            let text = text(&out);
            assert!(text.starts_with("\x1b[?2026h"));
            assert!(text.ends_with("\x1b[?2026l"));
            assert!(text.contains(DELETE_ALL));
            assert!(text.contains("\x1b[6;7H"), "2 by 2 cells in the middle");
            assert!(text.contains(TRANSMIT));
            assert!(app.transmitted_image_ids.is_empty());
        }

        #[test]
        fn fullscreen_without_an_image_only_deletes() {
            let mut app = App::new(Theme::default(), (8, 16), SharedImageList::new());
            let mut out = Vec::new();

            render_fullscreen_image_to(&mut app, &mut out).unwrap();

            let text = text(&out);
            assert!(text.contains(DELETE_ALL));
            assert!(!text.contains(TRANSMIT));
        }

        #[test]
        fn gallery_transmits_once_and_then_places_by_id() {
            let mut app = thumbnail_app(3);
            app.graphics_storage_dirty = true;
            let mut out = Vec::new();

            place_visible(&mut app, &mut out);

            let first = text(&out);
            assert!(first.contains(DELETE_ALL));
            assert_eq!(first.matches(TRANSMIT).count(), 3);
            assert!(!first.contains(PLACE));
            assert_eq!(app.transmitted_image_ids.len(), 3);
            assert!(!app.graphics_storage_dirty);

            out.clear();
            place_visible(&mut app, &mut out);

            let second = text(&out);
            assert!(second.starts_with(CLEAR_PLACEMENTS));
            assert!(!second.contains(DELETE_ALL));
            assert!(!second.contains(TRANSMIT));
            assert_eq!(second.matches(PLACE).count(), 3);
        }

        #[test]
        fn gallery_skips_a_cell_without_a_thumbnail() {
            let mut app = thumbnail_app(2);
            app.images.push(PathBuf::from("/photos/missing.jpg"));
            app.gallery.filtered_indices.push(2);
            let mut out = Vec::new();

            place_visible(&mut app, &mut out);

            assert_eq!(text(&out).matches(TRANSMIT).count(), 2);
            assert_eq!(app.transmitted_image_ids.len(), 2);
        }

        #[test]
        fn new_thumbnails_go_out_without_a_delete() {
            let mut app = thumbnail_app(3);
            let mut out = Vec::new();

            emit_new_thumbnails_to(&mut app, &[1, 2, 5], &mut out).unwrap();

            let text = text(&out);
            assert_eq!(text.matches(TRANSMIT).count(), 2);
            assert!(!text.contains("\x1b_Ga=d"));
            assert_eq!(app.transmitted_image_ids.len(), 2);
            assert!(text.contains(&format!("\x1b[2;{}H", app.gallery.thumb_cols + 2)));
        }

        #[cfg(feature = "video")]
        #[test]
        fn a_video_without_a_frame_writes_nothing() {
            let app = App::new(Theme::default(), (8, 16), SharedImageList::new());
            let mut out = Vec::new();

            super::super::render_video_frame_to(&app, &mut out).unwrap();

            assert!(out.is_empty());
        }

        #[test]
        fn the_cell_size_is_never_zero() {
            let (width, height) = query_cell_pixel_size();

            assert!(width >= 1 && height >= 1);
        }

        fn window(columns: u16, rows: u16, width: u16, height: u16) -> WindowSize {
            WindowSize {
                rows,
                columns,
                width,
                height,
            }
        }

        #[test]
        fn the_cell_size_divides_the_pixels_by_the_cells() {
            assert_eq!(cell_pixel_size(&window(100, 50, 1000, 1100)), (10, 22));
        }

        #[test]
        fn a_missing_pixel_report_gives_the_default_cell_size() {
            assert_eq!(cell_pixel_size(&window(100, 50, 0, 0)), (8, 16));
            assert_eq!(cell_pixel_size(&window(0, 0, 1000, 1100)), (8, 16));
            assert_eq!(cell_pixel_size(&window(100, 50, 0, 1100)), (8, 22));
        }

        #[test]
        fn a_tiny_pixel_report_gives_at_least_one_pixel() {
            assert_eq!(cell_pixel_size(&window(100, 50, 10, 10)), (1, 1));
        }

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
}
