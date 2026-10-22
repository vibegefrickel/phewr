use crate::decode::resize::fit_rgba_within;
use crate::decode::{EmbeddedThumbnail, decode_image};
use image::RgbaImage;
use lru::LruCache;
use ratatui::layout::Rect;
use std::collections::{HashSet, VecDeque};
use std::num::NonZeroUsize;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::mpsc;

const CACHE_CAPACITY: NonZeroUsize = NonZeroUsize::new(8).unwrap();
const PREFETCH_RADIUS: usize = 3;
/// The prefetch decodes that run at once. The nearest neighbors come first, and a decode that
/// the user waits for keeps most of the cores.
const MAX_PREFETCH_JOBS: usize = 2;

/// The result of one decode job. `image` is `None` when the decode failed.
struct Decoded {
    generation: u64,
    index: usize,
    image: Option<Prefetched>,
}

/// A decoded image and the box in pixels that it was fitted into. `None` is the full size.
struct Prefetched {
    image: RgbaImage,
    target: Option<(u32, u32)>,
}

impl Prefetched {
    /// The image fitted into `max`, or `None` if the decode used a smaller box. A smaller decode
    /// cannot fill `max` without an upscale, so the caller decodes again.
    fn fit_into(self, max: (u32, u32)) -> Option<RgbaImage> {
        if max.0 == 0 || max.1 == 0 || self.target == Some(max) {
            return Some(self.image);
        }
        let covers = self
            .target
            .is_none_or(|target| target.0 >= max.0 && target.1 >= max.1);
        covers.then(|| fit_rgba_within(self.image, max.0, max.1))
    }
}

pub(crate) struct Prefetcher {
    cache: LruCache<usize, Prefetched>,
    rx: mpsc::Receiver<Decoded>,
    tx: mpsc::Sender<Decoded>,
    /// The indices that decode right now, at most `MAX_PREFETCH_JOBS`.
    pending: HashSet<usize>,
    /// The indices that wait for a free job, nearest first.
    queue: VecDeque<(usize, PathBuf)>,
    /// Indices that did not decode. They stay skipped until the next `invalidate()`.
    failed: HashSet<usize>,
    /// Incremented by `invalidate()`, so that results of older jobs are dropped.
    generation: u64,
    target_hint: Option<(u32, u32)>,
    /// The position of the last `kick`, so that the next one knows the direction.
    last_current: Option<usize>,
    /// The direction of the last step: `Some(true)` is forward. A kick at the same position
    /// (a redraw) keeps it.
    last_direction: Option<bool>,
}

