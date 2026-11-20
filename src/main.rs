//! phewr: a terminal image and video gallery for the Kitty graphics protocol.

mod app;
#[cfg(test)]
mod bench;
mod cli;
mod config;
mod file_meta;
mod gallery;
mod icons;
mod image_list;
mod info;
mod keys;
mod kitty_output;
#[cfg(feature = "video")]
mod mpv;
mod numbers;
#[cfg(feature = "video")]
mod player;
mod prefetch;
mod run_loop;
mod scanner;
mod search;
mod styles;
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
use image_list::SharedImageList;
use ratatui::{Terminal, backend::CrosstermBackend};
use run_loop::{LoopState, run};
use scanner::canonicalize_or_self;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tuilibr::pictures::kitty::KittyBackend;
use tuilibr::terminal::answers::LateAnswerFilter;
use tuilibr::terminal::start::{self, StartOptions};
use tuilibr::terminal::{Modes, Session, install_panic_hook};
use yazi::{reveal_in_yazi, run_briefly, select_in_yazi};
// Only the tests and the benches name these crates. The binary decodes through tuilibr.
#[cfg(all(not(test), feature = "heic"))]
use libheif_rs as _;
#[cfg(all(not(test), feature = "turbo"))]
use turbojpeg as _;

/// Skips the probe of the Kitty transfer: `direct`, `png` or `file`.
const TRANSFER_OVERRIDE: &str = "PHEWR_KITTY_TRANSFER";
/// Fixes the loop path: `animation` or `ticks`. It wins over `[gallery] loops`.
const LOOPS_OVERRIDE: &str = "PHEWR_KITTY_LOOPS";
/// `0` turns the self check of the Kitty placements off, for a comparison. It is on by default.
const SELF_CHECK_OVERRIDE: &str = "PHEWR_KITTY_SELF_CHECK";

/// True unless the override turns the self check off with `0`, `off`, `false` or `no`.
fn self_check_on(override_value: Option<&str>) -> bool {
    !override_value.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "off" | "false" | "no"
        )
    })
}

/// Delete the pictures before the panic message prints, and end the process after it. The hook
/// of tuilibr leaves the alternate screen and exits nowhere.
///
/// A main thread panic ends the process after the message. The unwind must not restore the
/// terminal again on the normal screen, where `CSI ?1049l` moves the cursor.
fn install_panic_exit() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() != Some("main") {
            previous(info);
            return;
        }
        #[expect(
            clippy::let_underscore_must_use,
            reason = "the terminal is in a panic, a failed delete cannot be reported"
        )]
        let _ = delete_pictures(&mut io::stdout());
        previous(info);
        #[expect(
            clippy::exit,
            reason = "the unwind would restore the terminal a second time on the normal screen"
        )]
        std::process::exit(101);
    }));
}

