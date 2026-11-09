//! The builders and helpers that the tests of more than one module share.

mod input;
pub(crate) mod kitty;
#[cfg(feature = "video")]
pub(crate) mod live;
#[cfg(feature = "video")]
pub(crate) mod video;

pub(crate) use crate::pictures::test_support::*;
pub(crate) use input::*;

use crate::app::App;
use crate::config::Config;
use crate::image_list::SharedImageList;
use image::RgbaImage;
use ratatui::layout::Rect;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// A temp directory with the subdirectory `synthetic-images`. Both paths are canonical.
pub(crate) fn synthetic_tree() -> (TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let child = root.join("synthetic-images");
    fs::create_dir(&child).unwrap();
    (temp, root, child)
}

/// Paths that do not exist: a decode fails in the background and writes no cache.
pub(crate) fn photo_paths(count: usize) -> Vec<PathBuf> {
    (0..count)
        .map(|index| PathBuf::from(format!("/photos/photo-{index:02}.jpg")))
        .collect()
}

/// A scanned gallery of `count` photos in 3 columns and 2 visible rows.
pub(crate) fn gallery_app_with(count: usize, config: &Config) -> App {
    let mut app = App::new_with_config((8, 16), SharedImageList::new(), config);
    app.images = photo_paths(count);
    app.filenames = app
        .images
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    app.gallery.filtered_indices = (0..count).collect();
    app.gallery.zoom_rows = 2;
    // 3:2 cells of 10 rows are 30 columns wide: 3 columns of 38 cells and 4 gap columns.
    app.gallery.update_grid(Rect::new(0, 0, 120, 24));
    app.scan_complete = true;
    app.needs_render = false;
    app
}

pub(crate) fn gallery_app(count: usize) -> App {
    gallery_app_with(count, &Config::default())
}

/// A scanned grid of `folders` and then `files` below `/photos`. The paths do not exist.
pub(crate) fn folder_app(folders: &[&str], files: &[&str]) -> App {
    let mut app = gallery_app(0);
    app.images = folders
        .iter()
        .chain(files)
        .map(|name| PathBuf::from("/photos").join(name))
        .collect();
    app.filenames = folders
        .iter()
        .chain(files)
        .map(|name| (*name).to_owned())
        .collect();
    app.folder_count = folders.len();
    app.gallery.filtered_indices = (0..app.images.len()).collect();
    app
}

pub(crate) fn fullscreen_app(count: usize, index: usize) -> App {
    let mut app = gallery_app(count);
    app.enter_fullscreen_at(index);
    app
}

pub(crate) fn text(out: &[u8]) -> String {
    String::from_utf8_lossy(out).into_owned()
}

pub(crate) fn app() -> App {
    App::new((8, 16), SharedImageList::new())
}

/// An app with the synthetic paths `names`. The files do not exist.
pub(crate) fn app_with(names: &[&str]) -> App {
    let mut app = app();
    app.images = names.iter().map(PathBuf::from).collect();
    app.filenames = names.iter().map(|name| (*name).to_owned()).collect();
    app.gallery.filtered_indices = (0..names.len()).collect();
    app
}

pub(crate) fn app_with_files(dir: &Path, names: &[&str]) -> App {
    let mut app = app();
    for name in names {
        let path = dir.join(name);
        RgbaImage::new(8, 8).save(&path).unwrap();
        app.images.push(path);
        app.filenames.push((*name).to_owned());
    }
    app.gallery.filtered_indices = (0..names.len()).collect();
    app
}

/// Poll `done` every 10 ms for at most 2 s.
pub(crate) fn wait_until(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !done() {
        assert!(Instant::now() < deadline, "background work did not finish");
        thread::sleep(Duration::from_millis(10));
    }
}

pub(crate) fn overdue() -> Instant {
    Instant::now()
        .checked_sub(Duration::from_millis(1))
        .unwrap()
}
