//! Speed measurements on a real folder. Each test is ignored and reads `VIEWR_TEST_DIR`.
//! `just bench` runs them on `~/Pictures/viewr-test/01-mixed-big` and shows the numbers.
//!
//! The tests drive the steps of the run loop (`poll_background`, `render_images`,
//! `next_timeout`) without a terminal. A sleep stands in for `event::poll`, so the numbers
//! include the wait of the loop. The Kitty sequences go to the buffer of the test app, so the
//! tests also measure the size of the payload.

use crate::app::{App, ViewMode};
use crate::decode::color::{self, ColorTarget};
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

impl Drop for Bench {
    /// The next bench starts only after the decodes of this one end, so the cold numbers of
    /// the full `just bench` match a run of one bench.
    fn drop(&mut self) {
        settle(&mut self.app);
    }
}

/// Run passes until the background work of `app` ends, without an assert, because a drop
/// during a failed bench must not panic again.
fn settle(app: &mut App) {
    let deadline = Instant::now() + LIMIT;
    let mut pending = Vec::new();
    while (app.has_pending_work() || !pending.is_empty()) && Instant::now() < deadline {
        run_pass(app, &mut pending);
        thread::sleep(Duration::from_millis(2));
    }
}

fn test_dir() -> Option<PathBuf> {
    // The color target of `main`: Display P3, or `VIEWR_BENCH_COLORSPACE=srgb`. The bench runs
    // with one test thread, so the global target cannot change under another test.
    let target = std::env::var("VIEWR_BENCH_COLORSPACE")
        .ok()
        .and_then(|value| ColorTarget::parse(&value))
        .unwrap_or_default();
    color::set_target(target);
    // Canonical, as `main` passes the start folder: `/tmp` is a link to `/private/tmp`.
    std::env::var_os("VIEWR_TEST_DIR")
        .map(PathBuf::from)
        .map(|dir| std::fs::canonicalize(&dir).unwrap_or(dir))
}

fn report(line: &str) {
    println!("bench {line}");
}

fn millis(duration: Duration) -> String {
    format!("{:.0} ms", duration.as_secs_f64() * 1000.0)
}

/// A short time with one decimal, for the cost of one frame.
fn fine_millis(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.0)
}

fn median(times: &mut [Duration]) -> Duration {
    times.sort_unstable();
    times.get(times.len() / 2).copied().unwrap_or_default()
}

/// Delete the temp files that the sequences in `out` name, as the terminal would, and return
/// their bytes.
fn remove_temp_files(out: &[u8]) -> u64 {
    crate::encoder::kitty::temp_file_paths(out)
        .iter()
        .map(|path| {
            let bytes = std::fs::metadata(path).map_or(0, |metadata| metadata.len());
            std::fs::remove_file(path).unwrap();
            bytes
        })
        .sum()
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

#[cfg(feature = "video")]
fn is_animated(path: &Path) -> bool {
    crate::player::is_animated(path)
}

#[cfg(not(feature = "video"))]
fn is_animated(_path: &Path) -> bool {
    false
}

/// The real ffmpeg and a decoder with the real ffprobe, or `None` with a note.
#[cfg(feature = "video")]
fn video_programs(label: &str) -> Option<(PathBuf, crate::player::SystemDecoder)> {
    let tools = crate::video::Tools::find(std::env::var_os("PATH").as_deref());
    let (Some(ffmpeg), Some(ffprobe)) = (tools.ffmpeg, tools.ffprobe) else {
        report(&format!("{label}: no ffmpeg or ffprobe on PATH"));
        return None;
    };
    Some((ffmpeg, crate::player::SystemDecoder::new(Some(ffprobe))))
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
    keep_files_only(&mut app, deadline);
    Bench { app, _cache: cache }
}

/// Drop the folder entries, the videos and the animated images from the scanned list, wait
/// for the cell shape and lay out the grid. The bench measures the still images only, because
/// a video and an animation open in the player.
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
    // The complete scan started the header read for the cell shape. The grid waits for it.
    while app.cell_shape_pending() {
        assert!(Instant::now() < deadline, "the header read did not finish");
        app.poll_cell_shape();
        thread::sleep(Duration::from_millis(1));
    }
    app.gallery.update_grid(grid_rect());
    app.needs_render = true;
}

/// Switch `app` to `dir` and wait for its scan, as the run loop does after Enter or Backspace.
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
/// the loop has written the pending emits, as the user sees it. The times end with the pass
/// and leave out the scan of the Kitty output, which takes milliseconds for a pass of 4 MB.
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
        let shown = pending.is_empty() && visible.iter().all(|&idx| app.thumb_cache.contains(idx));
        if shown && all_shown.is_none() {
            all_shown = Some(now);
        }
        if shown && visible.iter().all(|&idx| app.thumb_cache.is_sharp(idx)) {
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

/// The median time of 10 runs of `program` with `args`, from the spawn to the exit.
fn median_process_time(program: &Path, args: &[&str]) -> Duration {
    let mut times: Vec<Duration> = (0..10)
        .map(|_| {
            let start = Instant::now();
            let status = std::process::Command::new(program)
                .args(args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "{} did not run", program.display());
            start.elapsed()
        })
        .collect();
    times.sort();
    times[times.len() / 2]
}

/// The times of one start. The first three are the times of their steps, the others count from
/// the entry into `main`.
#[derive(Default)]
struct StartTimes {
    config: Duration,
    probe: Duration,
    app: Duration,
    first_file: Duration,
    scan: Duration,
    shape: Duration,
    first_picture: Duration,
    all_shown: Duration,
    all_sharp: Duration,
}

impl StartTimes {
    fn report(&self, label: &str) {
        report(&format!(
            "start {label}: config {}, probe without the terminal {}, app {}, first file {}, scan {}, cell shape {}, first picture {}, all cells shown {}, all cells sharp {}",
            exact_millis(self.config),
            exact_millis(self.probe),
            exact_millis(self.app),
            millis(self.first_file),
            millis(self.scan),
            millis(self.shape),
            millis(self.first_picture),
            millis(self.all_shown),
            millis(self.all_sharp)
        ));
    }
}

/// A short time with two decimals.
fn exact_millis(duration: Duration) -> String {
    format!("{:.2} ms", duration.as_secs_f64() * 1000.0)
}

/// The steps of `main` and of the run loop up to the first picture of the grid, with the disk
/// cache in `cache_dir`. A `TestBackend` stands in for the terminal, so the round trips of the
/// cell size query and of the Kitty probe are not in the numbers. The probe time is its temp
/// file only.
fn start_once(dir: &Path, cache_dir: &Path) -> StartTimes {
    let mut times = StartTimes::default();
    let start = Instant::now();
    let config_dir = tempfile::tempdir().unwrap();
    let config_path = config_dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        "[gallery]\nmouse = true\n[slideshow]\ninterval = 5.0\n",
    )
    .unwrap();
    let step = Instant::now();
    let config = crate::config::load_at(&config_path, &config_dir.path().join("state.toml"));
    times.config = step.elapsed();

    let list = SharedImageList::new();
    scanner::spawn(vec![dir.to_path_buf()], list.clone());

    let step = Instant::now();
    let transfer = crate::encoder::kitty::detect_transfer(None, false, |_| None);
    times.probe = step.elapsed();

    let step = Instant::now();
    let mut app = App::new_with_config(CELL_PX, list, &config);
    app.graphics = KittyBackend::new(transfer);
    app.disk_cache_dir = cache_dir.to_path_buf();
    let initial_dir = scanner::canonicalize_or_self(dir);
    app.initial_dir = Some(initial_dir.clone());
    app.current_dir = Some(initial_dir.clone());
    app.apply_folder_zoom(&initial_dir);
    times.app = step.elapsed();

    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(COLS, ROWS)).unwrap();
    let mut pending = Vec::new();
    loop {
        poll_background(&mut app, &mut pending);
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app, None))
            .unwrap();
        render_images(&mut app, &mut pending).unwrap();
        let now = start.elapsed();
        if times.first_file.is_zero() && !app.images.is_empty() {
            times.first_file = now;
        }
        if times.scan.is_zero() && app.scan_complete {
            times.scan = now;
        }
        if times.shape.is_zero() && app.scan_complete && !app.cell_shape_pending() {
            times.shape = now;
        }
        if times.first_picture.is_zero() && count_transmissions(&app.graphics_out.take_bytes()) > 0
        {
            times.first_picture = now;
        }
        let cells: Vec<usize> = app
            .gallery
            .visible_items()
            .map(|(_, index)| index)
            .filter(|&index| index >= app.first_file())
            .collect();
        let shown = !times.first_picture.is_zero()
            && pending.is_empty()
            && cells.iter().all(|&index| app.thumb_cache.contains(index));
        if shown && times.all_shown.is_zero() {
            times.all_shown = now;
        }
        // The next start begins without the decodes of this one.
        if shown
            && !app.has_pending_work()
            && cells.iter().all(|&index| app.thumb_cache.is_sharp(index))
        {
            times.all_sharp = now;
            return times;
        }
        assert!(now < LIMIT, "the grid did not fill");
        thread::sleep(next_timeout(&app, &pending));
    }
}

