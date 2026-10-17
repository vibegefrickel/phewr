use crate::app::{EmbeddedThumbnail, decode_image_with_hint, resize_decoded_to_dims};
use image::RgbaImage;
use lru::LruCache;
use ratatui::layout::Rect;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::mpsc;

const CACHE_CAPACITY: NonZeroUsize = NonZeroUsize::new(8).unwrap();
const PREFETCH_RADIUS: usize = 3;

pub(crate) struct Prefetcher {
    cache: LruCache<usize, RgbaImage>,
    rx: mpsc::Receiver<(usize, RgbaImage)>,
    tx: mpsc::Sender<(usize, RgbaImage)>,
    pending: std::collections::HashSet<usize>,
    target_hint: Option<(u32, u32)>,
}

impl Prefetcher {
    pub(crate) fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            cache: LruCache::new(CACHE_CAPACITY),
            rx,
            tx,
            pending: std::collections::HashSet::new(),
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
        while let Ok((idx, img)) = self.rx.try_recv() {
            self.pending.remove(&idx);
            self.cache.push(idx, img);
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
    }

    fn ensure_decoding(&mut self, index: usize, images: &[PathBuf]) {
        if self.cache.contains(&index) || self.pending.contains(&index) {
            return;
        }
        self.pending.insert(index);
        let tx = self.tx.clone();
        let path = images[index].clone();
        let hint = self.target_hint;
        rayon::spawn(move || {
            if let Ok(img) = decode_image_with_hint(&path, hint, EmbeddedThumbnail::Deny) {
                let final_img = if let Some((max_w, max_h)) = hint {
                    resize_decoded_to_dims(&img, max_w, max_h)
                } else {
                    img.to_rgba8()
                };
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "a closed receiver means that nobody waits for the result"
                )]
                let _ = tx.send((index, final_img));
            }
        });
    }

    #[cfg(test)]
    pub(crate) fn pending_count(&self) -> usize {
        self.pending.len()
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

    fn synthetic_dir(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("viewr-prefetch-test-{name}-{}", std::process::id()));
        if root.exists() {
            fs::remove_dir_all(&root).unwrap();
        }
        fs::create_dir_all(&root).unwrap();
        root
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
        let dir = synthetic_dir("neighbors");
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

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn prefetch_gallery_decodes_selected() {
        let dir = synthetic_dir("gallery");
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

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn prefetch_zero_hint_keeps_original_size() {
        let dir = synthetic_dir("no-hint");
        let paths = vec![save_image(&dir, "0.png", 8, 8)];

        let mut prefetcher = Prefetcher::new();
        prefetcher.set_target_hint(Rect::new(0, 0, 0, 5), (1, 1));
        prefetcher.kick_gallery(0, &paths);
        wait_until_idle(&mut prefetcher);

        let img = prefetcher
            .take_resized(0, Rect::default(), (1, 1))
            .expect("image should be cached");
        assert_eq!(img.dimensions(), (8, 8));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn prefetch_invalidate_clears_cache() {
        let dir = synthetic_dir("invalidate");
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

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn prefetch_lru_evicts_oldest() {
        let dir = synthetic_dir("lru");
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

        fs::remove_dir_all(dir).unwrap();
    }
}
