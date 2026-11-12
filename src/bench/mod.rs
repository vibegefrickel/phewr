//! Speed measurements on a real folder (`PHEWR_TEST_DIR`). `just bench` runs them.
//!
//! The tests drive the run loop steps without a terminal. A sleep stands in for `event::poll`.
//! The Kitty sequences go to the buffer of the test app, so the tests measure the payload too.

use crate::app::{App, ViewMode};
use crate::image_list::SharedImageList;
use crate::pictures::decode::color::{self, ColorTarget};
use crate::run_loop::{next_timeout, poll_background, render_images};
use crate::scanner;
use ratatui::layout::Rect;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// A Retina terminal in Ghostty: 212 columns by 58 rows, cells of 16 by 36 pixels.
const COLS: u16 = 212;
const ROWS: u16 = 58;
const CELL_PX: (u32, u32) = (16, 36);
const LIMIT: Duration = Duration::from_secs(120);
/// A scanned app with its own empty thumbnail cache.
struct Bench {
    app: App,
    _cache: TempDir,
}

impl Drop for Bench {
    /// Let the decodes end: the next bench starts cold.
    fn drop(&mut self) {
        settle(&mut self.app);
    }
}

/// Run passes until the background work ends. No assert: a drop in a failed bench must not
/// panic again.
fn settle(app: &mut App) {
    let deadline = Instant::now() + LIMIT;
    let mut pending = Vec::new();
    while (app.has_pending_work() || !pending.is_empty()) && Instant::now() < deadline {
        run_pass(app, &mut pending);
        thread::sleep(Duration::from_millis(2));
    }
}

/// Set the color target of `main`. One test thread, so the global target cannot change.
fn set_color_target() {
    let target = std::env::var("PHEWR_BENCH_COLORSPACE")
        .ok()
        .and_then(|value| ColorTarget::parse(&value))
        .unwrap_or_default();
    color::set_target(target);
}

fn test_dir() -> Option<PathBuf> {
    set_color_target();
    // Canonical, as `main` passes the start folder: `/tmp` is a link to `/private/tmp`.
    std::env::var_os("PHEWR_TEST_DIR")
        .map(PathBuf::from)
        .map(|dir| std::fs::canonicalize(&dir).unwrap_or(dir))
}

fn report(line: &str) {
    println!("bench {line}");
}

fn millis(duration: Duration) -> String {
    format!("{:.0} ms", duration.as_secs_f64() * 1000.0)
}

fn fine_millis(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.0)
}

fn median(times: &mut [Duration]) -> Duration {
    times.sort_unstable();
    times.get(times.len() / 2).copied().unwrap_or_default()
}

/// Delete the temp files that `out` names, as the terminal does. Returns their bytes.
fn remove_temp_files(out: &[u8]) -> u64 {
    crate::pictures::kitty::protocol::temp_file_paths(out)
        .iter()
        .map(|path| {
            let bytes = std::fs::metadata(path).map_or(0, |metadata| metadata.len());
            std::fs::remove_file(path).unwrap();
            bytes
        })
        .sum()
}

fn grid_rect() -> Rect {
    Rect::new(0, 0, COLS, ROWS - 1)
}

fn fullscreen_rect() -> Rect {
    Rect::new(0, 0, COLS, ROWS - 1)
}

#[cfg(feature = "video")]
fn is_video(path: &Path) -> bool {
    crate::video::is_video(path)
}

#[cfg(not(feature = "video"))]
fn is_video(_path: &Path) -> bool {
    false
}

#[cfg(feature = "video")]
fn is_animated(path: &Path) -> bool {
    crate::player::is_animated(path)
}

#[cfg(not(feature = "video"))]
fn is_animated(_path: &Path) -> bool {
    false
}

/// Scan `dir` into an app with the Retina grid, without videos.
fn scanned_app(dir: &Path) -> Bench {
    let list = SharedImageList::new();
    scanner::spawn(vec![dir.to_path_buf()], list.clone());
    let cache = tempfile::tempdir().unwrap();
    let mut app = App::new(CELL_PX, list);
    app.disk_cache_dir = cache.path().to_path_buf();
    let deadline = Instant::now() + LIMIT;
    while !app.scan_complete {
        assert!(Instant::now() < deadline, "the scan did not finish");
        app.refresh_from_scanner();
        thread::sleep(Duration::from_millis(5));
    }
    keep_files_only(&mut app, deadline);
    Bench { app, _cache: cache }
}