impl Prefetcher {
    pub(crate) fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            cache: LruCache::new(CACHE_CAPACITY),
            rx,
            tx,
            pending: HashSet::new(),
            queue: VecDeque::new(),
            failed: HashSet::new(),
            generation: 0,
            target_hint: None,
            last_current: None,
            last_direction: None,
        }
    }

    pub(crate) fn set_target_hint(&mut self, rect: Rect, cell_px: (u32, u32)) {
        let w = u32::from(rect.width) * cell_px.0;
        let h = u32::from(rect.height) * cell_px.1;
        if w > 0 && h > 0 {
            self.target_hint = Some((w, h));
        }
    }

    pub(crate) fn poll(&mut self) {
        while let Ok(decoded) = self.rx.try_recv() {
            self.accept(decoded);
        }
        self.start_queued();
    }

    /// Fill the free jobs from the queue.
    fn start_queued(&mut self) {
        while self.pending.len() < MAX_PREFETCH_JOBS
            && let Some((index, path)) = self.queue.pop_front()
        {
            self.start_decoding(index, path);
        }
    }

    /// Store one job result, unless an `invalidate()` made its index meaningless.
    fn accept(&mut self, decoded: Decoded) {
        if decoded.generation != self.generation {
            return;
        }
        self.pending.remove(&decoded.index);
        match decoded.image {
            Some(image) => {
                self.cache.push(decoded.index, image);
            }
            None => {
                self.failed.insert(decoded.index);
            }
        }
    }

    /// Take the cached image of `index`, fitted into `rect`. The hint of the decode can be
    /// missing (a gallery pre-decode before the first fullscreen view) or stale (a resize, the
    /// info panel), so the image must not go to the screen at its decoded size.
    pub(crate) fn take_resized(
        &mut self,
        index: usize,
        rect: Rect,
        cell_px: (u32, u32),
    ) -> Option<RgbaImage> {
        let max = (
            u32::from(rect.width) * cell_px.0,
            u32::from(rect.height) * cell_px.1,
        );
        self.cache.pop(&index)?.fit_into(max)
    }

    /// Queue the neighbors of `current`. After a step, the neighbors in the direction of the
    /// step come first, nearest first. The queue of an older position goes.
    /// Decode the neighbors of `current` within `first..images.len()`. The entries before
    /// `first` are folders, and nothing decodes a folder.
    pub(crate) fn kick(&mut self, current: usize, first: usize, images: &[PathBuf]) {
        self.queue.clear();
        if self.last_current != Some(current) {
            self.last_direction = self.last_current.map(|last| current > last);
            self.last_current = Some(current);
        }
        for index in neighbor_order(current, first..images.len(), self.last_direction) {
            self.ensure_decoding(index, images);
        }
    }

    /// Keep the image of `index` that the screen showed, so that a step back needs no decode.
    pub(crate) fn store(
        &mut self,
        index: usize,
        image: RgbaImage,
        rect: Rect,
        cell_px: (u32, u32),
    ) {
        let target = (
            u32::from(rect.width) * cell_px.0,
            u32::from(rect.height) * cell_px.1,
        );
        self.cache.push(
            index,
            Prefetched {
                image,
                target: Some(target),
            },
        );
    }

    pub(crate) fn kick_gallery(&mut self, index: usize, images: &[PathBuf]) {
        self.queue.clear();
        self.ensure_decoding(index, images);
    }

    pub(crate) fn invalidate(&mut self) {
        self.cache.clear();
        self.pending.clear();
        self.queue.clear();
        self.failed.clear();
        self.generation = self.generation.wrapping_add(1);
    }

    fn ensure_decoding(&mut self, index: usize, images: &[PathBuf]) {
        if self.cache.contains(&index)
            || self.pending.contains(&index)
            || self.failed.contains(&index)
            || self.queue.iter().any(|job| job.0 == index)
        {
            return;
        }
        if self.pending.len() >= MAX_PREFETCH_JOBS {
            self.queue.push_back((index, images[index].clone()));
            return;
        }
        self.start_decoding(index, images[index].clone());
    }

    fn start_decoding(&mut self, index: usize, path: PathBuf) {
        self.pending.insert(index);
        let tx = self.tx.clone();
        let hint = self.target_hint;
        let generation = self.generation;
        rayon::spawn(move || {
            let image = decode_image(&path, hint, EmbeddedThumbnail::Deny)
                .ok()
                .map(|decoded| Prefetched {
                    image: match hint {
                        Some((max_w, max_h)) => decoded.fit_within(max_w, max_h),
                        None => decoded.into_display().into_rgba8(),
                    },
                    target: hint,
                });
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a closed receiver means that nobody waits for the result"
            )]
            let _ = tx.send(Decoded {
                generation,
                index,
                image,
            });
        });
    }

    /// True while a job runs or waits.
    pub(crate) fn has_jobs(&self) -> bool {
        !self.pending.is_empty() || !self.queue.is_empty()
    }

    /// The jobs that run or wait.
    #[cfg(test)]
    pub(crate) fn pending_count(&self) -> usize {
        self.pending.len() + self.queue.len()
    }

    #[cfg(test)]
    fn running_count(&self) -> usize {
        self.pending.len()
    }

    /// Wait for one job result, so that a test does not depend on the timing of rayon.
    #[cfg(test)]
    fn accept_next(&mut self, timeout: std::time::Duration) -> bool {
        match self.rx.recv_timeout(timeout) {
            Ok(decoded) => {
                self.accept(decoded);
                true
            }
            Err(_) => false,
        }
    }
}

