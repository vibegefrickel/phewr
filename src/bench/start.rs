//! The start of viewr up to the first grid picture.

use super::{CELL_PX, COLS, LIMIT, ROWS, count_transmissions, millis, report, test_dir};
use crate::app::App;
use crate::encoder::KittyBackend;
use crate::image_list::SharedImageList;
use crate::run_loop::next_timeout;
use crate::run_loop::poll_background;
use crate::run_loop::render_images;
use crate::scanner;
use std::path::Path;
use std::thread;
use std::time::Duration;
use std::time::Instant;

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

/// The times of one start. The first three are step times, the rest count from `main`.
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

fn exact_millis(duration: Duration) -> String {
    format!("{:.2} ms", duration.as_secs_f64() * 1000.0)
}

/// The steps of `main` and of the run loop up to the first grid picture. A `TestBackend` stands
/// in for the terminal: the probe round trips are not in the numbers.
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

/// The start up to the first grid picture, with a cold and a warm disk cache.
#[test]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn first_grid_picture() {
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
