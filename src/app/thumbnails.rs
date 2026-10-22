//! The thumbnail jobs of the grid: the queue, the embedded previews (HEIC, camera RAW, PSD),
//! the padding to whole cells, the parked thumbnails of the folders before, the generations.

use super::App;
use super::disk_cache::{get_cache_file_path, try_load_from_disk_cache, try_save_to_disk_cache};
use crate::decode::EmbeddedThumbnail;
#[cfg(all(feature = "sips", target_os = "macos"))]
use crate::decode::embedded_jpeg;
#[cfg(feature = "heic")]
use crate::decode::libheif::{decode_heif_preview, decodes_with_libheif};
use crate::decode::resize::load_and_resize_px;
use crate::gallery::{GridPicture, ParkKey};
use image::RgbaImage;
use ratatui::layout::Rect;
use std::collections::{HashSet, VecDeque};
use std::io;
use std::path::Path;

/// The thumbnail decodes that run at once. 24 parallel decodes of 24 MP HEIC frames held
/// 2 GB of pixels and ran 40 libheif threads on 10 cores, so the first thumbnail took 1 s.
/// With 4 jobs, each with its share of the cores, the first one comes after one decode.
const MAX_THUMB_JOBS: usize = 4;

/// The previews of a cold grid that decode at once. A preview is the embedded thumbnail of a
/// HEIC (320x240), or the embedded JPEG of a RAW or PSD file, a few milliseconds each. The sharp
/// decodes wait until every preview is out.
const MAX_PREVIEW_JOBS: usize = 4;

