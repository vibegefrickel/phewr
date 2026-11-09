//! phewr: a terminal image and video gallery for the Kitty graphics protocol.

mod app;
#[cfg(test)]
mod bench;
mod cli;
mod config;
mod file_meta;
mod gallery;
#[cfg(feature = "video")]
mod grid_loop;
mod icons;
mod image_list;
mod info;
mod keys;
mod kitty_output;
#[cfg(feature = "video")]
mod mpv;
// The shared picture module holds no lint level of its own: each project sets it here. Outside
// the tests, phewr leaves these items of the module without a caller:
// - always the box copies and the corners (`shape::copies`, `shape::corners`) and
//   `thumbnail_dir_below`;
// - without `video`, `Screen::forget_placements` and `protocol::clear_placements_to`;
// - with a `kitty-*` feature on (`just lint`), the engine parts that the feature gates.
// The tests call them, and clippy with `--all-targets` still finds a module item without any
// caller.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "phewr calls no box copy, no corner and no thumbnail_dir_below of the shared \
                  module, no placement clear without video, and no part of a kitty-* feature"
    )
)]
mod pictures;
#[cfg(feature = "video")]
mod player;
mod prefetch;
mod run_loop;
mod scanner;
mod search;
mod styles;
mod terminal;
#[cfg(test)]
mod test_support;
mod ui;
#[cfg(feature = "video")]
mod video;
#[cfg(feature = "video")]
mod video_handoff;
mod yazi;

use app::App;
use clap::Parser;
use cli::{Cli, initial_dir_from_paths, start_paths, write_cwd_file};
use config::Config;
use crossterm::{
    cursor, execute,
    terminal::{LeaveAlternateScreen, disable_raw_mode},
};
use image_list::SharedImageList;
use pictures::kitty::query::{
    StartAnswers, query_cell_pixel_size, query_cell_size_and_probe, window_cell_pixel_size,
};
use pictures::kitty::{KittyBackend, Loops};
use run_loop::{LoopState, run};
use scanner::canonicalize_or_self;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use terminal::TerminalSession;
use yazi::{reveal_in_yazi, run_briefly, select_in_yazi};

/// Skips the probe of the Kitty transfer: `direct`, `png` or `file`.
const TRANSFER_OVERRIDE: &str = "PHEWR_KITTY_TRANSFER";
/// Fixes the loop path: `animation` or `ticks`. It wins over `[gallery] loops`.
const LOOPS_OVERRIDE: &str = "PHEWR_KITTY_LOOPS";

/// Give the terminal back in the order of `TerminalSession::restore`. Every step runs, and the
/// first error returns. A failed picture delete cannot leave the shell in raw mode.
fn release_terminal_to(
    out: &mut impl Write,
    mouse: bool,
    disable_raw: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let mut first_error = pictures::kitty::protocol::delete_all_to(out)
        .and_then(|()| out.flush())
        .err();
    if mouse
        && let Err(error) = terminal::disable_mouse(out)
        && first_error.is_none()
    {
        first_error = Some(error);
    }
    if let Err(error) = disable_raw()
        && first_error.is_none()
    {
        first_error = Some(error);
    }
    if let Err(error) = execute!(out, cursor::Show, LeaveAlternateScreen)
        && first_error.is_none()
    {
        first_error = Some(error);
    }
    if let Err(error) = out.flush()
        && first_error.is_none()
    {
        first_error = Some(error);
    }
    first_error.map_or(Ok(()), Err)
}

/// Leave the alternate screen before the panic message prints. Otherwise the restore discards
/// the message. A worker panic does not release the terminal.
///
/// A main thread panic ends the process after the message. The unwind must not restore the
/// terminal again on the normal screen, where `CSI ?1049l` moves the cursor.
fn install_panic_hook(mouse: bool) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() != Some("main") {
            previous(info);
            return;
        }
        #[expect(
            clippy::let_underscore_must_use,
            reason = "the terminal is in a panic, a failed release cannot be reported"
        )]
        let _ = release_terminal_to(&mut io::stdout(), mouse, disable_raw_mode);
        previous(info);
        #[expect(
            clippy::exit,
            reason = "the unwind would restore the terminal a second time on the normal screen"
        )]
        std::process::exit(101);
    }));
}

/// The paths of the command line, or the working directory.
fn default_paths(files: Vec<PathBuf>) -> Vec<PathBuf> {
    if files.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        files
    }
}

/// Without `--threads`, rayon takes one thread per core.
fn configure_threads(threads: Option<usize>) -> io::Result<()> {
    let Some(n) = threads else {
        return Ok(());
    };
    rayon::ThreadPoolBuilder::new()
        .num_threads(n)
        .build_global()
        .map_err(|error| io::Error::other(format!("failed to configure thread pool: {error}")))
}

/// The CLI error for the first missing path or unsupported file.
fn check_paths(paths: &[PathBuf]) -> Result<(), String> {
    for p in paths {
        let Ok(metadata) = std::fs::metadata(p) else {
            return Err(format!("{}: file not found", p.display()));
        };
        if metadata.is_file() && !scanner::is_supported(p) {
            return Err(format!("{}: unsupported media format", p.display()));
        }
    }
    Ok(())
}

