//! The command line: the arguments of `viewr` and the start directory that they name.

use crate::scanner::canonicalize_or_self;
use clap::Parser;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "viewr",
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
/// viewr on the hovered file. The second value is the cursor target.
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
    use super::{Cli, initial_dir_from_paths, start_paths};
    use crate::test_support::synthetic_tree;
    use clap::Parser;
    use std::path::PathBuf;

    #[test]
    fn parses_files_and_options() {
        let cli = Cli::try_parse_from([
            "viewr", "-j", "4", "-s", "-n", "2.5", "-i", "a.jpg", "photos",
        ])
        .unwrap();

        assert_eq!(cli.files, [PathBuf::from("a.jpg"), PathBuf::from("photos")]);
        assert_eq!(cli.threads, Some(4));
        assert!(cli.slideshow);
        assert_eq!(cli.interval, Some(2.5));
        assert!(cli.info);
    }

    #[test]
    fn parses_long_options() {
        let cli = Cli::try_parse_from([
            "viewr",
            "--threads",
            "2",
            "--slideshow",
            "--interval",
            "7",
            "--info",
        ])
        .unwrap();

        assert!(cli.files.is_empty());
        assert_eq!(cli.threads, Some(2));
        assert!(cli.slideshow && cli.info);
        assert_eq!(cli.interval, Some(7.0));
    }

    #[test]
    fn defaults_are_empty() {
        let cli = Cli::try_parse_from(["viewr"]).unwrap();

        assert!(cli.files.is_empty());
        assert_eq!(cli.threads, None);
        assert_eq!(cli.interval, None);
        assert!(!cli.slideshow && !cli.info);
    }

    #[test]
    fn rejects_bad_values_and_unknown_flags() {
        assert!(Cli::try_parse_from(["viewr", "--interval", "soon"]).is_err());
        assert!(Cli::try_parse_from(["viewr", "--threads", "-1"]).is_err());
        assert!(Cli::try_parse_from(["viewr", "--fullscreen"]).is_err());
        assert!(Cli::try_parse_from(["viewr", "-t", "nord"]).is_err());
    }

    #[test]
    fn zero_threads_or_interval_fail() {
        assert!(Cli::try_parse_from(["viewr", "-j", "0"]).is_err());
        assert!(Cli::try_parse_from(["viewr", "-n", "0"]).is_err());
        assert!(Cli::try_parse_from(["viewr", "-n", "-1"]).is_err());
        assert!(Cli::try_parse_from(["viewr", "-n", "inf"]).is_err());
        assert!(Cli::try_parse_from(["viewr", "-n", "NaN"]).is_err());
        assert!(Cli::try_parse_from(["viewr", "-j", "1", "-n", "0.5"]).is_ok());
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