/// The most animation flags that the app keeps. One flag holds a path and a bool, so the map
/// stays below about 10 MB.
const MAX_ANIMATION_FLAGS: usize = 65_536;

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
/// sharp image when the disk cache has it. `animated` is the header check of a file that can
/// hold an animation.
pub(super) struct ThumbResult {
    generation: u32,
    img_idx: usize,
    phase: ThumbPhase,
    image: Option<GridPicture>,
    sharp: bool,
    animated: Option<bool>,
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
            if result.generation == self.thumb_generation {
                self.store_animation_flag(result.img_idx, result.animated);
            }
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
            && let Some(position) = self.next_sharp_job()
            && let Some(job) = self.thumb_jobs.waiting.remove(position)
        {
            self.start_thumb_job(ThumbPhase::Sharp, job);
        }
        new_indices
    }

    /// The position of the next sharp job in the queue: the first one of a cell that shows
    /// nothing yet, before the first one that only replaces a preview. A mixed folder then
    /// shows every cell before the slow HEIC decodes start.
    fn next_sharp_job(&self) -> Option<usize> {
        let ready = |job: &ThumbJob| !self.waiting_job_holds(job);
        let waiting = &self.thumb_jobs.waiting;
        waiting
            .iter()
            .position(|job| ready(job) && !self.thumb_cache.contains(job.index))
            .or_else(|| waiting.iter().position(ready))
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

    /// True while the sharp decode of a file with an embedded preview must wait for the
    /// previews. The other formats have no preview phase and start at once.
    fn sharp_job_waits(&self, path: &Path) -> bool {
        has_embedded_preview(path) && self.previews_pending()
    }

    /// Put a decoded thumbnail into the cache. A sharp image replaces a preview and drops the
    /// waiting sharp job of the cell. A preview never replaces an image. `false` when the cache
    /// keeps what it has.
    pub(crate) fn store_thumbnail(
        &mut self,
        img_idx: usize,
        picture: impl Into<GridPicture>,
        sharp: bool,
    ) -> bool {
        if sharp {
            if self.thumb_cache.is_sharp(img_idx) {
                return false;
            }
            self.thumb_jobs.waiting.retain(|job| job.index != img_idx);
            if let Some(old_id) = self.thumb_cache.insert(img_idx, picture) {
                self.stale_image_ids.push((img_idx, old_id));
            }
            return true;
        }
        if self.thumb_cache.contains(img_idx) {
            return false;
        }
        self.thumb_cache.insert_preview(img_idx, picture);
        true
    }

    /// The size that the grid decodes its thumbnails for: the key of the parked thumbnails.
    pub(crate) fn park_key(&self) -> ParkKey {
        ParkKey {
            target: thumb_target_px(self.gallery.thumb_box(), self.cell_px),
            cell_px: self.cell_px,
        }
    }

    /// Keep the sharp thumbnails of the grid for the way back to this folder. The switch of
    /// the folder calls this before it drops the image list.
    pub(crate) fn park_thumbnails(&mut self) {
        let key = self.park_key();
        self.thumb_cache.park(&self.images, key);
    }

    /// Keep the animation flag of a file that can hold an animation, for the grid marker.
    ///
    /// The flags outlive a folder switch, because a parked thumbnail comes back without a job.
    /// Each job reads the flag again, and a changed file gets a job, so a flag is as fresh as
    /// its thumbnail. Above `MAX_ANIMATION_FLAGS` entries, the map starts again.
    fn store_animation_flag(&mut self, img_idx: usize, animated: Option<bool>) {
        if let Some(animated) = animated
            && let Some(path) = self.images.get(img_idx)
        {
            if self.animated.len() >= MAX_ANIMATION_FLAGS && !self.animated.contains_key(path) {
                self.animated.clear();
            }
            self.animated.insert(path.clone(), animated);
        }
    }

    pub(super) fn previews_pending(&self) -> bool {
        !self.preview_jobs.is_idle()
    }

    /// Decode the thumbnail of `img_idx` for the cell `rect`, or queue it while `MAX_THUMB_JOBS`
    /// run. A sharp thumbnail in the cache, in a job or in the queue starts nothing, and a
    /// parked thumbnail of the folder before goes into the cache at once. A HEIC, RAW or PSD
    /// cell without an image gets a preview job first.
    pub(crate) fn spawn_thumb_decode(&mut self, img_idx: usize, rect: Rect) {
        if self.is_folder(img_idx)
            || self.thumb_cache.is_sharp(img_idx)
            || self.thumb_jobs.holds(img_idx)
        {
            return;
        }
        let key = ParkKey {
            target: thumb_target_px(rect, self.cell_px),
            cell_px: self.cell_px,
        };
        if let Some(parked) = self.thumb_cache.take_parked(&self.images[img_idx], key) {
            self.store_thumbnail(img_idx, parked, true);
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

    /// Decode `job` on the thread pool. The preview phase of a cold cell takes the embedded
    /// thumbnail at its own size. A disk cache hit, or an embedded JPEG that covers the box,
    /// sends the sharp image instead, so the cell skips the sharp phase. The sharp phase decodes
    /// the file and writes the result into the disk cache. The job pads the result for the
    /// grid, so the UI thread copies no pixels.
    /// A job that waited across a list change has no file any more and starts nothing.
    fn start_thumb_job(&mut self, phase: ThumbPhase, job: ThumbJob) {
        let Some(path) = self.images.get(job.index).cloned() else {
            return;
        };
        self.slots_of(phase).running.insert(job.index);
        let tx = self.thumb_tx.clone();
        let target = thumb_target_px(job.rect, self.cell_px);
        let cell_px = self.cell_px;
        let generation = self.thumb_generation;
        let cache_dir = self.disk_cache_dir.clone();
        rayon::spawn(move || {
            let (image, sharp) = match phase {
                ThumbPhase::Preview => preview_or_cached(&cache_dir, &path, target),
                ThumbPhase::Sharp => (sharp_thumbnail(&cache_dir, &path, target), true),
            };
            let image = image.map(|image| pad_for_grid(image, target, cell_px, sharp));
            let animated = animation_flag(&path);
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
                animated,
            });
        });
    }
}

/// The animation flag of a file that can hold an animation, `None` for the other formats. The
/// job reads it, because the draw must read no file.
#[cfg(feature = "video")]
fn animation_flag(path: &Path) -> Option<bool> {
    crate::player::animation::has_animation_extension(path)
        .then(|| crate::player::is_animated(path))
}

/// Without the `video` feature, an animated image is a still image.
#[cfg(not(feature = "video"))]
fn animation_flag(_path: &Path) -> Option<bool> {
    None
}