/// The start of viewr up to the first picture of the grid, with a cold and a warm disk cache.
/// The process start runs the test binary itself, which links the same libraries as viewr.
#[test]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn start_to_the_first_grid_picture() {
    let Some(dir) = test_dir() else {
        report("skipped: VIEWR_TEST_DIR is not set");
        return;
    };
    let test_binary = std::env::current_exe().unwrap();
    report(&format!(
        "start: process start {} (/usr/bin/true {}), the test binary without a test",
        exact_millis(median_process_time(
            &test_binary,
            &["--exact", "bench::no_such_test"]
        )),
        exact_millis(median_process_time(Path::new("/usr/bin/true"), &[]))
    ));
    let cache = tempfile::tempdir().unwrap();
    start_once(&dir, cache.path()).report("cold");
    start_once(&dir, cache.path()).report("warm");
}

/// One candidate file format of the thumbnail disk cache: its name, its encode and its decode.
struct CacheFormat {
    name: &'static str,
    encode: fn(&image::RgbaImage) -> Vec<u8>,
    decode: fn(&[u8]) -> image::RgbaImage,
}

fn encode_with(image: &image::RgbaImage, format: image::ImageFormat) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, format).unwrap();
    bytes.into_inner()
}

fn decode_with(bytes: &[u8], format: image::ImageFormat) -> image::RgbaImage {
    image::load_from_memory_with_format(bytes, format)
        .unwrap()
        .into_rgba8()
}

fn raw_header(image: &image::RgbaImage) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8 + image.as_raw().len());
    bytes.extend_from_slice(&image.width().to_le_bytes());
    bytes.extend_from_slice(&image.height().to_le_bytes());
    bytes
}

fn raw_dimensions(bytes: &[u8]) -> (u32, u32) {
    (
        u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
        u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
    )
}

/// PNG is the format of v10 and older, QOI the format of v11. Raw RGBA is the lower bound of
/// the read, at 2.3 times the bytes of QOI. LZ4 (`lz4_flex` 0.14, not a dependency) measured
/// 0.3 ms to 0.5 ms for the read and the decode, at 76 % of the pixel bytes.
const CACHE_FORMATS: [CacheFormat; 3] = [
    CacheFormat {
        name: "png",
        encode: |image| encode_with(image, image::ImageFormat::Png),
        decode: |bytes| decode_with(bytes, image::ImageFormat::Png),
    },
    CacheFormat {
        name: "qoi",
        encode: |image| encode_with(image, image::ImageFormat::Qoi),
        decode: |bytes| decode_with(bytes, image::ImageFormat::Qoi),
    },
    CacheFormat {
        name: "raw rgba",
        encode: |image| {
            let mut bytes = raw_header(image);
            bytes.extend_from_slice(image.as_raw());
            bytes
        },
        decode: |bytes| {
            let (width, height) = raw_dimensions(bytes);
            image::RgbaImage::from_raw(width, height, bytes[8..].to_vec()).unwrap()
        },
    },
];

/// The file formats of the thumbnail disk cache on the sharp thumbnails of the first screen:
/// the encode and the write of a cold grid, the read and the decode of a warm grid, and the
/// bytes on the disk.
#[test]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn disk_cache_formats() {
    let Some(dir) = test_dir() else {
        report("skipped: VIEWR_TEST_DIR is not set");
        return;
    };
    let bench = scanned_app(&dir);
    let app = &bench.app;
    let target = crate::app::thumbnails::thumb_target_px(app.gallery.thumb_box(), app.cell_px);
    let thumbnails: Vec<image::RgbaImage> = app
        .gallery
        .visible_items()
        .filter_map(|(_, index)| {
            crate::decode::resize::load_and_resize_px(
                &app.images[index],
                target,
                crate::decode::EmbeddedThumbnail::Allow,
            )
            .ok()
        })
        .collect();
    let pixel_bytes: usize = thumbnails.iter().map(|image| image.as_raw().len()).sum();
    report(&format!(
        "cache formats: {} thumbnails for a box of {target:?}, {} KB of pixels each on average",
        thumbnails.len(),
        pixel_bytes / thumbnails.len().max(1) / 1024
    ));
    for format in &CACHE_FORMATS {
        let files = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let encoded: Vec<Vec<u8>> = thumbnails.iter().map(format.encode).collect();
        let encode = start.elapsed();
        for (index, bytes) in encoded.iter().enumerate() {
            std::fs::write(files.path().join(index.to_string()), bytes).unwrap();
        }
        let write = start.elapsed();
        let start = Instant::now();
        for (index, image) in thumbnails.iter().enumerate() {
            let bytes = std::fs::read(files.path().join(index.to_string())).unwrap();
            assert_eq!((format.decode)(&bytes).as_raw(), image.as_raw());
        }
        let read = start.elapsed();
        let size: usize = encoded.iter().map(Vec::len).sum();
        let count = thumbnails.len().max(1) as f64;
        report(&format!(
            "cache format {}: encode {:.2} ms, with the write {:.2} ms, read and decode {:.2} ms per thumbnail, {} KB per file ({:.0} % of the pixels)",
            format.name,
            encode.as_secs_f64() * 1000.0 / count,
            write.as_secs_f64() * 1000.0 / count,
            read.as_secs_f64() * 1000.0 / count,
            size / thumbnails.len().max(1) / 1024,
            size as f64 * 100.0 / pixel_bytes.max(1) as f64
        ));
    }
}

