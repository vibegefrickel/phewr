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
}
