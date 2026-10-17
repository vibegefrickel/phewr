//! Video playback through mpv: inside this terminal with `--vo=kitty`, or in a window of its own.
//!
//! The process start sits behind [`Runner`], so the tests use a fake and never start mpv.

use crate::config::VideoPlayer;
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

/// The status note when the configuration asks for mpv and PATH has none.
pub(crate) const MISSING_NOTE: &str = "mpv not found on PATH: built-in player";

/// The player after the PATH lookup. The mpv variants hold the path of the program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Player {
    Terminal(PathBuf),
    Window(PathBuf),
    Builtin,
}

/// Turn the configured player into one that can run; without mpv, use the built-in player.
pub(crate) fn resolve(configured: VideoPlayer, mpv: Option<PathBuf>) -> Player {
    match (configured, mpv) {
        (VideoPlayer::Builtin, _) | (_, None) => Player::Builtin,
        (VideoPlayer::Terminal, Some(mpv)) => Player::Terminal(mpv),
        (VideoPlayer::Window, Some(mpv)) => Player::Window(mpv),
    }
}

/// The first executable `program` in the directories of `path`, like the lookup of a shell.
pub(crate) fn find_on_path(program: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    std::env::split_paths(path?)
        .filter(|directory| !directory.as_os_str().is_empty())
        .map(|directory| directory.join(program))
        .find(|candidate| {
            candidate.metadata().is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
}

/// The terminal size for mpv, in cells and in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Geometry {
    pub cols: u16,
    pub rows: u16,
    pub width: u32,
    pub height: u32,
}

/// The size that mpv must get on the command line, because it cannot read a pixel size of 0.
///
/// A terminal that reports its pixel size gets `None`: mpv reads the size itself and follows
/// a resize. `size` is the report of the terminal: columns, rows, width and height in pixels.
pub(crate) fn fallback_geometry(
    size: (u16, u16, u16, u16),
    cell_px: (u32, u32),
) -> Option<Geometry> {
    let (cols, rows, width, height) = size;
    if width > 0 && height > 0 {
        return None;
    }
    Some(Geometry {
        cols,
        rows,
        width: u32::from(cols) * cell_px.0,
        height: u32::from(rows) * cell_px.1,
    })
}

/// mpv in the foreground of this terminal: Kitty graphics, keys from the terminal, no text output.
///
/// mpv draws into the alternate screen of viewr. A screen of its own shows the main screen of
/// the shell for a moment at the start and at the end.
pub(crate) fn terminal_args(video: &Path, geometry: Option<Geometry>) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "--vo=kitty",
        "--vo-kitty-alt-screen=no",
        "--profile=sw-fast",
        "--really-quiet",
        "--terminal=yes",
        "--input-terminal=yes",
        "--loop-file=inf",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    if let Some(geometry) = geometry {
        args.extend(
            [
                format!("--vo-kitty-cols={}", geometry.cols),
                format!("--vo-kitty-rows={}", geometry.rows),
                format!("--vo-kitty-width={}", geometry.width),
                format!("--vo-kitty-height={}", geometry.height),
            ]
            .map(OsString::from),
        );
    }
    push_file(&mut args, video);
    args
}

/// mpv in a window of its own. `--no-terminal` keeps it away from the keys of viewr.
pub(crate) fn window_args(video: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "--no-terminal",
        "--force-window=immediate",
        "--loop-file=inf",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    push_file(&mut args, video);
    args
}

/// Append the file after `--`, so that a name that starts with `-` is not an option.
fn push_file(args: &mut Vec<OsString>, video: &Path) {
    args.push(OsString::from("--"));
    args.push(video.as_os_str().to_owned());
}

/// The status note for an mpv exit code, or `None` when the user quit mpv.
///
/// mpv exits with 0 when the user quits and with 4 after a signal. A process that a signal
/// killed has no code.
pub(crate) fn exit_note(code: Option<i32>, video: &Path) -> Option<String> {
    match code {
        None | Some(0 | 4) => None,
        Some(code) => Some(format!(
            "mpv could not play {} (exit code {code})",
            file_name(video)
        )),
    }
}