/// The embedded JPEG previews of RAW and PSD files: the find, the decode for a grid cell and
/// for the fullscreen box, against the sips decode. Synthetic files stand in for a camera: a
/// 6000x4000 full-size preview in a TIFF, and a 1620x1080 preview in a CR3. The folder adds the
/// first file of each RAW or PSD extension, when it has one.
#[test]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
#[cfg(all(feature = "sips", target_os = "macos"))]
fn embedded_previews_of_raw_and_psd() {
    use crate::decode::embedded_jpeg;
    use crate::decode::sips;
    use crate::test_support::{write_cr3_with_preview, write_tiff_raw_with_preview};

    let Some(dir) = test_dir() else {
        report("skipped: VIEWR_TEST_DIR is not set");
        return;
    };
    let synthetic = tempfile::tempdir().unwrap();
    let full = synthetic.path().join("full-size-preview.dng");
    write_tiff_raw_with_preview(&full, (6000, 4000));
    let cr3 = synthetic.path().join("camera.cr3");
    write_cr3_with_preview(&cr3, 1, (1620, 1080));
    let mut files = vec![full, cr3];
    let bench = scanned_app(&dir);
    let mut seen: Vec<String> = Vec::new();
    for path in &bench.app.images {
        let Some(extension) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        let extension = extension.to_ascii_lowercase();
        if embedded_jpeg::may_have_embedded_jpeg(path) && !seen.contains(&extension) {
            seen.push(extension);
            files.push(path.clone());
        }
    }
    if files.len() == 2 {
        report("embedded: no RAW or PSD file in the folder, synthetic files only");
    }
    let grid_box = crate::app::thumbnails::thumb_target_px(bench.app.gallery.thumb_box(), CELL_PX);
    let fullscreen_box = (u32::from(COLS) * CELL_PX.0, u32::from(ROWS - 1) * CELL_PX.1);
    for path in &files {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let start = Instant::now();
        let Some(jpeg) = embedded_jpeg::find(path) else {
            report(&format!(
                "embedded {name}: no JPEG found in {}",
                millis(start.elapsed())
            ));
            continue;
        };
        let find = start.elapsed();
        let start = Instant::now();
        let grid = embedded_jpeg::decode(path, &jpeg, Some(grid_box)).ok();
        let grid_time = start.elapsed();
        let start = Instant::now();
        let fullscreen = embedded_jpeg::decode(path, &jpeg, Some(fullscreen_box)).ok();
        let fullscreen_time = start.elapsed();
        report(&format!(
            "embedded {name}: JPEG {}x{} ({} bytes) found in {:.2} ms, grid decode {} to {:?} \
             (covers the cell: {}), fullscreen decode {} to {:?} (covers: {})",
            jpeg.width,
            jpeg.height,
            jpeg.length,
            find.as_secs_f64() * 1000.0,
            millis(grid_time),
            grid.map(|decoded| (decoded.image.width(), decoded.image.height())),
            jpeg.covers(grid_box),
            millis(fullscreen_time),
            fullscreen.map(|decoded| (decoded.image.width(), decoded.image.height())),
            jpeg.covers(fullscreen_box),
        ));
        let start = Instant::now();
        let result = sips::decode(path, Some(grid_box));
        report(&format!(
            "embedded {name}: sips decode for the grid cell {} ({})",
            millis(start.elapsed()),
            result.map_or_else(|e| format!("failed: {e}"), |_| "ok".to_owned())
        ));
    }
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

/// A root with the one subfolder `sub`, whose files are links to the files of `dir`. The scan
/// follows a linked file, and the disk cache key takes the canonical path of the target.
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

/// The grid of a root with the folder `sub`, a cold disk cache, and the cursor on `sub`. With
/// `rest`, the cursor rests on `sub` until its prefetch ends. Then Enter. Returns the time of
/// the prefetch, the time from Enter until every file cell of the first screen is sharp, and
/// the count of those cells.
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

/// Run passes with the folder prefetch tick of the run loop until the prefetch of `folder`
/// ends. Only this bench ticks, so that no other bench starts a prefetch.
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

/// True when the screen shows at least one file cell, and every file cell is sharp.
fn all_file_cells_sharp(app: &App) -> bool {
    let mut cells = app
        .gallery
        .visible_items()
        .filter(|&(_, index)| !app.is_folder(index))
        .peekable();
    cells.peek().is_some() && cells.all(|(_, index)| app.thumb_cache.is_sharp(index))
}

#[test]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn folder_switch_with_and_without_the_prefetch() {
    let Some(dir) = test_dir() else {
        report("skipped: VIEWR_TEST_DIR is not set");
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

#[test]
#[ignore = "reads the folders VIEWR_TEST_DIR and VIEWR_TEST_PREFETCH_DIR"]
fn grid_during_a_folder_prefetch() {
    let (Some(dir), Some(other)) = (
        test_dir(),
        std::env::var_os("VIEWR_TEST_PREFETCH_DIR").map(PathBuf::from),
    ) else {
        report("skipped: VIEWR_TEST_DIR or VIEWR_TEST_PREFETCH_DIR is not set");
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
    const RUNS: usize = 10;

    let Some(dir) = test_dir() else {
        report("skipped: VIEWR_TEST_DIR is not set");
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

/// One pan step of the zoomed fullscreen view: the new crop (`load_if_needed`) and its write
/// through the temp file medium. Each pan step builds a new crop, so the write converts each
/// crop one time.
#[test]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn zoom_pan_step() {
    const STEPS: usize = 10;

    let Some(dir) = test_dir() else {
        report("skipped: VIEWR_TEST_DIR is not set");
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
        // The same crop with one transparent pixel goes out as RGBA: the write without the
        // RGB step, as before the change to RGB.
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

/// The color conversion into the terminal space, alone: the first HEIC, JPEG and PNG of the
/// folder at the fullscreen box and at a grid cell, and a full 24 MP frame for the zoom. Each
/// time is the median of 5 runs.
#[test]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn color_conversion_cost() {
    use crate::decode::resize::fit_rgba_within;
    use crate::decode::{EmbeddedThumbnail, decode_image};
    let Some(dir) = test_dir() else { return };
    let median = |work: &dyn Fn() -> Duration| {
        let mut times: Vec<Duration> = (0..5).map(|_| work()).collect();
        times.sort();
        times[2]
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    files.sort();
    for extension in ["heic", "jpg", "png"] {
        let Some(path) = files.iter().find(|path| {
            path.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(extension))
        }) else {
            continue;
        };
        for (label, box_px) in [("fullscreen", (2688, 2016)), ("grid", (528, 396))] {
            let decoded = decode_image(path, Some(box_px), EmbeddedThumbnail::Deny).unwrap();
            let source = decoded.color.clone();
            let fitted = fit_rgba_within(decoded.image.into_rgba8(), box_px.0, box_px.1);
            let time = |target: ColorTarget| {
                median(&|| {
                    let copy = fitted.clone();
                    let start = Instant::now();
                    let converted = color::convert(copy, &source, target);
                    let elapsed = start.elapsed();
                    assert_eq!(converted.dimensions(), fitted.dimensions());
                    elapsed
                })
            };
            let kind = match &source {
                color::SourceColor::Icc(data) => format!("ICC of {} bytes", data.len()),
                other => format!("{other:?}"),
            };
            report(&format!(
                "color {extension} {label} {}x{} ({kind}): into Display P3 {:.2} ms, into sRGB {:.2} ms",
                fitted.width(),
                fitted.height(),
                time(ColorTarget::DisplayP3).as_secs_f64() * 1000.0,
                time(ColorTarget::Srgb).as_secs_f64() * 1000.0,
            ));
        }
    }
    let (w, h) = (5712u32, 4284u32);
    let frame = image::RgbaImage::from_fn(w, h, |x, y| {
        image::Rgba([(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8, 255])
    });
    let full = median(&|| {
        let copy = frame.clone();
        let start = Instant::now();
        let converted = color::convert(copy, &color::SourceColor::Srgb, ColorTarget::DisplayP3);
        let elapsed = start.elapsed();
        assert_eq!(converted.width(), w);
        elapsed
    });
    report(&format!(
        "color 24 MP sRGB frame into Display P3 (zoom source): {:.1} ms",
        full.as_secs_f64() * 1000.0
    ));
}

/// The decode side of the terminal player: ffmpeg frames into a sink, for the first video of
/// the folder, at a frame box of 1280x720 and of 1920x1080.
#[cfg(feature = "video")]
#[test]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn terminal_player_decode_rate() {
    use crate::player::{Decoder, Pipeline, VideoInfo, frame_box};
    use std::sync::mpsc::TryRecvError;

    const FRAMES: usize = 300;

    let Some(dir) = test_dir() else {
        return;
    };
    let Some((ffmpeg, decoder)) = video_programs("terminal player") else {
        return;
    };
    let mut videos: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| is_video(path))
        .collect();
    videos.sort();
    let Some(video) = videos.first() else {
        report("terminal player: no video in the folder");
        return;
    };
    let info: VideoInfo = decoder.probe(video).unwrap();
    let cache = tempfile::tempdir().unwrap();
    let cube = crate::player::cube_for(&info, color::target(), &[cache.path()]);
    report(&format!(
        "terminal player: {} {}x{} {:.2} fps, hdr {:?}, rotation {}",
        video.file_name().unwrap().to_string_lossy(),
        info.width,
        info.height,
        info.fps,
        info.hdr,
        info.rotation
    ));
    for limit in [(1280, 720), (1920, 1080)] {
        let frame = frame_box((info.width, info.height), limit);
        let args = crate::player::args(video, &info, frame, cube.as_deref(), 0.0, Pipeline::Gpu);
        let bytes = frame.0 as usize * frame.1 as usize * crate::player::pixel_format(&info).1;
        let start = Instant::now();
        let mut stream = decoder.start(&ffmpeg, &args, bytes).unwrap();
        let mut first = None;
        let mut frames = 0;
        while frames < FRAMES {
            match stream.try_next() {
                Ok(_) => {
                    frames += 1;
                    first.get_or_insert_with(Instant::now);
                }
                Err(TryRecvError::Empty) => {
                    assert!(start.elapsed() < LIMIT, "ffmpeg did not deliver");
                    thread::sleep(Duration::from_micros(200));
                }
                Err(TryRecvError::Disconnected) => break,
            }
        }
        let first = first.unwrap_or_else(Instant::now);
        let decode = first.elapsed();
        let fps = if decode.is_zero() {
            0.0
        } else {
            (frames.saturating_sub(1)) as f64 / decode.as_secs_f64()
        };
        report(&format!(
            "terminal player decode {}x{}: first frame after {}, {frames} frames in {} = {fps:.0} fps",
            frame.0,
            frame.1,
            millis(first - start),
            millis(decode)
        ));
    }
}

/// The whole terminal player without a terminal: the real ffmpeg, the pacing and the Kitty
/// writes into the buffer of the test app, for 5 seconds of the first video of the folder.
/// The result is the number of frames on the screen and the number of dropped frames.
#[cfg(feature = "video")]
#[test]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn terminal_player_realtime_into_the_buffer() {
    use crate::encoder::{KittyBackend, Transfer};
    use crate::player::Player;
    use std::rc::Rc;

    const PLAY: Duration = Duration::from_secs(5);

    let Some(dir) = test_dir() else {
        return;
    };
    let Some((ffmpeg, decoder)) = video_programs("terminal player") else {
        return;
    };
    let list = SharedImageList::new();
    scanner::spawn(vec![dir], list.clone());
    let mut app = App::new(CELL_PX, list);
    app.graphics = KittyBackend::new(Transfer::TempFile);
    let deadline = Instant::now() + LIMIT;
    while !app.scan_complete {
        assert!(Instant::now() < deadline, "the scan did not finish");
        app.refresh_from_scanner();
        thread::sleep(Duration::from_millis(5));
    }
    let Some(index) = app.images.iter().position(|path| is_video(path)) else {
        report("terminal player: no video in the folder");
        return;
    };
    let cache = tempfile::tempdir().unwrap();
    app.enter_video(index);
    app.image_rect = fullscreen_rect();
    let limit = crate::player::frame_limit(fullscreen_rect(), CELL_PX);
    let player = Player::open(
        Rc::new(decoder),
        &ffmpeg,
        &app.images[index],
        limit,
        &[cache.path()],
        true,
    )
    .unwrap();
    app.video = Some(player);
    let start = Instant::now();
    let mut pending = Vec::new();
    let mut frames = 0;
    let mut bytes = 0;
    let mut first = None;
    let mut file_bytes = 0;
    while start.elapsed() < PLAY {
        run_pass(&mut app, &mut pending);
        let out = app.graphics_out.take_bytes();
        file_bytes += remove_temp_files(&out);
        if !out.is_empty() {
            frames += count_transmissions(&out);
            bytes += out.len();
            first.get_or_insert_with(Instant::now);
        }
        thread::sleep(next_timeout(&app, &pending));
    }
    let player = app.video.as_ref().unwrap();
    let shown = first.map_or(Duration::ZERO, |first| first.elapsed());
    report(&format!(
        "terminal player realtime {}x{} in {}x{} cells: {frames} frames in {}, {} dropped, \
         {:.1} fps on the screen, position {:.1} s, {bytes} bytes of sequences",
        limit.0,
        limit.1,
        fullscreen_rect().width,
        fullscreen_rect().height,
        millis(shown),
        player.dropped,
        if shown.is_zero() {
            0.0
        } else {
            (frames.saturating_sub(1)) as f64 / shown.as_secs_f64()
        },
        player.position(),
    ));
    report_video_frame_write(&app, frames, file_bytes);
}

/// The bytes per frame of the realtime run, and the time of one frame write into a buffer.
#[cfg(feature = "video")]
fn report_video_frame_write(app: &App, frames: usize, file_bytes: u64) {
    const RUNS: usize = 20;

    let mut times = Vec::new();
    for _ in 0..RUNS {
        let mut out = Vec::new();
        let start = Instant::now();
        crate::kitty_output::render_video_frame_to(app, &mut out, false).unwrap();
        times.push(start.elapsed());
        remove_temp_files(&out);
    }
    report(&format!(
        "terminal player frame: {} bytes in temp files per frame, median {} per frame write",
        file_bytes / u64::try_from(frames.max(1)).unwrap_or(1),
        fine_millis(median(&mut times))
    ));
}

/// The time from Enter on the first video of the folder to its first frame write, after
/// rests of the gallery cursor of 0 ms to 1 s. During a rest, the player starts ahead of Enter
/// after `PRESTART_DELAY` (`video_handoff::prestart`), at the first pass of the run loop after
/// it. The probe is in the cache, as after the thumbnail job. After the first frame, the clip
/// plays for 3 s, for the frame rate and the dropped frames. Then the time of the pass that
/// starts the player, and of the pass that drops it on a cursor move.
#[cfg(feature = "video")]
#[test]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn terminal_player_first_frame_after_enter() {
    use crate::config::VideoPlayer;
    use crate::player::Decoder;
    use crate::run_loop::LoopState;
    use std::rc::Rc;

    let Some(dir) = test_dir() else {
        return;
    };
    let Some((ffmpeg, decoder)) = video_programs("terminal player enter") else {
        return;
    };
    let decoder: Rc<dyn Decoder> = Rc::new(decoder);
    let mut state = LoopState::new(&crate::config::Config::default());
    state.video.player = VideoPlayer::Terminal;
    state.video.ffmpeg = Some(ffmpeg);
    state.video.decoder = Rc::clone(&decoder);
    let cases = [
        (0, Passes::RunLoop),
        (300, Passes::RunLoop),
        (500, Passes::RunLoop),
        (1000, Passes::RunLoop),
        (300, Passes::Every10Ms),
    ];
    for (rest, passes) in cases {
        if report_enter(&dir, &mut state, Duration::from_millis(rest), passes).is_none() {
            report("terminal player enter: no video in the folder");
            return;
        }
    }
    report_start_and_drop(&dir, &mut state);
}

/// The rounds of each case of [`terminal_player_first_frame_after_enter`].
#[cfg(feature = "video")]
const ENTER_ROUNDS: usize = 3;

/// How the bench passes during the rest of the cursor.
#[cfg(feature = "video")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Passes {
    /// As the run loop of an idle grid: the background polls (the status-row read of the file
    /// under the cursor) and the wait of `next_timeout`, 250 ms when nothing runs.
    RunLoop,
    /// A pass every 10 ms, as a wake-up of the run loop at the end of the delay would give.
    Every10Ms,
}

/// Measure [`ENTER_ROUNDS`] Enters after `rest` and report them.
#[cfg(feature = "video")]
fn report_enter(
    dir: &Path,
    state: &mut crate::run_loop::LoopState,
    rest: Duration,
    passes: Passes,
) -> Option<()> {
    let mut firsts = Vec::new();
    let mut starts = Vec::new();
    let mut plays = Vec::new();
    for _ in 0..ENTER_ROUNDS {
        let run = enter_and_play(dir, state, rest, passes)?;
        firsts.push(run.first);
        starts.extend(run.started);
        plays.push(format!("{:.1} fps {} dropped", run.fps, run.dropped));
    }
    let started = if starts.is_empty() {
        "no player start".to_owned()
    } else {
        format!("player start {} into the rest", millis(median(&mut starts)))
    };
    let passes = match passes {
        Passes::RunLoop => "run loop waits",
        Passes::Every10Ms => "a pass every 10 ms",
    };
    report(&format!(
        "terminal player enter after a rest of {} ({passes}): {started}, first frame after {} \
         (median of {ENTER_ROUNDS}), then {}",
        millis(rest),
        millis(median(&mut firsts)),
        plays.join(", ")
    ));
    Some(())
}

/// Report the pass that starts the player, with the HDR cube on disk and with a new cache
/// directory where the pass writes the cube, and the pass that drops the player.
#[cfg(feature = "video")]
fn report_start_and_drop(dir: &Path, state: &mut crate::run_loop::LoopState) {
    let mut warm = Vec::new();
    let mut cold = Vec::new();
    let mut drops = Vec::new();
    for _ in 0..ENTER_ROUNDS {
        let cache = tempfile::tempdir().unwrap();
        for (times, cache) in [(&mut warm, None), (&mut cold, Some(cache.path()))] {
            let rested = rest_on_the_first_video(dir, state, REST, Passes::Every10Ms, cache);
            times.extend(rested.and_then(|rested| rested.start_pass));
        }
        drops.extend(drop_after_a_rest(dir, state));
    }
    report(&format!(
        "terminal player enter: the pass that starts the player takes {} with the cube on \
         disk, {} with the write of the cube; a cursor move drops the player in {} (median of \
         {ENTER_ROUNDS})",
        fine_millis(median(&mut warm)),
        fine_millis(median(&mut cold)),
        fine_millis(median(&mut drops))
    ));
}

/// The rest of the measures of the pass that starts the player and of its drop.
#[cfg(feature = "video")]
const REST: Duration = Duration::from_secs(1);

/// One Enter of [`terminal_player_first_frame_after_enter`].
#[cfg(feature = "video")]
struct EnterRun {
    /// From the begin of the rest to the start of the player, if it started.
    started: Option<Duration>,
    /// From Enter to the first frame write.
    first: Duration,
    /// The frames on the screen per second after the first one.
    fps: f64,
    dropped: u32,
}

/// Scan `dir`, rest the gallery cursor on its first video for `rest`, press Enter, wait for
/// the first frame write and play on for 3 s.
#[cfg(feature = "video")]
fn enter_and_play(
    dir: &Path,
    state: &mut crate::run_loop::LoopState,
    rest: Duration,
    passes: Passes,
) -> Option<EnterRun> {
    const PLAY: Duration = Duration::from_secs(3);

    let rested = rest_on_the_first_video(dir, state, rest, passes, None)?;
    let mut app = rested.app;
    let mut pending = Vec::new();
    let start = Instant::now();
    app.enter_fullscreen_selected();
    app.image_rect = fullscreen_rect();
    let mut first = None;
    let mut frames = 0;
    while first.is_none_or(|first: Instant| first.elapsed() < PLAY) {
        assert!(start.elapsed() < LIMIT + PLAY, "no frame");
        crate::video_handoff::play_videos(&mut app, state, &mut crate::mpv::SystemRunner);
        run_pass(&mut app, &mut pending);
        let out = app.graphics_out.take_bytes();
        remove_temp_files(&out);
        let written = count_transmissions(&out);
        if written > 0 {
            first.get_or_insert_with(Instant::now);
            frames += written;
        }
        // Until the first frame, a look every millisecond, for the measure.
        let wait = next_timeout(&app, &pending);
        thread::sleep(if first.is_some() {
            wait
        } else {
            wait.min(Duration::from_millis(1))
        });
    }
    let first = first?;
    let shown = first.elapsed();
    Some(EnterRun {
        started: rested.started,
        first: first - start,
        fps: (frames.saturating_sub(1)) as f64 / shown.as_secs_f64(),
        dropped: app.video.as_ref().map_or(0, |player| player.dropped),
    })
}

/// An app after the rest of its cursor on a video.
#[cfg(feature = "video")]
struct Rested {
    app: App,
    /// From the begin of the rest to the pass that started the player, if one did.
    started: Option<Duration>,
    /// The time of the pass that started the player.
    start_pass: Option<Duration>,
}

/// Scan `dir` into an idle grid and rest the cursor on its first video for `rest`, with the
/// passes of `passes`. `cache` replaces the cache directory, so the start writes its cube. The
/// probe is in the cache, as after the thumbnail job. The grid draws nothing, so no thumbnail
/// job and no loop job runs: the measure is the case of a warm grid.
#[cfg(feature = "video")]
fn rest_on_the_first_video(
    dir: &Path,
    state: &mut crate::run_loop::LoopState,
    rest: Duration,
    passes: Passes,
    cache: Option<&Path>,
) -> Option<Rested> {
    let list = SharedImageList::new();
    scanner::spawn(vec![dir.to_path_buf()], list.clone());
    let mut app = App::new(CELL_PX, list);
    app.graphics = KittyBackend::new(Transfer::TempFile);
    if let Some(cache) = cache {
        app.disk_cache_dir = cache.to_path_buf();
    }
    let deadline = Instant::now() + LIMIT;
    let mut pending = Vec::new();
    while !app.scan_complete || app.has_pending_work() {
        assert!(Instant::now() < deadline, "the scan did not finish");
        poll_background(&mut app, &mut pending);
        thread::sleep(Duration::from_millis(5));
    }
    let index = app.images.iter().position(|path| is_video(path))?;
    state.video.decoder.probe(&app.images[index]).unwrap();
    app.gallery.cursor = app
        .gallery
        .filtered_indices
        .iter()
        .position(|&entry| entry == index)?;
    app.fullscreen_box = fullscreen_rect();
    let mut rested = None;
    let rest_start = Instant::now();
    loop {
        if passes == Passes::RunLoop {
            poll_background(&mut app, &mut pending);
        }
        let pass = Instant::now();
        crate::video_handoff::play_videos(&mut app, state, &mut crate::mpv::SystemRunner);
        if rested.is_none() && app.video_prestart.running() {
            rested = Some((pass - rest_start, pass.elapsed()));
        }
        let Some(left) = rest.checked_sub(rest_start.elapsed()) else {
            break;
        };
        let wait = match passes {
            Passes::RunLoop => next_timeout(&app, &pending),
            Passes::Every10Ms => Duration::from_millis(10),
        };
        // The key press of Enter wakes the run loop at the end of the rest.
        thread::sleep(wait.min(left));
    }
    Some(Rested {
        app,
        started: rested.map(|(started, _)| started),
        start_pass: rested.map(|(_, pass)| pass),
    })
}

/// The time of the pass after a cursor move away from a video whose player started during
/// the rest: the drop of the player and the end of its ffmpeg.
#[cfg(feature = "video")]
fn drop_after_a_rest(dir: &Path, state: &mut crate::run_loop::LoopState) -> Option<Duration> {
    let mut app = rest_on_the_first_video(dir, state, REST, Passes::Every10Ms, None)?.app;
    assert!(app.video_prestart.running(), "the player started");
    let entries = app.gallery.filtered_indices.len();
    if entries < 2 {
        return None;
    }
    app.gallery.cursor = (app.gallery.cursor + 1) % entries;
    let start = Instant::now();
    crate::video_handoff::play_videos(&mut app, state, &mut crate::mpv::SystemRunner);
    let elapsed = start.elapsed();
    assert!(!app.video_prestart.running(), "the move drops the player");
    Some(elapsed)
}

/// The cost of an animated file before its first frame: the header check of the route and the
/// probe of `Player::open`, both on the UI thread, for each animated file in the folder.
#[test]
#[cfg(feature = "video")]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn animation_open() {
    let Some(dir) = test_dir() else {
        report("animation open: skipped, VIEWR_TEST_DIR is not set");
        return;
    };
    let Some((_, decoder)) = video_programs("animation open") else {
        return;
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| crate::player::is_animated(path))
        .collect();
    files.sort();
    if files.is_empty() {
        report("animation open: no animated file in the folder");
    }
    for path in files {
        let size = std::fs::metadata(&path).map_or(0, |meta| meta.len());
        let start = Instant::now();
        assert!(crate::player::is_animated(&path));
        let check = start.elapsed();
        let start = Instant::now();
        let info = crate::player::Decoder::probe(&decoder, &path).unwrap();
        let probe = start.elapsed();
        report(&format!(
            "animation open {}: {} MB, header check {:.2} ms, probe {:.1} ms, {:.1} fps, {:.2} s",
            path.file_name().unwrap_or_default().to_string_lossy(),
            size / 1_000_000,
            check.as_secs_f64() * 1000.0,
            probe.as_secs_f64() * 1000.0,
            info.fps,
            info.duration,
        ));
    }
}

