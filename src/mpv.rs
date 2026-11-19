//! Video playback through mpv in a fullscreen window of its own, with sound.
//!
//! The process start sits behind [`Runner`]. The tests use a fake and never start mpv.

use std::ffi::OsString;
use std::io::{self, BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

/// The option that names the IPC socket of mpv.
const SOCKET_OPTION: &str = "--input-ipc-server=";

/// A Unix socket path holds 104 bytes on macOS, with the terminating zero.
const MAX_SOCKET_PATH: usize = 100;

/// mpv creates the socket after its start. The reader stops its tries after this time.
const CONNECT_WINDOW: Duration = Duration::from_secs(2);
/// The pause between two connect tries.
const CONNECT_RETRY: Duration = Duration::from_millis(20);

/// The mpv exit code after a user quit in [`Playback::Once`]: `q`, `Q`, `Ctrl+C` and the close
/// button. mpv uses it after a signal too. In [`Playback::Loop`], a quit exits with 0.
pub(crate) const USER_QUIT: i32 = 4;

/// The mpv exit code after `l`: phewr steps to the next file.
pub(crate) const NEXT_FILE: i32 = 10;

/// The mpv exit code after `h`: phewr steps to the previous file.
pub(crate) const PREVIOUS_FILE: i32 = 11;

/// mpv 0.41 binds `l` to `ab-loop` and `L` to the loop toggle by default, and `h` is free. The
/// `keybind` of phewr wins in this window. The seek keys (the arrows) and the playlist keys
/// (`<`, `>`, Enter) keep their function.
const STEP_KEYS: [(&str, i32); 2] = [("l", NEXT_FILE), ("h", PREVIOUS_FILE)];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Playback {
    Loop,
    /// Play the file once for a running slideshow. Each quit key exits with [`USER_QUIT`], and
    /// only the end of the file exits with 0.
    Once,
}

/// The exit code of a window, with the `end-file` reason of the IPC socket folded in.
///
/// The app quit of macOS (Cmd+Q) exits with 0 like the end of the file, but it reports `quit` or
/// `stop`. A missing reason keeps the exit code.
pub(crate) fn exit_with_reason(code: Option<i32>, reason: Option<&str>) -> Option<i32> {
    match (code, reason) {
        (Some(0), Some("quit" | "stop")) => Some(USER_QUIT),
        _ => code,
    }
}

/// The socket path for one window below `dir`, or `None` if the path is too long.
pub(crate) fn socket_path(dir: &Path, process: u32, window: u32) -> Option<PathBuf> {
    let path = dir.join(format!("mpv-{process}-{window}.sock"));
    (path.as_os_str().len() <= MAX_SOCKET_PATH).then_some(path)
}

/// A new socket path for the next window: the thumbnail cache, then `$TMPDIR/phewr`. Each
/// window owns its path, because a window that phewr forgot can still run.
pub(crate) fn socket_for_window(cache_dir: &Path) -> Option<PathBuf> {
    static WINDOWS: AtomicU32 = AtomicU32::new(0);
    let process = std::process::id();
    let window = WINDOWS.fetch_add(1, Ordering::Relaxed);
    socket_path(cache_dir, process, window)
        .or_else(|| socket_path(&std::env::temp_dir().join("phewr"), process, window))
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

/// mpv in a window of its own. `--no-terminal` keeps it away from the keys of phewr.
///
/// `--hwdec=auto-safe` gives the GPU output the hardware frames without a copy. The GPU output
/// does its own HDR tone mapping.
pub(crate) fn window_args(
    video: &Path,
    playback: Playback,
    socket: Option<&Path>,
) -> Vec<OsString> {
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
    if let (Playback::Once, Some(socket)) = (playback, socket) {
        let mut option = OsString::from(SOCKET_OPTION);
        option.push(socket);
        args.push(option);
    }
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
/// The app quit ends mpv 0.41 with 0. The IPC socket tells it apart, see [`exit_with_reason`].
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
    /// code when the process ends. `socket` is the IPC socket that `args` name: the runner
    /// reads the end reason there and removes the file after the exit.
    fn spawn(
        &mut self,
        program: &Path,
        args: &[OsString],
        socket: Option<&Path>,
    ) -> io::Result<mpsc::Receiver<Option<i32>>>;
}

pub(crate) struct SystemRunner;

impl Runner for SystemRunner {
    fn spawn(
        &mut self,
        program: &Path,
        args: &[OsString],
        socket: Option<&Path>,
    ) -> io::Result<mpsc::Receiver<Option<i32>>> {
        let socket = socket.map(Path::to_path_buf);
        if let Some(socket) = &socket {
            prepare_socket(socket)?;
        }
        // A process group of its own keeps the job control signals of the terminal away.
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?;
        let done = Arc::new(AtomicBool::new(false));
        let reader = socket.clone().map(|socket| {
            let done = Arc::clone(&done);
            thread::spawn(move || last_end_reason(&socket, &done, CONNECT_WINDOW))
        });
        // The wait thread reaps the exited window process.
        let (exit_tx, exit_rx) = mpsc::channel();
        thread::spawn(move || {
            let code = child.wait().ok().and_then(|status| status.code());
            done.store(true, Ordering::Release);
            let reason = reader.and_then(|reader| reader.join().ok().flatten());
            if let Some(socket) = socket {
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "mpv removes its socket itself, a leftover file is harmless"
                )]
                let _ = std::fs::remove_file(socket);
            }
            #[expect(
                clippy::let_underscore_must_use,
                reason = "outside a slideshow, nobody waits for the exit of the window"
            )]
            let _ = exit_tx.send(exit_with_reason(code, reason.as_deref()));
        });
        Ok(exit_rx)
    }
}