/// Print a CLI error before the TUI starts and end the process.
#[expect(
    clippy::print_stderr,
    clippy::exit,
    reason = "the CLI reports a bad path before the TUI starts"
)]
fn exit_with(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}

fn apply_cli_overrides(config: &mut Config, interval: Option<f64>, info: bool) {
    if let Some(interval) = interval {
        config.slideshow_interval = interval;
    }
    if info {
        config.info_panel = true;
    }
}

struct StartProbe {
    graphics: KittyBackend,
    cell_px: Option<(u32, u32)>,
    /// The Kitty answers that can still come late, as key presses.
    late_answers: usize,
    /// The terminal foreground. Without the round trip of the probe, phewr does not ask for it.
    foreground: Option<[u8; 3]>,
}

/// Probe the transfer, the loop path and the cell size in one round trip. Without a probe, the
/// cell size query goes out alone.
fn probe_terminal(
    transfer_override: Option<&str>,
    loops: Option<Loops>,
    ssh: bool,
    query_start: impl FnOnce(&[u8], usize) -> StartAnswers,
    query_cell: impl FnOnce() -> Option<(u32, u32)>,
) -> StartProbe {
    let mut cell_px = None;
    let mut asked_cell_size = false;
    let mut late_answers = 0;
    let mut foreground = None;
    let graphics = pictures::kitty::protocol::detect_backend(
        transfer_override,
        loops,
        ssh,
        |sequence, answers| {
            let start = query_start(sequence, answers);
            asked_cell_size = true;
            cell_px = start.cell_px;
            late_answers = start.late_answers;
            foreground = start.foreground;
            start.probe
        },
    );
    if !asked_cell_size {
        cell_px = query_cell();
    }
    StartProbe {
        graphics,
        cell_px,
        late_answers,
        foreground,
    }
}

