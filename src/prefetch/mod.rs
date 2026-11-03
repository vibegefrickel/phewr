//! The decode of the neighbors of the fullscreen image ahead of time.

use crate::decode::{EmbeddedThumbnail, ScreenImage, decode_image};
use crate::encoder::capped_box;
use crate::gallery::box_px;
use lru::LruCache;
use ratatui::layout::Rect;
use std::collections::{HashSet, VecDeque};
use std::num::NonZeroUsize;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

const CACHE_CAPACITY: NonZeroUsize = NonZeroUsize::new(8).unwrap();
const PREFETCH_RADIUS: usize = 3;
/// The nearest neighbors come first, and a decode that the user waits for keeps most cores.
const MAX_PREFETCH_JOBS: usize = 2;

type DecodeFn = Arc<dyn Fn(&Path, Option<(u32, u32)>) -> Option<Prefetched> + Send + Sync>;

/// `image` is `None` when the decode failed.
struct JobResult {
    generation: u64,
    index: usize,
    image: Option<Prefetched>,
}

/// `target` is the box of the fit, `None` the full size.
struct Prefetched {
    image: ScreenImage,
    target: Option<(u32, u32)>,
}

impl Prefetched {
    /// `None` if the decode used a smaller box. The caller then decodes again.
    fn fit_within(self, max: (u32, u32)) -> Option<ScreenImage> {
        // A fitting image only turns from `Opaque` into RGB here.
        if max.0 == 0 || max.1 == 0 || self.target == Some(max) {
            return Some(self.image.fit_within(max.0, max.1));
        }
        let covers = self
            .target
            .is_none_or(|target| target.0 >= max.0 && target.1 >= max.1);
        covers.then(|| self.image.fit_within(max.0, max.1))
    }
}

/// Counts a job from its spawn to its end, also when the decode panics.
struct RunningJob(Arc<AtomicUsize>);

impl RunningJob {
    fn start(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(counter))
    }
}

impl Drop for RunningJob {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn decode_for_screen(path: &Path, hint: Option<(u32, u32)>) -> Option<Prefetched> {
    let decoded = decode_image(path, hint, EmbeddedThumbnail::Deny).ok()?;
    Some(Prefetched {
        image: match hint {
            Some((max_w, max_h)) => decoded.fit_for_screen(max_w, max_h),
            None => decoded.into_screen(),
        },
        target: hint,
    })
}

pub(crate) struct Prefetcher {
    cache: LruCache<usize, Prefetched>,
    rx: mpsc::Receiver<JobResult>,
    tx: mpsc::Sender<JobResult>,
    /// At most `MAX_PREFETCH_JOBS`.
    pending: HashSet<usize>,
    /// The jobs on the rayon pool, also those that `invalidate` dropped.
    running: Arc<AtomicUsize>,
    decode: DecodeFn,
    /// Nearest first.
    queue: VecDeque<(usize, PathBuf)>,
    /// Skipped until the next `invalidate()`.
    failed: HashSet<usize>,
    /// `accept` drops the results of an older generation.
    generation: u64,
    target_hint: Option<(u32, u32)>,
    /// The position of the last `kick`, for the direction of the next one.
    last_current: Option<usize>,
    /// `Some(true)` is forward. A redraw at the same position keeps it.
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
            running: Arc::new(AtomicUsize::new(0)),
            decode: Arc::new(decode_for_screen),
            queue: VecDeque::new(),
            failed: HashSet::new(),
            generation: 0,
            target_hint: None,
            last_current: None,
            last_direction: None,
        }
    }

    pub(crate) fn set_target_hint(&mut self, rect: Rect, cell_px: (u32, u32)) {
        let (w, h) = capped_box(box_px(rect, cell_px));
        if w > 0 && h > 0 {
            self.target_hint = Some((w, h));
        }
    }

    pub(crate) fn poll(&mut self) {
        while let Ok(result) = self.rx.try_recv() {
            self.accept(result);
        }
        self.start_queued();
    }

    fn start_queued(&mut self) {
        while self.running.load(Ordering::SeqCst) < MAX_PREFETCH_JOBS
            && let Some((index, path)) = self.queue.pop_front()
        {
            self.start_decoding(index, path);
        }
    }

    fn accept(&mut self, result: JobResult) {
        if result.generation != self.generation {
            return;
        }
        self.pending.remove(&result.index);
        match result.image {
            Some(image) => {
                self.cache.push(result.index, image);
            }
            None => {
                self.failed.insert(result.index);
            }
        }
    }

    /// The decode hint can be missing (a gallery pre-decode) or stale (a resize, the info panel).
    /// The image never goes to the screen at its decoded size.
    pub(crate) fn take_resized(
        &mut self,
        index: usize,
        rect: Rect,
        cell_px: (u32, u32),
    ) -> Option<ScreenImage> {
        self.cache
            .pop(&index)?
            .fit_within(capped_box(box_px(rect, cell_px)))
    }

    /// Queue the neighbors of `current`, nearest first, in the direction of the last step. The
    /// entries before `first` are folders and never decode.
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

    /// A step back to the image that the screen showed needs no decode.
    pub(crate) fn store(
        &mut self,
        index: usize,
        image: ScreenImage,
        rect: Rect,
        cell_px: (u32, u32),
    ) {
        let target = capped_box(box_px(rect, cell_px));
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
        if self.running.load(Ordering::SeqCst) >= MAX_PREFETCH_JOBS {
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
        let decode = Arc::clone(&self.decode);
        let running = RunningJob::start(&self.running);
        rayon::spawn(move || {
            let image = decode(&path, hint);
            drop(running);
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a closed receiver means that nobody waits for the result"
            )]
            let _ = tx.send(JobResult {
                generation,
                index,
                image,
            });
        });
    }

    pub(crate) fn has_jobs(&self) -> bool {
        !self.pending.is_empty() || !self.queue.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn pending_count(&self) -> usize {
        self.pending.len() + self.queue.len()
    }

    #[cfg(test)]
    fn pending_jobs(&self) -> usize {
        self.pending.len()
    }

    /// A test does not depend on the timing of rayon.
    #[cfg(test)]
    fn accept_next(&mut self, timeout: std::time::Duration) -> bool {
        match self.rx.recv_timeout(timeout) {
            Ok(result) => {
                self.accept(result);
                true
            }
            Err(_) => false,
        }
    }
}

/// `Some(true)` is a step forward: the next images first. `None` alternates.
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
mod tests;
