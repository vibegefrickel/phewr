//! The cell shape and the folder switch.

use super::measure_grid;

use super::{
    CELL_PX, LIMIT, grid_rect, millis, report, run_pass, run_until, scanned_app, settle, test_dir,
};
use crate::app::App;
use crate::image_list::SharedImageList;
use crate::run_loop::next_timeout;
use std::path::Path;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use std::time::Instant;
use tempfile::TempDir;

/// The header read that picks the cell shape.
#[test]
#[ignore = "reads the folder PHEWR_TEST_DIR"]
fn shape_from_headers() {
    use crate::pictures::shape::{aspect_of, folder_aspect};

    let Some(dir) = test_dir() else {
        report("skipped: PHEWR_TEST_DIR is not set");
        return;
    };
    let bench = scanned_app(&dir);
    let sample: Vec<PathBuf> = bench.app.images.iter().take(64).cloned().collect();
    for round in ["cold", "warm"] {
        let start = Instant::now();
        let aspect = folder_aspect(&sample);
        report(&format!(
            "shape: {} headers in {} ({round}), folder aspect {aspect:.3}",
            sample.len(),
            millis(start.elapsed())
        ));
    }
    let mut seen: Vec<String> = Vec::new();
    for path in &bench.app.images {
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if seen.contains(&extension) {
            continue;
        }
        seen.push(extension.clone());
        let start = Instant::now();
        let aspect = aspect_of(path);
        report(&format!(
            "shape: one .{extension} header in {:.2} ms, aspect {aspect:?}",
            start.elapsed().as_secs_f64() * 1000.0
        ));
    }
}

/// The wait for the cell shape after a directory switch, during the old cold decodes.
#[test]
#[ignore = "reads the folder PHEWR_TEST_DIR"]
fn switch_waits_for_shape() {
    let Some(dir) = test_dir() else {
        report("skipped: PHEWR_TEST_DIR is not set");
        return;
    };
    let mut bench = scanned_app(&dir);
    let images = bench.app.images.clone();
    let mut pending = Vec::new();
    // One pass starts the cold decodes of the first screen and fills the rayon pool.
    run_pass(&mut bench.app, &mut pending);
    thread::sleep(Duration::from_millis(20));

    bench.app.switch_to_dir(&dir);
    bench.app.images = images;
    bench.app.gallery.filtered_indices = (0..bench.app.images.len()).collect();
    let start = Instant::now();
    bench.app.request_cell_shape();
    while bench.app.cell_shape_pending() {
        assert!(start.elapsed() < LIMIT, "the header read did not finish");
        bench.app.poll_cell_shape();
        thread::sleep(Duration::from_millis(1));
    }
    report(&format!(
        "shape after a folder switch with cold decodes in flight: {}",
        millis(start.elapsed())
    ));
}

/// A root with the subfolder `sub`, whose files link to `dir`. The disk cache key uses the
/// canonical path of the target.
fn tree_with_subfolder(dir: &Path) -> (TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let sub = root.join("sub");
    std::fs::create_dir(&sub).unwrap();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_file()) {
            std::os::unix::fs::symlink(entry.path(), sub.join(entry.file_name())).unwrap();
        }
    }
    (temp, root)
}

/// Enter `sub` from a root grid with a cold disk cache. With `rest`, the cursor rests until the
/// prefetch ends. Returns the prefetch time, the time to the first sharp screen and the cell count.
fn enter_subfolder(dir: &Path, rest: bool) -> (Option<Duration>, Duration, usize) {
    let (_tree, root) = tree_with_subfolder(dir);
    let sub = root.join("sub");
    let cache = tempfile::tempdir().unwrap();
    let mut app = App::new(CELL_PX, SharedImageList::new());
    app.disk_cache_dir = cache.path().to_path_buf();
    app.gallery.update_grid(grid_rect());
    app.switch_to_dir(&root);
    run_until(&mut app, |app| app.scan_complete);
    // The entries are `..` and `sub`.
    app.gallery.cursor = 1;
    let prefetch = rest.then(|| rest_until_prefetched(&mut app, &sub));

    assert!(app.enter_selected_folder());
    let (time, _) = run_until(&mut app, |app| {
        app.scan_complete && !app.cell_shape_pending() && all_file_cells_sharp(app)
    });
    let cells = app
        .gallery
        .visible_items()
        .filter(|&(_, index)| !app.is_folder(index))
        .count();
    settle(&mut app);
    (prefetch, time, cells)
}