/// Point the app at the start folder: its zoom level, the cursor target and the slideshow.
fn prepare_app(
    app: &mut App,
    graphics: KittyBackend,
    initial_dir: &Path,
    cursor_target: Option<PathBuf>,
    slideshow: bool,
) {
    app.graphics = graphics;
    let initial_dir = canonicalize_or_self(initial_dir);
    app.initial_dir = Some(initial_dir.clone());
    app.current_dir = Some(initial_dir.clone());
    app.apply_folder_zoom(&initial_dir);
    if let Some(target) = cursor_target {
        app.place_cursor_after_scan(target);
    }
    app.slideshow_autostart = slideshow;
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();
    // The thumbnails and the player start ffmpeg and ffprobe from PATH.
    #[cfg(feature = "video")]
    video::find_session_tools();
    let paths = default_paths(cli.files);
    configure_threads(cli.threads)?;
    if let Err(message) = check_paths(&paths) {
        exit_with(&message);
    }

    let config_paths = config::Paths::from_env();
    let mut config = config::load(&config_paths);
    pictures::decode::color::set_target(config.colorspace);
    apply_cli_overrides(&mut config, cli.interval, cli.info);
    let mut state = LoopState::new(&config);

    let (paths, cursor_target) = start_paths(paths);
    let initial_dir = initial_dir_from_paths(&paths);

    let shared_list = SharedImageList::new();
    scanner::spawn_with(paths, shared_list.clone(), config.live_photos);

    install_panic_hook(config.mouse);
    let mut session = TerminalSession::enter(config.mouse)?;

    let loops = pictures::kitty::protocol::chosen_loops(
        std::env::var(LOOPS_OVERRIDE).ok().as_deref(),
        config.loops,
        pictures::kitty::protocol::is_herdr(),
    );
    let probe = probe_terminal(
        std::env::var(TRANSFER_OVERRIDE).ok().as_deref(),
        loops,
        pictures::kitty::protocol::is_ssh(),
        query_cell_size_and_probe,
        query_cell_pixel_size,
    );
    // A terminal that answers the cell size query gets it again after each resize.
    state.cell_size_from_terminal = probe.cell_px.is_some();
    let cell_px = probe.cell_px.unwrap_or_else(window_cell_pixel_size);
    state.late_answers = keys::LateAnswerFilter::new(probe.late_answers);
    let mut app = App::with_config(cell_px, shared_list, &config, config_paths.state);
    app.grid_icons.set_color(probe.foreground);
    prepare_app(
        &mut app,
        probe.graphics,
        &initial_dir,
        cursor_target,
        cli.slideshow,
    );
    let run_result = run(session.terminal_mut(), &mut app, &mut state);
    let restore_result = session.restore();
    let reveal_path = app.reveal_path();
    let marked = app.marked_paths();
    let gallery_dir = app.gallery_dir();
    // The drop cancels the loop jobs. Their ffmpeg runs end before the process.
    drop(app);
    let mut result = run_result.and(restore_result);
    if result.is_ok() {
        // phewr quits anyway. A yazi that does not answer keeps its old cursor.
        reveal_in_yazi(
            reveal_path.as_deref(),
            std::env::var_os("YAZI_ID").as_deref(),
            std::env::var_os("PATH").as_deref(),
            run_briefly,
        );
        // The marks become the selection of that yazi, which does the file operations.
        select_in_yazi(
            &marked,
            std::env::var_os("YAZI_ID").as_deref(),
            std::env::var_os("PATH").as_deref(),
            &std::env::temp_dir(),
            run_briefly,
        );
        // The shell wrapper changes into the folder of the grid, like after yazi.
        if let Some(file) = &cli.cwd_file {
            result = write_cwd_file(file, &gallery_dir);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{
        apply_cli_overrides, check_paths, default_paths, prepare_app, probe_terminal,
        release_terminal_to,
    };
    use crate::config::Config;
    use crate::pictures::kitty::query::StartAnswers;
    use crate::pictures::kitty::{KittyBackend, Loops, Transfer};
    use std::path::PathBuf;

    #[test]
    fn release_deletes_pictures_first() {
        let mut out = Vec::new();
        let mut raw_calls = 0;
        release_terminal_to(&mut out, true, || {
            raw_calls += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(raw_calls, 1);
        let text = String::from_utf8_lossy(&out);
        let delete = text.find("a=d").expect("delete all");
        let mouse = text.find("\x1b[?1006l").expect("mouse off");
        let show = text.find("\x1b[?25h").expect("cursor shown");
        let leave = text.find("\x1b[?1049l").expect("alternate screen left");
        assert!(delete < mouse && mouse < show && show < leave);

        let mut without_mouse = Vec::new();
        release_terminal_to(&mut without_mouse, false, || Ok(())).unwrap();
        assert!(!String::from_utf8_lossy(&without_mouse).contains("\x1b[?1006l"));
    }

    #[test]
    fn no_files_mean_working_directory() {
        assert_eq!(default_paths(Vec::new()), vec![PathBuf::from(".")]);
        let files = vec![PathBuf::from("a.jpg"), PathBuf::from("b")];
        assert_eq!(default_paths(files.clone()), files);
    }

    #[test]
    fn check_paths_names_first_bad_path() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("photo.jpg");
        let text = dir.path().join("notes.txt");
        std::fs::write(&photo, b"").unwrap();
        std::fs::write(&text, b"").unwrap();
        let missing = dir.path().join("gone.jpg");
        assert_eq!(
            check_paths(&[dir.path().to_path_buf(), photo.clone()]),
            Ok(())
        );
        assert_eq!(
            check_paths(&[photo, missing.clone(), text.clone()]),
            Err(format!("{}: file not found", missing.display()))
        );
        assert_eq!(
            check_paths(&[text.clone(), missing]),
            Err(format!("{}: unsupported media format", text.display()))
        );
    }

    #[test]
    fn flags_override_config() {
        let mut config = Config::default();
        apply_cli_overrides(&mut config, None, false);
        assert_eq!(config, Config::default());
        apply_cli_overrides(&mut config, Some(1.5), true);
        assert_eq!(config.slideshow_interval, 1.5);
        assert!(config.info_panel);
    }

    #[test]
    fn probe_brings_cell_size() {
        let probe = probe_terminal(
            Some("direct"),
            None,
            false,
            |sequence, answers| {
                assert!(!sequence.is_empty());
                assert_eq!(answers, 1, "only the animation probe goes out");
                StartAnswers {
                    cell_px: Some((10, 20)),
                    probe: None,
                    late_answers: 1,
                    foreground: Some([200, 200, 200]),
                }
            },
            || panic!("the cell size goes out with the probe"),
        );
        assert_eq!(probe.cell_px, Some((10, 20)));
        assert_eq!(probe.late_answers, 1);
        assert_eq!(probe.foreground, Some([200, 200, 200]));
        assert_eq!(probe.graphics.transfer(), Transfer::Direct);
        assert_eq!(probe.graphics.loops(), Loops::Ticks);
    }

    #[test]
    fn fixed_backend_asks_cell_size_alone() {
        let probe = probe_terminal(
            Some("png"),
            Some(Loops::Animation),
            false,
            |_, _| panic!("a fixed backend sends no probe"),
            || Some((7, 14)),
        );
        assert_eq!(probe.cell_px, Some((7, 14)));
        assert_eq!(probe.late_answers, 0);
        assert_eq!(probe.foreground, None);
        assert_eq!(probe.graphics.transfer(), Transfer::Png);
        assert_eq!(probe.graphics.loops(), Loops::Animation);
    }

    #[test]
    fn prepare_points_app_at_start_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = crate::test_support::gallery_app(3);
        prepare_app(
            &mut app,
            KittyBackend::new(Transfer::TempFile),
            dir.path(),
            Some(dir.path().join("photo.jpg")),
            true,
        );
        let canonical = dir.path().canonicalize().unwrap();
        assert_eq!(app.graphics.transfer(), Transfer::TempFile);
        assert_eq!(app.initial_dir.as_deref(), Some(canonical.as_path()));
        assert_eq!(app.current_dir.as_deref(), Some(canonical.as_path()));
        assert!(app.slideshow_autostart);
    }
}