/// Delete every picture of the terminal. The session deletes none.
fn delete_pictures(out: &mut impl Write) -> io::Result<()> {
    tuilibr::pictures::kitty::protocol::delete_all_to(out)?;
    out.flush()
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

const fn apply_cli_overrides(config: &mut Config, interval: Option<f64>, info: bool) {
    if let Some(interval) = interval {
        config.slideshow_interval = interval;
    }
    if info {
        config.info_panel = true;
    }
}

/// Point the app at the start folder: its zoom level, the cursor target and the slideshow. The
/// screen gets the self check of its placements.
fn prepare_app(
    app: &mut App,
    graphics: KittyBackend,
    self_check: bool,
    initial_dir: &Path,
    cursor_target: Option<PathBuf>,
    slideshow: bool,
) {
    app.graphics = graphics;
    app.screen = std::mem::take(&mut app.screen).with_self_check(self_check);
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
    tuilibr::pictures::names::set_app_name(env!("CARGO_PKG_NAME"));
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
    tuilibr::pictures::decode::color::set_target(config.colorspace);
    apply_cli_overrides(&mut config, cli.interval, cli.info);
    let mut state = LoopState::new(&config);

    let (paths, cursor_target) = start_paths(paths);
    let initial_dir = initial_dir_from_paths(&paths);
    let file_list = !matches!(paths.as_slice(), [only] if only.is_dir());

    let shared_list = SharedImageList::new();
    scanner::spawn_with(paths, shared_list.clone(), config.live_photos);

    let modes = Modes {
        mouse: config.mouse,
        ..Modes::default()
    };
    install_panic_hook(modes);
    install_panic_exit();
    let mut session = Session::enter(modes)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let loops = tuilibr::pictures::kitty::protocol::chosen_loops(
        std::env::var(LOOPS_OVERRIDE).ok().as_deref(),
        config.loops,
        tuilibr::pictures::kitty::protocol::is_herdr(),
    );
    let transfer_override = std::env::var(TRANSFER_OVERRIDE).ok();
    let start = start::query(StartOptions {
        transfer_override: transfer_override.as_deref(),
        loops,
        ssh: tuilibr::pictures::kitty::protocol::is_ssh(),
    });
    // A terminal that answers the cell size query gets it again after each resize.
    state.cell_size_from_terminal = start.cell_from_terminal;
    let cell_px = (u32::from(start.cell.width), u32::from(start.cell.height));
    state.answers = LateAnswerFilter::new(start.owed);
    let mut app = App::with_config(cell_px, shared_list, &config, config_paths.state);
    app.grid_icons.set_color(start.foreground);
    prepare_app(
        &mut app,
        start.backend,
        self_check_on(std::env::var(SELF_CHECK_OVERRIDE).ok().as_deref()),
        &initial_dir,
        cursor_target,
        cli.slideshow,
    );
    app.file_list = file_list;
    app.home_dir = std::env::home_dir();
    let run_result = run(&mut terminal, &mut session, &mut app, &mut state);
    let delete_result = delete_pictures(terminal.backend_mut());
    let restore_result = session.restore();
    let reveal_path = app.reveal_path();
    let marked = app.marked_paths();
    let gallery_dir = app.gallery_dir();
    // The drop cancels the loop jobs. Their ffmpeg runs end before the process.
    drop(app);
    let mut result = run_result.and(delete_result).and(restore_result);
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
        apply_cli_overrides, check_paths, default_paths, delete_pictures, prepare_app,
        self_check_on,
    };
    use crate::config::Config;
    use std::path::PathBuf;
    use tuilibr::pictures::kitty::query::{LateAnswers, StartAnswers};
    use tuilibr::pictures::kitty::{KittyBackend, Loops, Transfer};
    use tuilibr::terminal::start::{StartOptions, query_with};

    #[test]
    fn delete_pictures_writes_delete_all() {
        let mut out = Vec::new();
        delete_pictures(&mut out).unwrap();
        assert!(String::from_utf8_lossy(&out).contains("a=d"));
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
    fn start_brings_cell_size() {
        let start = query_with(
            StartOptions {
                transfer_override: Some("direct"),
                ..StartOptions::default()
            },
            |sequence, answers| {
                assert!(!sequence.is_empty());
                assert_eq!(answers, 2, "the plain query and the animation probe");
                StartAnswers {
                    cell_px: Some((10, 20)),
                    probe: None,
                    late_answers: LateAnswers {
                        kitty: 1,
                        foreground: false,
                    },
                    foreground: Some([200, 200, 200]),
                }
            },
        );
        assert_eq!((start.cell.width, start.cell.height), (10, 20));
        assert!(start.cell_from_terminal);
        assert_eq!(
            start.owed,
            LateAnswers {
                kitty: 1,
                foreground: false
            }
        );
        assert_eq!(start.foreground, Some([200, 200, 200]));
        assert_eq!(start.backend.transfer(), Transfer::Direct);
        assert_eq!(start.backend.loops(), Loops::Ticks);
    }

    #[test]
    fn fixed_backend_asks_the_plain_query_alone() {
        let start = query_with(
            StartOptions {
                transfer_override: Some("png"),
                loops: Some(Loops::Animation),
                ssh: false,
            },
            |sequence, answers| {
                assert_eq!(answers, 1, "only the plain query goes out");
                assert!(sequence.starts_with(b"\x1b_Ga=q,i=30"));
                StartAnswers {
                    cell_px: Some((7, 14)),
                    probe: None,
                    late_answers: LateAnswers::default(),
                    foreground: None,
                }
            },
        );
        assert_eq!((start.cell.width, start.cell.height), (7, 14));
        assert_eq!(start.owed, LateAnswers::default());
        assert_eq!(start.foreground, None);
        assert_eq!(start.backend.transfer(), Transfer::Png);
        assert_eq!(start.backend.loops(), Loops::Animation);
    }

    #[test]
    fn prepare_points_app_at_start_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = crate::test_support::gallery_app(3);
        prepare_app(
            &mut app,
            KittyBackend::new(Transfer::TempFile),
            true,
            dir.path(),
            Some(dir.path().join("photo.jpg")),
            true,
        );
        let canonical = dir.path().canonicalize().unwrap();
        assert_eq!(app.graphics.transfer(), Transfer::TempFile);
        assert_eq!(app.initial_dir.as_deref(), Some(canonical.as_path()));
        assert_eq!(app.current_dir.as_deref(), Some(canonical.as_path()));
        assert!(app.slideshow_autostart);
        assert!(app.screen.self_checks());
    }

    #[test]
    fn self_check_is_on_unless_the_override_turns_it_off() {
        assert!(self_check_on(None));
        assert!(self_check_on(Some("1")));
        assert!(self_check_on(Some("on")));
        for off in ["0", "off", "false", "no", " OFF "] {
            assert!(!self_check_on(Some(off)), "{off}");
        }
    }
}
