//! The grid loops of videos and animated images.

use super::{
    Bench, CELL_PX, COLS, LIMIT, ROWS, fullscreen_rect, grid_rect, is_animated, is_video, millis,
    remove_temp_files, report, test_dir,
};
use crate::app::App;
use crate::encoder::KittyBackend;
use crate::encoder::Transfer;
use crate::image_list::SharedImageList;
use crate::run_loop::next_timeout;
use crate::run_loop::poll_background;
use crate::run_loop::render_images;
use std::path::Path;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use std::time::Instant;

/// The folders of the loop bench, beside `PHEWR_TEST_DIR`.
const LOOP_FOLDERS: [&str; 2] = ["07-video", "06-animation"];

const LOOP_CELLS: usize = 30;

/// The grid zoom of the loop bench: 6 rows hold more than `LOOP_CELLS` cells.
const LOOP_ROWS: usize = 6;

const LOOP_STEADY: Duration = Duration::from_secs(5);

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

/// A grid of `files` with `LOOP_ROWS` rows, the temp file medium of Ghostty and its own disk cache.
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

/// The backend of a terminal like Ghostty 1.4: a fake terminal answers `OK` to both probes.
pub(super) fn animation_backend() -> KittyBackend {
    let backend = crate::encoder::kitty::detect_backend(None, None, false, |sequence, _| {
        remove_temp_files(sequence);
        Some(b"\x1b_Gi=31;OK\x1b\\\x1b_Gi=32;OK\x1b\\".to_vec())
    });
    assert_eq!(backend.transfer(), Transfer::TempFile);
    assert_eq!(backend.loops(), crate::encoder::Loops::Animation);
    backend
}

/// One run loop pass with the screen draw. Returns the wire bytes, the temp file bytes and the placements.
pub(super) fn loop_pass(
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

/// The times of a grid fill with loops: stills out, first loop frame, all loops out, Kitty bytes.
struct LoopFill {
    stills: Duration,
    first_loop: Option<Duration>,
    all_loops: Duration,
    playing: usize,
    bytes: usize,
    file_bytes: u64,
}

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

/// The CPU time of this process so far, from `ps`.
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

/// The run loop for `LOOP_STEADY` with real waits: wakeups, writes, bytes and CPU time per second.
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

/// The grid fill with loops: the first loop frame, all loops, and the steady CPU cost.
#[test]
#[ignore = "reads the folder PHEWR_TEST_DIR"]
fn grid() {
    let Some(dir) = test_dir() else {
        report("skipped: PHEWR_TEST_DIR is not set");
        return;
    };
    crate::video::find_session_tools();
    if crate::video::session_tools().ffmpeg.is_none() {
        report("loops: no ffmpeg on PATH");
        return;
    }
    let files = loop_files(&dir);
    if files.is_empty() {
        report("loops: no video or animation beside PHEWR_TEST_DIR");
        return;
    }
    let mut screen =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(COLS, ROWS)).unwrap();
    // The stills of a cold grid without loops, for comparison. The probes fill the session probe
    // cache and speed up the next stills.
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
