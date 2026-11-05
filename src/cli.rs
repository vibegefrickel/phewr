//! The command line: the arguments of `phewr` and the start directory that they name.

use crate::scanner::canonicalize_or_self;
use clap::Parser;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "phewr",
    version,
    about = "Terminal gallery for images, videos and animations: thumbnail grid, slideshow, EXIF"
)]
pub(crate) struct Cli {
    /// Image file(s) or directory to display [default: .]
    pub(crate) files: Vec<PathBuf>,

    /// Number of threads for image decoding [default: all cores]
    #[arg(short = 'j', long, value_parser = parse_threads)]
    pub(crate) threads: Option<usize>,

    /// Start a fullscreen slideshow right away
    #[arg(short, long)]
    pub(crate) slideshow: bool,

    /// Seconds between slides (overrides the configured interval)
    #[arg(short = 'n', long, value_name = "SECONDS", value_parser = parse_interval)]
    pub(crate) interval: Option<f64>,

    /// Open the info panel on start
    #[arg(short, long)]
    pub(crate) info: bool,

    /// Write the folder of the grid into this file on exit, for a `cd` in the shell
    #[arg(long, value_name = "PATH")]
    pub(crate) cwd_file: Option<PathBuf>,
}

/// The pool needs one thread at least.
fn parse_threads(value: &str) -> Result<usize, String> {
    match value.parse::<usize>() {
        Ok(0) => Err("must be greater than zero".to_owned()),
        Ok(threads) => Ok(threads),
        Err(error) => Err(error.to_string()),
    }
}

/// A slide interval is a positive, finite number of seconds.
fn parse_interval(value: &str) -> Result<f64, String> {
    match value.parse::<f64>() {
        Ok(seconds) if seconds.is_finite() && seconds > 0.0 => Ok(seconds),
        Ok(_) => Err("must be a positive number of seconds".to_owned()),
        Err(error) => Err(error.to_string()),
    }
}

/// One file argument opens its folder with the cursor on that file. A file manager starts
/// phewr on the hovered file. The second value is the cursor target.
pub(crate) fn start_paths(paths: Vec<PathBuf>) -> (Vec<PathBuf>, Option<PathBuf>) {
    let [file] = paths.as_slice() else {
        return (paths, None);
    };
    let (true, Some(parent), Some(name)) = (file.is_file(), file.parent(), file.file_name()) else {
        return (paths, None);
    };
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    // The scanner lists the folder by its canonical path, and the cursor target uses it too.
    let folder = canonicalize_or_self(parent);
    let target = folder.join(name);
    (vec![folder], Some(target))
}

/// Write `dir` into `file`, like `yazi --cwd-file`, so a shell wrapper can `cd` there.
///
/// The path goes out without a newline. A wrapper reads it with `$(cat -- "$file")`.
pub(crate) fn write_cwd_file(file: &Path, dir: &Path) -> io::Result<()> {
    std::fs::write(file, dir.as_os_str().as_encoded_bytes())
}

/// The first directory argument, else the parent of the first file.
pub(crate) fn initial_dir_from_paths(paths: &[PathBuf]) -> PathBuf {
    for p in paths {
        if p.is_dir() {
            return p.clone();
        }
    }
    if let Some(first) = paths.first()
        && let Some(parent) = first.parent()
        && !parent.as_os_str().is_empty()
    {
        return parent.to_path_buf();
    }
    PathBuf::from(".")
}

#[cfg(test)]
mod tests {
    use super::{Cli, initial_dir_from_paths, start_paths, write_cwd_file};
    use crate::test_support::synthetic_tree;
    use clap::Parser;
    use std::path::{Path, PathBuf};

    #[test]
    fn parses_files_and_options() {
        let cli = Cli::try_parse_from([
            "phewr", "-j", "4", "-s", "-n", "2.5", "-i", "a.jpg", "photos",
        ])
        .unwrap();

        assert_eq!(cli.files, [PathBuf::from("a.jpg"), PathBuf::from("photos")]);
        assert_eq!(cli.threads, Some(4));
        assert!(cli.slideshow);
        assert_eq!(cli.interval, Some(2.5));
        assert!(cli.info);
        assert_eq!(cli.cwd_file, None);
    }

