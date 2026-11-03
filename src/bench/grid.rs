//! The cold grid fill.

use super::{LIMIT, keep_files_only};
use crate::app::App;
use std::path::Path;
use std::thread;
use std::time::Duration;

use super::{measure_grid, millis, report, scanned_app, test_dir};
use std::time::Instant;

fn switch_and_scan(app: &mut App, dir: &Path) {
    let deadline = Instant::now() + LIMIT;
    app.switch_to_dir(dir);
    while !app.scan_complete {
        assert!(Instant::now() < deadline, "the scan did not finish");
        app.refresh_from_scanner();
        thread::sleep(Duration::from_millis(5));
    }
    keep_files_only(app, deadline);
}

/// The first thumbnail and the full grid: cold, warm (disk cache) and on the way back (parked).
#[test]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn first_thumbnail_and_full_grid() {
    let Some(dir) = test_dir() else {
        report("skipped: VIEWR_TEST_DIR is not set");
        return;
    };
    let mut bench = scanned_app(&dir);
    let visible = bench.app.gallery.visible_items().count();
    report(&format!(
        "grid: {} images, {} visible cells of {}x{} cells, folder {}",
        bench.app.images.len(),
        visible,
        bench.app.gallery.thumb_cols,
        bench.app.gallery.thumb_rows,
        dir.display()
    ));

    measure_grid(&mut bench.app).report("cold");

    bench.app.mark_dirty();
    measure_grid(&mut bench.app).report("warm (disk cache)");

    // Backspace to an empty folder and Enter back: the parked thumbnails need no decode and no
    // disk read.
    let empty = tempfile::tempdir().unwrap();
    switch_and_scan(&mut bench.app, empty.path());
    let start = Instant::now();
    switch_and_scan(&mut bench.app, &dir);
    let switch = start.elapsed();
    measure_grid(&mut bench.app).report("way back (parked)");
    report(&format!(
        "grid way back: scan and shape {}, parked pictures left {} (0 = every cell took one)",
        millis(switch),
        bench.app.thumb_cache.parked_count()
    ));
}