/// The file name for a status note, or the whole path if it has no file name.
pub(crate) fn file_name(video: &Path) -> String {
    video.file_name().map_or_else(
        || video.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Starts mpv. The tests swap in a fake that records the calls.
pub(crate) trait Runner {
    /// Run `program` in the foreground with the terminal of viewr, and wait for its exit code.
    fn run(&mut self, program: &Path, args: &[OsString]) -> io::Result<Option<i32>>;
    /// Start `program` without a terminal and return at once.
    fn spawn(&mut self, program: &Path, args: &[OsString]) -> io::Result<()>;
}

/// The [`Runner`] that starts real processes.
pub(crate) struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&mut self, program: &Path, args: &[OsString]) -> io::Result<Option<i32>> {
        Ok(Command::new(program).args(args).status()?.code())
    }

    fn spawn(&mut self, program: &Path, args: &[OsString]) -> io::Result<()> {
        // A process group of its own keeps the job control signals of the terminal away.
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?;
        // Wait in a thread, so that the exited window process does not stay as a zombie.
        thread::spawn(move || {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "nobody reads the exit status of a detached window"
            )]
            let _ = child.wait();
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn terminal_args_use_kitty_and_loop_and_end_with_the_file() {
        let args = terminal_args(Path::new("-clip.mp4"), None);
        assert_eq!(
            strings(&args),
            [
                "--vo=kitty",
                "--vo-kitty-alt-screen=no",
                "--profile=sw-fast",
                "--really-quiet",
                "--terminal=yes",
                "--input-terminal=yes",
                "--loop-file=inf",
                "--",
                "-clip.mp4",
            ]
        );
    }

    #[test]
    fn terminal_args_carry_the_geometry_when_the_terminal_reports_no_pixels() {
        let geometry = fallback_geometry((100, 30, 0, 0), (8, 16));
        assert_eq!(
            geometry,
            Some(Geometry {
                cols: 100,
                rows: 30,
                width: 800,
                height: 480,
            })
        );
        let args = strings(&terminal_args(Path::new("/videos/a.mov"), geometry));
        let options = &args[7..11];
        assert_eq!(
            options,
            [
                "--vo-kitty-cols=100",
                "--vo-kitty-rows=30",
                "--vo-kitty-width=800",
                "--vo-kitty-height=480",
            ]
        );
        assert_eq!(&args[11..], ["--", "/videos/a.mov"]);
    }

    #[test]
    fn a_terminal_with_a_pixel_size_lets_mpv_measure() {
        assert_eq!(fallback_geometry((100, 30, 1600, 960), (8, 16)), None);
    }

    #[test]
    fn window_args_detach_from_the_terminal() {
        assert_eq!(
            strings(&window_args(Path::new("b.webm"))),
            [
                "--no-terminal",
                "--force-window=immediate",
                "--loop-file=inf",
                "--",
                "b.webm",
            ]
        );
    }

    #[test]
    fn a_missing_mpv_falls_back_to_the_builtin_player() {
        let mpv = PathBuf::from("/opt/homebrew/bin/mpv");
        assert_eq!(resolve(VideoPlayer::Terminal, None), Player::Builtin);
        assert_eq!(resolve(VideoPlayer::Window, None), Player::Builtin);
        assert_eq!(
            resolve(VideoPlayer::Builtin, Some(mpv.clone())),
            Player::Builtin
        );
        assert_eq!(
            resolve(VideoPlayer::Terminal, Some(mpv.clone())),
            Player::Terminal(mpv.clone())
        );
        assert_eq!(
            resolve(VideoPlayer::Window, Some(mpv.clone())),
            Player::Window(mpv)
        );
    }

    #[test]
    fn find_on_path_takes_the_first_executable_file() {
        let root = std::env::temp_dir().join(format!("viewr-mpv-path-{}", std::process::id()));
        let plain = root.join("plain");
        let executable = root.join("executable");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::create_dir_all(&executable).unwrap();
        std::fs::write(plain.join("mpv"), "").unwrap();
        std::fs::write(executable.join("mpv"), "").unwrap();
        std::fs::set_permissions(
            executable.join("mpv"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();

        let path = std::env::join_paths([&plain, &executable]).unwrap();
        assert_eq!(
            find_on_path("mpv", Some(&path)),
            Some(executable.join("mpv"))
        );
        assert_eq!(find_on_path("mpv", Some(OsStr::new(""))), None);
        assert_eq!(find_on_path("mpv", None), None);
        let only_plain = std::env::join_paths([&plain]).unwrap();
        assert_eq!(find_on_path("mpv", Some(&only_plain)), None);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exit_codes_of_a_quit_give_no_note() {
        let video = Path::new("/videos/c.mkv");
        assert_eq!(exit_note(Some(0), video), None);
        assert_eq!(exit_note(Some(4), video), None);
        assert_eq!(exit_note(None, video), None);
        assert_eq!(
            exit_note(Some(2), video).as_deref(),
            Some("mpv could not play c.mkv (exit code 2)")
        );
    }
}