/// Put the sharp thumbnail of `path` at `target` into the disk cache, as the sharp job of the
/// grid does. A file that the cache holds costs one `stat` and no decode. The folder prefetch
/// calls it.
pub(super) fn warm_disk_cache(cache_dir: &Path, path: &Path, target: (u32, u32)) {
    if get_cache_file_path(cache_dir, path, target).is_some_and(|file| file.exists()) {
        return;
    }
    sharp_thumbnail(cache_dir, path, target);
}

/// The sharp thumbnail from the disk cache, or from a decode that then fills the cache.
fn sharp_thumbnail(cache_dir: &Path, path: &Path, target: (u32, u32)) -> Option<RgbaImage> {
    try_load_from_disk_cache(cache_dir, path, target).or_else(|| {
        let image = decode_thumbnail(path, target).ok()?;
        try_save_to_disk_cache(cache_dir, path, target, &image);
        Some(image)
    })
}

/// The sharp thumbnail from the disk cache (`true`), or the embedded preview. An embedded
/// JPEG that covers the box is sharp too, and goes into the disk cache.
fn preview_or_cached(
    cache_dir: &Path,
    path: &Path,
    target: (u32, u32),
) -> (Option<RgbaImage>, bool) {
    if let Some(image) = try_load_from_disk_cache(cache_dir, path, target) {
        return (Some(image), true);
    }
    match decode_preview(path, target) {
        Ok((image, true)) => {
            try_save_to_disk_cache(cache_dir, path, target, &image);
            (Some(image), true)
        }
        Ok((image, false)) => (Some(image), false),
        Err(_) => (None, false),
    }
}

/// The pixel box of a thumbnail in the cell `rect`: the whole cell. The gaps of the grid lie
/// outside the cell.
pub(crate) fn thumb_target_px(rect: Rect, cell_px: (u32, u32)) -> (u32, u32) {
    (
        (u32::from(rect.width) * cell_px.0).max(1),
        (u32::from(rect.height) * cell_px.1).max(1),
    )
}

/// Pad a decoded thumbnail with transparent pixels to whole cells, so that it starts at a cell
/// border without a pixel offset. A sharp image gets the smallest box of whole cells that
/// holds it, at most `target`. A preview keeps its own resolution and gets the shape of that
/// box, so the terminal scales it to the box without a change of its shape. The decode job
/// runs this, after the unpadded disk cache write.
pub(crate) fn pad_for_grid(
    image: RgbaImage,
    target: (u32, u32),
    cell_px: (u32, u32),
    sharp: bool,
) -> GridPicture {
    let shown = if sharp {
        image.dimensions()
    } else {
        scaled_up_to(image.dimensions(), target)
    };
    let cells = whole_cells(shown, target, cell_px);
    let box_px = (
        u32::from(cells.0) * cell_px.0.max(1),
        u32::from(cells.1) * cell_px.1.max(1),
    );
    let canvas = if sharp {
        box_px
    } else {
        box_shaped_canvas(image.dimensions(), box_px)
    };
    GridPicture {
        image: pad_centered(image, canvas),
        cells,
    }
}

/// The whole cells that a picture of `size` pixels covers, at least one and at most the cells
/// of `target`.
fn whole_cells(size: (u32, u32), target: (u32, u32), cell_px: (u32, u32)) -> (u16, u16) {
    let cells = |side: u32, box_side: u32, cell: u32| {
        let cell = cell.max(1);
        let count = side.div_ceil(cell).clamp(1, (box_side / cell).max(1));
        u16::try_from(count).unwrap_or(u16::MAX)
    };
    (
        cells(size.0, target.0, cell_px.0),
        cells(size.1, target.1, cell_px.1),
    )
}

/// `size` scaled up to fit `target`: the size that the sharp decode of a preview gets.
fn scaled_up_to(size: (u32, u32), target: (u32, u32)) -> (u32, u32) {
    let (width, height) = (f64::from(size.0.max(1)), f64::from(size.1.max(1)));
    let scale = f64::min(f64::from(target.0) / width, f64::from(target.1) / height);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the result is at most the target, which came from u32 values"
    )]
    let scaled = (
        (width * scale).round() as u32,
        (height * scale).round() as u32,
    );
    (scaled.0.max(1), scaled.1.max(1))
}

