//! The Live Photo folder: the scan with the pairing, and the cold grid until every Live cell
//! plays its loop.
//!
//! The scan benches build two synthetic folders in a temp dir. They hold empty files with the
//! right names (`IMG_00000.HEIC`, `.MOV`, `x.JPG`), because the scan reads no file: the pairing
//! pass works on the names. The grid benches read the real files of `PHEWR_TEST_LIVE_DIR`
//! (default `~/Pictures/phewr-test/12-live-photos`) and run the real ffmpeg.
//! Each test prints the 1-minute load average, so that a report states the load of its run.

use super::loops::{animation_backend, loop_pass};
use super::{
    Bench, CELL_PX, COLS, LIMIT, ROWS, grid_rect, median, millis, report, set_color_target,
};
use crate::app::App;
use crate::encoder::{KittyBackend, Transfer};
use crate::image_list::SharedImageList;
use crate::run_loop::next_timeout;
use crate::scanner;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

/// The scans of each folder and mode in one run. The report holds their median.
const SCAN_RUNS: usize = 11;

/// The names of the folder with pairs: a HEIC, its MOV and a JPG in each third of 10 002 numbers.
const PAIR_NUMBERS: usize = 10_002;

/// The names of the folder without pairs.
const PLAIN_NAMES: usize = 10_000;

/// The grid zoom of the grid benches: 6 rows hold every cell of the real folder.
const LIVE_ROWS: usize = 6;

/// The 1-minute load average from `uptime`, or `None` without a number.
fn load_average() -> Option<f64> {
    let output = std::process::Command::new("uptime").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let (_, averages) = text.rsplit_once("load averages:")?;
    averages
        .split_whitespace()
        .next()?
        .replace(',', ".")
        .parse()
        .ok()
}

/// A time in milliseconds with three decimals: the pairing pass takes about 0.1 ms.
fn micro_millis(duration: Duration) -> String {
    format!("{:.3} ms", duration.as_secs_f64() * 1000.0)
}

fn report_load(label: &str) {
    let load = load_average().map_or_else(|| "unknown".to_owned(), |load| format!("{load:.2}"));
    report(&format!("live {label}: load average (1 min) {load}"));
}

fn live_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("PHEWR_TEST_LIVE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join("Pictures/phewr-test/12-live-photos"))
        })
        .filter(|dir| dir.is_dir());
    if dir.is_none() {
        report("live: skipped, because PHEWR_TEST_LIVE_DIR is not a folder");
    }
    dir.map(|dir| std::fs::canonicalize(&dir).unwrap_or(dir))
}

/// A temp folder of empty files with the given names.
fn folder_of(names: impl Iterator<Item = String>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for name in names {
        std::fs::write(dir.path().join(name), []).unwrap();
    }
    dir
}

/// The time to the first entry and to the end of one scan of `dir`, and the entries listed.
fn scan_once(dir: &Path, live_photos: bool) -> (Duration, Duration, usize) {
    let list = SharedImageList::new();
    let start = Instant::now();
    scanner::spawn_with(vec![dir.to_path_buf()], list.clone(), live_photos);
    while list.len() == 0 && !list.is_complete() {
        assert!(start.elapsed() < LIMIT, "the scan gave no entry");
        thread::yield_now();
    }
    let first = start.elapsed();
    while !list.is_complete() {
        assert!(start.elapsed() < LIMIT, "the scan did not finish");
        thread::yield_now();
    }
    (first, start.elapsed(), list.len())
}

/// The medians of `SCAN_RUNS` scans in turns: (first, complete) with pairs off, then on, and
/// the entries that the scan with pairs lists.
fn scan_medians(dir: &Path) -> ([(Duration, Duration); 2], usize) {
    scan_once(dir, true);
    let mut runs = [(Vec::new(), Vec::new()), (Vec::new(), Vec::new())];
    let mut listed = 0;
    for _ in 0..SCAN_RUNS {
        for (live_photos, (firsts, completes)) in [false, true].into_iter().zip(runs.iter_mut()) {
            let (first, complete, len) = scan_once(dir, live_photos);
            firsts.push(first);
            completes.push(complete);
            if live_photos {
                listed = len;
            }
        }
    }
    let medians =
        runs.map(|(mut firsts, mut completes)| (median(&mut firsts), median(&mut completes)));
    (medians, listed)
}

fn report_scan(label: &str, count: usize, dir: &Path) {
    let ([off, on], listed) = scan_medians(dir);
    let share = |off: Duration, on: Duration| {
        100.0 * (on.as_secs_f64() - off.as_secs_f64()) / off.as_secs_f64()
    };
    report(&format!(
        "live scan {label} ({count} directory entries, {listed} listed with pairs on, median of \
         {SCAN_RUNS}): first entry off {} on {} (pairing share {:.1} %), complete off {} on {} (pairing share {:.1} %)",
        micro_millis(off.0),
        micro_millis(on.0),
        share(off.0, on.0),
        micro_millis(off.1),
        micro_millis(on.1),
        share(off.1, on.1),
    ));
}

