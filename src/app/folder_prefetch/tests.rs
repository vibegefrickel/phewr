//! The folder prefetch on a tree of PNG folders with its own disk cache.

use super::worker::{Control, first_screen, prefetch};
use super::{FOLDER_PREFETCH_DELAY, Running, START_POLL};
use crate::app::{App, ViewMode};
use crate::config;
use crate::pictures::jobs::disk_cache::get_cache_file_path;
use crate::pictures::jobs::queue::ThumbJob;
use crate::pictures::resize::padding::thumb_target_px;
use crate::test_support::{app, wait_until};
use image::RgbaImage;
use ratatui::layout::Rect;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// A root with the subfolders `a` (5 landscape files) and `b` (40 portrait files), and an app
/// on a terminal of 120 by 40 cells. The root lists `..`, `a` and `b`, and the cursor rests on
/// `a`.
struct Tree {
    temp: TempDir,
    root: PathBuf,
    app: App,
}

impl Tree {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap().join("root");
        pngs(&root.join("a"), 5, (40, 30));
        pngs(&root.join("b"), 40, (30, 40));
        let mut app = app();
        app.disk_cache_dir = temp.path().join("cache");
        app.state_file = temp.path().join("state.toml");
        app.gallery.update_grid(Rect::new(0, 0, 120, 40));
        app.switch_to_dir(&root);
        wait_for_scan(&mut app);
        assert_eq!(app.filenames, ["..", "a", "b"]);
        app.gallery.cursor = 1;
        Self { temp, root, app }
    }
}

fn cache_files(temp: &TempDir) -> usize {
    fs::read_dir(temp.path().join("cache")).map_or(0, Iterator::count)
}

fn pngs(dir: &Path, count: usize, size: (u32, u32)) {
    fs::create_dir_all(dir).unwrap();
    for index in 0..count {
        RgbaImage::new(size.0, size.1)
            .save(dir.join(format!("{index:02}.png")))
            .unwrap();
    }
}

/// Wait for the scan and the cell shape, as the first screen does.
fn wait_for_scan(app: &mut App) {
    wait_until(|| {
        app.refresh_from_scanner();
        app.scan_complete
    });
    wait_until(|| {
        app.poll_cell_shape();
        !app.cell_shape_pending()
    });
}

fn finish(app: &mut App, folder: &Path, now: Instant) {
    wait_until(|| {
        app.tick_folder_prefetch(now);
        app.folder_prefetched(folder)
    });
}

/// The files of the first screen of the grid and their decode box.
fn shown_files(app: &App) -> (Vec<PathBuf>, (u32, u32)) {
    let files = app
        .gallery
        .visible_items()
        .filter(|&(_, index)| !app.is_folder(index))
        .map(|(_, index)| app.images[index].clone())
        .collect();
    (files, thumb_target_px(app.gallery.thumb_box(), app.cell_px))
}

fn queue_grid_job(app: &mut App) {
    app.thumb_jobs.sharp.waiting.push_back(ThumbJob {
        index: 0,
        rect: Rect::new(0, 0, 10, 5),
    });
}

#[test]
fn starts_after_cursor_rest() {
    let mut tree = Tree::new();
    let app = &mut tree.app;
    let a = tree.root.join("a");
    let start = Instant::now();
    app.tick_folder_prefetch(start);
    assert_eq!(app.folder_prefetch_running(), None);
    assert_eq!(
        app.folder_prefetch_timeout(start + Duration::from_millis(100)),
        Some(Duration::from_millis(200)),
        "the loop wakes when the rest is due"
    );

    // A cursor that walks on to the next folder starts nothing.
    app.gallery.cursor = 2;
    app.tick_folder_prefetch(start + Duration::from_millis(200));
    app.tick_folder_prefetch(start + FOLDER_PREFETCH_DELAY);
    assert_eq!(app.folder_prefetch_running(), None);

    app.gallery.cursor = 1;
    let rest = start + Duration::from_secs(1);
    app.tick_folder_prefetch(rest);
    app.tick_folder_prefetch(rest + Duration::from_millis(299));
    assert_eq!(app.folder_prefetch_running(), None);
    app.tick_folder_prefetch(rest + FOLDER_PREFETCH_DELAY);
    assert_eq!(app.folder_prefetch_running(), Some(a.as_path()));
    assert_eq!(app.folder_prefetch_timeout(rest), None, "a thread runs");
    finish(app, &a, rest + FOLDER_PREFETCH_DELAY);
    assert_eq!(app.folder_prefetch_running(), None);
    assert_eq!(cache_files(&tree.temp), 5, "every file of the first screen");
}

