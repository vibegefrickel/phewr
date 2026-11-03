//! The thumbnail jobs of the grid: a disk cache read, a preview from the embedded thumbnail
//! (HEIC, camera RAW, PSD) and the sharp decode of each cell.
//!
//! At most `MAX_THUMB_JOBS` sharp decodes run in parallel, and the other cells wait in grid
//! order. A cache read runs on its own threads and never waits behind a cold decode.

mod decode;
mod padding;
mod queue;
#[cfg(test)]
#[cfg(feature = "heic")]
mod tests;

pub(super) use self::decode::warm_disk_cache;
use self::decode::{
    animation_flag, decode_into_cache, has_embedded_preview, is_playable, preview_or_cached,
    sharp_thumbnail,
};
pub(crate) use self::padding::{pad_for_grid, pad_loop_frame, thumb_target_px};
pub(super) use self::queue::{JobSlots, ThumbJob};
use super::App;
use super::disk_cache::{FileStamp, LazyEntry};
use crate::gallery::{GridPicture, ParkKey};
use ratatui::layout::Rect;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, mpsc};

/// With 4 jobs, each with its share of the cores, the first thumbnail comes after one decode.
const MAX_THUMB_JOBS: usize = 4;

/// The sharp decodes of the preview formats wait until every preview is out.
const MAX_PREVIEW_JOBS: usize = 4;

/// A warm grid reads all its cells together, not 4 at a time behind the decode slots.
const MAX_CACHE_READS: usize = 64;

/// A QOI read takes one core. More threads than cores give nothing.
const MAX_CACHE_READ_THREADS: usize = 8;

/// One flag holds a path and a bool: the map stays below about 10 MB.
const MAX_ANIMATION_FLAGS: usize = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThumbPhase {
    CacheRead,
    Preview,
    Sharp,
}

/// A preview job sends a sharp picture on a cache hit.
pub(super) enum ThumbImage {
    Sharp(GridPicture),
    Preview(GridPicture),
    /// The decode failed or the cache read missed. The slot still comes free.
    Miss,
}

impl ThumbImage {
    fn of(picture: Option<GridPicture>, sharp: bool) -> Self {
        match picture {
            Some(picture) if sharp => Self::Sharp(picture),
            Some(picture) => Self::Preview(picture),
            None => Self::Miss,
        }
    }
}

/// The file stamp is the one that the header check read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AnimationFlag {
    animated: bool,
    stamp: Option<FileStamp>,
}

pub(super) struct ThumbResult {
    generation: u32,
    index: usize,
    phase: ThumbPhase,
    image: ThumbImage,
    animated: Option<AnimationFlag>,
}

pub(super) struct ThumbnailJobs {
    tx: mpsc::Sender<ThumbResult>,
    rx: mpsc::Receiver<ThumbResult>,
    pub(super) sharp: JobSlots,
    previews: JobSlots,
    pub(super) cache_reads: JobSlots,
    /// The cells whose cache read missed in this generation. They read again after their decode.
    cache_misses: HashSet<usize>,
    /// The file stamp of each flag in `App::animated`. An unchanged file skips the header read.
    flag_stamps: HashMap<PathBuf, FileStamp>,
    generation: u32,
}

impl ThumbnailJobs {
    pub(super) fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            sharp: JobSlots::new(MAX_THUMB_JOBS),
            previews: JobSlots::new(MAX_PREVIEW_JOBS),
            cache_reads: JobSlots::new(MAX_CACHE_READS),
            cache_misses: HashSet::new(),
            flag_stamps: HashMap::new(),
            generation: 0,
        }
    }
}

impl App {
    pub(super) fn forget_thumbnails(&mut self) {
        self.thumb_cache.clear();
        self.thumb_jobs.sharp.clear();
        self.thumb_jobs.previews.clear();
        self.thumb_jobs.cache_reads.clear();
        self.thumb_jobs.cache_misses.clear();
        self.thumb_jobs.generation += 1;
        self.forget_grid_loops();
    }

    /// Drop the waiting jobs. The next grid render queues the visible cells again.
    pub(crate) fn clear_thumb_queue(&mut self) {
        self.thumb_jobs.sharp.waiting.clear();
        self.thumb_jobs.previews.waiting.clear();
        self.thumb_jobs.cache_reads.waiting.clear();
    }

