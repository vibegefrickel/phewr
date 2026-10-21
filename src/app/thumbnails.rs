//! The thumbnail jobs of the grid: the queue, the HEIC previews, the generations.

use super::App;
use super::disk_cache::{try_load_from_disk_cache, try_save_to_disk_cache};
use crate::decode::EmbeddedThumbnail;
#[cfg(feature = "heic")]
use crate::decode::libheif::{decode_heif_preview, decodes_with_libheif};
use crate::decode::resize::load_and_resize_px;
use image::RgbaImage;
use ratatui::layout::Rect;
use std::collections::{HashSet, VecDeque};
use std::io;
use std::path::Path;

/// The thumbnail decodes that run at once. 24 parallel decodes of 24 MP HEIC frames held
/// 2 GB of pixels and ran 40 libheif threads on 10 cores, so the first thumbnail took 1 s.
/// With 4 jobs, each with its share of the cores, the first one comes after one decode.
const MAX_THUMB_JOBS: usize = 4;

/// The previews of a cold HEIC grid that decode at once. A preview is the embedded 320x240
/// thumbnail, a few milliseconds each. The sharp decodes wait until every preview is out.
const MAX_PREVIEW_JOBS: usize = 4;

/// The job that produced a thumbnail result, so that `poll_thumbnails` frees the right slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThumbPhase {
    Preview,
    Sharp,
}

/// A thumbnail decode that waits for a free slot: the image index and its grid cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ThumbJob {
    pub(super) index: usize,
    pub(super) rect: Rect,
}

/// The slots of one decode phase: the jobs that run, at most `limit`, and the jobs that wait
/// in the order of the grid.
pub(super) struct JobSlots {
    pub(super) running: HashSet<usize>,
    pub(super) waiting: VecDeque<ThumbJob>,
    limit: usize,
}

impl JobSlots {
    fn new(limit: usize) -> Self {
        Self {
            running: HashSet::new(),
            waiting: VecDeque::new(),
            limit,
        }
    }

    fn has_free_slot(&self) -> bool {
        self.running.len() < self.limit
    }

    /// True while the image `index` runs or waits in this phase.
    fn holds(&self, index: usize) -> bool {
        self.running.contains(&index) || self.waiting.iter().any(|job| job.index == index)
    }

    fn is_idle(&self) -> bool {
        self.running.is_empty() && self.waiting.is_empty()
    }

    fn clear(&mut self) {
        self.running.clear();
        self.waiting.clear();
    }
}

/// The result of a thumbnail job. `image` is `None` when the decode failed, so the slot still
/// comes free. `sharp` is false for a preview from an embedded thumbnail. A preview job sends a
/// sharp image when the disk cache has it.
pub(super) struct ThumbResult {
    generation: u32,
    img_idx: usize,
    phase: ThumbPhase,
    image: Option<RgbaImage>,
    sharp: bool,
}

impl App {
    pub(super) fn thumb_slots() -> JobSlots {
        JobSlots::new(MAX_THUMB_JOBS)
    }

    pub(super) fn preview_slots() -> JobSlots {
        JobSlots::new(MAX_PREVIEW_JOBS)
    }

    /// Drop every thumbnail and every job, so that a new list or a new size starts clean.
    pub(super) fn forget_thumbnails(&mut self) {
        self.thumb_cache.clear();
        self.thumb_jobs.clear();
        self.preview_jobs.clear();
        self.thumb_generation += 1;
    }

    /// Drop the waiting thumbnail jobs. A render of the grid queues the visible cells again.
    pub(crate) fn clear_thumb_queue(&mut self) {
        self.thumb_jobs.waiting.clear();
        self.preview_jobs.waiting.clear();
    }

    /// True while a thumbnail job runs or waits in either phase.
    pub(super) fn thumbnails_pending(&self) -> bool {
        !self.thumb_jobs.is_idle() || self.previews_pending()
    }