/// The smallest canvas with the shape of `target` that holds `size`. The rounding up changes
/// the shape by less than one pixel.
fn box_shaped_canvas(size: (u32, u32), target: (u32, u32)) -> (u32, u32) {
    let (width, height) = (f64::from(size.0), f64::from(size.1));
    let (box_w, box_h) = (f64::from(target.0.max(1)), f64::from(target.1.max(1)));
    let scale = (width / box_w).max(height / box_h);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the canvas is at most the box, which came from u32 values"
    )]
    let canvas = ((scale * box_w).ceil() as u32, (scale * box_h).ceil() as u32);
    (canvas.0.max(size.0), canvas.1.max(size.1))
}

/// `image` in the middle of a transparent canvas of `canvas` pixels. An image of the canvas
/// size is returned as it is.
fn pad_centered(image: RgbaImage, canvas: (u32, u32)) -> RgbaImage {
    if image.dimensions() == canvas {
        return image;
    }
    let mut padded = RgbaImage::new(canvas.0, canvas.1);
    let x = i64::from(canvas.0.saturating_sub(image.width()) / 2);
    let y = i64::from(canvas.1.saturating_sub(image.height()) / 2);
    image::imageops::overlay(&mut padded, &image, x, y);
    padded
}

/// The sharp thumbnail of a grid cell: the first frame of a video, or the fitted image.
fn decode_thumbnail(path: &Path, target: (u32, u32)) -> io::Result<RgbaImage> {
    #[cfg(feature = "video")]
    if crate::video::is_video(path) {
        return crate::video::decode_first_frame(path, target);
    }
    load_and_resize_px(path, target, EmbeddedThumbnail::Allow)
}

/// True for a file with an embedded preview that the grid shows first: the libheif formats,
/// camera RAW and PSD.
fn has_embedded_preview(path: &Path) -> bool {
    #[cfg(feature = "heic")]
    if decodes_with_libheif(path) {
        return true;
    }
    #[cfg(all(feature = "sips", target_os = "macos"))]
    if embedded_jpeg::may_have_embedded_jpeg(path) {
        return true;
    }
    #[cfg(not(any(feature = "heic", all(feature = "sips", target_os = "macos"))))]
    let _ = path;
    false
}

/// The preview of a cell from its embedded thumbnail, and whether it is the sharp thumbnail.
/// A HEIC preview is never sharp. The embedded JPEG of a RAW or PSD file is sharp when it
/// covers the box, and then fitted into the box like a sharp decode. A smaller one keeps its
/// own resolution.
fn decode_preview(path: &Path, target: (u32, u32)) -> io::Result<(RgbaImage, bool)> {
    #[cfg(feature = "heic")]
    if decodes_with_libheif(path) {
        return decode_heif_preview(path, target).map(|image| (image, false));
    }
    #[cfg(all(feature = "sips", target_os = "macos"))]
    if let Some(jpeg) = embedded_jpeg::find(path) {
        let sharp = jpeg.covers(target);
        let image = embedded_jpeg::decode(path, &jpeg, Some(target))?;
        return Ok((image.fit_within(target.0, target.1), sharp));
    }
    #[cfg(not(any(feature = "heic", all(feature = "sips", target_os = "macos"))))]
    let _ = (path, target);
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no embedded preview in this file",
    ))
}

#[cfg(test)]
#[cfg(feature = "heic")]
mod tests {
    use crate::app::App;
    use crate::test_support::{app_with, app_with_files, test_dir, wait_until};