/// The scan of the synthetic folders and of the real folder, with `live_photos` on and off.
#[test]
#[ignore = "builds folders of 10 000 files"]
fn scan_cost() {
    set_color_target();
    report_load("scan, start");
    let plain = folder_of((0..PLAIN_NAMES).map(|index| format!("IMG_{index:05}.HEIC")));
    let plain_dir = std::fs::canonicalize(plain.path()).unwrap();
    report_scan("without pairs", PLAIN_NAMES, &plain_dir);

    let pairs = folder_of(
        (0..PAIR_NUMBERS)
            .filter(|index| index % 3 == 0)
            .flat_map(|index| {
                [
                    format!("IMG_{index:05}.HEIC"),
                    format!("IMG_{index:05}.MOV"),
                    format!("IMG_{index:05}x.JPG"),
                ]
            }),
    );
    let pairs_dir = std::fs::canonicalize(pairs.path()).unwrap();
    report_scan(
        &format!("with {} pairs", PAIR_NUMBERS.div_ceil(3)),
        PAIR_NUMBERS.div_ceil(3) * 3,
        &pairs_dir,
    );

    if let Some(real) = live_dir() {
        let count = std::fs::read_dir(&real).map_or(0, Iterator::count);
        report_scan("real folder", count, &real);
    }
    report_load("scan, end");
}

/// A copy of the files of `source` in a temp folder.
fn copy_folder(source: &Path) -> tempfile::TempDir {
    let copy = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(source).unwrap().flatten() {
        if entry.path().is_file() {
            std::fs::copy(entry.path(), copy.path().join(entry.file_name())).unwrap();
        }
    }
    copy
}

/// A grid of the real folder: scanned with the pairs, 6 rows, the temp file medium of Ghostty.
fn live_app(dir: &Path, cache: &Path, graphics: KittyBackend) -> App {
    let list = SharedImageList::new();
    scanner::spawn_with(vec![dir.to_path_buf()], list.clone(), true);
    let mut app = App::new(CELL_PX, list);
    app.disk_cache_dir = cache.to_path_buf();
    app.graphics = graphics;
    let deadline = Instant::now() + LIMIT;
    while !app.scan_complete {
        assert!(Instant::now() < deadline, "the scan did not finish");
        app.refresh_from_scanner();
        thread::sleep(Duration::from_millis(1));
    }
    while app.cell_shape_pending() {
        assert!(Instant::now() < deadline, "the header read did not finish");
        app.poll_cell_shape();
        thread::sleep(Duration::from_millis(1));
    }
    app.gallery.set_zoom_rows(LIVE_ROWS);
    app.gallery.update_grid(grid_rect());
    app.needs_render = true;
    app
}

/// The cold grid of the real folder: the time until every visible Live cell plays its loop.
fn measure_live_grid(label: &str, graphics: KittyBackend) {
    let Some(source) = live_dir() else { return };
    set_color_target();
    crate::video::find_session_tools();
    if crate::video::session_tools().ffmpeg.is_none() {
        report("live: no ffmpeg on PATH");
        return;
    }
    report_load(label);
    let mut screen =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(COLS, ROWS)).unwrap();
    // A copy has a new path, so the session probe cache is cold for each bench.
    let copy = copy_folder(&source);
    let dir = std::fs::canonicalize(copy.path()).unwrap();
    let cache = tempfile::tempdir().unwrap();
    let app = live_app(&dir, cache.path(), graphics);
    let mut bench = Bench { app, _cache: cache };
    let live: Vec<usize> = bench
        .app
        .gallery
        .visible_items()
        .map(|(_, index)| index)
        .filter(|&index| bench.app.is_live(index))
        .collect();
    assert!(!live.is_empty(), "the folder holds no Live Photo");
    let visible = bench.app.gallery.visible_items().count();

    let start = Instant::now();
    let mut pending = Vec::new();
    let (mut stills, mut first_loop) = (None, None);
    loop {
        loop_pass(&mut bench.app, &mut screen, &mut pending);
        let now = start.elapsed();
        let app = &bench.app;
        let playing = live.iter().filter(|&&index| app.loop_plays(index)).count();
        if stills.is_none()
            && pending.is_empty()
            && live.iter().all(|&i| app.thumb_cache.is_sharp(i))
        {
            stills = Some(now);
        }
        if first_loop.is_none() && playing > 0 {
            first_loop = Some(now);
        }
        if playing == live.len() {
            report(&format!(
                "live grid {label}: {} Live cells of {visible} cells, Live stills sharp {}, \
                 first loop {}, every Live cell loops {}",
                live.len(),
                stills.map_or_else(|| "none".to_owned(), millis),
                first_loop.map_or_else(|| "none".to_owned(), millis),
                millis(now),
            ));
            return;
        }
        assert!(start.elapsed() < LIMIT, "the Live loops did not start");
        thread::sleep(next_timeout(&bench.app, &pending));
    }
}

/// The cold grid in a terminal that plays Kitty animations (Ghostty), with ffmpeg.
#[test]
#[ignore = "reads PHEWR_TEST_LIVE_DIR and runs ffmpeg"]
fn grid_animation() {
    measure_live_grid("grid, animation", animation_backend());
}

/// The cold grid with ticks (herdr).
#[test]
#[ignore = "reads PHEWR_TEST_LIVE_DIR and runs ffmpeg"]
fn grid_ticks() {
    measure_live_grid("grid, ticks", KittyBackend::new(Transfer::TempFile));
}
