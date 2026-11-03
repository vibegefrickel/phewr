//! The reveal of the last file in the yazi that started phewr.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// A yazi that does not answer must not hold phewr.
const YA_TIMEOUT: Duration = Duration::from_secs(1);

/// Move the cursor of the yazi that started phewr to `path`.
///
/// yazi sets `YAZI_ID` for each program that it starts. Without it, or without `ya` on PATH,
/// nothing happens. `run` starts `ya emit reveal <path>`.
pub(crate) fn reveal_in_yazi(
    path: Option<&Path>,
    yazi_id: Option<&OsStr>,
    search_path: Option<&OsStr>,
    run: impl FnOnce(&Path, &[OsString]) -> io::Result<()>,
) -> Option<io::Result<()>> {
    yazi_id.filter(|id| !id.is_empty())?;
    let ya = find_on_path("ya", search_path)?;
    let path = std::path::absolute(path?).ok()?;
    let args = [
        OsString::from("emit"),
        OsString::from("reveal"),
        path.into_os_string(),
    ];
    Some(run(&ya, &args))
}

/// Run `program` without output and wait at most [`YA_TIMEOUT`] for its exit. Then kill it.
pub(crate) fn run_briefly(program: &Path, args: &[OsString]) -> io::Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + YA_TIMEOUT;
    while child.try_wait()?.is_none() {
        if Instant::now() >= deadline {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "the child can exit between the check and the kill"
            )]
            let _ = child.kill();
            child.wait()?;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

/// The first executable `program` in `path`, like a shell lookup.
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

#[cfg(test)]
mod tests {
    use super::{find_on_path, reveal_in_yazi, run_briefly};
    use std::ffi::{OsStr, OsString};
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    fn path_with_ya() -> (tempfile::TempDir, OsString) {
        let dir = tempfile::tempdir().unwrap();
        let ya = dir.path().join("ya");
        std::fs::write(&ya, "").unwrap();
        std::fs::set_permissions(&ya, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths([dir.path()]).unwrap();
        (dir, path)
    }

    fn reveal(
        path: Option<&Path>,
        yazi_id: Option<&str>,
        search_path: &OsStr,
    ) -> (Option<bool>, Vec<(PathBuf, Vec<OsString>)>) {
        let mut calls = Vec::new();
        let result = reveal_in_yazi(
            path,
            yazi_id.map(OsStr::new),
            Some(search_path),
            |program, args| {
                calls.push((program.to_path_buf(), args.to_vec()));
                Ok(())
            },
        );
        (result.map(|result| result.is_ok()), calls)
    }

    #[test]
    fn yazi_id_asks_ya_to_reveal() {
        let (dir, path) = path_with_ya();
        let (result, calls) = reveal(Some(Path::new("/photos/a.jpg")), Some("4711"), &path);
        assert_eq!(result, Some(true));
        assert_eq!(
            calls,
            [(
                dir.path().join("ya"),
                ["emit", "reveal", "/photos/a.jpg"]
                    .map(OsString::from)
                    .to_vec()
            )]
        );
    }

    #[test]
    fn relative_path_becomes_absolute() {
        let (_dir, path) = path_with_ya();
        let (_result, calls) = reveal(Some(Path::new("./photos/a.jpg")), Some("1"), &path);
        let revealed = PathBuf::from(&calls[0].1[2]);
        assert!(revealed.is_absolute());
        assert_eq!(
            revealed,
            std::env::current_dir().unwrap().join("photos/a.jpg")
        );
    }

    #[test]
    fn missing_yazi_runs_nothing() {
        let (_dir, path) = path_with_ya();
        let file = Some(Path::new("/photos/a.jpg"));
        let empty = tempfile::tempdir().unwrap();
        let no_ya = std::env::join_paths([empty.path()]).unwrap();
        for (case, path, yazi_id, search_path) in [
            ("no YAZI_ID", file, None, &path),
            ("an empty YAZI_ID", file, Some(""), &path),
            ("no file", None, Some("1"), &path),
            ("no ya on PATH", file, Some("1"), &no_ya),
        ] {
            let (result, calls) = reveal(path, yazi_id, search_path);
            assert_eq!(result, None, "{case}");
            assert!(calls.is_empty(), "{case}");
        }
    }

    #[test]
    fn find_on_path_takes_first_executable() {
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
    fn run_briefly_returns_on_exit() {
        let start = Instant::now();
        run_briefly(Path::new("/usr/bin/true"), &[]).unwrap();
        assert!(start.elapsed() < Duration::from_millis(900));
    }

    #[test]
    fn run_briefly_stops_at_timeout() {
        let start = Instant::now();
        run_briefly(Path::new("/bin/sleep"), &[OsString::from("2")]).unwrap();
        let waited = start.elapsed();
        assert!(waited >= Duration::from_millis(900), "{waited:?}");
        assert!(waited < Duration::from_millis(1900), "{waited:?}");
    }

    #[test]
    fn timeout_kills_child() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let script = format!("echo $$ > {}; exec sleep 30", pid_file.display());
        run_briefly(
            Path::new("/bin/sh"),
            &[OsString::from("-c"), OsString::from(script)],
        )
        .unwrap();
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        let alive = std::process::Command::new("/bin/kill")
            .args(["-0", pid.trim()])
            .output()
            .unwrap()
            .status
            .success();
        assert!(!alive, "the child {pid} still runs");
    }
}