#[test]
fn other_view_starts_nothing() {
    let mut tree = Tree::new();
    let app = &mut tree.app;
    app.mode = ViewMode::Fullscreen;
    let start = Instant::now();
    app.tick_folder_prefetch(start);
    app.tick_folder_prefetch(start + FOLDER_PREFETCH_DELAY);
    assert_eq!(app.folder_prefetch_running(), None);
    assert_eq!(app.folder_prefetch_timeout(start), None, "no rest waits");
}

#[test]
fn target_box_follows_folder() {
    let mut tree = Tree::new();
    let app = &mut tree.app;
    let b = tree.root.join("b");
    // `b` holds portraits and has its own zoom level. The root has neither.
    config::save_visible_rows(&app.state_file, Some(&b), 3);
    assert_eq!(app.gallery.zoom_rows, 4);
    app.gallery.cursor = 2;
    let here = thumb_target_px(app.gallery.thumb_box(), app.cell_px);

    let screen = first_screen(&app.folder_request(&b));
    assert_ne!(screen.target, here);
    assert!(screen.target.0 < screen.target.1, "a portrait box");

    assert!(app.enter_selected_folder());
    wait_for_scan(app);
    assert_eq!(app.gallery.zoom_rows, 3);
    let (files, target) = shown_files(app);
    assert_eq!(screen.target, target);
    assert_eq!(screen.files, files);
    assert!(files.len() < 40, "the first screen only");
}

#[test]
fn enter_finds_every_cell_cached() {
    let mut tree = Tree::new();
    let app = &mut tree.app;
    let b = tree.root.join("b");
    app.gallery.cursor = 2;
    let start = Instant::now();
    app.tick_folder_prefetch(start);
    app.tick_folder_prefetch(start + FOLDER_PREFETCH_DELAY);
    finish(app, &b, start + FOLDER_PREFETCH_DELAY);

    assert!(app.enter_selected_folder());
    wait_for_scan(app);
    let (files, target) = shown_files(app);
    assert!(!files.is_empty());
    for file in &files {
        let cached = get_cache_file_path(&app.disk_cache_dir, file, target).unwrap();
        assert!(
            cached.exists(),
            "{} is not in the disk cache",
            file.display()
        );
    }
    assert_eq!(
        cache_files(&tree.temp),
        files.len(),
        "nothing beyond the first screen"
    );
}

#[test]
fn cursor_move_cancels_before_next_file() {
    let mut tree = Tree::new();
    let app = &mut tree.app;
    let start = Instant::now();
    app.tick_folder_prefetch(start);
    // A grid job holds the new thread before its first file.
    queue_grid_job(app);
    let key = app.folder_prefetch.rest.clone().unwrap().0;
    app.start_folder_prefetch(key);
    let control = Arc::clone(&app.folder_prefetch.running.as_ref().unwrap().control);
    assert!(control.paused.load(Ordering::Acquire));
    app.clear_thumb_queue();

    app.gallery.cursor = 2;
    app.tick_folder_prefetch(start);
    assert!(control.cancelled());
    assert_eq!(app.folder_prefetch_running(), None);
    wait_until(|| !app.folder_prefetch.is_stopping());
    assert!(!app.folder_prefetched(&tree.root.join("a")));
    assert_eq!(cache_files(&tree.temp), 0);
}