/// Make the folder and remove a stale socket, because mpv fails to bind over a file.
fn prepare_socket(socket: &Path) -> io::Result<()> {
    if let Some(folder) = socket.parent() {
        std::fs::create_dir_all(folder)?;
    }
    match std::fs::remove_file(socket) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Connect to the socket of mpv and return the reason of the last `end-file` event.
///
/// `None` if the socket never opens or `done` is set first. A read error ends the read and keeps
/// the reason seen so far.
fn last_end_reason(socket: &Path, done: &AtomicBool, window: Duration) -> Option<String> {
    let start = Instant::now();
    let stream = loop {
        if let Ok(stream) = UnixStream::connect(socket) {
            break stream;
        }
        if done.load(Ordering::Acquire) || start.elapsed() >= window {
            return None;
        }
        thread::sleep(CONNECT_RETRY);
    };
    end_reason(BufReader::new(stream))
}

/// The reason of the last `end-file` line. A line that is no JSON is skipped.
fn end_reason(events: impl BufRead) -> Option<String> {
    let mut reason = None;
    for line in events.lines() {
        let Ok(line) = line else {
            break;
        };
        let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if event["event"] == "end-file"
            && let Some(found) = event["reason"].as_str()
        {
            reason = Some(found.to_owned());
        }
    }
    reason
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    const STEP_BINDINGS: &str = r#"keybind l "quit 10",keybind h "quit 11""#;

    #[test]
    fn window_args_detach_from_terminal() {
        assert_eq!(
            strings(&window_args(Path::new("b.webm"), Playback::Loop, None)),
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
        let window = strings(&window_args(Path::new("b.webm"), Playback::Once, None));
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
    fn once_names_socket() {
        let socket = Path::new("/tmp/phewr/mpv-1.sock");
        let args = strings(&window_args(
            Path::new("b.webm"),
            Playback::Once,
            Some(socket),
        ));
        assert!(args.contains(&"--input-ipc-server=/tmp/phewr/mpv-1.sock".to_owned()));
        let args = strings(&window_args(
            Path::new("b.webm"),
            Playback::Loop,
            Some(socket),
        ));
        assert!(!args.iter().any(|arg| arg.starts_with(SOCKET_OPTION)));
    }

    #[test]
    fn windows_own_sockets() {
        let cache = Path::new("/tmp/phewr");
        let first = socket_for_window(cache).unwrap();
        let second = socket_for_window(cache).unwrap();
        assert_ne!(first, second);
        let process = std::process::id();
        assert!(first.to_string_lossy().contains(&format!("mpv-{process}-")));
    }

    #[test]
    fn long_socket_path_falls_back() {
        let long = PathBuf::from("/").join("x".repeat(120));
        assert_eq!(socket_path(&long, 7, 3), None);
        assert_eq!(
            socket_path(Path::new("/tmp/phewr"), 7, 3),
            Some(PathBuf::from("/tmp/phewr/mpv-7-3.sock"))
        );
        let fallback = socket_for_window(&long).unwrap();
        assert!(fallback.starts_with(std::env::temp_dir().join("phewr")));
    }

    #[test]
    fn quit_reason_stops() {
        assert_eq!(exit_with_reason(Some(0), Some("eof")), Some(0));
        assert_eq!(exit_with_reason(Some(0), Some("quit")), Some(USER_QUIT));
        assert_eq!(exit_with_reason(Some(0), Some("stop")), Some(USER_QUIT));
        assert_eq!(exit_with_reason(Some(0), None), Some(0));
        assert_eq!(
            exit_with_reason(Some(NEXT_FILE), Some("quit")),
            Some(NEXT_FILE)
        );
        assert_eq!(exit_with_reason(None, Some("quit")), None);
    }

    /// Serve `lines` on a socket in a temp dir, then close it. Returns the reason read.
    fn read_from_fake_mpv(lines: &'static str) -> Option<String> {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("m.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            io::Write::write_all(&mut stream, lines.as_bytes()).unwrap();
        });
        let done = AtomicBool::new(false);
        let reason = last_end_reason(&socket, &done, Duration::from_secs(5));
        server.join().unwrap();
        reason
    }

    #[test]
    fn socket_reports_eof() {
        let lines = "{\"event\":\"start-file\"}\n{\"event\":\"end-file\",\"reason\":\"eof\"}\n";
        assert_eq!(read_from_fake_mpv(lines).as_deref(), Some("eof"));
    }

    #[test]
    fn socket_reports_quit() {
        let lines = "{\"event\":\"end-file\",\"reason\":\"quit\",\"playlist_entry_id\":1}\n";
        assert_eq!(read_from_fake_mpv(lines).as_deref(), Some("quit"));
    }

    #[test]
    fn broken_line_is_skipped() {
        let lines = "{\"event\":\"end-file\",\"reason\":\"stop\"}\nnot json {\n{\"event\":\n";
        assert_eq!(read_from_fake_mpv(lines).as_deref(), Some("stop"));
        assert_eq!(read_from_fake_mpv("garbage\n"), None);
    }

    #[test]
    fn missing_socket_gives_none() {
        let dir = tempfile::tempdir().unwrap();
        let done = AtomicBool::new(false);
        let reason = last_end_reason(
            &dir.path().join("none.sock"),
            &done,
            Duration::from_millis(60),
        );
        assert_eq!(reason, None);
        done.store(true, Ordering::Release);
        assert_eq!(
            last_end_reason(&dir.path().join("none.sock"), &done, Duration::from_secs(5)),
            None
        );
    }

    #[test]
    fn stale_socket_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("sub").join("m.sock");
        prepare_socket(&socket).unwrap();
        std::fs::write(&socket, b"").unwrap();
        prepare_socket(&socket).unwrap();
        assert!(!socket.exists());
    }

    /// Run a shell that lives 0.5 s as the program, with `line` served on the socket.
    fn run_shell_with_socket(line: &'static str) -> (Option<i32>, bool) {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("m.sock");
        let args = [OsString::from("-c"), OsString::from("sleep 0.5")];
        let exit = SystemRunner
            .spawn(Path::new("/bin/sh"), &args, Some(&socket))
            .unwrap();
        // The runner removed a stale socket, so the bind comes after the spawn.
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            io::Write::write_all(&mut stream, line.as_bytes()).unwrap();
        });
        let code = exit.recv_timeout(Duration::from_secs(10)).unwrap();
        server.join().unwrap();
        (code, socket.exists())
    }

    #[test]
    fn runner_folds_quit_reason() {
        let quit = "{\"event\":\"end-file\",\"reason\":\"quit\"}\n";
        assert_eq!(run_shell_with_socket(quit), (Some(USER_QUIT), false));
    }

    #[test]
    fn runner_keeps_eof_code() {
        let eof = "{\"event\":\"end-file\",\"reason\":\"eof\"}\n";
        assert_eq!(run_shell_with_socket(eof), (Some(0), false));
    }

    #[test]
    fn system_runner_reports_exit() {
        let shell = Path::new("/bin/sh");
        let args = [OsString::from("-c"), OsString::from("exit 5")];
        let exit = SystemRunner.spawn(shell, &args, None).unwrap();
        assert_eq!(exit.recv_timeout(Duration::from_secs(10)), Ok(Some(5)));
        SystemRunner
            .spawn(Path::new("/nonexistent/mpv"), &[], None)
            .unwrap_err();
    }
}
