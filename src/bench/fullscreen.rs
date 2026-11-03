//! Opening, stepping and zooming in the fullscreen view.

use super::{LIMIT, run_pass};
use crate::app::App;
use crate::run_loop::next_timeout;
use std::thread;

use super::{
    CELL_PX, fine_millis, fullscreen_rect, median, millis, remove_temp_files, report, run_until,
    scanned_app, test_dir,
};
use crate::encoder::KittyBackend;
use crate::encoder::Transfer;
use crate::kitty_output::render_fullscreen_image_to;
use std::time::Duration;
use std::time::Instant;

const STEPS: usize = 10;

fn dwell(app: &mut App, dwell: Duration) {
    let until = Instant::now() + dwell;
    let mut pending = Vec::new();
    loop {
        run_pass(app, &mut pending);
        let now = Instant::now();
        if now >= until {
            return;
        }
        thread::sleep(next_timeout(app, &pending).min(until - now));
    }
}

fn first_with_extension(app: &App, extensions: &[&str]) -> Option<usize> {
    app.images.iter().position(|path| {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| extensions.contains(&e.to_ascii_lowercase().as_str()))
    })
}

fn wait_for_prefetch(app: &mut App) {
    let deadline = Instant::now() + LIMIT;
    while app.prefetcher.pending_count() > 0 {
        assert!(Instant::now() < deadline, "the pre-decode did not finish");
        app.prefetcher.poll();
        thread::sleep(Duration::from_millis(2));
    }
}

/// Enter on a grid cell until the fullscreen image shows, cold and after the gallery pre-decode.
#[test]
#[ignore = "reads the folder PHEWR_TEST_DIR"]
fn open_from_grid() {
    let Some(dir) = test_dir() else {
        report("skipped: PHEWR_TEST_DIR is not set");
        return;
    };
    for extensions in [&["heic"][..], &["jpg", "jpeg"][..], &["png"][..]] {
        let mut bench = scanned_app(&dir);
        let Some(index) = first_with_extension(&bench.app, extensions) else {
            report(&format!("open {}: no file in the folder", extensions[0]));
            continue;
        };
        let name = bench.app.filenames[index].clone();

        // A cold open: Enter in the grid before any pre-decode.
        bench.app.enter_fullscreen_at(index);
        let (total, longest) = run_until(&mut bench.app, |app| app.loaded.is_some());
        let loaded = bench.app.loaded.as_ref().unwrap();
        report(&format!(
            "open {name} cold: {} to the image, longest UI pass {}, image {}x{}",
            millis(total),
            millis(longest),
            loaded.width(),
            loaded.height()
        ));

        // An open after the gallery pre-decode of the hovered cell.
        let mut bench = scanned_app(&dir);
        bench.app.gallery.cursor = index;
        bench.app.fullscreen_box = fullscreen_rect();
        bench.app.pre_decode_hovered();
        wait_for_prefetch(&mut bench.app);
        bench.app.enter_fullscreen_at(index);
        bench.app.image_rect = fullscreen_rect();
        let take = Instant::now();
        bench.app.load_if_needed();
        let take = take.elapsed();
        let hit = bench.app.loaded.is_some();
        let (total, longest) = run_until(&mut bench.app, |app| app.loaded.is_some());
        report(&format!(
            "open {name} after pre-decode: hit {hit}, take {} on the UI thread, {} to the image, longest UI pass {}",
            millis(take),
            millis(total),
            millis(longest)
        ));
    }
}

/// The steps to the next and the previous image with the pre-decode of the neighbors.
#[test]
#[ignore = "reads the folder PHEWR_TEST_DIR"]
fn prefetch_hits_for_next_and_previous() {
    let Some(dir) = test_dir() else {
        report("skipped: PHEWR_TEST_DIR is not set");
        return;
    };
    for pause in [Duration::ZERO, Duration::from_millis(300)] {
        let mut bench = scanned_app(&dir);
        assert!(
            bench.app.images.len() > STEPS + 2,
            "the folder is too small"
        );
        bench.app.enter_fullscreen_at(1);
        run_until(&mut bench.app, |app| app.loaded.is_some());

        let mut hits = 0;
        let mut longest_take = Duration::ZERO;
        let mut total_wait = Duration::ZERO;
        for _ in 0..STEPS {
            dwell(&mut bench.app, pause);
            let take = Instant::now();
            bench.app.next();
            longest_take = longest_take.max(take.elapsed());
            if bench.app.loaded.is_some() {
                hits += 1;
            }
            let (wait, _) = run_until(&mut bench.app, |app| app.loaded.is_some());
            total_wait += wait;
        }
        report(&format!(
            "next, dwell {}: {hits}/{STEPS} prefetch hits, longest take {} on the UI thread, mean wait {}",
            millis(pause),
            millis(longest_take),
            millis(total_wait / STEPS as u32)
        ));

        let mut hits = 0;
        let mut longest_take = Duration::ZERO;
        let mut total_wait = Duration::ZERO;
        for _ in 0..STEPS {
            dwell(&mut bench.app, pause);
            let take = Instant::now();
            bench.app.prev();
            longest_take = longest_take.max(take.elapsed());
            if bench.app.loaded.is_some() {
                hits += 1;
            }
            let (wait, _) = run_until(&mut bench.app, |app| app.loaded.is_some());
            total_wait += wait;
        }
        report(&format!(
            "previous, dwell {}: {hits}/{STEPS} prefetch hits, longest take {} on the UI thread, mean wait {}",
            millis(pause),
            millis(longest_take),
            millis(total_wait / STEPS as u32)
        ));
    }
}