/// The probe and the first-frame thumbnail of each video in the folder, process start included.
#[test]
#[cfg(feature = "video")]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn video_probe_and_first_frame() {
    let Some(dir) = test_dir() else {
        report("video probe: skipped, VIEWR_TEST_DIR is not set");
        return;
    };
    let Some((ffmpeg, decoder)) = video_programs("video probe") else {
        return;
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| is_video(path))
        .collect();
    files.sort();
    let target = crate::app::thumbnails::thumb_target_px(Rect::new(0, 0, 40, 9), CELL_PX);
    let cache = tempfile::tempdir().unwrap();
    let cube_dirs = [cache.path()];
    let wait = Duration::from_secs(10);
    let ffprobe = crate::yazi::find_on_path("ffprobe", std::env::var_os("PATH").as_deref());
    let Some(ffprobe) = ffprobe else {
        return;
    };
    for path in files {
        // The probe without the cache of the session, as at the first open of a video.
        let start = Instant::now();
        let info = crate::player::probe_file(&ffprobe, &path).unwrap();
        let probe = start.elapsed();
        // The thumbnail job: the probe misses the cache and fills it.
        let start = Instant::now();
        let frame =
            crate::video::decode_first_frame(&decoder, &ffmpeg, &path, target, &cube_dirs, wait)
                .unwrap();
        let first = start.elapsed();
        // The probe of `Player::open` after the thumbnail: a cache hit.
        let start = Instant::now();
        crate::player::Decoder::probe(&decoder, &path).unwrap();
        let cached = start.elapsed();
        report(&format!(
            "video {}: probe {:.1} ms, first frame {:.1} ms (probe included), probe after \
             the thumbnail {:.2} ms, {}x{} hdr {:?} -> {:?}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            probe.as_secs_f64() * 1000.0,
            first.as_secs_f64() * 1000.0,
            cached.as_secs_f64() * 1000.0,
            info.width,
            info.height,
            info.hdr,
            frame.dimensions(),
        ));
    }
}

