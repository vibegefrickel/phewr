//! The folder prefetch: while the cursor rests on a folder cell, one thread decodes the first
//! screen of that folder into the disk cache, so that Enter shows it sharp at once.
//!
//! The thread computes what a switch to the folder computes: the files in the order of the
//! scan, the cell shape from the headers of the scan sample, the zoom level of the folder, the
//! grid on this terminal and the cells of its first screen. It pauses while the visible grid
//! has work, and a cursor move, a folder switch, a zoom, a resize or the quit cancel it.

use super::cell_shape::SHAPE_SAMPLE;
use super::thumbnails::{thumb_target_px, warm_disk_cache};
use super::{App, ViewMode};
use crate::decode::shape;
use crate::gallery::GalleryState;
use crate::scanner;
use ratatui::layout::Rect;
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// The time that the cursor rests on a folder cell before the prefetch starts. A cursor that
/// walks across the folders starts nothing.
pub(crate) const FOLDER_PREFETCH_DELAY: Duration = Duration::from_millis(300);

/// The shortest wait of the run loop for the start, so that a start that waits for a stopping
/// thread does not spin the loop.
const START_POLL: Duration = Duration::from_millis(10);

/// The wait of a paused prefetch between two looks at its flags. An unpark ends it early.
const PAUSE_POLL: Duration = Duration::from_millis(50);

/// The folder under the cursor and the grid around it. A change of any part cancels the
/// prefetch, because the first screen of the folder can change with it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RestKey {
    folder: PathBuf,
    current_dir: Option<PathBuf>,
    grid_rect: Rect,
    zoom_rows: usize,
    show_filenames: bool,
    cell_px: (u32, u32),
}

/// The flags that the UI thread sets and the prefetch thread reads before each file.
#[derive(Default)]
struct Control {
    cancel: AtomicBool,
    paused: AtomicBool,
}

impl Control {
    /// Wait while the prefetch is paused. `false` once it is cancelled.
    fn proceed(&self) -> bool {
        while self.paused.load(Ordering::Acquire) && !self.cancelled() {
            thread::park_timeout(PAUSE_POLL);
        }
        !self.cancelled()
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
}

/// The prefetch thread of one folder.
struct Running {
    key: RestKey,
    control: Arc<Control>,
    handle: JoinHandle<()>,
}

impl Running {
    /// Stop the thread before its next file. It can still finish the file that it decodes.
    fn cancel(&self) {
        self.control.cancel.store(true, Ordering::Release);
        self.handle.thread().unpark();
    }

    fn set_paused(&self, paused: bool) {
        let was_paused = self.control.paused.swap(paused, Ordering::AcqRel);
        if was_paused && !paused {
            self.handle.thread().unpark();
        }
    }
}

/// The state of the folder prefetch: the rest of the cursor, the one thread and the folders
/// that this session prefetched.
#[derive(Default)]
pub(crate) struct FolderPrefetch {
    rest: Option<(RestKey, Instant)>,
    running: Option<Running>,
    /// The cancelled threads that still decode their last file. The next start waits for all of
    /// them, so that at most one prefetch decodes at a time.
    stopping: Vec<JoinHandle<()>>,
    done: HashSet<PathBuf>,
    /// True after the cursor left the `..` entry in this folder. The start and every folder
    /// switch put the cursor on `..`, so only a real move there asks for the parent.
    parent_armed: bool,
}

impl FolderPrefetch {
    /// A folder switch puts the cursor on `..` again.
    pub(super) fn disarm_parent(&mut self) {
        self.parent_armed = false;
    }

    fn cancel(&mut self) {
        if let Some(running) = self.running.take() {
            running.cancel();
            self.stopping.push(running.handle);
        }
    }

    /// True while a cancelled thread still runs.
    fn is_stopping(&mut self) -> bool {
        self.stopping.retain(|handle| !handle.is_finished());
        !self.stopping.is_empty()
    }