    use super::{ThumbJob, ThumbPhase, ThumbResult, pad_for_grid};
    use crate::app::disk_cache::{get_cache_file_path, try_load_from_disk_cache};
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
            image: Some(RgbaImage::new(1, 1).into()),
            sharp,
            animated: None,
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
                return app.thumb_cache.peek(0).unwrap().0.image.clone();
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
        assert_eq!(
            fresh.thumb_cache.peek(0).unwrap().0.image.dimensions(),
            (8, 16),
            "the 8x8 image padded to one cell of 8x16 px"
        );
        let on_disk = try_load_from_disk_cache(&fresh.disk_cache_dir, &path, (24, 32)).unwrap();
        assert_eq!(
            on_disk.dimensions(),
            (8, 8),
            "the disk cache stays unpadded"
        );
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
                animated: None,
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
        assert_eq!(
            app.thumb_cache.peek(0).unwrap().0.image.dimensions(),
            (2, 2)
        );
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
            (64, 32),
            "the 64x32 thumbnail has the shape of the 160x80 sharp image: no padding"
        );
        assert_eq!(app.thumb_cache.peek(0).unwrap().0.cells, (20, 5));
        assert_close(rgba_center(&preview), BLUE);
        assert_eq!(
            fs::read_dir(&cache).unwrap().count(),
            0,
            "no preview on disk"
        );
        let (_, preview_id) = app.thumb_cache.peek(0).unwrap();

        let sharp = wait_for_new_image(&mut app);
        assert!(app.thumb_cache.is_sharp(0));
        assert_eq!(
            sharp.dimensions(),
            (160, 80),
            "20x5 whole cells, no padding"
        );
        assert_eq!(app.thumb_cache.peek(0).unwrap().0.cells, (20, 5));
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
        assert_eq!(
            first.dimensions(),
            (80, 160),
            "10x10 whole cells, no padding"
        );
        assert_eq!(app.thumb_cache.peek(0).unwrap().0.cells, (10, 10));
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
            (32, 64),
            "portrait like the primary image, at its own size"
        );
        assert_close(rgba_center(&preview), BLUE);

        // A box smaller than the thumbnail scales it down to the size of the sharp image.
        let small = decode_heif_preview(&path, (40, 40)).unwrap();
        assert_eq!(small.dimensions(), (20, 40));
    }

    #[test]
    fn a_sharp_thumbnail_is_padded_to_whole_cells_with_the_content_in_the_middle() {
        let red = image::Rgba([255, 0, 0, 255]);
        let clear = image::Rgba([0, 0, 0, 0]);
        // Cells of 10x20 px, a box of 10x3 cells (100x60 px). A landscape of 100x40 px takes
        // 2 rows (40 px): no padding at all.
        let landscape = pad_for_grid(
            RgbaImage::from_pixel(100, 40, red),
            (100, 60),
            (10, 20),
            true,
        );
        assert_eq!(landscape.cells, (10, 2));
        assert_eq!(landscape.image.dimensions(), (100, 40));
        assert_eq!(*landscape.image.get_pixel(50, 0), red);

        // A landscape of 100x30 px takes 2 rows: 5 px of padding above and below.
        let padded = pad_for_grid(
            RgbaImage::from_pixel(100, 30, red),
            (100, 60),
            (10, 20),
            true,
        );
        assert_eq!(padded.cells, (10, 2));
        assert_eq!(padded.image.dimensions(), (100, 40));
        assert_eq!(*padded.image.get_pixel(50, 4), clear);
        assert_eq!(*padded.image.get_pixel(50, 5), red);
        assert_eq!(*padded.image.get_pixel(50, 34), red);
        assert_eq!(*padded.image.get_pixel(50, 35), clear);

        // A portrait of 39x60 px takes 4 columns (40 px): the odd pixel goes to the right.
        let portrait = pad_for_grid(
            RgbaImage::from_pixel(39, 60, red),
            (100, 60),
            (10, 20),
            true,
        );
        assert_eq!(portrait.cells, (4, 3));
        assert_eq!(portrait.image.dimensions(), (40, 60));
        assert_eq!(*portrait.image.get_pixel(0, 30), red);
        assert_eq!(*portrait.image.get_pixel(38, 30), red);
        assert_eq!(*portrait.image.get_pixel(39, 30), clear);

        // An image of the box size is not copied, and never more cells than the box has.
        let full = pad_for_grid(
            RgbaImage::from_pixel(100, 60, red),
            (100, 60),
            (10, 20),
            true,
        );
        assert_eq!(full.cells, (10, 3));
        assert_eq!(*full.image.get_pixel(0, 0), red);
        let over = pad_for_grid(
            RgbaImage::from_pixel(120, 70, red),
            (100, 60),
            (10, 20),
            true,
        );
        assert_eq!(over.cells, (10, 3));
        assert_eq!(over.image.dimensions(), (100, 60));
        // A tiny image takes one cell.
        assert_eq!(
            pad_for_grid(RgbaImage::from_pixel(1, 1, red), (100, 60), (10, 20), true).cells,
            (1, 1)
        );
    }

    #[test]
    fn a_preview_is_padded_to_the_shape_of_its_cells_at_its_own_size() {
        let red = image::Rgba([255, 0, 0, 255]);
        // A 4:3 preview of 40x30 px in a 100x60 box of 10x20 px cells: the sharp image would
        // be 80x60 px, 8x3 cells. The canvas has that shape: 40x30 stays 40x30.
        let preview = pad_for_grid(
            RgbaImage::from_pixel(40, 30, red),
            (100, 60),
            (10, 20),
            false,
        );
        assert_eq!(preview.cells, (8, 3));
        assert_eq!(preview.image.dimensions(), (40, 30));

        // A 5:2 preview of 50x20 px: the sharp image would be 100x40 px, 10x2 cells. The
        // canvas has the shape 10:4, so 50x20 stays 50x20.
        let wide = pad_for_grid(
            RgbaImage::from_pixel(50, 20, red),
            (100, 60),
            (10, 20),
            false,
        );
        assert_eq!(wide.cells, (10, 2));
        assert_eq!(wide.image.dimensions(), (50, 20));

        // A 3:2 preview of 30x20 px: the sharp image would be 90x60, 9x3 cells (90x60 px). The
        // canvas has that shape, 30x20, with the content in the middle.
        let odd = pad_for_grid(
            RgbaImage::from_pixel(33, 21, red),
            (160, 96),
            (8, 16),
            false,
        );
        assert!(odd.image.width() >= 33 && odd.image.height() >= 21);
        let cells_px = (f64::from(odd.cells.0) * 8.0, f64::from(odd.cells.1) * 16.0);
        let shape = f64::from(odd.image.width()) / f64::from(odd.image.height());
        assert!((shape - cells_px.0 / cells_px.1).abs() < 0.05, "{shape}");
    }

    #[test]
    fn the_queue_starts_the_empty_cells_before_the_heic_sharp_decodes() {
        let mut app = app_with(&["a.heic", "b.jpg", "c.heic", "d.png"]);
        let rect = Rect::new(0, 0, 1, 1);
        // The HEIC cells have their previews; the sharp jobs wait in grid order.
        assert!(app.store_thumbnail(0, RgbaImage::new(1, 1), false));
        assert!(app.store_thumbnail(2, RgbaImage::new(1, 1), false));
        for index in 0..4 {
            app.thumb_jobs.waiting.push_back(ThumbJob { index, rect });
        }
        app.thumb_jobs.running.extend(10..13);

        assert_eq!(app.next_sharp_job(), Some(1), "b.jpg shows nothing yet");
        app.thumb_jobs.waiting.remove(1);
        assert_eq!(app.next_sharp_job(), Some(2), "d.png, now at position 2");
        app.thumb_jobs.waiting.remove(2);
        assert_eq!(
            app.next_sharp_job(),
            Some(0),
            "then the HEIC cells in grid order"
        );

        // A HEIC cell whose preview is still on its way holds its place in the queue.
        app.preview_jobs.running.insert(2);
        assert_eq!(app.next_sharp_job(), None, "every HEIC sharp job waits");
    }
}