/// The folders of the loop bench, beside `VIEWR_TEST_DIR`.
#[cfg(feature = "video")]
const LOOP_FOLDERS: [&str; 2] = ["07-video", "06-animation"];

/// The cells of the loop bench: the videos and animations of `LOOP_FOLDERS`, repeated.
#[cfg(feature = "video")]
const LOOP_CELLS: usize = 30;

/// The grid zoom of the loop bench: 6 rows hold more than `LOOP_CELLS` cells.
#[cfg(feature = "video")]
const LOOP_ROWS: usize = 6;

/// The time of each steady measurement of the loop bench.
#[cfg(feature = "video")]
const LOOP_STEADY: Duration = Duration::from_secs(5);

/// The video and animated files of `LOOP_FOLDERS` beside `dir`, repeated to `LOOP_CELLS`.
#[cfg(feature = "video")]
fn loop_files(dir: &Path) -> Vec<PathBuf> {
    let Some(parent) = dir.parent() else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = LOOP_FOLDERS
        .iter()
        .filter_map(|folder| std::fs::read_dir(parent.join(folder)).ok())
        .flat_map(|entries| entries.flatten().map(|entry| entry.path()))
        .filter(|path| is_video(path) || is_animated(path))
        .collect();
    files.sort();
    files.iter().cycle().take(LOOP_CELLS).cloned().collect()
}