    pub(crate) fn thumbnails_pending(&self) -> bool {
        !self.thumb_jobs.sharp.is_idle() || self.previews_pending() || self.cache_reads_pending()
    }

    /// True while the grid decodes and shows no picture yet. No event wakes the run loop then.
    pub(crate) fn waits_for_the_first_picture(&self) -> bool {
        self.mode == super::ViewMode::Gallery
            && self.thumbnails_pending()
            && !self
                .gallery
                .visible_items()
                .any(|(_, index)| self.thumb_cache.contains(index))
    }

    /// Take the finished thumbnails and start the waiting jobs. Returns the cells with a new image.
    pub(crate) fn poll_thumbnails(&mut self) -> Vec<usize> {
        let mut new_indices = Vec::new();
        while let Ok(result) = self.thumb_jobs.rx.try_recv() {
            // The forget cleared the slots of an old generation: the slot now belongs to a new job.
            if result.generation != self.thumb_jobs.generation {
                continue;
            }
            self.slots_of(result.phase).running.remove(&result.index);
            self.store_animation_flag(result.index, result.animated);
            let stored = match result.image {
                ThumbImage::Sharp(picture) => self.store_thumbnail(result.index, picture, true),
                ThumbImage::Preview(picture) => self.store_thumbnail(result.index, picture, false),
                ThumbImage::Miss => {
                    if result.phase == ThumbPhase::CacheRead {
                        self.thumb_jobs.cache_misses.insert(result.index);
                    }
                    false
                }
            };
            if stored {
                new_indices.push(result.index);
            }
        }
        self.start_waiting_jobs();
        new_indices
    }

    /// A preview does not wait for the cache read of its cell. It reads the cache itself.
    fn start_waiting_jobs(&mut self) {
        while self.thumb_jobs.cache_reads.has_free_slot()
            && let Some(job) = self.thumb_jobs.cache_reads.waiting.pop_front()
        {
            self.start_thumb_job(ThumbPhase::CacheRead, job);
        }
        while self.thumb_jobs.previews.has_free_slot()
            && let Some(job) = self.thumb_jobs.previews.waiting.pop_front()
        {
            self.start_thumb_job(ThumbPhase::Preview, job);
        }
        if !self.thumb_jobs.sharp.has_free_slot() {
            return;
        }
        let reading = self.thumb_jobs.cache_reads.indices();
        while self.thumb_jobs.sharp.has_free_slot()
            && let Some(position) = self.next_sharp_job(&reading)
            && let Some(job) = self.thumb_jobs.sharp.waiting.remove(position)
        {
            self.start_thumb_job(ThumbPhase::Sharp, job);
        }
    }

    /// A cell that shows nothing goes before a cell with a preview.
    fn next_sharp_job(&self, reading: &HashSet<usize>) -> Option<usize> {
        let ready = |job: &ThumbJob| !reading.contains(&job.index) && !self.waiting_job_holds(job);
        let waiting = &self.thumb_jobs.sharp.waiting;
        waiting
            .iter()
            .position(|job| ready(job) && !self.thumb_cache.contains(job.index))
            .or_else(|| waiting.iter().position(ready))
    }

    fn slots_of(&mut self, phase: ThumbPhase) -> &mut JobSlots {
        match phase {
            ThumbPhase::CacheRead => &mut self.thumb_jobs.cache_reads,
            ThumbPhase::Preview => &mut self.thumb_jobs.previews,
            ThumbPhase::Sharp => &mut self.thumb_jobs.sharp,
        }
    }

    /// True while a waiting sharp job must stay queued for the previews.
    fn waiting_job_holds(&self, job: &ThumbJob) -> bool {
        self.images
            .get(job.index)
            .is_some_and(|path| self.sharp_job_waits(path))
    }

    fn sharp_job_waits(&self, path: &Path) -> bool {
        has_embedded_preview(path) && self.previews_pending()
    }

    /// A sharp image replaces a preview and drops the waiting jobs of the cell. A preview never
    /// replaces an image. `false` when the cache keeps what it has.
    pub(crate) fn store_thumbnail(
        &mut self,
        index: usize,
        picture: impl Into<GridPicture>,
        sharp: bool,
    ) -> bool {
        if sharp {
            if self.thumb_cache.is_sharp(index) {
                return false;
            }
            self.thumb_jobs.sharp.waiting.remove_index(index);
            self.thumb_jobs.previews.waiting.remove_index(index);
            self.thumb_jobs.cache_reads.waiting.remove_index(index);
            self.thumb_jobs.cache_misses.remove(&index);
            if let Some(old_id) = self.thumb_cache.insert(index, picture) {
                self.stale_image_ids.push((index, old_id));
            }
            return true;
        }
        if self.thumb_cache.contains(index) {
            return false;
        }
        self.thumb_cache.insert_preview(index, picture);
        true
    }