    /// Take the finished thumbnails and start the waiting jobs. Returns the cells with a new
    /// image, a preview or a sharp one. The sharp jobs wait until every preview is done, so
    /// that a cold HEIC grid shows all its previews first.
    pub(crate) fn poll_thumbnails(&mut self) -> Vec<usize> {
        let mut new_indices = Vec::new();
        while let Ok(result) = self.thumb_rx.try_recv() {
            self.slots_of(result.phase).running.remove(&result.img_idx);
            if result.generation == self.thumb_generation
                && let Some(image) = result.image
                && self.store_thumbnail(result.img_idx, image, result.sharp)
            {
                new_indices.push(result.img_idx);
            }
        }
        while self.preview_jobs.has_free_slot()
            && let Some(job) = self.preview_jobs.waiting.pop_front()
        {
            self.start_thumb_job(ThumbPhase::Preview, job);
        }
        while self.thumb_jobs.has_free_slot()
            && let Some(position) = self
                .thumb_jobs
                .waiting
                .iter()
                .position(|job| !self.waiting_job_holds(job))
            && let Some(job) = self.thumb_jobs.waiting.remove(position)
        {
            self.start_thumb_job(ThumbPhase::Sharp, job);
        }
        new_indices
    }

    fn slots_of(&mut self, phase: ThumbPhase) -> &mut JobSlots {
        match phase {
            ThumbPhase::Preview => &mut self.preview_jobs,
            ThumbPhase::Sharp => &mut self.thumb_jobs,
        }
    }

    /// True while a waiting sharp job must stay in the queue. A job whose index left the list
    /// comes out at once, and `start_thumb_job` drops it.
    fn waiting_job_holds(&self, job: &ThumbJob) -> bool {
        self.images
            .get(job.index)
            .is_some_and(|path| self.sharp_job_waits(path))
    }

    /// True while the sharp decode of a libheif file must wait for the previews. The other
    /// formats have no preview phase and start at once.
    fn sharp_job_waits(&self, path: &Path) -> bool {
        has_embedded_preview(path) && self.previews_pending()
    }

    /// Put a decoded thumbnail into the cache. A sharp image replaces a preview and drops the
    /// waiting sharp job of the cell. A preview never replaces an image. `false` when the cache
    /// keeps what it has.
    pub(crate) fn store_thumbnail(
        &mut self,
        img_idx: usize,
        image: RgbaImage,
        sharp: bool,
    ) -> bool {
        if sharp {
            if self.thumb_cache.is_sharp(img_idx) {
                return false;
            }
            self.thumb_jobs.waiting.retain(|job| job.index != img_idx);
            if let Some(old_id) = self.thumb_cache.insert(img_idx, image) {
                self.stale_image_ids.push((img_idx, old_id));
            }
            return true;
        }
        if self.thumb_cache.contains(img_idx) {
            return false;
        }
        self.thumb_cache.insert_preview(img_idx, image);
        true
    }

    pub(super) fn previews_pending(&self) -> bool {
        !self.preview_jobs.is_idle()
    }

    /// Decode the thumbnail of `img_idx` for the cell `rect`, or queue it while `MAX_THUMB_JOBS`
    /// run. A sharp thumbnail in the cache, in a job or in the queue starts nothing. A HEIC cell
    /// without an image gets a preview job first.
    pub(crate) fn spawn_thumb_decode(&mut self, img_idx: usize, rect: Rect) {
        if self.thumb_cache.is_sharp(img_idx) || self.thumb_jobs.holds(img_idx) {
            return;
        }
        let job = ThumbJob {
            index: img_idx,
            rect,
        };
        if self.wants_preview(img_idx) {
            if self.preview_jobs.has_free_slot() {
                self.start_thumb_job(ThumbPhase::Preview, job);
            } else {
                self.preview_jobs.waiting.push_back(job);
            }
        }
        if self.sharp_job_waits(&self.images[img_idx]) || !self.thumb_jobs.has_free_slot() {
            self.thumb_jobs.waiting.push_back(job);
            return;
        }
        self.start_thumb_job(ThumbPhase::Sharp, job);
    }

    /// True for a file with an embedded preview, when the cell has no image and no preview job.
    fn wants_preview(&self, img_idx: usize) -> bool {
        has_embedded_preview(&self.images[img_idx])
            && !self.thumb_cache.contains(img_idx)
            && !self.preview_jobs.holds(img_idx)
    }