/// Keep the still files, wait for the cell shape and lay out the grid.
fn keep_files_only(app: &mut App, deadline: Instant) {
    let keep: Vec<usize> = (app.first_file()..app.images.len())
        .filter(|&index| !is_video(&app.images[index]) && !is_animated(&app.images[index]))
        .collect();
    app.folder_count = 0;
    app.images = keep
        .iter()
        .map(|&index| app.images[index].clone())
        .collect();
    app.filenames = keep
        .iter()
        .map(|&index| app.filenames[index].clone())
        .collect();
    app.gallery.filtered_indices = (0..app.images.len()).collect();
    // The complete scan started the header read. The grid waits for it.
    while app.cell_shape_pending() {
        assert!(Instant::now() < deadline, "the header read did not finish");
        app.poll_cell_shape();
        thread::sleep(Duration::from_millis(1));
    }
    app.gallery.update_grid(grid_rect());
    app.needs_render = true;
}

/// One run loop pass. It sets the fullscreen box that `ui::draw` sets.
fn run_pass(app: &mut App, pending: &mut Vec<usize>) {
    poll_background(app, pending);
    app.fullscreen_box = fullscreen_rect();
    if app.mode == ViewMode::Fullscreen {
        app.image_rect = fullscreen_rect();
    }
    render_images(app, pending).unwrap();
}

/// Run passes until `done`. Returns the time and the longest pass (the UI thread block).
fn run_until(app: &mut App, mut done: impl FnMut(&App) -> bool) -> (Duration, Duration) {
    let start = Instant::now();
    let mut pending = Vec::new();
    let mut longest = Duration::ZERO;
    loop {
        let pass = Instant::now();
        run_pass(app, &mut pending);
        longest = longest.max(pass.elapsed());
        if done(app) {
            return (start.elapsed(), longest);
        }
        assert!(start.elapsed() < LIMIT, "the loop did not reach its goal");
        thread::sleep(next_timeout(app, &pending));
    }
}

fn count_transmissions(bytes: &[u8]) -> usize {
    let text = String::from_utf8_lossy(bytes);
    text.matches("\x1b_Ga=T,").count()
}

/// The times of a grid fill: first image, all cells shown, all cells sharp, and the Kitty bytes.
struct GridTimes {
    first: Duration,
    all_shown: Duration,
    all_sharp: Duration,
    bytes: usize,
}

impl GridTimes {
    fn report(&self, label: &str) {
        report(&format!(
            "grid {label}: first image {}, all cells shown {}, all cells sharp {}, kitty bytes {}",
            millis(self.first),
            millis(self.all_shown),
            millis(self.all_sharp),
            self.bytes
        ));
    }
}

/// Fill the grid and take the times. They leave out the scan of the Kitty output.
fn measure_grid(app: &mut App) -> GridTimes {
    let visible: Vec<usize> = app.gallery.visible_items().map(|(_, idx)| idx).collect();
    let start = Instant::now();
    let mut pending = Vec::new();
    let mut first = None;
    let mut all_shown = None;
    let mut bytes = 0;
    let mut scans = Duration::ZERO;
    loop {
        run_pass(app, &mut pending);
        let now = start.elapsed().saturating_sub(scans);
        let scan = Instant::now();
        let out = app.graphics_out.take_bytes();
        bytes += out.len();
        let transmitted = count_transmissions(&out) > 0;
        scans += scan.elapsed();
        if first.is_none() && transmitted {
            first = Some(now);
        }
        let shown = pending.is_empty() && visible.iter().all(|&idx| app.thumb_cache.contains(&idx));
        if shown && all_shown.is_none() {
            all_shown = Some(now);
        }
        if shown && visible.iter().all(|&idx| app.thumb_cache.is_sharp(&idx)) {
            return GridTimes {
                first: first.unwrap_or_default(),
                all_shown: all_shown.unwrap_or_default(),
                all_sharp: now,
                bytes,
            };
        }
        assert!(start.elapsed() < LIMIT, "the grid did not fill");
        thread::sleep(next_timeout(app, &pending));
    }
}

mod cache;
mod decoders;
mod folders;
mod frame_format;
mod fullscreen;
mod grid;
#[cfg(feature = "heic")]
mod heic;
#[cfg(feature = "turbo")]
mod layout_bench;
#[cfg(feature = "video")]
mod live;
#[cfg(feature = "video")]
mod loops;
#[cfg(all(feature = "sips", target_os = "macos"))]
mod previews;
mod start;
#[cfg(feature = "video")]
mod video;
