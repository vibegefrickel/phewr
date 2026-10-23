mod app;
#[cfg(test)]
mod bench;
mod cli;
mod config;
mod decode;
mod encoder;
mod file_meta;
mod gallery;
mod icons;
mod image_list;
mod info;
mod keys;
mod kitty_output;
#[cfg(feature = "video")]
mod mpv;
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
use cli::{Cli, initial_dir_from_paths, start_paths};
use image_list::SharedImageList;
use run_loop::{LoopState, run};
use scanner::canonicalize_or_self;
use std::io;
use std::path::PathBuf;
use terminal::{
    TerminalSession, query_cell_pixel_size, query_cell_size_and_probe, window_cell_pixel_size,
};
use yazi::{reveal_in_yazi, run_briefly};

fn main() -> io::Result<()> {
    let cli = Cli::parse();
    // The thumbnails and the terminal player start ffmpeg and ffprobe when PATH has them.
    #[cfg(feature = "video")]
    video::find_session_tools();
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

    let mut config = config::load();
    decode::color::set_target(config.colorspace);
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

    let (paths, cursor_target) = start_paths(paths);
    let initial_dir = initial_dir_from_paths(&paths);

    let shared_list = SharedImageList::new();
    scanner::spawn(paths, shared_list.clone());

    let mut session = TerminalSession::enter(config.mouse)?;

    // The cell size query goes out with the probe, in one round trip. Without a probe, it goes
    // out alone.
    let mut cell_size_answer = None;
    let mut asked_cell_size = false;
    // A probe without an answer in time can still get one: it then arrives as key presses.
    let mut probe_unanswered = false;
    let transfer = encoder::kitty::detect_transfer(
        std::env::var(encoder::kitty::TRANSFER_OVERRIDE)
            .ok()
            .as_deref(),
        encoder::kitty::is_ssh(),
        |sequence| {
            let answers = query_cell_size_and_probe(sequence);
            asked_cell_size = true;
            cell_size_answer = answers.cell_px;
            probe_unanswered = answers.probe.is_none();
            answers.probe
        },
    );
    if !asked_cell_size {
        cell_size_answer = query_cell_pixel_size();
    }
    // A terminal that answers the cell size query at start gets it again after each resize.
    state.cell_size_from_terminal = cell_size_answer.is_some();
    let cell_px = cell_size_answer.unwrap_or_else(window_cell_pixel_size);
    state.late_answers = keys::LateAnswerFilter::new(probe_unanswered);
    let mut app = App::with_config(cell_px, shared_list, &config);
    app.graphics = encoder::KittyBackend::new(transfer);
    // The first status note tells which medium the probe found. The first key clears it.
    state.status = Some(format!(
        "Kitty transfer: {}",
        encoder::kitty::describe(transfer)
    ));
    let initial_dir = canonicalize_or_self(&initial_dir);
    app.initial_dir = Some(initial_dir.clone());
    app.current_dir = Some(initial_dir.clone());
    app.apply_folder_zoom(&initial_dir);
    if let Some(target) = cursor_target {
        app.place_cursor_after_scan(target);
    }
    app.slideshow_autostart = cli.slideshow;
    let run_result = run(session.terminal_mut(), &mut app, &mut state);
    let restore_result = session.restore();
    if run_result.is_ok() && restore_result.is_ok() {
        // viewr quits anyway. A yazi that does not answer keeps its old cursor.
        reveal_in_yazi(
            app.reveal_path().as_deref(),
            std::env::var_os("YAZI_ID").as_deref(),
            std::env::var_os("PATH").as_deref(),
            run_briefly,
        );
    }
    run_result.and(restore_result)
}