#[cfg(test)]
#[cfg(all(feature = "sips", target_os = "macos"))]
mod embedded_jpeg_tests {
    use crate::app::App;
    use crate::image_list::SharedImageList;
    use crate::test_support::{BLUE, RED, assert_close, test_dir, write_psd, write_tiff_raw};
    use image::RgbaImage;
    use ratatui::layout::Rect;
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, Instant};

    /// An app with the one file `path` in a grid cell `rect` of 8x16 px cells and a disk cache
    /// in `cache`. The thumbnail job of the cell starts at once.
    fn grid_app(path: &Path, cache: &Path, rect: Rect) -> App {
        let mut app = App::new((8, 16), SharedImageList::new());
        app.images = vec![path.to_path_buf()];
        app.filenames = vec![path.file_name().unwrap().to_string_lossy().into_owned()];
        app.gallery.filtered_indices = vec![0];
        app.disk_cache_dir = cache.to_path_buf();
        app.scan_complete = true;
        app.spawn_thumb_decode(0, rect);
        app
    }

    /// Poll until the cell 0 gets a new image, for at most 5 s (sips is slow). Returns it.
    fn wait_for_new_image(app: &mut App) -> RgbaImage {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if app.poll_thumbnails() == vec![0] {
                return app.thumb_cache.peek(0).unwrap().0.image.clone();
            }
            assert!(Instant::now() < deadline, "no thumbnail arrived");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn left_right(image: &RgbaImage) -> ([u8; 4], [u8; 4]) {
        let y = image.height() / 2;
        (
            image.get_pixel(1, y).0,
            image.get_pixel(image.width() - 2, y).0,
        )
    }

    #[test]
    fn a_raw_cell_takes_the_embedded_jpeg_as_the_sharp_thumbnail_without_sips() {
        let dir = test_dir();
        let path = dir.path().join("photo.dng");
        write_tiff_raw(&path, 1);
        let cache = dir.path().join("cache");
        // A box of 32x32 px: the 64x32 JPEG covers it, so the preview phase gives the sharp
        // thumbnail, and the sharp job of the cell is dropped before it runs sips.
        let mut app = grid_app(&path, &cache, Rect::new(0, 0, 4, 2));
        assert!(app.preview_jobs.running.contains(&0));
        assert_eq!(
            app.thumb_jobs.waiting.len(),
            1,
            "the sips job waits for the preview"
        );

        let sharp = wait_for_new_image(&mut app);
        assert!(app.thumb_cache.is_sharp(0));
        assert_eq!(
            sharp.dimensions(),
            (32, 16),
            "fitted into the box, 4x1 cells"
        );
        assert_eq!(app.thumb_cache.peek(0).unwrap().0.cells, (4, 1));
        let (left, right) = left_right(&sharp);
        assert_close(left, RED);
        assert_close(right, BLUE);
        assert!(app.thumb_jobs.waiting.is_empty(), "no sips decode");
        assert!(app.thumb_jobs.running.is_empty());
        assert!(!app.has_pending_work());
        assert_eq!(
            fs::read_dir(&cache).unwrap().count(),
            1,
            "the embedded JPEG went into the disk cache"
        );
    }

    #[test]
    fn a_psd_cell_shows_its_thumbnail_first_and_then_the_sips_decode() {
        let dir = test_dir();
        let path = dir.path().join("design.psd");
        write_psd(&path, true);
        let cache = dir.path().join("cache");
        // A box of 160x160 px: the 64x32 thumbnail is too small, so it is the preview.
        let mut app = grid_app(&path, &cache, Rect::new(0, 0, 20, 10));

        let preview = wait_for_new_image(&mut app);
        assert!(!app.thumb_cache.is_sharp(0));
        assert_eq!(preview.dimensions(), (64, 32), "its own resolution");
        assert_eq!(app.thumb_cache.peek(0).unwrap().0.cells, (20, 5));
        let (left, right) = left_right(&preview);
        assert_close(left, RED);
        assert_close(right, BLUE);
        assert!(fs::read_dir(&cache).is_err(), "no preview on disk");
        let (_, preview_id) = app.thumb_cache.peek(0).unwrap();

        let sharp = wait_for_new_image(&mut app);
        assert!(app.thumb_cache.is_sharp(0));
        assert_eq!(
            sharp.dimensions(),
            (64, 32),
            "sips does not enlarge the 64x32 PSD"
        );
        assert_eq!(app.thumb_cache.peek(0).unwrap().0.cells, (8, 2));
        let (left, right) = left_right(&sharp);
        assert_close(left, RED);
        assert_close(right, BLUE);
        assert_eq!(app.stale_image_ids, vec![(0, preview_id)]);
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 1);
        assert!(!app.has_pending_work());
    }

    #[test]
    fn a_psd_without_a_thumbnail_gets_only_the_sips_decode() {
        let dir = test_dir();
        let path = dir.path().join("plain.psd");
        write_psd(&path, false);
        let mut app = grid_app(&path, &dir.path().join("cache"), Rect::new(0, 0, 20, 10));

        let first = wait_for_new_image(&mut app);
        assert!(
            app.thumb_cache.is_sharp(0),
            "the first image is the sharp one"
        );
        assert_eq!(first.dimensions(), (64, 32));
        assert!(app.stale_image_ids.is_empty());
        assert!(!app.has_pending_work(), "the failed preview freed its slot");
    }
}