/// A grid of `files` with `LOOP_ROWS` rows, the temp file medium of Ghostty and its own disk
/// cache in `cache`.
#[cfg(feature = "video")]
fn loop_app(files: &[PathBuf], cache: &Path) -> App {
    let mut app = App::new(CELL_PX, SharedImageList::new());
    app.disk_cache_dir = cache.to_path_buf();
    app.graphics = KittyBackend::new(Transfer::TempFile);
    app.images = files.to_vec();
    app.filenames = files
        .iter()
        .map(|path| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    app.gallery.filtered_indices = (0..files.len()).collect();
    app.gallery.set_zoom_rows(LOOP_ROWS);
    app.gallery.update_grid(grid_rect());
    app.scan_complete = true;
    app.needs_render = true;
    app
}

/// The backend that the start picks for a terminal like Ghostty 1.4: the probes go to a fake
/// terminal that reads and deletes the probe file and answers `OK` to both probes.
#[cfg(feature = "video")]
fn animation_backend() -> KittyBackend {
    let backend = crate::encoder::kitty::detect_backend(None, None, false, |sequence, _| {
        remove_temp_files(sequence);
        Some(b"\x1b_Gi=31;OK\x1b\\\x1b_Gi=32;OK\x1b\\".to_vec())
    });
    assert_eq!(backend.transfer(), Transfer::TempFile);
    assert_eq!(backend.loops(), crate::encoder::Loops::Animation);
    backend
}

/// One pass of the run loop with the draw of the screen: the polls, the draw into a test
/// backend and the Kitty writes. Returns the bytes on the wire, the bytes in temp files and the
/// placements.
#[cfg(feature = "video")]
fn loop_pass(
    app: &mut App,
    screen: &mut ratatui::Terminal<ratatui::backend::TestBackend>,
    pending: &mut Vec<usize>,
) -> (usize, u64, usize) {
    poll_background(app, pending);
    screen
        .draw(|frame| crate::ui::draw(frame, app, None))
        .unwrap();
    app.fullscreen_box = fullscreen_rect();
    if render_images(app, pending).unwrap() {
        screen
            .draw(|frame| crate::ui::draw(frame, app, None))
            .unwrap();
    }
    let out = app.graphics_out.take_bytes();
    let places = String::from_utf8_lossy(&out).matches("\x1b_Ga=p,").count();
    (out.len(), remove_temp_files(&out), places)
}

/// The times of a grid fill with loops: every still out, the first loop frame on the screen,
/// every loop out, and the bytes of the Kitty output.
#[cfg(feature = "video")]
struct LoopFill {
    stills: Duration,
    first_loop: Option<Duration>,
    all_loops: Duration,
    playing: usize,
    bytes: usize,
    file_bytes: u64,
}

#[cfg(feature = "video")]
impl LoopFill {
    fn report(&self, label: &str) {
        report(&format!(
            "loops {label}: all stills {}, first loop frame {}, all loops {}, {} cells loop, \
             kitty bytes {} and {} in temp files",
            millis(self.stills),
            self.first_loop.map_or_else(|| "none".to_owned(), millis),
            millis(self.all_loops),
            self.playing,
            self.bytes,
            self.file_bytes
        ));
    }
}

/// Fill the grid of `app` until the loops of every visible cell play or failed.
#[cfg(feature = "video")]
fn measure_loops(
    app: &mut App,
    screen: &mut ratatui::Terminal<ratatui::backend::TestBackend>,
) -> LoopFill {
    let start = Instant::now();
    let mut pending = Vec::new();
    let mut stills = None;
    let mut first_loop = None;
    let mut bytes = 0;
    let mut file_bytes = 0;
    loop {
        let (written, files, _) = loop_pass(app, screen, &mut pending);
        bytes += written;
        file_bytes += files;
        let now = start.elapsed();
        let visible: Vec<usize> = app
            .gallery
            .visible_items()
            .map(|(_, index)| index)
            .collect();
        let playing = visible
            .iter()
            .filter(|&&index| app.loop_plays(index))
            .count();
        let sharp = visible.iter().all(|&index| app.thumb_cache.is_sharp(index));
        if stills.is_none() && sharp && pending.is_empty() {
            stills = Some(now);
        }
        if first_loop.is_none() && playing > 0 {
            first_loop = Some(now);
        }
        if stills.is_some() && !app.has_pending_work() && !app.loops_to_send() {
            return LoopFill {
                stills: stills.unwrap_or_default(),
                first_loop,
                all_loops: now,
                playing,
                bytes,
                file_bytes,
            };
        }
        assert!(start.elapsed() < LIMIT, "the loops did not start");
        thread::sleep(next_timeout(app, &pending));
    }
}

/// The CPU time of this process so far, from `ps` (user and system, all threads).
#[cfg(feature = "video")]
fn process_cpu() -> Duration {
    let output = std::process::Command::new("ps")
        .args(["-o", "time=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    let seconds = text.trim().split(':').fold(0.0, |total, part| {
        total * 60.0 + part.parse::<f64>().unwrap_or(0.0)
    });
    Duration::from_secs_f64(seconds)
}

/// The run loop of `app` for `LOOP_STEADY` with its real waits: the wakeups and the writes
/// per second, the bytes per write, and the CPU time per second of the process and of the
/// passes.
#[cfg(feature = "video")]
fn measure_steady(
    app: &mut App,
    screen: &mut ratatui::Terminal<ratatui::backend::TestBackend>,
    label: &str,
) {
    let mut pending = Vec::new();
    let mut wakeups = 0_u32;
    let mut writes = 0_u32;
    let mut bytes = 0;
    let mut places = 0;
    let mut file_bytes = 0;
    let mut busy = Duration::ZERO;
    let cpu = process_cpu();
    let start = Instant::now();
    while start.elapsed() < LOOP_STEADY {
        let pass = Instant::now();
        let (written, files, placed) = loop_pass(app, screen, &mut pending);
        busy += pass.elapsed();
        wakeups += 1;
        if written > 0 {
            writes += 1;
            bytes += written;
            places += placed;
        }
        file_bytes += files;
        let left = LOOP_STEADY.saturating_sub(start.elapsed());
        thread::sleep(next_timeout(app, &pending).min(left));
    }
    let cpu = process_cpu().saturating_sub(cpu);
    let seconds = start.elapsed().as_secs_f64();
    let playing = app
        .gallery
        .visible_items()
        .filter(|&(_, index)| app.loop_plays(index))
        .count();
    report(&format!(
        "loops steady, {label}: {playing} cells loop, {:.1} wakeups and {:.1} writes per \
         second, {} bytes per write, {} bytes per frame swap, {file_bytes} bytes in temp \
         files, CPU of the process {:.1} ms per second, run loop busy {:.1} ms per second",
        f64::from(wakeups) / seconds,
        f64::from(writes) / seconds,
        bytes / writes.max(1) as usize,
        bytes / places.max(1),
        cpu.as_secs_f64() * 1000.0 / seconds,
        busy.as_secs_f64() * 1000.0 / seconds,
    ));
}

#[cfg(feature = "video")]
#[test]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn grid_loops() {
    let Some(dir) = test_dir() else {
        report("skipped: VIEWR_TEST_DIR is not set");
        return;
    };
    crate::video::find_session_tools();
    if crate::video::session_tools().ffmpeg.is_none() {
        report("loops: no ffmpeg on PATH");
        return;
    }
    let files = loop_files(&dir);
    if files.is_empty() {
        report("loops: no video or animation beside VIEWR_TEST_DIR");
        return;
    }
    let mut screen =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(COLS, ROWS)).unwrap();
    // The stills of a cold grid without loops, for the comparison. The probes of this run
    // fill the probe cache of the session, so the stills of the next run are a little faster.
    let cache = tempfile::tempdir().unwrap();
    let mut still_app = loop_app(&files, cache.path());
    still_app.grid_loops.enabled = false;
    let mut bench = Bench {
        app: still_app,
        _cache: cache,
    };
    measure_loops(&mut bench.app, &mut screen).report("cold, animate = false");
    drop(bench);

    let cache = tempfile::tempdir().unwrap();
    let app = loop_app(&files, cache.path());
    let mut bench = Bench { app, _cache: cache };
    measure_loops(&mut bench.app, &mut screen).report("cold");
    report(&format!(
        "loops: {} files of {} in {} visible cells of {}x{} cells",
        files.len(),
        LOOP_FOLDERS.join(" and "),
        bench.app.gallery.visible_items().count(),
        bench.app.gallery.thumb_cols,
        bench.app.gallery.thumb_rows,
    ));

    bench.app.mark_dirty();
    measure_loops(&mut bench.app, &mut screen).report("warm (disk cache), ticks");
    measure_steady(&mut bench.app, &mut screen, "animate = true, ticks");

    bench.app.grid_loops.enabled = false;
    bench.app.needs_render = true;
    measure_steady(
        &mut bench.app,
        &mut screen,
        "animate = false (the static grid)",
    );

    // The same grid in a terminal that plays Kitty animations.
    bench.app.graphics = animation_backend();
    bench.app.grid_loops.enabled = true;
    bench.app.mark_dirty();
    measure_loops(&mut bench.app, &mut screen).report("warm (disk cache), animation");
    measure_steady(&mut bench.app, &mut screen, "animate = true, animation");
}