    /// Decode `job` on the thread pool. The preview phase of a cold HEIC cell takes the embedded
    /// thumbnail, scaled to the size that the sharp image gets. A disk cache hit sends the
    /// sharp image instead, so the cell skips the preview. The sharp phase decodes the file and
    /// writes the result into the disk cache.
    /// A job that waited across a list change has no file any more and starts nothing.
    fn start_thumb_job(&mut self, phase: ThumbPhase, job: ThumbJob) {
        let Some(path) = self.images.get(job.index).cloned() else {
            return;
        };
        self.slots_of(phase).running.insert(job.index);
        let tx = self.thumb_tx.clone();
        let target = thumb_target_px(job.rect, self.cell_px);
        let generation = self.thumb_generation;
        let cache_dir = self.disk_cache_dir.clone();
        rayon::spawn(move || {
            let (image, sharp) = match phase {
                ThumbPhase::Preview => preview_or_cached(&cache_dir, &path, target),
                ThumbPhase::Sharp => (sharp_thumbnail(&cache_dir, &path, target), true),
            };
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a closed receiver means that nobody waits for the result"
            )]
            let _ = tx.send(ThumbResult {
                generation,
                img_idx: job.index,
                phase,
                image,
                sharp,
            });
        });
    }
}

/// The sharp thumbnail from the disk cache, or from a decode that then fills the cache.
fn sharp_thumbnail(cache_dir: &Path, path: &Path, target: (u32, u32)) -> Option<RgbaImage> {
    try_load_from_disk_cache(cache_dir, path, target).or_else(|| {
        let image = decode_thumbnail(path, target).ok()?;
        try_save_to_disk_cache(cache_dir, path, target, &image);
        Some(image)
    })
}

/// The sharp thumbnail from the disk cache (`true`), or the embedded preview (`false`).
fn preview_or_cached(
    cache_dir: &Path,
    path: &Path,
    target: (u32, u32),
) -> (Option<RgbaImage>, bool) {
    match try_load_from_disk_cache(cache_dir, path, target) {
        Some(image) => (Some(image), true),
        None => (decode_preview(path, target).ok(), false),
    }
}

/// The pixel box of a thumbnail in the cell `rect`: the whole cell. The gaps of the grid lie
/// outside the cell, so the picture needs no padding.
pub(crate) fn thumb_target_px(rect: Rect, cell_px: (u32, u32)) -> (u32, u32) {
    (
        (u32::from(rect.width) * cell_px.0).max(1),
        (u32::from(rect.height) * cell_px.1).max(1),
    )
}

/// The sharp thumbnail of a grid cell: the first frame of a video, or the fitted image.
fn decode_thumbnail(path: &Path, target: (u32, u32)) -> io::Result<RgbaImage> {
    #[cfg(feature = "video")]
    if crate::video::is_video(path) {
        return crate::video::decode_first_frame(path, target);
    }
    load_and_resize_px(path, target, EmbeddedThumbnail::Allow)
}

/// Only the libheif formats carry an embedded thumbnail that viewr reads as a preview.
#[cfg(feature = "heic")]
fn has_embedded_preview(path: &Path) -> bool {
    decodes_with_libheif(path)
}

#[cfg(not(feature = "heic"))]
fn has_embedded_preview(_path: &Path) -> bool {
    false
}

/// The preview of a HEIC cell from its embedded thumbnail. Only libheif files have one.
#[cfg(feature = "heic")]
fn decode_preview(path: &Path, target: (u32, u32)) -> io::Result<RgbaImage> {
    decode_heif_preview(path, target)
}

#[cfg(not(feature = "heic"))]
fn decode_preview(_path: &Path, _target: (u32, u32)) -> io::Result<RgbaImage> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "no embedded preview without the heic feature",
    ))
}

#[cfg(test)]
#[cfg(feature = "heic")]
mod tests {
    use crate::app::App;
    use crate::test_support::{app_with, app_with_files, test_dir, wait_until};

    use super::{ThumbJob, ThumbPhase, ThumbResult};
    use crate::app::disk_cache::get_cache_file_path;
    use crate::decode::libheif::decode_heif_preview;
    use crate::image_list::SharedImageList;
    use crate::test_support::write_heif;
    use crate::test_support::{BLUE, RED, assert_close};
    use image::RgbaImage;
    use libheif_rs::{CompressionFormat, ImageOrientation};
    use ratatui::layout::Rect;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::thread;
    use std::time::{Duration, Instant};

    /// A thumbnail result as a job sends it.
    fn result(generation: u32, img_idx: usize, phase: ThumbPhase, sharp: bool) -> ThumbResult {
        ThumbResult {
            generation,
            img_idx,
            phase,
            image: Some(RgbaImage::new(1, 1)),
            sharp,
        }
    }