    fn park_key(&self) -> ParkKey {
        ParkKey {
            target: thumb_target_px(self.gallery.thumb_box(), self.cell_px),
            cell_px: self.cell_px,
        }
    }

    /// Keep the sharp thumbnails for the way back. Call it before the switch drops the list.
    pub(crate) fn park_thumbnails(&mut self) {
        let key = self.park_key();
        self.thumb_cache.park(&self.images, key);
    }

    /// The flags outlive a folder switch: a parked thumbnail comes back without a job. Above
    /// `MAX_ANIMATION_FLAGS` entries, the map starts again.
    fn store_animation_flag(&mut self, index: usize, flag: Option<AnimationFlag>) {
        if let Some(flag) = flag
            && let Some(path) = self.images.get(index)
        {
            if self.animated.len() >= MAX_ANIMATION_FLAGS && !self.animated.contains_key(path) {
                self.animated.clear();
                self.thumb_jobs.flag_stamps.clear();
            }
            self.animated.insert(path.clone(), flag.animated);
            match flag.stamp {
                Some(stamp) => self.thumb_jobs.flag_stamps.insert(path.clone(), stamp),
                None => self.thumb_jobs.flag_stamps.remove(path),
            };
        }
    }

    fn known_animation_flag(&self, path: &Path) -> Option<AnimationFlag> {
        Some(AnimationFlag {
            animated: *self.animated.get(path)?,
            stamp: Some(*self.thumb_jobs.flag_stamps.get(path)?),
        })
    }

    pub(crate) fn cache_reads_pending(&self) -> bool {
        !self.thumb_jobs.cache_reads.is_idle()
    }

    pub(super) fn previews_pending(&self) -> bool {
        !self.thumb_jobs.previews.is_idle()
    }

    /// Decode the thumbnail of `index` for the cell `rect`, or queue it. A HEIC, RAW or PSD cell
    /// without an image gets a preview job first.
    pub(crate) fn spawn_thumb_decode(&mut self, index: usize, rect: Rect) {
        if self.is_folder(index)
            || self.thumb_cache.is_sharp(index)
            || self.thumb_jobs.sharp.holds(index)
        {
            return;
        }
        let key = ParkKey {
            target: thumb_target_px(rect, self.cell_px),
            cell_px: self.cell_px,
        };
        let Some(path) = self.images.get(index) else {
            return;
        };
        if let Some(parked) = self.thumb_cache.take_parked(path, key) {
            self.store_thumbnail(index, parked, true);
            return;
        }
        let job = ThumbJob { index, rect };
        if self.wants_preview(index) {
            if self.thumb_jobs.previews.has_free_slot() {
                self.start_thumb_job(ThumbPhase::Preview, job);
            } else {
                self.thumb_jobs.previews.waiting.push_back(job);
            }
        }
        if self.waiting_job_holds(&job) || !self.thumb_jobs.sharp.has_free_slot() {
            self.thumb_jobs.sharp.waiting.push_back(job);
            self.read_cache_ahead(job);
            return;
        }
        self.start_thumb_job(ThumbPhase::Sharp, job);
    }

    /// A running preview reads the cache itself, and a cell that missed in this generation reads
    /// nothing. A waiting read always has a waiting sharp job, and only running reads need a
    /// lookup.
    fn read_cache_ahead(&mut self, job: ThumbJob) {
        if self.thumb_jobs.previews.running.contains(&job.index)
            || self.thumb_jobs.cache_reads.running.contains(&job.index)
            || self.thumb_jobs.cache_misses.contains(&job.index)
        {
            return;
        }
        if self.thumb_jobs.cache_reads.has_free_slot() {
            self.start_thumb_job(ThumbPhase::CacheRead, job);
        } else {
            self.thumb_jobs.cache_reads.waiting.push_back(job);
        }
    }

