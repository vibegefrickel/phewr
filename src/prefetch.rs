use crate::app::{EmbeddedThumbnail, decode_image_with_hint, resize_decoded_to_dims};
use image::RgbaImage;
use lru::LruCache;
use ratatui::layout::Rect;
use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::mpsc;

const CACHE_CAPACITY: NonZeroUsize = NonZeroUsize::new(8).unwrap();
const PREFETCH_RADIUS: usize = 3;

/// The result of one decode job. `image` is `None` when the decode failed.
struct Decoded {
    generation: u64,
    index: usize,
    image: Option<RgbaImage>,
}

pub(crate) struct Prefetcher {
    cache: LruCache<usize, RgbaImage>,
    rx: mpsc::Receiver<Decoded>,
    tx: mpsc::Sender<Decoded>,
    pending: HashSet<usize>,
    /// Indices that did not decode. They stay skipped until the next `invalidate()`.
    failed: HashSet<usize>,
    /// Incremented by `invalidate()`, so that results of older jobs are dropped.
    generation: u64,
    target_hint: Option<(u32, u32)>,
}

impl Prefetcher {
    pub(crate) fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            cache: LruCache::new(CACHE_CAPACITY),
            rx,
            tx,
            pending: HashSet::new(),
            failed: HashSet::new(),
            generation: 0,
            target_hint: None,
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

    pub(crate) fn take_resized(
        &mut self,
        index: usize,
        _rect: Rect,
        _cell_px: (u32, u32),
    ) -> Option<RgbaImage> {
        self.cache.pop(&index)
    }

    pub(crate) fn kick(&mut self, current: usize, images: &[PathBuf]) {
        for offset in 1..=PREFETCH_RADIUS {
            if current + offset < images.len() {
                self.ensure_decoding(current + offset, images);
            }
            if current >= offset {
                self.ensure_decoding(current - offset, images);
            }
        }
    }

    pub(crate) fn kick_gallery(&mut self, index: usize, images: &[PathBuf]) {
        self.ensure_decoding(index, images);
    }

    pub(crate) fn invalidate(&mut self) {
        self.cache.clear();
        self.pending.clear();
        self.failed.clear();
        self.generation = self.generation.wrapping_add(1);
    }

    fn ensure_decoding(&mut self, index: usize, images: &[PathBuf]) {
        if self.cache.contains(&index)
            || self.pending.contains(&index)
            || self.failed.contains(&index)
        {
            return;
        }
        self.pending.insert(index);
        let tx = self.tx.clone();
        let path = images[index].clone();
        let hint = self.target_hint;
        let generation = self.generation;
        rayon::spawn(move || {
            let image = decode_image_with_hint(&path, hint, EmbeddedThumbnail::Deny)
                .ok()
                .map(|img| match hint {
                    Some((max_w, max_h)) => resize_decoded_to_dims(&img, max_w, max_h),
                    None => img.to_rgba8(),
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

    #[cfg(test)]
    pub(crate) fn pending_count(&self) -> usize {
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
        prefetcher.kick(2, &paths);
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
        prefetcher.kick(1, &paths);
        wait_until_idle(&mut prefetcher);
        prefetcher.kick(1, &paths);
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
}