    /// An app with the one file `path` in a grid cell of 160x160 px, so a thumbnail box of
    /// 160x160 px, and a disk cache in `cache`. The 256x128 primary image fits as 160x80.
    fn grid_app(path: &Path, cache: &Path) -> App {
        let mut app = App::new((8, 16), SharedImageList::new());
        app.images = vec![path.to_path_buf()];
        app.filenames = vec!["photo.heic".to_owned()];
        app.gallery.filtered_indices = vec![0];
        app.disk_cache_dir = cache.to_path_buf();
        app.scan_complete = true;
        app.spawn_thumb_decode(0, Rect::new(0, 0, 20, 10));
        app
    }

    /// Poll until the cell 0 gets a new image, for at most 2 s. Returns the image.
    fn wait_for_new_image(app: &mut App) -> RgbaImage {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if app.poll_thumbnails() == vec![0] {
                return app.thumb_cache.peek(0).unwrap().0.clone();
            }
            assert!(Instant::now() < deadline, "no thumbnail arrived");
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn rgba_center(image: &RgbaImage) -> [u8; 4] {
        image.get_pixel(image.width() / 2, image.height() / 2).0
    }

    #[test]
    fn thumbnails_decode_once_and_come_back_from_the_disk_cache() {
        let dir = test_dir();
        let cache = dir.path().join("cache");
        let mut app = app_with_files(dir.path(), &["a.png"]);
        app.disk_cache_dir = cache.clone();
        // 3x2 cells of 8x16 px: a box of 24x32 px.
        let rect = Rect::new(0, 0, 3, 2);
        let path = app.images[0].clone();

        app.spawn_thumb_decode(0, rect);
        // A second request while the first one runs starts nothing.
        app.spawn_thumb_decode(0, rect);
        let mut arrived = Vec::new();
        wait_until(|| {
            arrived.extend(app.poll_thumbnails());
            !arrived.is_empty()
        });
        assert_eq!(arrived, vec![0]);
        assert!(app.thumb_cache.contains(0));
        let cached = get_cache_file_path(&cache, &path, (24, 32)).unwrap();
        assert!(cached.is_file());
        app.spawn_thumb_decode(0, rect);
        assert!(
            !app.thumb_jobs.running.contains(&0),
            "a cached thumbnail needs no decode"
        );

        // A fresh app reads the disk cache.
        let mut fresh = app_with(&[path.to_str().unwrap()]);
        fresh.disk_cache_dir = cache;
        fresh.spawn_thumb_decode(0, rect);
        let mut arrived = Vec::new();
        wait_until(|| {
            arrived.extend(fresh.poll_thumbnails());
            !arrived.is_empty()
        });
        assert_eq!(fresh.thumb_cache.peek(0).unwrap().0.dimensions(), (8, 8));
    }

    #[test]
    fn at_most_four_thumbnails_decode_at_once_and_the_rest_wait() {
        let dir = test_dir();
        let names = ["a.png", "b.png", "c.png", "d.png", "e.png", "f.png"];
        let mut app = app_with_files(dir.path(), &names);
        app.scan_complete = true;
        let rect = Rect::new(0, 0, 1, 1);
        assert!(!app.has_pending_work());

        for index in 0..names.len() {
            app.spawn_thumb_decode(index, rect);
        }
        assert_eq!(app.thumb_jobs.running.len(), 4, "four jobs run");
        assert_eq!(app.thumb_jobs.waiting.len(), 2, "two jobs wait");
        assert!(app.has_pending_work());
        // A second request for a waiting thumbnail queues nothing.
        app.spawn_thumb_decode(5, rect);
        assert_eq!(app.thumb_jobs.waiting.len(), 2);

        let mut arrived = Vec::new();
        wait_until(|| {
            arrived.extend(app.poll_thumbnails());
            arrived.len() == names.len()
        });
        arrived.sort_unstable();
        assert_eq!(arrived, (0..names.len()).collect::<Vec<_>>());
        assert!(app.thumb_jobs.running.is_empty() && app.thumb_jobs.waiting.is_empty());
        assert!(!app.has_pending_work());

        // A new render of the grid drops the waiting jobs, the running ones finish.
        app.mark_dirty();
        for index in 0..names.len() {
            app.spawn_thumb_decode(index, rect);
        }
        app.clear_thumb_queue();
        assert_eq!(app.thumb_jobs.running.len(), 4);
        assert!(app.thumb_jobs.waiting.is_empty());
        let mut arrived = Vec::new();
        wait_until(|| {
            arrived.extend(app.poll_thumbnails());
            arrived.len() == 4
        });
    }

    #[test]
    fn thumbnails_of_an_old_generation_are_dropped() {
        let mut app = app_with(&["a.png", "b.png"]);
        app.mark_dirty();
        let current = app.thumb_generation;
        // A job of the old generation and a job of the current one, in this order.
        app.thumb_tx
            .send(result(current - 1, 0, ThumbPhase::Sharp, true))
            .unwrap();
        app.thumb_tx
            .send(result(current, 1, ThumbPhase::Sharp, true))
            .unwrap();
        app.thumb_jobs.running.insert(0);

        assert_eq!(app.poll_thumbnails(), vec![1]);
        assert!(!app.thumb_cache.contains(0));
        assert!(app.thumb_cache.contains(1));
        assert!(
            !app.thumb_jobs.running.contains(&0),
            "a stale result still ends the load"
        );
    }

    #[test]
    fn a_preview_of_the_old_zoom_is_dropped_and_a_failed_job_frees_its_slot() {
        let mut app = app_with(&["a.heic", "b.heic"]);
        app.scan_complete = true;
        app.gallery.update_grid(Rect::new(0, 0, 100, 40));
        let before = app.thumb_generation;
        app.preview_jobs.running.insert(0);
        app.thumb_jobs.running.insert(1);
        app.thumb_zoom_in();
        let current = app.thumb_generation;
        assert_ne!(current, before, "a grid zoom starts a new generation");
        assert!(app.preview_jobs.running.is_empty() && app.thumb_jobs.running.is_empty());

        // The preview of the old zoom, then a preview and a failed sharp decode of the new one.
        app.thumb_tx
            .send(result(before, 0, ThumbPhase::Preview, false))
            .unwrap();
        app.thumb_tx
            .send(result(current, 1, ThumbPhase::Preview, false))
            .unwrap();
        app.preview_jobs.running.insert(1);
        app.thumb_jobs.running.insert(1);
        app.thumb_tx
            .send(ThumbResult {
                generation: current,
                img_idx: 1,
                phase: ThumbPhase::Sharp,
                image: None,
                sharp: true,
            })
            .unwrap();

        assert_eq!(app.poll_thumbnails(), vec![1]);
        assert!(!app.thumb_cache.contains(0), "the stale preview is dropped");
        assert!(app.thumb_cache.contains(1) && !app.thumb_cache.is_sharp(1));
        assert!(!app.has_pending_work(), "a failed decode frees its slot");
    }

    #[test]
    fn only_the_libheif_sharp_jobs_wait_for_the_previews() {
        let dir = test_dir();
        let mut app = app_with_files(dir.path(), &["b.png"]);
        app.images.push(PathBuf::from("a.heic"));
        app.filenames.push("a.heic".to_owned());
        app.gallery.filtered_indices.push(1);
        let rect = Rect::new(0, 0, 1, 1);
        // A preview job runs. The HEIC cell waits, the PNG cell starts at once.
        app.preview_jobs.running.insert(1);
        app.spawn_thumb_decode(1, rect);
        app.spawn_thumb_decode(0, rect);

        {
            assert_eq!(app.thumb_jobs.waiting.len(), 1, "the HEIC sharp job waits");
            assert!(
                app.thumb_jobs.running.contains(&0),
                "the PNG decodes at once"
            );
            assert_eq!(app.poll_thumbnails(), Vec::<usize>::new());
            assert_eq!(
                app.thumb_jobs.waiting.len(),
                1,
                "the poll keeps the HEIC job waiting"
            );
        }
        #[cfg(not(feature = "heic"))]
        assert_eq!(
            app.thumb_jobs.running.len(),
            2,
            "without libheif, no job waits"
        );
    }

    #[test]
    fn a_sharp_result_replaces_the_preview_and_drops_the_waiting_job() {
        let mut app = app_with(&["a.heic", "b.heic"]);
        let rect = Rect::new(0, 0, 1, 1);
        assert!(app.store_thumbnail(0, RgbaImage::new(1, 1), false));
        let (_, preview_id) = app.thumb_cache.peek(0).unwrap();
        app.thumb_jobs
            .waiting
            .push_back(ThumbJob { index: 0, rect });
        app.thumb_jobs
            .waiting
            .push_back(ThumbJob { index: 1, rect });

        assert!(
            !app.store_thumbnail(0, RgbaImage::new(1, 1), false),
            "a second preview changes nothing"
        );
        assert!(app.store_thumbnail(0, RgbaImage::new(2, 2), true));
        assert!(app.thumb_cache.is_sharp(0));
        assert_eq!(app.stale_image_ids, vec![(0, preview_id)]);
        assert_eq!(
            app.thumb_jobs.waiting.len(),
            1,
            "the disk cache hit made the job unnecessary"
        );
        assert!(
            !app.store_thumbnail(0, RgbaImage::new(3, 3), true),
            "a second sharp image changes nothing"
        );
        assert_eq!(app.thumb_cache.peek(0).unwrap().0.dimensions(), (2, 2));
    }

    #[test]
    fn heic_grid_shows_the_blue_preview_and_then_the_red_sharp_thumbnail() {
        let dir = test_dir();
        let path = dir.path().join("photo.heic");
        let normal = ImageOrientation::Normal;
        write_heif(&path, CompressionFormat::Hevc, normal, Some(normal));
        let cache = dir.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let mut app = grid_app(&path, &cache);
        assert_eq!(
            app.preview_jobs.running.len(),
            1,
            "the preview starts at once"
        );
        assert_eq!(
            app.thumb_jobs.waiting.len(),
            1,
            "the sharp decode waits for the preview"
        );

        let preview = wait_for_new_image(&mut app);
        assert!(!app.thumb_cache.is_sharp(0));
        assert_eq!(
            preview.dimensions(),
            (160, 80),
            "the size of the sharp image"
        );
        assert_close(rgba_center(&preview), BLUE);
        assert_eq!(
            fs::read_dir(&cache).unwrap().count(),
            0,
            "no preview on disk"
        );
        let (_, preview_id) = app.thumb_cache.peek(0).unwrap();

        let sharp = wait_for_new_image(&mut app);
        assert!(app.thumb_cache.is_sharp(0));
        assert_eq!(sharp.dimensions(), (160, 80));
        assert_close(rgba_center(&sharp), RED);
        assert_eq!(
            fs::read_dir(&cache).unwrap().count(),
            1,
            "the sharp image on disk"
        );
        assert_eq!(app.stale_image_ids, vec![(0, preview_id)]);
        assert!(!app.has_pending_work());

        // A warm cell comes from the disk cache and skips the preview.
        let mut warm = grid_app(&path, &cache);
        let first = wait_for_new_image(&mut warm);
        assert!(warm.thumb_cache.is_sharp(0));
        assert_close(rgba_center(&first), RED);
        assert!(warm.stale_image_ids.is_empty());
        let deadline = Instant::now() + Duration::from_secs(2);
        while warm.has_pending_work() {
            assert!(Instant::now() < deadline, "the jobs did not finish");
            assert!(warm.poll_thumbnails().is_empty(), "no second image");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn heic_thumbnail_with_another_transform_gives_no_preview() {
        let dir = test_dir();
        let path = dir.path().join("rotated.heic");
        write_heif(
            &path,
            CompressionFormat::Hevc,
            ImageOrientation::Rotate90Cw,
            Some(ImageOrientation::Normal),
        );
        let error = decode_heif_preview(&path, (160, 160)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);

        let mut app = grid_app(&path, dir.path());
        let first = wait_for_new_image(&mut app);
        assert!(
            app.thumb_cache.is_sharp(0),
            "the first image is the sharp one"
        );
        assert_eq!(first.dimensions(), (80, 160));
        assert_close(rgba_center(&first), RED);
        assert!(app.stale_image_ids.is_empty());
    }

    #[test]
    fn heic_preview_keeps_the_rotation_of_the_primary_image() {
        let dir = test_dir();
        let path = dir.path().join("rotated.heic");
        let rotation = ImageOrientation::Rotate90Cw;
        write_heif(&path, CompressionFormat::Hevc, rotation, Some(rotation));

        let preview = decode_heif_preview(&path, (160, 160)).unwrap();
        assert_eq!(
            preview.dimensions(),
            (80, 160),
            "portrait like the primary image"
        );
        assert_close(rgba_center(&preview), BLUE);
    }
}