/// The layouts of `decode/screen/mod.rs` against the RGBA layout before them, step by step, in one
/// process: each step runs both ways in turn, so the load of the machine hits both the same.
#[cfg(feature = "turbo")]
mod layout_bench {
    use crate::app::App;
    use crate::decode::color::{self, ColorTarget};
    use crate::decode::resize::{rgb_resize_is_fastest, scaled_down_size};
    use crate::decode::screen::ScreenImage;
    use crate::decode::{EmbeddedThumbnail, decode_image};
    use crate::encoder::{KittyBackend, Transfer, kitty};
    use crate::image_list::SharedImageList;
    use crate::kitty_output::render_fullscreen_image_to;
    use image::{DynamicImage, RgbaImage};
    use ratatui::layout::Rect;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    /// The fullscreen box of `bench.rs`: 106x38 cells of 32x54 px.
    const BOX_PX: (u32, u32) = (3392, 2052);
    const RUNS: usize = 9;

    /// The median of `RUNS` runs of the RGBA way and of the new way, in turns.
    fn medians(
        mut before: impl FnMut() -> Duration,
        mut after: impl FnMut() -> Duration,
    ) -> (f64, f64) {
        let (mut old, mut new) = (Vec::new(), Vec::new());
        for _ in 0..RUNS {
            old.push(before());
            new.push(after());
        }
        let median = |times: &mut Vec<Duration>| {
            times.sort();
            times[RUNS / 2].as_secs_f64() * 1000.0
        };
        (median(&mut old), median(&mut new))
    }

    fn report(step: &str, (before, after): (f64, f64)) {
        // clippy.toml allows output in tests. `just bench` shows the lines.
        eprintln!("bench layout {step}: before {before:.1} ms, after {after:.1} ms");
    }

    fn time<T>(work: impl FnOnce() -> T) -> Duration {
        let start = Instant::now();
        let result = work();
        let elapsed = start.elapsed();
        drop(result);
        elapsed
    }

    /// The fullscreen write of `image` through the temp file medium, without the temp file.
    fn write(app: &mut App, image: ScreenImage) -> Duration {
        app.loaded = Some(image);
        let mut out = Vec::new();
        let elapsed = time(|| render_fullscreen_image_to(app, &mut out).unwrap());
        for path in kitty::temp_file_paths(&out) {
            std::fs::remove_file(path).unwrap();
        }
        elapsed
    }

    fn fullscreen_app() -> App {
        let mut app = App::new((32, 54), SharedImageList::new());
        app.graphics = KittyBackend::new(Transfer::TempFile);
        app.image_rect = Rect::new(0, 0, 106, 38);
        app
    }

