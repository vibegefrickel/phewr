//! Speed measurements on a real folder. Each test is ignored and reads `VIEWR_TEST_DIR`.
//! `VIEWR_TEST_DIR=/tmp/viewr-bench just test-real bench` runs them and shows the numbers.
//!
//! The tests drive the steps of the run loop (`poll_background`, `render_images`,
//! `next_timeout`) without a terminal. A sleep stands in for `event::poll`, so the numbers
//! include the wait of the loop. The Kitty sequences go to the buffer of the test app, so the
//! tests also measure the size of the payload.

use crate::app::{App, ViewMode};
use crate::encoder::{KittyBackend, Transfer};
use crate::image_list::SharedImageList;
use crate::kitty_output::render_fullscreen_image_to;
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
/// The steps of the prefetch measurement in each direction.
const STEPS: usize = 10;

/// A scanned app with its own empty thumbnail cache. The cache goes away with the `TempDir`.
struct Bench {
    app: App,
    _cache: TempDir,
}

fn test_dir() -> Option<PathBuf> {
    std::env::var_os("VIEWR_TEST_DIR").map(PathBuf::from)
}

fn report(line: &str) {
    println!("bench {line}");
}

fn millis(duration: Duration) -> String {
    format!("{:.0} ms", duration.as_secs_f64() * 1000.0)
}

/// The grid area of the gallery: the whole terminal above the status row.
fn grid_rect() -> Rect {
    Rect::new(0, 0, COLS, ROWS - 1)
}

/// The fullscreen box: the whole terminal above the status row.
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

/// Scan `dir` into an app with the Retina grid. Videos leave the list, so that `next` never
/// starts a player.
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
    // The bench measures the files only: no folder entry and no video.
    let keep: Vec<usize> = (app.first_file()..app.images.len())
        .filter(|&index| !is_video(&app.images[index]))
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
    // The complete scan started the header read for the cell shape. The grid waits for it.
    while app.cell_shape_pending() {
        assert!(Instant::now() < deadline, "the header read did not finish");
        app.poll_cell_shape();
        thread::sleep(Duration::from_millis(1));
    }
    app.gallery.update_grid(grid_rect());
    app.needs_render = true;
    Bench { app, _cache: cache }
}

/// One pass of the run loop without a terminal. `ui::draw` would set the fullscreen box.
fn run_pass(app: &mut App, pending: &mut Vec<usize>) {
    poll_background(app, pending);
    app.fullscreen_box = fullscreen_rect();
    if app.mode == ViewMode::Fullscreen {
        app.image_rect = fullscreen_rect();
    }
    render_images(app, pending).unwrap();
}

/// Run passes until `done`, with the wait of the loop between them. Returns the time and the
/// longest pass, which is the time that the UI thread was blocked.
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

/// Run passes for `dwell`, as a user who looks at the image.
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

fn count_transmissions(bytes: &[u8]) -> usize {
    let text = String::from_utf8_lossy(bytes);
    text.matches("\x1b_Ga=T,").count()
}

/// The times of a grid fill: the first image on the screen, every cell with an image (a
/// preview or a sharp one), every cell sharp, and the bytes of the Kitty output.
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

