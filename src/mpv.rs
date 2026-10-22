//! Video playback through mpv in a fullscreen window of its own, with sound.
//!
//! The process start sits behind [`Runner`], so the tests use a fake and never start mpv.

use std::ffi::OsString;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

/// The exit code of mpv after a quit by the user in [`Playback::Once`]: `q`, `Q`, `Ctrl+C` and,
/// in a window, the close button. mpv also uses it after a signal.
pub(crate) const USER_QUIT: i32 = 4;

/// The exit code of mpv after `n`: viewr goes on to the next file of the list.
pub(crate) const NEXT_FILE: i32 = 10;

/// The exit code of mpv after `N` (Shift+n): viewr goes back to the previous file of the list.
pub(crate) const PREVIOUS_FILE: i32 = 11;

/// The keys that step through the list from inside mpv, with their exit codes.
///
/// mpv 0.41 binds neither `n` nor `N` by default, so the seek keys (the arrows) and the
/// playlist keys (`<`, `>`, Enter) keep their function.
const STEP_KEYS: [(&str, i32); 2] = [("n", NEXT_FILE), ("N", PREVIOUS_FILE)];

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

/// A step through the list that the user asked for inside mpv.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Next,
    Previous,
}

/// The step that the exit code of mpv asks for, or `None` for each other exit.
pub(crate) fn step(code: Option<i32>) -> Option<Step> {
    match code {
        Some(NEXT_FILE) => Some(Step::Next),
        Some(PREVIOUS_FILE) => Some(Step::Previous),
        _ => None,
    }
}

/// mpv in a window of its own. `--no-terminal` keeps it away from the keys of viewr.
///
/// The window uses the GPU output of mpv. `--hwdec=auto-safe` gives it the hardware frames
/// without a copy, and the GPU output does its own HDR tone mapping.
pub(crate) fn window_args(video: &Path, playback: Playback) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "--no-terminal",
        "--force-window=immediate",
        "--fs",
        "--native-fs=no",
        "--hwdec=auto-safe",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    push_playback(&mut args, playback, &["q", "Q", "CLOSE_WIN"]);
    push_file(&mut args, video);
    args
}

/// Append the loop options and the key bindings.
///
/// Each playback binds `n` and `N` to an exit with [`NEXT_FILE`] and [`PREVIOUS_FILE`], see
/// [`STEP_KEYS`]. [`Playback::Once`] turns off each option of a user `mpv.conf` that keeps mpv
/// open after the file: `keep-open`, `idle` and `loop-playlist`. It also binds each key in
/// `quit_keys` to an exit with [`USER_QUIT`], because a plain `quit` exits with 0, like the end
/// of the file. `Ctrl+C` quits with that code already. `--input-commands` needs mpv 0.38 or
/// later.
///
/// A key binding cannot catch the quit of the macOS app (Cmd+Q, "Quit mpv" in the app menu).
/// The quit event of the app also ends mpv 0.41 with 0, so a window slideshow continues.
fn push_playback(args: &mut Vec<OsString>, playback: Playback, quit_keys: &[&str]) {
    let mut bindings: Vec<(&str, i32)> = STEP_KEYS.to_vec();
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
            bindings.extend(quit_keys.iter().map(|key| (*key, USER_QUIT)));
        }
    }
    let bindings: Vec<String> = bindings
        .iter()
        .map(|(key, code)| format!("keybind {key} \"quit {code}\""))
        .collect();
    args.push(OsString::from(format!(
        "--input-commands={}",
        bindings.join(",")
    )));
}

/// Append the file after `--`, so that a name that starts with `-` is not an option.
fn push_file(args: &mut Vec<OsString>, video: &Path) {
    args.push(OsString::from("--"));
    args.push(video.as_os_str().to_owned());
}

/// The status note for an mpv exit code, or `None` when the user quit mpv or stepped on.
///
/// mpv exits with 0 when the user quits and with 4 after a signal. A process that a signal
/// killed has no code.
pub(crate) fn exit_note(code: Option<i32>, video: &Path) -> Option<String> {
    match code {
        None | Some(0 | USER_QUIT | NEXT_FILE | PREVIOUS_FILE) => None,
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

    const STEP_BINDINGS: &str = r#"keybind n "quit 10",keybind N "quit 11""#;

    #[test]
    fn window_args_detach_from_the_terminal() {
        assert_eq!(
            strings(&window_args(Path::new("b.webm"), Playback::Loop)),
            [
                "--no-terminal",
                "--force-window=immediate",
                "--fs",
                "--native-fs=no",
                "--hwdec=auto-safe",
                "--loop-file=inf",
                &format!("--input-commands={STEP_BINDINGS}"),
                "--",
                "b.webm",
            ]
        );
    }

    #[test]
    fn once_plays_without_the_loop_and_gives_the_quit_keys_their_own_code() {
        let window = strings(&window_args(Path::new("b.webm"), Playback::Once));
        assert_eq!(
            window,
            [
                "--no-terminal",
                "--force-window=immediate",
                "--fs",
                "--native-fs=no",
                "--hwdec=auto-safe",
                "--loop-file=no",
                "--keep-open=no",
                "--idle=no",
                "--loop-playlist=no",
                &format!(
                    r#"--input-commands={STEP_BINDINGS},keybind q "quit 4",keybind Q "quit 4",keybind CLOSE_WIN "quit 4""#
                ),
                "--",
                "b.webm",
            ]
        );
    }

    #[test]
    fn the_step_keys_have_their_own_exit_codes() {
        assert_eq!(step(Some(NEXT_FILE)), Some(Step::Next));
        assert_eq!(step(Some(PREVIOUS_FILE)), Some(Step::Previous));
        for other in [Some(0), Some(USER_QUIT), Some(2), None] {
            assert_eq!(step(other), None);
        }
        assert!(!reached_the_end(Some(NEXT_FILE)));
        assert_eq!(exit_note(Some(NEXT_FILE), Path::new("a.mp4")), None);
        assert_eq!(exit_note(Some(PREVIOUS_FILE), Path::new("a.mp4")), None);
    }

    #[test]
    fn only_exit_code_zero_is_the_end_of_the_file() {
        assert!(reached_the_end(Some(0)));
        assert!(!reached_the_end(Some(USER_QUIT)));
        assert!(!reached_the_end(Some(2)));
        assert!(!reached_the_end(None));
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