    fn wants_preview(&self, index: usize) -> bool {
        self.images
            .get(index)
            .is_some_and(|path| has_embedded_preview(path))
            && !self.thumb_cache.contains(index)
            && !self.thumb_jobs.previews.holds(index)
    }

    /// The job pads the picture off the UI thread. A job whose file left the list starts nothing.
    fn start_thumb_job(&mut self, phase: ThumbPhase, job: ThumbJob) {
        let Some(path) = self.images.get(job.index).cloned() else {
            return;
        };
        self.slots_of(phase).running.insert(job.index);
        let tx = self.thumb_jobs.tx.clone();
        let target = thumb_target_px(job.rect, self.cell_px);
        let cell_px = self.cell_px;
        let generation = self.thumb_jobs.generation;
        let cache_dir = self.disk_cache_dir.clone();
        let missed = self.thumb_jobs.cache_misses.contains(&job.index);
        let known = self.known_animation_flag(&path);
        spawn_job(phase, move || {
            let entry = LazyEntry::new(&cache_dir, &path, target);
            let (image, sharp) = match phase {
                ThumbPhase::CacheRead => (entry.load(), true),
                ThumbPhase::Preview => preview_or_cached(&path, target, &entry, !missed),
                ThumbPhase::Sharp if missed => {
                    (decode_into_cache(&cache_dir, &path, target, &entry), true)
                }
                ThumbPhase::Sharp => (sharp_thumbnail(&cache_dir, &path, target, &entry), true),
            };
            // A missed cache read leaves the header read to the decode job.
            let animated = if phase == ThumbPhase::CacheRead && image.is_none() {
                None
            } else {
                animation_flag(&path, &entry, known)
            };
            // The still carries the play triangle until its loop starts. The disk cache keeps
            // the still without the stamp.
            let playable = is_playable(&path, animated.map(|flag| flag.animated));
            let picture = image.map(|mut image| {
                if playable {
                    crate::icons::stamp_play(&mut image);
                }
                pad_for_grid(image, target, cell_px, sharp)
            });
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a closed receiver means that nobody waits for the result"
            )]
            let _ = tx.send(ThumbResult {
                generation,
                index: job.index,
                phase,
                image: ThumbImage::of(picture, sharp),
                animated,
            });
        });
    }
}

/// A cache hit never waits for a cold HEIC or sips decode to free a thread.
fn spawn_job(phase: ThumbPhase, job: impl FnOnce() + Send + 'static) {
    match cache_read_pool() {
        Some(pool) if phase == ThumbPhase::CacheRead => pool.spawn(job),
        _ => rayon::spawn(job),
    }
}

/// `None` when the build failed: the reads then share the global pool.
fn cache_read_pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(rayon::current_num_threads().min(MAX_CACHE_READ_THREADS))
            .thread_name(|index| format!("phewr-cache-read-{index}"))
            .build()
            .ok()
    })
    .as_ref()
}

#[cfg(test)]
mod folder_switch_tests {
    use crate::test_support::{app_with_files, test_dir, wait_until};
    use image::RgbaImage;
    use ratatui::layout::Rect;
    use std::fs;

    fn decode_sharp(app: &mut crate::app::App, index: usize, rect: Rect) {
        app.spawn_thumb_decode(index, rect);
        wait_until(|| {
            app.poll_thumbnails();
            app.thumb_cache.is_sharp(index)
        });
    }

    /// A parked thumbnail comes back without a job. Its animation flag must survive the switch.
    #[test]
    #[cfg(feature = "video")]
    fn parked_animation_keeps_marker() {
        let dir = test_dir();
        let root = fs::canonicalize(dir.path()).unwrap();
        let other = root.join("other");
        fs::create_dir(&other).unwrap();
        fs::copy(
            crate::test_support::fixture("anim.gif"),
            root.join("anim.gif"),
        )
        .unwrap();
        fs::copy(
            crate::test_support::fixture("still.gif"),
            other.join("still.gif"),
        )
        .unwrap();
        let mut app = crate::test_support::app();
        let cache = test_dir();
        app.disk_cache_dir = cache.path().join("cache");
        let scan = |app: &mut crate::app::App, dir: &std::path::Path| {
            app.switch_to_dir(dir);
            wait_until(|| {
                app.refresh_from_scanner();
                !app.is_scanning()
            });
        };
        let position =
            |app: &crate::app::App| app.filenames.iter().position(|n| n == "anim.gif").unwrap();
        scan(&mut app, &root);
        let rect = app.gallery.thumb_box();
        let anim = position(&app);
        decode_sharp(&mut app, anim, rect);
        assert!(app.is_animated(anim), "the job reads the flag");

        scan(&mut app, &other);
        scan(&mut app, &root);
        let anim = position(&app);
        app.spawn_thumb_decode(anim, rect);
        assert!(app.thumb_cache.is_sharp(anim), "the parked picture, no job");
        assert!(!app.thumbnails_pending());
        assert!(
            app.is_animated(anim),
            "the marker comes back with the picture"
        );
    }

