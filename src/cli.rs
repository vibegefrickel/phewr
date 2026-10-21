//! The command line: the arguments of `viewr` and the start directory that they name.

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "viewr",
    version,
    about = "Terminal image gallery: thumbnail grid, slideshow, EXIF, HEIC"
)]
pub(crate) struct Cli {
    /// Image file(s) or directory to display [default: .]
    pub(crate) files: Vec<PathBuf>,

    /// Number of threads for image decoding [default: all cores]
    #[arg(short = 'j', long)]
    pub(crate) threads: Option<usize>,

    /// Start a fullscreen slideshow right away
    #[arg(short, long)]
    pub(crate) slideshow: bool,

    /// Seconds between slides (overrides the configured interval)
    #[arg(short = 'n', long, value_name = "SECONDS")]
    pub(crate) interval: Option<f64>,

    /// Open the info panel on start
    #[arg(short, long)]
    pub(crate) info: bool,
}

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
    use super::{Cli, initial_dir_from_paths};
    use crate::test_support::synthetic_tree;
    use clap::Parser;
    use std::path::PathBuf;

    #[test]
    fn parses_files_and_all_options() {
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
    fn parses_the_long_options() {
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
    fn a_directory_argument_is_the_start_directory() {
        let (_temp, root, child) = synthetic_tree();

        let paths = [root.join("missing.jpg"), child.clone()];
        assert_eq!(initial_dir_from_paths(&paths), child);
    }

    #[test]
    fn a_file_argument_starts_in_its_parent_directory() {
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
}
