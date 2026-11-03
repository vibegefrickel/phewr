//! Video playback through mpv in a fullscreen window of its own, with sound.
//!
//! The process start sits behind [`Runner`]. The tests use a fake and never start mpv.

use std::ffi::OsString;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

/// The mpv exit code after a user quit in [`Playback::Once`]: `q`, `Q`, `Ctrl+C` and the close
/// button. mpv uses it after a signal too. In [`Playback::Loop`], a quit exits with 0.
pub(crate) const USER_QUIT: i32 = 4;

/// The mpv exit code after `n`: viewr steps to the next file.
pub(crate) const NEXT_FILE: i32 = 10;

/// The mpv exit code after `N` (Shift+n): viewr steps to the previous file.
pub(crate) const PREVIOUS_FILE: i32 = 11;

/// mpv 0.41 binds neither `n` nor `N` by default. The seek keys (the arrows) and the playlist
/// keys (`<`, `>`, Enter) keep their function.
const STEP_KEYS: [(&str, i32); 2] = [("n", NEXT_FILE), ("N", PREVIOUS_FILE)];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Playback {
    Loop,
    /// Play the file once for a running slideshow. Each quit key exits with [`USER_QUIT`], and
    /// only the end of the file exits with 0.
    Once,
}

/// True when mpv in [`Playback::Once`] played the file to its end.
pub(crate) fn reached_the_end(code: Option<i32>) -> bool {
    code == Some(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Next,
    Previous,
}

/// `None` for an exit that is no step.
pub(crate) fn step(code: Option<i32>) -> Option<Step> {
    match code {
        Some(NEXT_FILE) => Some(Step::Next),
        Some(PREVIOUS_FILE) => Some(Step::Previous),
        _ => None,
    }
}

/// mpv in a window of its own. `--no-terminal` keeps it away from the keys of viewr.
///
/// `--hwdec=auto-safe` gives the GPU output the hardware frames without a copy. The GPU output
/// does its own HDR tone mapping.
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
/// [`Playback::Once`] turns off the `mpv.conf` options that keep mpv open after the file:
/// `keep-open`, `idle` and `loop-playlist`. It binds each key in `quit_keys` to an exit with
/// [`USER_QUIT`], because a plain `quit` exits with 0 like the end of the file. `Ctrl+C` quits
/// with that code already. `--input-commands` needs mpv 0.38 or later.
///
/// A key binding cannot catch the quit of the macOS app (Cmd+Q, "Quit mpv" in the app menu).
/// The app quit ends mpv 0.41 with 0, and a window slideshow continues.
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

/// After `--`, a name that starts with `-` is not an option.
fn push_file(args: &mut Vec<OsString>, video: &Path) {
    args.push(OsString::from("--"));
    args.push(video.as_os_str().to_owned());
}

/// The status note for an mpv exit code, or `None` after a quit or a step.
///
/// A process that a signal killed has no code.
pub(crate) fn exit_note(code: Option<i32>, video: &Path) -> Option<String> {
    match code {
        None | Some(0 | USER_QUIT | NEXT_FILE | PREVIOUS_FILE) => None,
        Some(code) => Some(format!(
            "mpv could not play {} (exit code {code})",
            file_name(video)
        )),
    }
}

/// The whole path when it has no file name.
pub(crate) fn file_name(video: &Path) -> String {
    video.file_name().map_or_else(
        || video.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Starts mpv. The tests use a fake that records the calls.
pub(crate) trait Runner {
    /// Start `program` without a terminal and return without a wait. The receiver gets the exit
    /// code when the process ends.
    fn spawn(
        &mut self,
        program: &Path,
        args: &[OsString],
    ) -> io::Result<mpsc::Receiver<Option<i32>>>;
}

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
        // The wait thread reaps the exited window process.
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
    fn window_args_detach_from_terminal() {
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
    fn once_binds_quit_keys() {
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
    fn step_keys_have_own_codes() {
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
    fn only_zero_is_file_end() {
        assert!(reached_the_end(Some(0)));
        assert!(!reached_the_end(Some(USER_QUIT)));
        assert!(!reached_the_end(Some(2)));
        assert!(!reached_the_end(None));
    }

    #[test]
    fn quit_codes_give_no_note() {
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
    fn nameless_path_shows_whole_path() {
        assert_eq!(file_name(Path::new("/")), "/");
        assert_eq!(file_name(Path::new("/videos/d.mp4")), "d.mp4");
    }
    #[test]
    fn system_runner_reports_exit() {
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