    #[test]
    fn parses_long_options() {
        let cli = Cli::try_parse_from([
            "phewr",
            "--threads",
            "2",
            "--slideshow",
            "--interval",
            "7",
            "--info",
            "--cwd-file",
            "/tmp/phewr-cwd",
        ])
        .unwrap();

        assert!(cli.files.is_empty());
        assert_eq!(cli.threads, Some(2));
        assert!(cli.slideshow && cli.info);
        assert_eq!(cli.interval, Some(7.0));
        assert_eq!(cli.cwd_file, Some(PathBuf::from("/tmp/phewr-cwd")));
    }

    #[test]
    fn defaults_are_empty() {
        let cli = Cli::try_parse_from(["phewr"]).unwrap();

        assert!(cli.files.is_empty());
        assert_eq!(cli.threads, None);
        assert_eq!(cli.interval, None);
        assert!(!cli.slideshow && !cli.info);
    }

    #[test]
    fn rejects_bad_values_and_unknown_flags() {
        assert!(Cli::try_parse_from(["phewr", "--interval", "soon"]).is_err());
        assert!(Cli::try_parse_from(["phewr", "--threads", "-1"]).is_err());
        assert!(Cli::try_parse_from(["phewr", "--fullscreen"]).is_err());
        assert!(Cli::try_parse_from(["phewr", "-t", "nord"]).is_err());
        assert!(Cli::try_parse_from(["phewr", "--cwd-file"]).is_err());
    }

    #[test]
    fn cwd_file_holds_the_folder_without_a_newline() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("cwd");

        write_cwd_file(&file, dir.path()).unwrap();
        assert_eq!(
            std::fs::read(&file).unwrap(),
            dir.path().as_os_str().as_encoded_bytes()
        );

        // A second exit replaces the first folder.
        write_cwd_file(&file, Path::new("/photos")).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "/photos");

        let missing = dir.path().join("missing").join("cwd");
        assert!(write_cwd_file(&missing, Path::new("/photos")).is_err());
    }

    #[test]
    fn zero_threads_or_interval_fail() {
        assert!(Cli::try_parse_from(["phewr", "-j", "0"]).is_err());
        assert!(Cli::try_parse_from(["phewr", "-n", "0"]).is_err());
        assert!(Cli::try_parse_from(["phewr", "-n", "-1"]).is_err());
        assert!(Cli::try_parse_from(["phewr", "-n", "inf"]).is_err());
        assert!(Cli::try_parse_from(["phewr", "-n", "NaN"]).is_err());
        assert!(Cli::try_parse_from(["phewr", "-j", "1", "-n", "0.5"]).is_ok());
    }

    #[test]
    fn directory_argument_is_start() {
        let (_temp, root, child) = synthetic_tree();

        let paths = [root.join("missing.jpg"), child.clone()];
        assert_eq!(initial_dir_from_paths(&paths), child);
    }

    #[test]
    fn file_argument_starts_in_parent() {
        assert_eq!(
            initial_dir_from_paths(&[PathBuf::from("/photos/a.jpg")]),
            PathBuf::from("/photos")
        );
        assert_eq!(
            initial_dir_from_paths(&[PathBuf::from("a.jpg")]),
            PathBuf::from(".")
        );
        assert_eq!(initial_dir_from_paths(&[]), PathBuf::from("."));
    }

    #[test]
    fn one_file_opens_folder_at_file() {
        let (_temp, root, child) = synthetic_tree();
        let file = child.join("a.png");
        std::fs::write(&file, b"").unwrap();

        assert_eq!(
            start_paths(vec![file.clone()]),
            (vec![child.clone()], Some(file.clone()))
        );
        let two = vec![file, root];
        assert_eq!(start_paths(two.clone()), (two, None));
        assert_eq!(start_paths(vec![child.clone()]), (vec![child], None));
    }
}