/// The neighbors of `current` within `PREFETCH_RADIUS` and `files`. `Some(true)` is a step
/// forward: the next images first, then the previous ones. `None` alternates.
fn neighbor_order(current: usize, files: Range<usize>, direction: Option<bool>) -> Vec<usize> {
    let next = (1..=PREFETCH_RADIUS)
        .map(|offset| current + offset)
        .filter(|index| files.contains(index));
    let previous = (1..=PREFETCH_RADIUS)
        .filter_map(|offset| current.checked_sub(offset))
        .filter(|index| files.contains(index));
    match direction {
        Some(true) => next.chain(previous).collect(),
        Some(false) => previous.chain(next).collect(),
        None => (1..=PREFETCH_RADIUS)
            .flat_map(|offset| [Some(current + offset), current.checked_sub(offset)])
            .flatten()
            .filter(|index| files.contains(index))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::Prefetcher;
    use image::RgbaImage;
    use ratatui::layout::Rect;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Duration;
    use tempfile::TempDir;

    /// A temp directory and its path; the directory goes away with the `TempDir`.
    fn synthetic_dir() -> (TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        (temp, root)
    }

    fn save_image(dir: &Path, name: &str, width: u32, height: u32) -> PathBuf {
        let path = dir.join(name);
        RgbaImage::new(width, height).save(&path).unwrap();
        path
    }

    fn wait_until_idle(prefetcher: &mut Prefetcher) {
        for _ in 0..200 {
            prefetcher.poll();
            if prefetcher.pending_count() == 0 {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("prefetch jobs did not finish in time");
    }

    #[test]
    fn prefetch_decodes_neighbors_async() {
        let (_temp, dir) = synthetic_dir();
        let paths: Vec<_> = (0..5)
            .map(|i| save_image(&dir, &format!("{i}.png"), 8, 8))
            .collect();

        let mut prefetcher = Prefetcher::new();
        prefetcher.set_target_hint(Rect::new(0, 0, 2, 2), (2, 2));
        prefetcher.kick(2, 0, &paths);
        wait_until_idle(&mut prefetcher);

        for i in [0, 1, 3, 4] {
            let img = prefetcher
                .take_resized(i, Rect::default(), (1, 1))
                .expect("neighbor should be cached");
            assert_eq!(img.dimensions(), (4, 4));
        }
        assert!(
            prefetcher
                .take_resized(2, Rect::default(), (1, 1))
                .is_none()
        );
    }

    #[test]
    fn prefetch_gallery_decodes_selected() {
        let (_temp, dir) = synthetic_dir();
        let paths: Vec<_> = (0..3)
            .map(|i| save_image(&dir, &format!("{i}.png"), 4, 4))
            .collect();

        let mut prefetcher = Prefetcher::new();
        prefetcher.kick_gallery(1, &paths);
        wait_until_idle(&mut prefetcher);

        let img = prefetcher
            .take_resized(1, Rect::default(), (1, 1))
            .expect("gallery target should be cached");
        assert_eq!(img.dimensions(), (4, 4));
    }

    #[test]
    fn prefetch_zero_hint_keeps_original_size() {
        let (_temp, dir) = synthetic_dir();
        let paths = vec![save_image(&dir, "0.png", 8, 8)];

        let mut prefetcher = Prefetcher::new();
        prefetcher.set_target_hint(Rect::new(0, 0, 0, 5), (1, 1));
        prefetcher.kick_gallery(0, &paths);
        wait_until_idle(&mut prefetcher);

        let img = prefetcher
            .take_resized(0, Rect::default(), (1, 1))
            .expect("image should be cached");
        assert_eq!(img.dimensions(), (8, 8));
    }

    #[test]
    fn prefetch_invalidate_clears_cache() {
        let (_temp, dir) = synthetic_dir();
        let paths: Vec<_> = (0..3)
            .map(|i| save_image(&dir, &format!("{i}.png"), 4, 4))
            .collect();

        let mut prefetcher = Prefetcher::new();
        prefetcher.kick(1, 0, &paths);
        wait_until_idle(&mut prefetcher);
        prefetcher.kick(1, 0, &paths);
        wait_until_idle(&mut prefetcher);
        prefetcher.invalidate();

        for i in 0..3 {
            assert!(
                prefetcher
                    .take_resized(i, Rect::default(), (1, 1))
                    .is_none()
            );
        }
    }

    #[test]
    fn prefetch_lru_evicts_oldest() {
        let (_temp, dir) = synthetic_dir();
        let paths: Vec<_> = (0..10)
            .map(|i| save_image(&dir, &format!("{i}.png"), 4, 4))
            .collect();

        let mut prefetcher = Prefetcher::new();
        for i in 0..10 {
            prefetcher.kick_gallery(i, &paths);
            wait_until_idle(&mut prefetcher);
        }

        assert!(
            prefetcher
                .take_resized(0, Rect::default(), (1, 1))
                .is_none()
        );
        assert!(
            prefetcher
                .take_resized(1, Rect::default(), (1, 1))
                .is_none()
        );
        for i in 2..10 {
            assert!(
                prefetcher
                    .take_resized(i, Rect::default(), (1, 1))
                    .is_some()
            );
        }
    }

    #[test]
    fn prefetch_drops_a_result_that_finishes_after_invalidate() {
        let (_temp, dir) = synthetic_dir();
        let paths = vec![save_image(&dir, "0.png", 4, 4)];

        let mut prefetcher = Prefetcher::new();
        prefetcher.kick_gallery(0, &paths);
        // The job of the old list is in flight: its result is not polled yet.
        prefetcher.invalidate();
        assert!(
            prefetcher.accept_next(Duration::from_secs(10)),
            "the stale job did not send its result in time"
        );

        assert_eq!(prefetcher.pending_count(), 0);
        assert!(
            prefetcher
                .take_resized(0, Rect::default(), (1, 1))
                .is_none(),
            "a result of an older generation must not enter the cache"
        );

        prefetcher.kick_gallery(0, &paths);
        assert_eq!(prefetcher.pending_count(), 1, "the new list decodes again");
        wait_until_idle(&mut prefetcher);
        assert!(
            prefetcher
                .take_resized(0, Rect::default(), (1, 1))
                .is_some()
        );
    }

    #[test]
    fn prefetch_remembers_a_failed_decode_until_invalidate() {
        let (_temp, dir) = synthetic_dir();
        let path = dir.join("0.png");
        fs::write(&path, b"this is not a PNG file").unwrap();
        let paths = vec![path];

        let mut prefetcher = Prefetcher::new();
        prefetcher.kick_gallery(0, &paths);
        wait_until_idle(&mut prefetcher);
        assert!(
            prefetcher
                .take_resized(0, Rect::default(), (1, 1))
                .is_none()
        );

        prefetcher.kick_gallery(0, &paths);
        assert_eq!(
            prefetcher.pending_count(),
            0,
            "a corrupt file must not decode again on each kick"
        );

        prefetcher.invalidate();
        prefetcher.kick_gallery(0, &paths);
        assert_eq!(prefetcher.pending_count(), 1, "invalidate allows a new try");
        wait_until_idle(&mut prefetcher);
    }

    #[test]
    fn a_decode_without_a_hint_is_fitted_into_the_box() {
        let (_temp, dir) = synthetic_dir();
        let paths = vec![save_image(&dir, "0.png", 64, 32)];

        let mut prefetcher = Prefetcher::new();
        prefetcher.kick_gallery(0, &paths);
        wait_until_idle(&mut prefetcher);

        let img = prefetcher
            .take_resized(0, Rect::new(0, 0, 2, 2), (8, 8))
            .expect("image should be cached");
        assert_eq!(img.dimensions(), (16, 8));
    }

    #[test]
    fn the_neighbor_order_follows_the_direction_of_the_step() {
        use super::neighbor_order;
        assert_eq!(neighbor_order(4, 0..9, None), vec![5, 3, 6, 2, 7, 1]);
        assert_eq!(neighbor_order(4, 0..9, Some(true)), vec![5, 6, 7, 3, 2, 1]);
        assert_eq!(neighbor_order(4, 0..9, Some(false)), vec![3, 2, 1, 5, 6, 7]);
        assert_eq!(neighbor_order(0, 0..2, None), vec![1]);
        assert_eq!(neighbor_order(1, 0..2, Some(true)), vec![0]);
        assert!(neighbor_order(0, 0..1, Some(false)).is_empty());
        // The folders before the first file are never a neighbor.
        assert_eq!(neighbor_order(3, 2..6, None), vec![4, 2, 5]);
        assert_eq!(neighbor_order(2, 2..4, Some(false)), vec![3]);
    }

    #[test]
    fn a_step_forward_queues_the_next_images_first() {
        let (_temp, dir) = synthetic_dir();
        let paths: Vec<_> = (0..12)
            .map(|i| save_image(&dir, &format!("{i}.png"), 4, 4))
            .collect();

        let mut prefetcher = Prefetcher::new();
        prefetcher.kick(4, 0, &paths);
        assert_eq!(prefetcher.running_count(), 2, "5 and 3 run");

        // Only `poll` frees a job, so the next kicks queue everything that does not run.
        prefetcher.kick(5, 0, &paths);
        let waiting: Vec<usize> = prefetcher.queue.iter().map(|job| job.0).collect();
        assert_eq!(
            waiting,
            vec![6, 7, 8, 4, 2],
            "forward: the next images first"
        );

        prefetcher.kick(4, 0, &paths);
        let waiting: Vec<usize> = prefetcher.queue.iter().map(|job| job.0).collect();
        assert_eq!(waiting, vec![2, 1, 6, 7], "back: the previous images first");

        // A redraw kicks the same position again and keeps the direction of the step.
        prefetcher.kick(4, 0, &paths);
        let waiting: Vec<usize> = prefetcher.queue.iter().map(|job| job.0).collect();
        assert_eq!(
            waiting,
            vec![2, 1, 6, 7],
            "the same position keeps the order"
        );
        assert!(prefetcher.has_jobs());
        wait_until_idle(&mut prefetcher);
        assert!(!prefetcher.has_jobs());
    }

    #[test]
    fn a_stored_image_comes_back_for_its_box() {
        let mut prefetcher = Prefetcher::new();
        let rect = Rect::new(0, 0, 4, 2);
        prefetcher.store(3, RgbaImage::new(16, 8), rect, (4, 4));
        assert!(
            prefetcher
                .take_resized(3, Rect::new(0, 0, 8, 4), (4, 4))
                .is_none(),
            "a larger box needs a new decode"
        );
        prefetcher.store(3, RgbaImage::new(16, 8), rect, (4, 4));
        let image = prefetcher.take_resized(3, rect, (4, 4)).unwrap();
        assert_eq!(image.dimensions(), (16, 8));
        assert!(prefetcher.take_resized(3, rect, (4, 4)).is_none());
    }

    #[test]
    fn at_most_two_prefetch_jobs_run_and_the_rest_wait_nearest_first() {
        let (_temp, dir) = synthetic_dir();
        let paths: Vec<_> = (0..9)
            .map(|i| save_image(&dir, &format!("{i}.png"), 4, 4))
            .collect();

        let mut prefetcher = Prefetcher::new();
        prefetcher.kick(4, 0, &paths);
        assert_eq!(
            prefetcher.running_count(),
            2,
            "the two nearest neighbors decode"
        );
        assert_eq!(prefetcher.pending_count(), 6);
        let waiting: Vec<usize> = prefetcher.queue.iter().map(|job| job.0).collect();
        assert_eq!(waiting, vec![6, 2, 7, 1], "the others wait, nearest first");

        // A second kick at the same position changes nothing.
        prefetcher.kick(4, 0, &paths);
        assert_eq!(prefetcher.pending_count(), 6);

        wait_until_idle(&mut prefetcher);
        for i in [1, 2, 3, 5, 6, 7] {
            assert!(
                prefetcher
                    .take_resized(i, Rect::default(), (1, 1))
                    .is_some(),
                "neighbor {i} should be cached"
            );
        }

        // A kick at another position drops the queue of the old one.
        let mut prefetcher = Prefetcher::new();
        prefetcher.kick(4, 0, &paths);
        prefetcher.kick_gallery(8, &paths);
        let waiting: Vec<usize> = prefetcher.queue.iter().map(|job| job.0).collect();
        assert_eq!(waiting, vec![8]);
        assert_eq!(prefetcher.running_count(), 2);
        wait_until_idle(&mut prefetcher);
    }

    #[test]
    fn a_decode_for_a_smaller_box_is_not_used() {
        let (_temp, dir) = synthetic_dir();
        let paths = vec![
            save_image(&dir, "0.png", 64, 32),
            save_image(&dir, "1.png", 64, 32),
        ];

        let mut prefetcher = Prefetcher::new();
        prefetcher.set_target_hint(Rect::new(0, 0, 2, 2), (8, 8));
        prefetcher.kick_gallery(0, &paths);
        prefetcher.kick_gallery(1, &paths);
        wait_until_idle(&mut prefetcher);

        assert!(
            prefetcher
                .take_resized(0, Rect::new(0, 0, 4, 4), (8, 8))
                .is_none(),
            "a 16x8 decode must not fill a 32x32 box"
        );
        let img = prefetcher
            .take_resized(1, Rect::new(0, 0, 1, 1), (8, 8))
            .expect("a larger decode fits a smaller box");
        assert_eq!(img.dimensions(), (8, 4));
    }
}
