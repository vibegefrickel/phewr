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
use std::sync::mpsc;
use std::thread;

/// The status note when the configuration asks for mpv and PATH has none.
pub(crate) const MISSING_NOTE: &str = "mpv not found on PATH: built-in player";

/// The exit code of mpv after a quit by the user in [`Playback::Once`]: `q`, `Q`, `Ctrl+C` and,
/// in a window, the close button. mpv also uses it after a signal.
pub(crate) const USER_QUIT: i32 = 4;

/// How mpv plays the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Playback {
    /// Play the file again and again, until the user quits mpv.
    Loop,
    /// Play the file one time for a running slideshow. Only the end of the file exits with 0,
    /// because each key that quits mpv exits with [`USER_QUIT`].
    Once,
}

/// True when mpv in [`Playback::Once`] played the file to its end, and the user did not quit.
pub(crate) fn reached_the_end(code: Option<i32>) -> bool {
    code == Some(0)
}

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
pub(crate) fn terminal_args(
    video: &Path,
    geometry: Option<Geometry>,
    playback: Playback,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "--vo=kitty",
        "--vo-kitty-alt-screen=no",
        "--profile=sw-fast",
        "--really-quiet",
        "--terminal=yes",
        "--input-terminal=yes",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    push_playback(&mut args, playback, &["q", "Q"]);
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
pub(crate) fn window_args(video: &Path, playback: Playback) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["--no-terminal", "--force-window=immediate"]
        .into_iter()
        .map(OsString::from)
        .collect();
    push_playback(&mut args, playback, &["q", "Q", "CLOSE_WIN"]);
    push_file(&mut args, video);
    args
}

/// Append the loop options.
///
/// [`Playback::Once`] turns off each option of a user `mpv.conf` that keeps mpv open after the
/// file: `keep-open`, `idle` and `loop-playlist`. It also binds each key in `quit_keys` to an
/// exit with [`USER_QUIT`], because a plain `quit` exits with 0, like the end of the file.
/// `Ctrl+C` quits with that code already. `--input-commands` needs mpv 0.38 or later.
///
/// A key binding cannot catch the quit of the macOS app (Cmd+Q, "Quit mpv" in the app menu).
/// The quit event of the app also ends mpv 0.41 with 0, so a window slideshow continues.
fn push_playback(args: &mut Vec<OsString>, playback: Playback, quit_keys: &[&str]) {
    match playback {
        Playback::Loop => args.push(OsString::from("--loop-file=inf")),
        Playback::Once => {
            args.extend(
                [
                    "--loop-file=no",
                    "--keep-open=no",
                    "--idle=no",
                    "--loop-playlist=no",
                ]
                .map(OsString::from),
            );
            let bindings: Vec<String> = quit_keys
                .iter()
                .map(|key| format!("keybind {key} \"quit {USER_QUIT}\""))
                .collect();
            args.push(OsString::from(format!(
                "--input-commands={}",
                bindings.join(",")
            )));
        }
    }
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
    /// Start `program` without a terminal and return at once. The receiver gets the exit code
    /// when the process ends, so that a slideshow can wait for the window without a block.
    fn spawn(
        &mut self,
        program: &Path,
        args: &[OsString],
    ) -> io::Result<mpsc::Receiver<Option<i32>>>;
}

/// The [`Runner`] that starts real processes.
pub(crate) struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&mut self, program: &Path, args: &[OsString]) -> io::Result<Option<i32>> {
        Ok(Command::new(program).args(args).status()?.code())
    }

    fn spawn(
        &mut self,
        program: &Path,
        args: &[OsString],
    ) -> io::Result<mpsc::Receiver<Option<i32>>> {
        // A process group of its own keeps the job control signals of the terminal away.
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?;
        // Wait in a thread, so that the exited window process does not stay as a zombie.
        let (exit_tx, exit_rx) = mpsc::channel();
        thread::spawn(move || {
            let code = child.wait().ok().and_then(|status| status.code());
            #[expect(
                clippy::let_underscore_must_use,
                reason = "outside a slideshow, nobody waits for the exit of the window"
            )]
            let _ = exit_tx.send(code);
        });
        Ok(exit_rx)
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
        let args = terminal_args(Path::new("-clip.mp4"), None, Playback::Loop);
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
        let args = strings(&terminal_args(
            Path::new("/videos/a.mov"),
            geometry,
            Playback::Loop,
        ));
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
            strings(&window_args(Path::new("b.webm"), Playback::Loop)),
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
    fn once_plays_without_the_loop_and_gives_the_quit_keys_their_own_code() {
        let terminal = strings(&terminal_args(Path::new("a.mp4"), None, Playback::Once));
        assert_eq!(
            terminal[6..],
            [
                "--loop-file=no",
                "--keep-open=no",
                "--idle=no",
                "--loop-playlist=no",
                r#"--input-commands=keybind q "quit 4",keybind Q "quit 4""#,
                "--",
                "a.mp4",
            ]
        );
        let window = strings(&window_args(Path::new("b.webm"), Playback::Once));
        assert_eq!(
            window,
            [
                "--no-terminal",
                "--force-window=immediate",
                "--loop-file=no",
                "--keep-open=no",
                "--idle=no",
                "--loop-playlist=no",
                r#"--input-commands=keybind q "quit 4",keybind Q "quit 4",keybind CLOSE_WIN "quit 4""#,
                "--",
                "b.webm",
            ]
        );
    }

    #[test]
    fn only_exit_code_zero_is_the_end_of_the_file() {
        assert!(reached_the_end(Some(0)));
        assert!(!reached_the_end(Some(USER_QUIT)));
        assert!(!reached_the_end(Some(2)));
        assert!(!reached_the_end(None));
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
        let root = tempfile::tempdir().unwrap();
        let plain = root.path().join("plain");
        let executable = root.path().join("executable");
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

    #[test]
    fn a_path_without_a_file_name_names_the_whole_path() {
        assert_eq!(file_name(Path::new("/")), "/");
        assert_eq!(file_name(Path::new("/videos/d.mp4")), "d.mp4");
    }

    #[test]
    fn system_runner_returns_the_exit_code_of_the_program() {
        let shell = Path::new("/bin/sh");
        let args = [OsString::from("-c"), OsString::from("exit 3")];
        assert_eq!(SystemRunner.run(shell, &args).unwrap(), Some(3));
        assert!(
            SystemRunner
                .run(Path::new("/nonexistent/mpv"), &[])
                .is_err()
        );
    }

    #[test]
    fn system_runner_spawns_a_detached_program_and_reports_its_exit() {
        let shell = Path::new("/bin/sh");
        let args = [OsString::from("-c"), OsString::from("exit 5")];
        let exit = SystemRunner.spawn(shell, &args).unwrap();
        assert_eq!(
            exit.recv_timeout(std::time::Duration::from_secs(10)),
            Ok(Some(5))
        );
        assert!(
            SystemRunner
                .spawn(Path::new("/nonexistent/mpv"), &[])
                .is_err()
        );
    }
}