/// Tick the folder prefetch until `folder` ends. Only this bench ticks.
fn rest_until_prefetched(app: &mut App, folder: &Path) -> Duration {
    let start = Instant::now();
    let mut pending = Vec::new();
    while !app.folder_prefetched(folder) {
        assert!(
            start.elapsed() < LIMIT,
            "the folder prefetch did not finish"
        );
        run_pass(app, &mut pending);
        app.tick_folder_prefetch(Instant::now());
        thread::sleep(next_timeout(app, &pending));
    }
    start.elapsed()
}

fn all_file_cells_sharp(app: &App) -> bool {
    let mut cells = app
        .gallery
        .visible_items()
        .filter(|&(_, index)| !app.is_folder(index))
        .peekable();
    cells.peek().is_some() && cells.all(|(_, index)| app.thumb_cache.is_sharp(&index))
}

/// The first screen after Enter on a folder, with and without the folder prefetch.
#[test]
#[ignore = "reads the folder PHEWR_TEST_DIR"]
fn switch_with_and_without_prefetch() {
    let Some(dir) = test_dir() else {
        report("skipped: PHEWR_TEST_DIR is not set");
        return;
    };
    let (_, cold, cells) = enter_subfolder(&dir, false);
    report(&format!(
        "enter a subfolder, cold: all {cells} cells sharp {}",
        millis(cold)
    ));
    let (prefetch, warm, cells) = enter_subfolder(&dir, true);
    report(&format!(
        "enter a subfolder after the folder prefetch ({} for the prefetch): all {cells} cells \
         sharp {}",
        millis(prefetch.unwrap_or_default()),
        millis(warm)
    ));
}

/// The cold grid while a folder prefetch of `PHEWR_TEST_PREFETCH_DIR` runs.
#[test]
#[ignore = "reads the folders PHEWR_TEST_DIR and PHEWR_TEST_PREFETCH_DIR"]
fn grid_during_prefetch() {
    let (Some(dir), Some(other)) = (
        test_dir(),
        std::env::var_os("PHEWR_TEST_PREFETCH_DIR").map(PathBuf::from),
    ) else {
        report("skipped: PHEWR_TEST_DIR or PHEWR_TEST_PREFETCH_DIR is not set");
        return;
    };
    // The app of the other folder: the cursor rests on the folder `sub` and its prefetch starts.
    let (_tree, root) = tree_with_subfolder(&other);
    let sub = root.join("sub");
    let other_cache = tempfile::tempdir().unwrap();
    let mut other_app = App::new(CELL_PX, SharedImageList::new());
    other_app.disk_cache_dir = other_cache.path().to_path_buf();
    other_app.gallery.update_grid(grid_rect());
    other_app.switch_to_dir(&root);
    run_until(&mut other_app, |app| app.scan_complete);
    other_app.gallery.cursor = 1;
    let mut bench = scanned_app(&dir);

    // 400 ms is more than `FOLDER_PREFETCH_DELAY`.
    let now = Instant::now();
    other_app.tick_folder_prefetch(now);
    other_app.tick_folder_prefetch(now + Duration::from_millis(400));
    assert!(other_app.folder_prefetch_running().is_some());
    measure_grid(&mut bench.app).report("cold during a folder prefetch");
    let rest = rest_until_prefetched(&mut other_app, &sub);
    report(&format!(
        "grid during a folder prefetch: the prefetch needed {} more after the grid",
        millis(rest)
    ));
    settle(&mut other_app);
}
