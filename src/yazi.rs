//! The reveal of the last file and the selection of the marks in the yazi that started phewr.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::ffi::OsStrExt;
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

/// The yazi plugin that selects the files of a list (README, "Marks to yazi").
const SELECT_PLUGIN: &str = "phewr-select";

/// Select `paths` in the yazi that started phewr, so yazi moves, copies or deletes them.
///
/// The paths go into a list file in `list_dir`, and `run` starts
/// `ya emit plugin phewr-select <list>`. The plugin reads the list and deletes it. Without
/// paths, without `YAZI_ID` or without `ya` on PATH, nothing happens.
pub(crate) fn select_in_yazi(
    paths: &[PathBuf],
    yazi_id: Option<&OsStr>,
    search_path: Option<&OsStr>,
    list_dir: &Path,
    run: impl FnOnce(&Path, &[OsString]) -> io::Result<()>,
) -> Option<io::Result<()>> {
    if paths.is_empty() {
        return None;
    }
    yazi_id.filter(|id| !id.is_empty())?;
    let ya = find_on_path("ya", search_path)?;
    Some(write_list(paths, list_dir).and_then(|list| {
        let args = [
            OsString::from("emit"),
            OsString::from("plugin"),
            OsString::from(SELECT_PLUGIN),
            list.into_os_string(),
        ];
        run(&ya, &args)
    }))
}

/// One absolute path per line. A name with a line break does not fit the list and stays out.
fn write_list(paths: &[PathBuf], list_dir: &Path) -> io::Result<PathBuf> {
    let mut text = Vec::new();
    for path in paths {
        let path = std::path::absolute(path)?;
        let bytes = path.as_os_str().as_bytes();
        if !bytes.contains(&b'\n') {
            text.extend_from_slice(bytes);
            text.push(b'\n');
        }
    }
    let list = list_dir.join(format!("phewr-marks-{}.txt", std::process::id()));
    std::fs::write(&list, text)?;
    Ok(list)
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
    use super::{find_on_path, reveal_in_yazi, run_briefly, select_in_yazi};
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

    fn select(
        paths: &[PathBuf],
        yazi_id: Option<&str>,
        search_path: &OsStr,
        list_dir: &Path,
    ) -> (Option<bool>, Vec<Vec<OsString>>) {
        let mut calls = Vec::new();
        let result = select_in_yazi(
            paths,
            yazi_id.map(OsStr::new),
            Some(search_path),
            list_dir,
            |_program, args| {
                calls.push(args.to_vec());
                Ok(())
            },
        );
        (result.map(|result| result.is_ok()), calls)
    }

    /// The marks go into a list file, one absolute path per line, and the plugin gets the list.
    #[test]
    fn marks_go_to_select_plugin() {
        let (_dir, path) = path_with_ya();
        let lists = tempfile::tempdir().unwrap();
        let marks = [
            PathBuf::from("/photos/a.jpg"),
            PathBuf::from("photos/b.jpg"),
            PathBuf::from("/photos/line\nbreak.jpg"),
        ];
        let (result, calls) = select(&marks, Some("4711"), &path, lists.path());
        assert_eq!(result, Some(true));
        let list = PathBuf::from(&calls[0][3]);
        assert_eq!(
            calls[0][..3],
            ["emit", "plugin", "phewr-select"].map(OsString::from)
        );
        assert!(list.starts_with(lists.path()), "{list:?}");
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(
            std::fs::read_to_string(&list).unwrap(),
            format!("/photos/a.jpg\n{}\n", cwd.join("photos/b.jpg").display()),
            "a name with a line break stays out"
        );
    }

    #[test]
    fn no_marks_or_no_yazi_selects_nothing() {
        let (_dir, path) = path_with_ya();
        let lists = tempfile::tempdir().unwrap();
        let marks = [PathBuf::from("/photos/a.jpg")];
        for (case, marks, yazi_id) in [
            ("no marks", &marks[..0], Some("1")),
            ("no YAZI_ID", &marks[..], None),
            ("an empty YAZI_ID", &marks[..], Some("")),
        ] {
            let (result, calls) = select(marks, yazi_id, &path, lists.path());
            assert_eq!(result, None, "{case}");
            assert!(calls.is_empty(), "{case}");
        }
        assert_eq!(std::fs::read_dir(lists.path()).unwrap().count(), 0);
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
