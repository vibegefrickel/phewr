mod app;
#[cfg(test)]
mod bench;
mod cli;
mod config;
mod decode;
mod encoder;
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
use crossterm::{
    cursor, execute,
    terminal::{LeaveAlternateScreen, disable_raw_mode},
};
use image_list::SharedImageList;
use run_loop::{LoopState, run};
use scanner::canonicalize_or_self;
use std::io::{self, Write};
use std::path::PathBuf;
use terminal::{
    TerminalSession, query_cell_pixel_size, query_cell_size_and_probe, window_cell_pixel_size,
};
use yazi::{reveal_in_yazi, run_briefly};

/// Give the terminal back in the order of `TerminalSession::restore`. `disable_raw` runs
/// between the mouse release and the screen leave. Every step runs, and the first error
/// returns, so that a failed picture delete cannot leave the shell in raw mode.
fn release_terminal_to(
    out: &mut impl Write,
    mouse: bool,
    disable_raw: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let mut first_error = encoder::kitty::delete_all_to(out)
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

/// Leave the alternate screen before the panic message prints, because the restore at the end
/// of the session would discard text that the default hook writes to the alternate screen. Only
/// a panic of the main thread releases the terminal: viewr survives a panic of a worker thread.
/// A main thread panic ends the process after the message, so that the unwind does not restore
/// the terminal a second time on the normal screen, where `CSI ?1049l` moves the cursor.
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
        let Ok(metadata) = std::fs::metadata(p) else {
            eprintln!("{}: file not found", p.display());
            std::process::exit(1);
        };
        if metadata.is_file() && !scanner::is_supported(p) {
            eprintln!("{}: unsupported media format", p.display());
            std::process::exit(1);
        }
    }

    let mut config = config::load(&config::Paths::from_env());
    decode::color::set_target(config.colorspace);
    if let Some(interval) = cli.interval {
        config.slideshow_interval = interval;
    }
    if cli.info {
        config.info_panel = true;
    }
    let mut state = LoopState::new(&config);

    let (paths, cursor_target) = start_paths(paths);
    let initial_dir = initial_dir_from_paths(&paths);

    let shared_list = SharedImageList::new();
    scanner::spawn(paths, shared_list.clone());

    install_panic_hook(config.mouse);
    let mut session = TerminalSession::enter(config.mouse)?;

    // The cell size query goes out with the probe, in one round trip. Without a probe, it goes
    // out alone.
    let mut cell_size_answer = None;
    let mut asked_cell_size = false;
    // A probe without an answer in time can still get one: it then arrives as key presses.
    let mut late_answers = 0;
    let loops = encoder::kitty::chosen_loops(
        std::env::var(encoder::kitty::LOOPS_OVERRIDE)
            .ok()
            .as_deref(),
        config.loops,
    );
    let graphics = encoder::kitty::detect_backend(
        std::env::var(encoder::kitty::TRANSFER_OVERRIDE)
            .ok()
            .as_deref(),
        loops,
        encoder::kitty::is_ssh(),
        |sequence, answers| {
            let start = query_cell_size_and_probe(sequence, answers);
            asked_cell_size = true;
            cell_size_answer = start.cell_px;
            late_answers = start.late_answers;
            start.probe
        },
    );
    if !asked_cell_size {
        cell_size_answer = query_cell_pixel_size();
    }
    // A terminal that answers the cell size query at start gets it again after each resize.
    state.cell_size_from_terminal = cell_size_answer.is_some();
    let cell_px = cell_size_answer.unwrap_or_else(window_cell_pixel_size);
    state.late_answers = keys::LateAnswerFilter::new(late_answers);
    let mut app = App::with_config(cell_px, shared_list, &config);
    app.graphics = graphics;
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
    let reveal_path = app.reveal_path();
    // The drop cancels the loop jobs, so their ffmpeg runs end before the process does.
    drop(app);
    if run_result.is_ok() && restore_result.is_ok() {
        // viewr quits anyway. A yazi that does not answer keeps its old cursor.
        reveal_in_yazi(
            reveal_path.as_deref(),
            std::env::var_os("YAZI_ID").as_deref(),
            std::env::var_os("PATH").as_deref(),
            run_briefly,
        );
    }
    run_result.and(restore_result)
}

#[cfg(test)]
mod tests {
    use super::release_terminal_to;

    #[test]
    fn the_release_deletes_pictures_then_leaves_the_alternate_screen() {
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
}