    /// The fit of the full decode into the box and the color conversion: RGBA with the alpha
    /// multiply before, the layout of `rgb_resize_is_fastest` now.
    fn measure_fit(name: &str, full: &RgbaImage, color: &color::SourceColor) {
        let size = full.dimensions();
        let dst = scaled_down_size(size, BOX_PX).unwrap_or(size);
        let after_input = if rgb_resize_is_fastest(size, Some(dst)) {
            ScreenImage::Rgb(DynamicImage::ImageRgba8(full.clone()).into_rgb8())
        } else {
            ScreenImage::Opaque(full.clone())
        };
        let before_input = ScreenImage::Rgba(full.clone());
        for target in [ColorTarget::DisplayP3, ColorTarget::Srgb] {
            let times = medians(
                || {
                    time(|| {
                        before_input
                            .crop_resized((0, 0, size.0, size.1), dst)
                            .convert(color, target)
                    })
                },
                || {
                    let input = after_input.clone();
                    time(|| input.fit_within(BOX_PX.0, BOX_PX.1).convert(color, target))
                },
            );
            report(
                &format!(
                    "{name} fit {}x{} into {}x{} and convert into {target:?}",
                    size.0, size.1, dst.0, dst.1
                ),
                times,
            );
        }
    }

    fn measure(path: &Path) {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let decoded = decode_image(path, None, EmbeddedThumbnail::Deny).unwrap();
        let color = decoded.color.clone();
        let full = decoded.image.to_rgba8();
        measure_fit(&name, &full, &color);

        let shown = ScreenImage::Opaque(full.clone()).fit_within(BOX_PX.0, BOX_PX.1);
        let rgba = ScreenImage::Rgba(shown.clone().into_rgba());
        let (mut rgba_app, mut rgb_app) = (fullscreen_app(), fullscreen_app());
        let writes = medians(
            || write(&mut rgba_app, rgba.clone()),
            || write(&mut rgb_app, shown.clone()),
        );
        report(
            &format!(
                "{name} fullscreen write {}x{}",
                shown.width(),
                shown.height()
            ),
            writes,
        );

        let (w, h) = full.dimensions();
        let old_source = DynamicImage::ImageRgba8(full.clone());
        let new_source = ScreenImage::Opaque(full).for_crops();
        for zoom in [1.25, 2.0, 4.0] {
            // The crop of `build_zoom_view` in the middle of the picture.
            let eff = f64::min(
                f64::from(BOX_PX.0) / f64::from(w),
                f64::from(BOX_PX.1) / f64::from(h),
            )
            .min(1.0)
                * zoom;
            let crop_w = ((f64::from(BOX_PX.0) / eff).round() as u32).clamp(1, w);
            let crop_h = ((f64::from(BOX_PX.1) / eff).round() as u32).clamp(1, h);
            let crop = ((w - crop_w) / 2, (h - crop_h) / 2, crop_w, crop_h);
            let dst_w = ((f64::from(crop_w) * eff).round() as u32).clamp(1, BOX_PX.0);
            let dst_h = ((f64::from(crop_h) * eff).round() as u32).clamp(1, BOX_PX.1);
            let (mut rgba_app, mut rgb_app) = (fullscreen_app(), fullscreen_app());
            let times = medians(
                || {
                    // Before: a crop copy, an RGBA copy of it, the resize with the alpha
                    // multiply and the write with the scan and the RGB copy.
                    let start = Instant::now();
                    let copy = old_source
                        .crop_imm(crop.0, crop.1, crop_w, crop_h)
                        .to_rgba8();
                    let resized = ScreenImage::Rgba(copy)
                        .crop_resized((0, 0, crop_w, crop_h), (dst_w, dst_h));
                    write(&mut rgba_app, resized);
                    start.elapsed()
                },
                || {
                    let start = Instant::now();
                    let crop = new_source.crop_resized(crop, (dst_w, dst_h));
                    write(&mut rgb_app, crop);
                    start.elapsed()
                },
            );
            report(
                &format!(
                    "{name} zoom {zoom} pan step {crop_w}x{crop_h} into {dst_w}x{dst_h}, crop and write"
                ),
                times,
            );
        }
    }

    /// The decoders alone: libjpeg-turbo and libheif at full size in both layouts.
    fn measure_decoders(jpeg: Option<&PathBuf>, heic: Option<&PathBuf>) {
        if let Some(path) = jpeg {
            let data = std::fs::read(path).unwrap();
            let decode = |format| time(|| turbojpeg::decompress(&data, format).unwrap());
            let times = medians(
                || decode(turbojpeg::PixelFormat::RGBA),
                || decode(turbojpeg::PixelFormat::RGB),
            );
            report("libjpeg-turbo full decode, RGBA before, RGB after", times);
        }
        #[cfg(feature = "heic")]
        if let Some(path) = heic {
            use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};
            let ctx = HeifContext::read_from_file(path.to_str().unwrap()).unwrap();
            let handle = ctx.primary_image_handle().unwrap();
            let decode = |chroma| {
                time(|| {
                    LibHeif::new()
                        .decode(&handle, ColorSpace::Rgb(chroma), None)
                        .unwrap()
                })
            };
            let times = medians(|| decode(RgbChroma::Rgba), || decode(RgbChroma::Rgb));
            report("libheif full decode, RGBA before, RGB after", times);
        }
        #[cfg(not(feature = "heic"))]
        let _ = heic;
    }

    #[test]
    #[ignore = "reads the folder VIEWR_TEST_DIR"]
    fn bench_the_layouts_against_the_rgba_layout() {
        let Some(dir) = std::env::var_os("VIEWR_TEST_DIR").map(PathBuf::from) else {
            eprintln!("skipped: VIEWR_TEST_DIR is not set");
            return;
        };
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        files.sort();
        let first = |extension: &str| {
            files.iter().find(|path| {
                path.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case(extension))
            })
        };
        for path in ["heic", "jpg", "png"].into_iter().filter_map(first) {
            measure(path);
        }
        measure_decoders(first("jpg"), first("heic"));
    }
}

/// The cost of the three ways to send an opaque RGBA animation frame through the temp file medium.
mod frame_format {
    use crate::encoder::kitty::{self, Keys, write_pixels};
    use crate::encoder::{Pixels, Transfer};
    use image::RgbaImage;

    #[test]
    #[ignore = "measurement: cargo test --release frame_format_timings -- --ignored --nocapture"]
    fn frame_format_timings() {
        for (w, h) in [(400u32, 300u32), (1280, 720), (1920, 1080)] {
            let image = RgbaImage::from_pixel(w, h, image::Rgba([1, 2, 3, 255]));
            let keys = Keys {
                first: &|f| format!("a=f,q=2,f={f},s={w},v={h},i=7,z=83,N=1"),
                chunk: "a=f,",
            };
            for name in ["scan and copy", "copy only (scan once)", "as is (RGBA)"] {
                let mut times = Vec::new();
                for _ in 0..200 {
                    let mut out = Vec::new();
                    let start = std::time::Instant::now();
                    let pixels = Pixels::from(&image);
                    match name {
                        "as is (RGBA)" => {
                            write_pixels(
                                &mut out,
                                pixels,
                                &keys,
                                Transfer::TempFile,
                                &std::env::temp_dir(),
                            )
                            .unwrap();
                        }
                        "scan and copy" => {
                            let rgb = pixels.opaque_rgb().unwrap();
                            let rgb_pixels = Pixels {
                                alpha: false,
                                data: &rgb,
                                ..pixels
                            };
                            write_pixels(
                                &mut out,
                                rgb_pixels,
                                &keys,
                                Transfer::TempFile,
                                &std::env::temp_dir(),
                            )
                            .unwrap();
                        }
                        _ => {
                            let (source, _) = pixels.data.as_chunks::<4>();
                            let mut rgb = vec![0; source.len() * 3];
                            for (&[r, g, b, _], target) in
                                source.iter().zip(rgb.as_chunks_mut::<3>().0)
                            {
                                *target = [r, g, b];
                            }
                            let rgb_pixels = Pixels {
                                alpha: false,
                                data: &rgb,
                                ..pixels
                            };
                            write_pixels(
                                &mut out,
                                rgb_pixels,
                                &keys,
                                Transfer::TempFile,
                                &std::env::temp_dir(),
                            )
                            .unwrap();
                        }
                    }
                    times.push(start.elapsed());
                    for path in kitty::temp_file_paths(&out) {
                        std::fs::remove_file(path).unwrap();
                    }
                }
                times.sort();
                eprintln!("MICRO {w}x{h} {name}: median {:?}", times[100]);
            }
        }
    }
}