#[test]
fn prefetches_never_overlap() {
    let mut tree = Tree::new();
    let app = &mut tree.app;
    let a = tree.root.join("a");
    // A cancelled thread that still decodes its last file.
    let (release, hold) = mpsc::channel::<()>();
    app.folder_prefetch.stopping = vec![thread::spawn(move || {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "the test only waits for the release"
        )]
        let _ = hold.recv();
    })];
    let start = Instant::now();
    app.tick_folder_prefetch(start);
    let due = start + FOLDER_PREFETCH_DELAY;
    app.tick_folder_prefetch(due);
    assert_eq!(app.folder_prefetch_running(), None, "the old thread runs");
    assert_eq!(app.folder_prefetch_timeout(due), Some(START_POLL));

    release.send(()).unwrap();
    wait_until(|| {
        app.tick_folder_prefetch(due);
        app.folder_prefetch_running().is_some() || app.folder_prefetched(&a)
    });
}

#[test]
fn two_cancels_wait_for_both_threads() {
    let mut tree = Tree::new();
    let key = tree.app.rest_key().unwrap();
    let prefetch = &mut tree.app.folder_prefetch;
    let (release_old, hold_old) = mpsc::channel::<()>();
    let (release_new, hold_new) = mpsc::channel::<()>();
    let hold = |receiver: mpsc::Receiver<()>| {
        thread::spawn(move || {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "the test only waits for the release"
            )]
            let _ = receiver.recv();
        })
    };
    prefetch.stopping.push(hold(hold_old));
    prefetch.running = Some(Running {
        key,
        control: Arc::new(Control::default()),
        handle: hold(hold_new),
    });
    prefetch.cancel();
    assert_eq!(prefetch.stopping.len(), 2, "no handle is lost");

    release_old.send(()).unwrap();
    wait_until(|| prefetch.stopping.iter().filter(|h| h.is_finished()).count() == 1);
    assert!(prefetch.is_stopping(), "the second thread still decodes");
    release_new.send(()).unwrap();
    wait_until(|| !prefetch.is_stopping());
}

#[test]
fn grid_jobs_hold_start_and_pause() {
    let mut tree = Tree::new();
    let app = &mut tree.app;
    let a = tree.root.join("a");
    let start = Instant::now();
    queue_grid_job(app);
    app.tick_folder_prefetch(start);
    let due = start + FOLDER_PREFETCH_DELAY;
    app.tick_folder_prefetch(due);
    assert_eq!(app.folder_prefetch_running(), None, "the grid has a job");

    let key = app.folder_prefetch.rest.clone().unwrap().0;
    app.start_folder_prefetch(key);
    app.tick_folder_prefetch(due);
    let control = Arc::clone(&app.folder_prefetch.running.as_ref().unwrap().control);
    assert!(control.paused.load(Ordering::Acquire));
    assert_eq!(cache_files(&tree.temp), 0);

    app.clear_thumb_queue();
    app.tick_folder_prefetch(due);
    assert!(!control.paused.load(Ordering::Acquire));
    finish(app, &a, due);
    assert_eq!(cache_files(&tree.temp), 5);
}

#[test]
fn grid_cache_read_holds_start() {
    let mut tree = Tree::new();
    let app = &mut tree.app;
    let a = tree.root.join("a");
    let start = Instant::now();
    app.thumb_jobs.cache_reads.waiting.push_back(ThumbJob {
        index: 0,
        rect: Rect::new(0, 0, 10, 5),
    });
    app.tick_folder_prefetch(start);
    let due = start + FOLDER_PREFETCH_DELAY;
    app.tick_folder_prefetch(due);
    assert_eq!(
        app.folder_prefetch_running(),
        None,
        "the grid reads the cache"
    );

    app.clear_thumb_queue();
    app.tick_folder_prefetch(due);
    finish(app, &a, due);
    assert_eq!(cache_files(&tree.temp), 5);
}