/// The bytes and the write time of one fullscreen frame through the temp file medium.
#[test]
#[ignore = "reads the folder PHEWR_TEST_DIR"]
fn kitty_payload() {
    const RUNS: usize = 10;

    let Some(dir) = test_dir() else {
        report("skipped: PHEWR_TEST_DIR is not set");
        return;
    };
    for extensions in [&["heic"][..], &["png"][..]] {
        let mut bench = scanned_app(&dir);
        let Some(index) = first_with_extension(&bench.app, extensions) else {
            report(&format!("payload {}: no file in the folder", extensions[0]));
            continue;
        };
        bench.app.enter_fullscreen_at(index);
        run_until(&mut bench.app, |app| app.loaded.is_some());
        let loaded = bench.app.loaded.as_ref().unwrap();
        report(&format!(
            "payload {}: image {}x{} in a box of {}x{} pixels",
            bench.app.filenames[index],
            loaded.width(),
            loaded.height(),
            u32::from(fullscreen_rect().width) * CELL_PX.0,
            u32::from(fullscreen_rect().height) * CELL_PX.1
        ));
        for transfer in [Transfer::Direct, Transfer::Png, Transfer::TempFile] {
            bench.app.graphics = KittyBackend::new(transfer);
            let mut times = Vec::new();
            let mut sizes = (0, 0);
            for _ in 0..RUNS {
                let mut out = Vec::new();
                let start = Instant::now();
                render_fullscreen_image_to(&mut bench.app, &mut out).unwrap();
                times.push(start.elapsed());
                sizes = (out.len(), remove_temp_files(&out));
            }
            report(&format!(
                "payload {transfer:?}: {} bytes to the terminal, {} bytes in temp files, median {} per frame",
                sizes.0,
                sizes.1,
                fine_millis(median(&mut times))
            ));
        }
    }
}

/// One pan step of the zoomed view: the new crop and its write through the temp file medium.
#[test]
#[ignore = "reads the folder PHEWR_TEST_DIR"]
fn zoom_pan_step() {
    const STEPS: usize = 10;

    let Some(dir) = test_dir() else {
        report("skipped: PHEWR_TEST_DIR is not set");
        return;
    };
    let mut bench = scanned_app(&dir);
    let index = first_with_extension(&bench.app, &["heic"]).unwrap_or(0);
    bench.app.graphics = KittyBackend::new(Transfer::TempFile);
    bench.app.enter_fullscreen_at(index);
    bench.app.image_rect = fullscreen_rect();
    run_until(&mut bench.app, |app| app.loaded.is_some());
    bench.app.zoom_in();
    run_until(&mut bench.app, |app| !app.has_pending_work());
    let (mut crops, mut writes, mut rgba_writes) = (Vec::new(), Vec::new(), Vec::new());
    for step in 0..STEPS {
        bench
            .app
            .pan_fraction(if step % 2 == 0 { 0.05 } else { -0.05 }, 0.0);
        let start = Instant::now();
        bench.app.load_if_needed();
        crops.push(start.elapsed());
        let mut out = Vec::new();
        let start = Instant::now();
        render_fullscreen_image_to(&mut bench.app, &mut out).unwrap();
        writes.push(start.elapsed());
        remove_temp_files(&out);
        // One transparent pixel sends the crop as RGBA: the write before the RGB step.
        if let Some(loaded) = bench.app.loaded.as_mut() {
            loaded.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
        }
        let mut out = Vec::new();
        let start = Instant::now();
        render_fullscreen_image_to(&mut bench.app, &mut out).unwrap();
        rgba_writes.push(start.elapsed());
        remove_temp_files(&out);
    }
    let loaded = bench.app.loaded.as_ref().unwrap();
    report(&format!(
        "zoom pan step {}x{}: median {} for the crop, {} for the temp file write as RGB, {} as RGBA",
        loaded.width(),
        loaded.height(),
        fine_millis(median(&mut crops)),
        fine_millis(median(&mut writes)),
        fine_millis(median(&mut rgba_writes))
    ));
}
