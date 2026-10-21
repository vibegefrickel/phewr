//! The builders and helpers that the tests of more than one module share.

use crate::app::App;
use crate::config::Config;
use crate::image_list::SharedImageList;
use crate::keys::handle_event;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use image::RgbaImage;
#[cfg(feature = "heic")]
use libheif_rs::{
    Channel, ColorSpace, CompressionFormat, EncoderQuality, EncodingOptions, HeifContext, Image,
    ImageOrientation, LibHeif, RgbChroma,
};
use ratatui::layout::Rect;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// A temp directory with the subdirectory `synthetic-images`. The tree goes away on drop.
/// Both paths are canonical, because the picker canonicalizes its directories.
pub(crate) fn synthetic_tree() -> (TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let child = root.join("synthetic-images");
    fs::create_dir(&child).unwrap();
    (temp, root, child)
}

pub(crate) fn press(app: &mut App, code: KeyCode) -> bool {
    press_with(app, code, KeyModifiers::NONE)
}

pub(crate) fn press_with(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> bool {
    handle_event(app, &Event::Key(KeyEvent::new(code, modifiers))).unwrap()
}

pub(crate) fn char_key(app: &mut App, c: char) -> bool {
    press(app, KeyCode::Char(c))
}

/// Paths that do not exist: a decode fails in the background and writes no cache.
pub(crate) fn photo_paths(count: usize) -> Vec<PathBuf> {
    (0..count)
        .map(|index| PathBuf::from(format!("/photos/photo-{index:02}.jpg")))
        .collect()
}

/// A scanned gallery of `count` photos in a grid of 3 columns and 2 visible rows.
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

pub(crate) fn fullscreen_app(count: usize, index: usize) -> App {
    let mut app = gallery_app(count);
    app.enter_fullscreen_at(index);
    app
}

pub(crate) fn text(out: &[u8]) -> String {
    String::from_utf8_lossy(out).into_owned()
}

pub(crate) const RED: [u8; 3] = [255, 0, 0];
pub(crate) const BLUE: [u8; 3] = [0, 0, 255];

/// Give each test its own temp directory; it goes away with the `TempDir`.
pub(crate) fn test_dir() -> TempDir {
    tempfile::tempdir().unwrap()
}

pub(crate) fn assert_close(pixel: [u8; 4], expected: [u8; 3]) {
    let close = pixel
        .iter()
        .zip(expected)
        .all(|(&got, want)| got.abs_diff(want) < 60);
    assert!(close, "pixel {pixel:?}, expected {expected:?}");
}

pub(crate) fn app() -> App {
    App::new((8, 16), SharedImageList::new())
}

/// An app with the synthetic paths `names` in the list and the grid. The files do not exist.
pub(crate) fn app_with(names: &[&str]) -> App {
    let mut app = app();
    app.images = names.iter().map(PathBuf::from).collect();
    app.filenames = names.iter().map(|name| (*name).to_owned()).collect();
    app.gallery.filtered_indices = (0..names.len()).collect();
    app
}

/// An app with real PNG files `names` in `dir`.
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

/// Poll `done` every 10 ms for at most 2 s, so background work cannot hang a test.
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

#[cfg(feature = "heic")]
pub(crate) fn solid_image(width: u32, height: u32, rgb: [u8; 3]) -> Image {
    let mut image = Image::new(width, height, ColorSpace::Rgb(RgbChroma::Rgb)).unwrap();
    image
        .create_plane(Channel::Interleaved, width, height, 8)
        .unwrap();
    let planes = image.planes_mut();
    let plane = planes.interleaved.unwrap();
    for y in 0..height as usize {
        for x in 0..width as usize {
            let start = y * plane.stride + x * 3;
            plane.data[start..start + 3].copy_from_slice(&rgb);
        }
    }
    image
}

#[cfg(feature = "heic")]
pub(crate) fn options(orientation: ImageOrientation) -> EncodingOptions {
    let mut options = EncodingOptions::new().unwrap();
    options.set_image_orientation(orientation);
    options
}

#[cfg(feature = "heic")]
/// Write a red 256x128 primary image. A blue thumbnail of 64x32 is optional, with
/// its own orientation.
pub(crate) fn write_heif(
    path: &Path,
    format: CompressionFormat,
    orientation: ImageOrientation,
    thumbnail: Option<ImageOrientation>,
) {
    let lib_heif = LibHeif::new();
    let mut ctx = HeifContext::new().unwrap();
    let mut encoder = lib_heif.encoder_for_format(format).unwrap();
    encoder.set_quality(EncoderQuality::Lossy(90)).unwrap();
    let primary = ctx
        .encode_image(
            &solid_image(256, 128, RED),
            &mut encoder,
            Some(options(orientation)),
        )
        .unwrap();
    if let Some(thumbnail_orientation) = thumbnail {
        ctx.encode_thumbnail(
            &solid_image(256, 128, BLUE),
            &primary,
            64,
            &mut encoder,
            Some(options(thumbnail_orientation)),
        )
        .unwrap();
    }
    ctx.write_to_file(path.to_str().unwrap()).unwrap();
}