    #[test]
    fn last_folder_returns_without_decode() {
        let dir = test_dir();
        let root = fs::canonicalize(dir.path()).unwrap();
        let other = root.join("other");
        fs::create_dir(&other).unwrap();
        RgbaImage::new(4, 4).save(other.join("z.png")).unwrap();
        let mut app = app_with_files(&root, &["a.png", "b.png"]);
        // The disk cache lives outside the scanned tree, else it is a folder of the grid.
        let cache = test_dir();
        app.disk_cache_dir = cache.path().join("cache");
        let scan = |app: &mut crate::app::App, dir: &std::path::Path| {
            app.switch_to_dir(dir);
            wait_until(|| {
                app.refresh_from_scanner();
                !app.is_scanning()
            });
        };
        scan(&mut app, &root);
        let position = |app: &crate::app::App, name: &str| {
            app.filenames.iter().position(|n| n == name).unwrap()
        };
        let (a, b) = (position(&app, "a.png"), position(&app, "b.png"));
        let rect = app.gallery.thumb_box();
        decode_sharp(&mut app, a, rect);
        decode_sharp(&mut app, b, rect);
        let thumbnail = app.thumb_cache.peek(a).unwrap().0.clone();

        scan(&mut app, &other);
        assert_eq!(app.thumb_cache.parked_count(), 2);
        assert!(!app.thumb_cache.contains(a), "the grid cache is empty");
        assert!(
            !app.thumbnails_pending(),
            "no job of the old folder runs or waits"
        );
        // A preview is not worth a park.
        let z = position(&app, "z.png");
        assert!(app.store_thumbnail(z, RgbaImage::new(1, 1), false));

        scan(&mut app, &root);
        assert_eq!(
            app.thumb_cache.parked_count(),
            2,
            "the preview was not parked"
        );
        app.spawn_thumb_decode(a, rect);
        assert!(app.thumb_cache.is_sharp(a), "the parked picture, no job");
        assert_eq!(*app.thumb_cache.peek(a).unwrap().0, thumbnail);
        assert!(!app.thumbnails_pending());
        assert_eq!(app.thumb_cache.parked_count(), 1);

        let larger = Rect::new(0, 0, rect.width + 1, rect.height);
        app.spawn_thumb_decode(b, larger);
        assert!(!app.thumb_cache.contains(b));
        assert!(app.thumbnails_pending());
        assert_eq!(app.thumb_cache.parked_count(), 1);
    }

    #[test]
    fn switch_drops_waiting_and_running_jobs() {
        let dir = test_dir();
        let root = fs::canonicalize(dir.path()).unwrap();
        let names = ["a.png", "b.png", "c.png", "d.png", "e.png", "f.png"];
        let mut app = app_with_files(&root, &names);
        app.disk_cache_dir = root.join("cache");
        let rect = Rect::new(0, 0, 3, 2);
        for index in 0..names.len() {
            app.spawn_thumb_decode(index, rect);
        }
        assert_eq!(app.thumb_jobs.sharp.running.len(), 4);
        assert_eq!(app.thumb_jobs.sharp.waiting.len(), 2);
        let generation = app.thumb_jobs.generation;
        let started = app.thumb_jobs.sharp.running.len()
            + app.thumb_jobs.previews.running.len()
            + app.thumb_jobs.cache_reads.running.len();

        app.switch_to_dir(&root.join("missing"));
        assert!(app.thumb_jobs.sharp.running.is_empty() && app.thumb_jobs.sharp.waiting.is_empty());
        assert_ne!(app.thumb_jobs.generation, generation);
        assert_eq!(app.thumb_cache.parked_count(), 0, "nothing was sharp yet");
        let mut stale = Vec::new();
        wait_until(|| {
            stale.extend(app.thumb_jobs.rx.try_iter());
            stale.len() == started
        });
        for result in stale {
            assert_eq!(result.generation, generation);
            app.thumb_jobs.tx.send(result).unwrap();
        }
        assert!(
            app.poll_thumbnails().is_empty(),
            "a stale thumbnail was stored"
        );
        assert!(!app.thumb_cache.contains(0));
    }
}