/// Fill the grid and take the times. A cell counts as shown when its image is in the cache and
/// the loop has written the pending emits, as the user sees it.
fn measure_grid(app: &mut App) -> GridTimes {
    let visible: Vec<usize> = app.gallery.visible_items().map(|(_, idx)| idx).collect();
    let start = Instant::now();
    let mut pending = Vec::new();
    let mut first = None;
    let mut all_shown = None;
    let mut bytes = 0;
    loop {
        run_pass(app, &mut pending);
        let out = app.graphics_out.take_bytes();
        bytes += out.len();
        if first.is_none() && count_transmissions(&out) > 0 {
            first = Some(start.elapsed());
        }
        let shown = pending.is_empty() && visible.iter().all(|&idx| app.thumb_cache.contains(idx));
        if shown && all_shown.is_none() {
            all_shown = Some(start.elapsed());
        }
        if shown && visible.iter().all(|&idx| app.thumb_cache.is_sharp(idx)) {
            return GridTimes {
                first: first.unwrap_or_default(),
                all_shown: all_shown.unwrap_or_default(),
                all_sharp: start.elapsed(),
                bytes,
            };
        }
        assert!(start.elapsed() < LIMIT, "the grid did not fill");
        thread::sleep(next_timeout(app, &pending));
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

#[test]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn grid_first_thumbnail_and_full_grid() {
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
}

/// The cost of the header read that picks the cell shape: the first 64 files of the folder,
/// as `App::request_cell_shape` reads them, and one file per extension.
#[test]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn folder_shape_from_the_headers() {
    use crate::decode::shape::{aspect_of, folder_aspect};

    let Some(dir) = test_dir() else {
        report("skipped: VIEWR_TEST_DIR is not set");
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

/// The wait for the cell shape after a directory switch, while the cold decodes of the old
/// folder still run. The first screen of the new folder waits for this read.
#[test]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn folder_switch_waits_for_the_shape() {
    let Some(dir) = test_dir() else {
        report("skipped: VIEWR_TEST_DIR is not set");
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

#[test]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn fullscreen_open_from_the_grid() {
    let Some(dir) = test_dir() else {
        report("skipped: VIEWR_TEST_DIR is not set");
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

#[test]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn prefetch_hits_for_next_and_previous() {
    let Some(dir) = test_dir() else {
        report("skipped: VIEWR_TEST_DIR is not set");
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

#[test]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn kitty_payload_of_a_fullscreen_frame() {
    let Some(dir) = test_dir() else {
        report("skipped: VIEWR_TEST_DIR is not set");
        return;
    };
    let mut bench = scanned_app(&dir);
    let index = first_with_extension(&bench.app, &["heic"]).unwrap_or(0);
    bench.app.enter_fullscreen_at(index);
    run_until(&mut bench.app, |app| app.loaded.is_some());
    let loaded = bench.app.loaded.as_ref().unwrap();
    report(&format!(
        "payload: image {}x{} in a box of {}x{} pixels",
        loaded.width(),
        loaded.height(),
        u32::from(fullscreen_rect().width) * CELL_PX.0,
        u32::from(fullscreen_rect().height) * CELL_PX.1
    ));
    for transfer in [Transfer::Direct, Transfer::Png, Transfer::TempFile] {
        bench.app.graphics = KittyBackend::new(transfer);
        let mut out = Vec::new();
        let start = Instant::now();
        render_fullscreen_image_to(&mut bench.app, &mut out).unwrap();
        let elapsed = start.elapsed();
        let files = crate::encoder::kitty::temp_file_paths(&out);
        let file_bytes: u64 = files
            .iter()
            .filter_map(|path| std::fs::metadata(path).ok())
            .map(|metadata| metadata.len())
            .sum();
        for path in &files {
            std::fs::remove_file(path).unwrap();
        }
        report(&format!(
            "payload {transfer:?}: {} bytes to the terminal in {}, {file_bytes} bytes in {} temp file(s)",
            out.len(),
            millis(elapsed),
            files.len()
        ));
    }
}

/// The phases of one HEIC decode, alone and with the parallel decodes of a grid.
#[cfg(feature = "heic")]
#[test]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn heic_decode_phases() {
    use crate::decode::{EmbeddedThumbnail, decode_image_with_hint};
    use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};

    let Some(dir) = test_dir() else {
        report("skipped: VIEWR_TEST_DIR is not set");
        return;
    };
    let bench = scanned_app(&dir);
    let heics: Vec<PathBuf> = bench
        .app
        .images
        .iter()
        .filter(|path| {
            path.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("heic"))
        })
        .cloned()
        .collect();
    let Some(path) = heics.first() else {
        report("heic phases: no HEIC file in the folder");
        return;
    };
    let box_px = (
        u32::from(fullscreen_rect().width) * CELL_PX.0,
        u32::from(fullscreen_rect().height) * CELL_PX.1,
    );

    let start = Instant::now();
    let ctx = HeifContext::read_from_file(path.to_str().unwrap()).unwrap();
    let primary = ctx.primary_image_handle().unwrap();
    let read = start.elapsed();
    let start = Instant::now();
    let image = LibHeif::new()
        .decode(&primary, ColorSpace::Rgb(RgbChroma::Rgba), None)
        .unwrap();
    let decode = start.elapsed();
    let plane = image.planes().interleaved.unwrap();
    report(&format!(
        "heic phases: read {}, libheif decode {}, plane {}x{} stride {} (row bytes {})",
        millis(read),
        millis(decode),
        plane.width,
        plane.height,
        plane.stride,
        plane.width * 4
    ));
    drop(image);

    for threads in [1u32, 2, 4, 8, 10] {
        let mut ctx = HeifContext::read_from_file(path.to_str().unwrap()).unwrap();
        ctx.set_max_decoding_threads(threads);
        let primary = ctx.primary_image_handle().unwrap();
        let start = Instant::now();
        let image = LibHeif::new()
            .decode(&primary, ColorSpace::Rgb(RgbChroma::Rgba), None)
            .unwrap();
        report(&format!(
            "heic phases: libheif decode with {threads} tile threads {}",
            millis(start.elapsed())
        ));
        drop(image);
    }

    let start = Instant::now();
    let fitted = decode_image_with_hint(path, Some(box_px), EmbeddedThumbnail::Deny).unwrap();
    let total = start.elapsed();
    report(&format!(
        "heic phases: decode_image_with_hint to {}x{} in {} (resize and copy {})",
        fitted.width(),
        fitted.height(),
        millis(total),
        millis(total.saturating_sub(decode))
    ));

    let cell_px = (528, 396);
    let start = Instant::now();
    let ctx = HeifContext::read_from_file(path.to_str().unwrap()).unwrap();
    let primary = ctx.primary_image_handle().unwrap();
    let read = start.elapsed();
    let start = Instant::now();
    let file = std::fs::File::open(path).unwrap();
    let size = file.metadata().unwrap().len();
    let reader = libheif_rs::StreamReader::new(std::io::BufReader::new(file), size);
    let streamed = HeifContext::read_from_reader(Box::new(reader)).unwrap();
    let streamed_read = start.elapsed();
    drop(streamed);
    let start = Instant::now();
    let items = crate::decode::heif_boxes::HeifItems::read(path);
    let boxes = start.elapsed();
    let mut ids = vec![0; primary.number_of_thumbnails()];
    primary.thumbnail_ids(&mut ids);
    report(&format!(
        "heic phases: preview steps: context from file {}, from stream {}, boxes {}",
        millis(read),
        millis(streamed_read),
        millis(boxes)
    ));
    match ids.first().and_then(|&id| primary.thumbnail(id).ok()) {
        Some(thumbnail) => {
            let matches = items.is_some_and(|items| items.thumbnail_matches_primary(ids[0]));
            let start = Instant::now();
            let image = LibHeif::new()
                .decode(&thumbnail, ColorSpace::Rgb(RgbChroma::Rgba), None)
                .unwrap();
            report(&format!(
                "heic phases: preview steps: thumbnail decode {}x{} {} (matches the primary image: {matches})",
                image.width(),
                image.height(),
                millis(start.elapsed())
            ));
        }
        None => report("heic phases: preview steps: no embedded thumbnail"),
    }
    let start = Instant::now();
    match crate::decode::libheif::decode_heif_preview(path, cell_px) {
        Ok(preview) => report(&format!(
            "heic phases: embedded preview {}x{} in {}",
            preview.width(),
            preview.height(),
            millis(start.elapsed())
        )),
        Err(error) => report(&format!("heic phases: no embedded preview ({error})")),
    }

    let count = heics.len().min(10);
    let start = Instant::now();
    let times: Vec<Duration> = {
        use rayon::prelude::*;
        heics[..count]
            .par_iter()
            .map(|path| {
                let one = Instant::now();
                decode_image_with_hint(path, Some(cell_px), EmbeddedThumbnail::Allow).unwrap();
                one.elapsed()
            })
            .collect()
    };
    report(&format!(
        "heic phases: {count} grid decodes in parallel: all done after {}, single decode min {} max {}",
        millis(start.elapsed()),
        millis(times.iter().copied().min().unwrap_or_default()),
        millis(times.iter().copied().max().unwrap_or_default())
    ));
}

/// The resize filters of `fast_image_resize` on a 24 MP frame.
#[test]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn resize_filters_on_a_24_mp_frame() {
    use fast_image_resize as fir;
    let (w, h) = (5712u32, 4284u32);
    let pixels: Vec<u8> = (0..w * h * 4).map(|i| (i % 251) as u8).collect();
    let src = fir::images::ImageRef::new(w, h, &pixels, fir::PixelType::U8x4).unwrap();
    let algs = [
        (
            "lanczos3",
            fir::ResizeAlg::Convolution(fir::FilterType::Lanczos3),
        ),
        (
            "catmull_rom",
            fir::ResizeAlg::Convolution(fir::FilterType::CatmullRom),
        ),
        (
            "bilinear",
            fir::ResizeAlg::Convolution(fir::FilterType::Bilinear),
        ),
        ("box", fir::ResizeAlg::Convolution(fir::FilterType::Box)),
        (
            "supersampling_bilinear_2",
            fir::ResizeAlg::SuperSampling(fir::FilterType::Bilinear, 2),
        ),
        ("nearest", fir::ResizeAlg::Nearest),
    ];
    for (name, alg) in algs {
        let mut dst = fir::images::Image::new(2688, 2016, fir::PixelType::U8x4);
        let options = fir::ResizeOptions::new().resize_alg(alg);
        let start = Instant::now();
        fir::Resizer::new()
            .resize(&src, &mut dst, &options)
            .unwrap();
        let full = start.elapsed();
        let mut dst = fir::images::Image::new(528, 396, fir::PixelType::U8x4);
        let start = Instant::now();
        fir::Resizer::new()
            .resize(&src, &mut dst, &options)
            .unwrap();
        report(&format!(
            "resize {name}: to 2688x2016 {}, to 528x396 {}",
            millis(full),
            millis(start.elapsed())
        ));
    }
}