    /// The rest that waits for its start: no thread runs, and the folder is not done.
    fn waiting_rest(&self) -> Option<&(RestKey, Instant)> {
        self.rest
            .as_ref()
            .filter(|(key, _)| self.running.is_none() && !self.done.contains(&key.folder))
    }
}

impl Drop for FolderPrefetch {
    /// The quit stops the thread before its next file. It does not join the thread, so the quit
    /// never waits for a decode. The prune of the disk cache removes a temp file that a write
    /// left behind.
    fn drop(&mut self) {
        if let Some(running) = &self.running {
            running.cancel();
        }
    }
}

/// What the prefetch thread needs: the folder, the folder of the grid now and the grid as the
/// switch starts from it.
struct Request {
    folder: PathBuf,
    /// The cursor of a step to the parent lands on this folder.
    from: PathBuf,
    grid: GalleryState,
    zoom_rows: usize,
    cell_px: (u32, u32),
    cache_dir: PathBuf,
}

/// The files of the first screen of a folder and the pixel box that the grid decodes them for.
#[derive(Debug, Default, PartialEq)]
struct FirstScreen {
    files: Vec<PathBuf>,
    target: (u32, u32),
}

/// The first screen that a switch to `request.folder` shows. A folder with fewer than 2 files
/// gets no prefetch. Its grid holds one picture at most, so the prefetch saves one decode at
/// most.
fn first_screen(request: &Request) -> FirstScreen {
    let (folders, files, names) = scanner::folder_listing(&request.folder);
    if files.len() < 2 {
        return FirstScreen::default();
    }
    // The scan samples the first files by name, and the files come sorted.
    let aspect = shape::folder_aspect(&files[..files.len().min(SHAPE_SAMPLE)]);
    let media: Vec<(PathBuf, String)> = files.into_iter().zip(names).collect();
    let folder_count = folders.len();
    let entries: Vec<PathBuf> = folders
        .into_iter()
        .map(|folder| folder.path)
        .chain(media.into_iter().map(|(path, _)| path))
        .collect();
    // As `enter_selected_folder`: a step to the parent puts the cursor on the folder before.
    let cursor = (request.from.parent() == Some(request.folder.as_path()))
        .then(|| entries.iter().position(|entry| *entry == request.from))
        .flatten()
        .unwrap_or(0);
    // The switch places the cursor at the end of the scan, and the shape comes when its header
    // read ends. Either can come first, and the scroll offset can differ, so both screens count.
    let base = request.grid.for_folder(entries.len(), request.zoom_rows);
    let mut cursor_first = base.clone();
    cursor_first.move_cursor_to(cursor);
    cursor_first.set_cell_shape(aspect, request.cell_px);
    let mut shape_first = base;
    shape_first.set_cell_shape(aspect, request.cell_px);
    shape_first.move_cursor_to(cursor);
    let positions: BTreeSet<usize> = cursor_first
        .visible_range()
        .chain(shape_first.visible_range())
        .filter(|&position| position >= folder_count)
        .collect();
    FirstScreen {
        files: positions
            .into_iter()
            .map(|position| entries[position].clone())
            .collect(),
        target: thumb_target_px(cursor_first.thumb_box(), request.cell_px),
    }
}

/// The body of the prefetch thread: one file after the other, never two at once.
fn prefetch(request: &Request, control: &Control) {
    if !control.proceed() {
        return;
    }
    let screen = first_screen(request);
    for path in &screen.files {
        if !control.proceed() {
            return;
        }
        warm_disk_cache(&request.cache_dir, path, screen.target);
    }
}

impl App {
    /// The run loop calls this after each draw. It tracks the rest of the cursor on a folder
    /// cell, starts the prefetch after `FOLDER_PREFETCH_DELAY`, pauses it while the visible grid
    /// has work and cancels it when the cursor, the folder or the grid changes.
    pub(crate) fn tick_folder_prefetch(&mut self, now: Instant) {
        if self
            .selected_entry()
            .is_some_and(|index| !self.is_parent_entry(index))
        {
            self.folder_prefetch.parent_armed = true;
        }
        let key = self.rest_key();
        let busy = self.has_pending_work();
        let prefetch = &mut self.folder_prefetch;
        if let Some(running) = prefetch
            .running
            .take_if(|running| running.handle.is_finished())
        {
            prefetch.done.insert(running.key.folder);
        }
        if prefetch
            .running
            .as_ref()
            .is_some_and(|running| Some(&running.key) != key.as_ref())
        {
            prefetch.cancel();
        }
        if let Some(running) = &prefetch.running {
            running.set_paused(busy);
            return;
        }
        let Some(key) = key else {
            prefetch.rest = None;
            return;
        };
        if prefetch.rest.as_ref().is_none_or(|(rest, _)| *rest != key) {
            prefetch.rest = Some((key, now));
        }
        let due = prefetch
            .waiting_rest()
            .is_some_and(|(_, since)| now.duration_since(*since) >= FOLDER_PREFETCH_DELAY);
        if due && !busy && !prefetch.is_stopping() {
            self.start_folder_prefetch();
        }
    }

    /// The wait until the rest of the cursor starts the prefetch. `None` without a waiting rest.
    pub(crate) fn folder_prefetch_timeout(&self, now: Instant) -> Option<Duration> {
        let (_, since) = self.folder_prefetch.waiting_rest()?;
        let due = *since + FOLDER_PREFETCH_DELAY;
        Some(due.saturating_duration_since(now).max(START_POLL))
    }

    /// The folder that the prefetch thread works on.
    #[cfg(test)]
    pub(crate) fn folder_prefetch_running(&self) -> Option<&Path> {
        self.folder_prefetch
            .running
            .as_ref()
            .map(|running| running.key.folder.as_path())
    }

    /// True when this session prefetched `folder` to the end.
    #[cfg(test)]
    pub(crate) fn folder_prefetched(&self, folder: &Path) -> bool {
        self.folder_prefetch.done.contains(folder)
    }