#[cfg(test)]
mod slot_tests {
    use super::{
        AnimationFlag, MAX_ANIMATION_FLAGS, MAX_CACHE_READS, MAX_THUMB_JOBS, ThumbImage, ThumbJob,
        ThumbPhase, ThumbResult,
    };
    use crate::test_support::{app_with, app_with_files, test_dir, wait_until};
    use ratatui::layout::Rect;
    use std::path::PathBuf;

    /// A late result of the old generation must not free the slot of the new job.
    #[test]
    fn old_result_keeps_new_job_slot() {
        let mut app = app_with(&["a.png"]);
        app.thumb_jobs.sharp.running.insert(0);
        let old_generation = app.thumb_jobs.generation;
        app.forget_thumbnails();
        app.thumb_jobs.sharp.running.insert(0);
        app.thumb_jobs
            .tx
            .send(ThumbResult {
                generation: old_generation,
                index: 0,
                phase: ThumbPhase::Sharp,
                image: ThumbImage::Miss,
                animated: None,
            })
            .unwrap();
        assert!(app.poll_thumbnails().is_empty());
        assert!(
            app.thumb_jobs.sharp.running.contains(&0),
            "the new job runs"
        );
    }

    #[test]
    fn index_past_list_starts_nothing() {
        let mut app = app_with(&["a.heic"]);
        app.spawn_thumb_decode(5, Rect::new(0, 0, 1, 1));
        assert!(!app.wants_preview(5));
        assert!(app.thumb_jobs.sharp.is_idle() && app.thumb_jobs.previews.is_idle());
    }

    #[test]
    fn full_read_slots_queue_reads() {
        let dir = test_dir();
        let mut app = app_with_files(dir.path(), &["a.png"]);
        let rect = Rect::new(0, 0, 3, 2);
        app.thumb_jobs
            .sharp
            .running
            .extend(100..100 + MAX_THUMB_JOBS);
        let reads = 200..200 + MAX_CACHE_READS;
        app.thumb_jobs.cache_reads.running.extend(reads);
        app.spawn_thumb_decode(0, rect);
        assert_eq!(app.thumb_jobs.sharp.waiting, [ThumbJob { index: 0, rect }]);
        assert_eq!(
            app.thumb_jobs.cache_reads.waiting,
            [ThumbJob { index: 0, rect }]
        );

        app.thumb_jobs.sharp.running.clear();
        app.thumb_jobs.cache_reads.running.clear();
        app.poll_thumbnails();
        assert!(app.thumb_jobs.cache_reads.running.contains(&0));
        assert!(app.thumb_jobs.cache_reads.waiting.is_empty());
        assert_eq!(app.thumb_jobs.sharp.waiting, [ThumbJob { index: 0, rect }]);
        wait_until(|| {
            app.poll_thumbnails();
            !app.thumbnails_pending()
        });
        assert!(app.thumb_cache.is_sharp(0));
    }

    #[test]
    fn job_of_removed_file_starts_nothing() {
        let mut app = app_with(&["a.png"]);
        let rect = Rect::new(0, 0, 1, 1);
        app.thumb_jobs
            .sharp
            .waiting
            .push_back(ThumbJob { index: 5, rect });
        assert!(app.poll_thumbnails().is_empty());
        assert!(app.thumb_jobs.sharp.waiting.is_empty());
        assert!(app.thumb_jobs.sharp.running.is_empty());
    }

    #[test]
    fn full_flag_map_starts_again() {
        let mut app = app_with(&["a.gif"]);
        for index in 0..MAX_ANIMATION_FLAGS {
            app.animated
                .insert(PathBuf::from(format!("/other/{index}.gif")), false);
        }
        let flag = AnimationFlag {
            animated: true,
            stamp: None,
        };
        app.store_animation_flag(0, Some(flag));
        assert_eq!(app.animated.len(), 1);
        assert_eq!(app.animated.get(&app.images[0]), Some(&true));
        assert!(app.thumb_jobs.flag_stamps.is_empty());
    }
}