#[test]
fn prefetched_folder_runs_once() {
    let mut tree = Tree::new();
    let app = &mut tree.app;
    let a = tree.root.join("a");
    let start = Instant::now();
    app.tick_folder_prefetch(start);
    app.tick_folder_prefetch(start + FOLDER_PREFETCH_DELAY);
    finish(app, &a, start + FOLDER_PREFETCH_DELAY);

    app.gallery.cursor = 2;
    app.tick_folder_prefetch(start);
    app.gallery.cursor = 1;
    let again = start + Duration::from_secs(1);
    app.tick_folder_prefetch(again);
    assert_eq!(app.folder_prefetch_timeout(again), None);
    app.tick_folder_prefetch(again + FOLDER_PREFETCH_DELAY);
    assert_eq!(app.folder_prefetch_running(), None);
}

#[test]
fn parent_entry_waits_for_cursor_move() {
    let mut tree = Tree::new();
    let app = &mut tree.app;
    let parent = tree.root.parent().unwrap().to_path_buf();
    // The switch put the cursor on `..`. A rest there starts nothing.
    app.gallery.cursor = 0;
    let start = Instant::now();
    app.tick_folder_prefetch(start);
    app.tick_folder_prefetch(start + FOLDER_PREFETCH_DELAY);
    assert_eq!(app.folder_prefetch_running(), None);
    assert_eq!(app.folder_prefetch_timeout(start), None);

    // A move to `a` and back to `..` asks for the parent.
    app.gallery.cursor = 1;
    app.tick_folder_prefetch(start + FOLDER_PREFETCH_DELAY);
    app.gallery.cursor = 0;
    let back = start + Duration::from_secs(1);
    app.tick_folder_prefetch(back);
    app.tick_folder_prefetch(back + FOLDER_PREFETCH_DELAY);
    assert!(
        app.folder_prefetch_running() == Some(parent.as_path()) || app.folder_prefetched(&parent)
    );

    app.switch_to_dir(&tree.root.join("a"));
    wait_for_scan(app);
    assert_eq!(app.gallery.cursor, 0);
    let later = start + Duration::from_secs(2);
    app.tick_folder_prefetch(later);
    app.tick_folder_prefetch(later + FOLDER_PREFETCH_DELAY);
    assert_eq!(app.folder_prefetch_running(), None);
    assert_eq!(app.folder_prefetch_timeout(later), None);
}

#[test]
fn parent_step_covers_screen_around_folder() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap().join("root");
    pngs(&root, 30, (40, 30));
    // A folder prefix longer than one screen, with the folder before at its end.
    for index in 0..40 {
        fs::create_dir(root.join(format!("f{index:02}"))).unwrap();
    }
    let from = root.join("f39");
    let mut app = app();
    app.disk_cache_dir = temp.path().join("cache");
    app.state_file = temp.path().join("state.toml");
    app.gallery.update_grid(Rect::new(0, 0, 120, 40));
    app.switch_to_dir(&from);
    wait_for_scan(&mut app);
    assert_eq!(app.filenames, [".."]);
    let screen = first_screen(&app.folder_request(&root));

    assert!(app.enter_selected_folder());
    wait_for_scan(&mut app);
    let selected = app.gallery.selected_index().map(|index| &app.images[index]);
    assert_eq!(selected, Some(&from));
    assert!(
        app.gallery.scroll_offset > 0,
        "the screen around the folder"
    );
    let (files, target) = shown_files(&app);
    assert!(!files.is_empty());
    assert_eq!(screen.target, target);
    for file in &files {
        assert!(screen.files.contains(file), "{} is missing", file.display());
    }
}

#[test]
fn cancelled_thread_decodes_nothing() {
    let tree = Tree::new();
    let request = tree.app.folder_request(&tree.root.join("a"));
    let control = Control::default();
    control.cancel.store(true, Ordering::Release);
    prefetch(&request, &control);
    assert_eq!(cache_files(&tree.temp), 0);
    prefetch(&request, &Control::default());
    assert_eq!(cache_files(&tree.temp), 5);
}