    /// The folder under the cursor and the grid around it, in the gallery view only. The `..`
    /// entry counts only after a real cursor move, see `FolderPrefetch::parent_armed`.
    fn rest_key(&self) -> Option<RestKey> {
        if self.mode != ViewMode::Gallery {
            return None;
        }
        let index = self
            .gallery
            .selected_index()
            .filter(|&index| self.is_folder(index))
            .filter(|&index| self.folder_prefetch.parent_armed || !self.is_parent_entry(index))?;
        Some(RestKey {
            folder: self.images[index].clone(),
            current_dir: self.current_dir.clone(),
            grid_rect: self.gallery.grid_rect,
            zoom_rows: self.gallery.zoom_rows,
            show_filenames: self.gallery.show_filenames,
            cell_px: self.cell_px,
        })
    }

    /// The entry under the cursor, while the list holds it.
    fn selected_entry(&self) -> Option<usize> {
        self.gallery
            .selected_index()
            .filter(|&index| index < self.images.len())
    }

    /// Start the thread for the folder of the rest. It starts paused while the grid has work.
    fn start_folder_prefetch(&mut self) {
        let Some((key, _)) = self.folder_prefetch.rest.clone() else {
            return;
        };
        let request = self.folder_request(&key.folder);
        let control = Arc::new(Control::default());
        control
            .paused
            .store(self.has_pending_work(), Ordering::Release);
        let worker = Arc::clone(&control);
        let handle = thread::spawn(move || prefetch(&request, &worker));
        self.folder_prefetch.running = Some(Running {
            key,
            control,
            handle,
        });
    }

    /// What a switch from this grid to `folder` starts from.
    fn folder_request(&self, folder: &Path) -> Request {
        Request {
            folder: folder.to_path_buf(),
            from: self.gallery_dir(),
            grid: self.gallery.clone(),
            zoom_rows: self.folder_zoom_rows(folder),
            cell_px: self.cell_px,
            cache_dir: self.disk_cache_dir.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Control, FOLDER_PREFETCH_DELAY, Running, START_POLL, first_screen, prefetch,
        thumb_target_px,
    };
    use crate::app::App;
    use crate::app::disk_cache::get_cache_file_path;
    use crate::app::thumbnails::ThumbJob;
    use crate::config;
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

    /// A root with the subfolders `a` (5 landscape files) and `b` (40 portrait files), and an
    /// app on the root with its own cache and state file, on a terminal of 120 by 40 cells.
    /// The entries of the root are `..`, `a` and `b`, and the cursor rests on `a`.
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

    /// The files in the disk cache of the tree.
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

    /// Tick until the prefetch of `folder` ends.
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
    fn the_prefetch_starts_after_the_cursor_rests_on_a_folder() {
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
    fn the_target_box_has_the_cell_shape_and_the_zoom_of_the_target_folder() {
        let mut tree = Tree::new();
        let app = &mut tree.app;
        let b = tree.root.join("b");
        // `b` holds portraits and has its own zoom level. The root has none of either.
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
    fn enter_after_the_prefetch_finds_every_cell_in_the_disk_cache() {
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
    fn a_cursor_move_cancels_the_prefetch_before_its_next_file() {
        let mut tree = Tree::new();
        let app = &mut tree.app;
        let start = Instant::now();
        app.tick_folder_prefetch(start);
        // A grid job holds the new thread before its first file.
        queue_grid_job(app);
        app.start_folder_prefetch();
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
    fn one_prefetch_runs_at_a_time() {
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
    fn two_cancels_inside_one_decode_keep_both_threads_in_the_wait() {
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
    fn grid_jobs_hold_the_start_and_pause_the_thread() {
        let mut tree = Tree::new();
        let app = &mut tree.app;
        let a = tree.root.join("a");
        let start = Instant::now();
        queue_grid_job(app);
        app.tick_folder_prefetch(start);
        let due = start + FOLDER_PREFETCH_DELAY;
        app.tick_folder_prefetch(due);
        assert_eq!(app.folder_prefetch_running(), None, "the grid has a job");

        // A thread that started is paused while the grid has a job.
        app.start_folder_prefetch();
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
    fn a_disk_cache_read_of_the_grid_holds_the_start_like_a_decode() {
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
    fn a_prefetched_folder_is_not_prefetched_again() {
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
    fn the_parent_entry_waits_for_a_real_cursor_move() {
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
            app.folder_prefetch_running() == Some(parent.as_path())
                || app.folder_prefetched(&parent)
        );

        // The next folder puts the cursor on `..` again.
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
    fn a_step_to_the_parent_covers_the_screen_around_the_folder_before() {
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
    fn a_cancelled_thread_decodes_nothing() {
        let tree = Tree::new();
        let request = tree.app.folder_request(&tree.root.join("a"));
        let control = Control::default();
        control.cancel.store(true, Ordering::Release);
        prefetch(&request, &control);
        assert_eq!(cache_files(&tree.temp), 0);
        prefetch(&request, &Control::default());
        assert_eq!(cache_files(&tree.temp), 5);
    }
}