#[cfg(test)]
mod folder_switch_tests {
    use crate::test_support::{app_with_files, test_dir, wait_until};
    use image::RgbaImage;
    use ratatui::layout::Rect;
    use std::fs;

    /// Decode the thumbnail of the cell `index` of `app` for `rect` and wait for it.
    fn decode_sharp(app: &mut crate::app::App, index: usize, rect: Rect) {
        app.spawn_thumb_decode(index, rect);
        wait_until(|| {
            app.poll_thumbnails();
            app.thumb_cache.is_sharp(index)
        });
    }

    /// A parked thumbnail comes back without a job, so its animation flag must survive the
    /// folder switch, or the grid marker goes away.
    #[test]
    #[cfg(feature = "video")]
    fn a_parked_animation_keeps_its_grid_marker_on_the_way_back() {
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
        assert!(app.is_animated_at(anim), "the job reads the flag");

        scan(&mut app, &other);
        scan(&mut app, &root);
        let anim = position(&app);
        app.spawn_thumb_decode(anim, rect);
        assert!(app.thumb_cache.is_sharp(anim), "the parked picture, no job");
        assert!(!app.thumbnails_pending());
        assert!(
            app.is_animated_at(anim),
            "the marker comes back with the picture"
        );
    }

    #[test]
    fn the_thumbnails_of_the_last_folder_come_back_without_a_decode() {
        let dir = test_dir();
        let root = fs::canonicalize(dir.path()).unwrap();
        let other = root.join("other");
        fs::create_dir(&other).unwrap();
        RgbaImage::new(4, 4).save(other.join("z.png")).unwrap();
        let mut app = app_with_files(&root, &["a.png", "b.png"]);
        // The disk cache lives outside the scanned tree, so it is no folder of the grid.
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
        // The scan lists the folders first, so the files come after them.
        let position = |app: &crate::app::App, name: &str| {
            app.filenames.iter().position(|n| n == name).unwrap()
        };
        let (a, b) = (position(&app, "a.png"), position(&app, "b.png"));
        // The grid decodes for its thumbnail box, and the park key names that box.
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

        // Back in the first folder, the cells take the parked pictures without a job.
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

        // A cell of another size cannot use a parked picture: it decodes.
        let larger = Rect::new(0, 0, rect.width + 1, rect.height);
        app.spawn_thumb_decode(b, larger);
        assert!(!app.thumb_cache.contains(b));
        assert!(app.thumbnails_pending());
        assert_eq!(app.thumb_cache.parked_count(), 1);
    }

    #[test]
    fn a_folder_switch_drops_the_waiting_and_the_running_jobs_at_once() {
        let dir = test_dir();
        let root = fs::canonicalize(dir.path()).unwrap();
        let names = ["a.png", "b.png", "c.png", "d.png", "e.png", "f.png"];
        let mut app = app_with_files(&root, &names);
        app.disk_cache_dir = root.join("cache");
        let rect = Rect::new(0, 0, 3, 2);
        for index in 0..names.len() {
            app.spawn_thumb_decode(index, rect);
        }
        assert_eq!(app.thumb_jobs.running.len(), 4);
        assert_eq!(app.thumb_jobs.waiting.len(), 2);
        let generation = app.thumb_generation;

        app.switch_to_dir(&root.join("missing"));
        assert!(app.thumb_jobs.running.is_empty() && app.thumb_jobs.waiting.is_empty());
        assert_ne!(app.thumb_generation, generation);
        assert_eq!(app.thumb_cache.parked_count(), 0, "nothing was sharp yet");
        // The results of the old jobs arrive within 2 s and are dropped.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            assert!(
                app.poll_thumbnails().is_empty(),
                "a stale thumbnail was stored"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!app.thumb_cache.contains(0));
    }
}
