//! The images that the terminal stores for viewr, with their bytes, and the deletes that keep
//! them below the budget.

use crate::app::App;
use crate::encoder::kitty;
use lru::LruCache;
use std::collections::HashSet;
use std::io::{self, Write};

/// The bytes of RGBA pixels that viewr leaves in the storage of the terminal, at most. Ghostty
/// evicts images at 320 MB, and herdr stores each image twice, so the budget stays well below
/// half of that. Above the budget, the grid deletes its oldest images that no cell shows.
pub(crate) const TERMINAL_IMAGE_BUDGET: usize = 128 * 1024 * 1024;

/// The images that viewr transmitted and the terminal still stores, newest last, with their
/// pixel bytes. A stored image can be placed again by its id (`a=p`) instead of a transmit.
pub(crate) struct TerminalImages {
    stored: LruCache<u32, usize>,
    bytes: usize,
    /// The bytes that the stored images may take. A test sets a small one.
    pub(crate) budget: usize,
}

impl TerminalImages {
    pub(crate) fn new() -> Self {
        Self {
            stored: LruCache::unbounded(),
            bytes: 0,
            budget: TERMINAL_IMAGE_BUDGET,
        }
    }

    /// Record a transmitted image of `bytes` pixel bytes as the newest one.
    pub(crate) fn insert(&mut self, id: u32, bytes: usize) {
        self.remove(id);
        self.stored.put(id, bytes);
        self.bytes += bytes;
    }

    pub(crate) fn remove(&mut self, id: u32) {
        if let Some(bytes) = self.stored.pop(&id) {
            self.bytes -= bytes;
        }
    }

    pub(crate) fn contains(&self, id: u32) -> bool {
        self.stored.contains(&id)
    }

    /// The pixel bytes of the stored image `id`, 0 for an image that the terminal does not store.
    pub(crate) fn bytes_of(&self, id: u32) -> usize {
        self.stored.peek(&id).copied().unwrap_or(0)
    }

    /// Make `id` the newest image, because a cell shows it again.
    pub(crate) fn touch(&mut self, id: u32) {
        self.stored.promote(&id);
    }

    pub(crate) fn clear(&mut self) {
        self.stored.clear();
        self.bytes = 0;
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.stored.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.stored.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    /// The oldest ids that must go so that the rest fits the budget. The ids in `keep` stay,
    /// because a cell shows them. They are the newest ones, so the search stops at the first.
    fn over_budget(&mut self, keep: &HashSet<u32>) -> Vec<u32> {
        let mut dropped = Vec::new();
        while self.bytes > self.budget {
            let Some((&id, _)) = self.stored.peek_lru() else {
                break;
            };
            if keep.contains(&id) {
                break;
            }
            self.remove(id);
            dropped.push(id);
        }
        dropped
    }
}

/// Delete the image `id` in the terminal, if viewr transmitted it, and forget it.
pub(super) fn delete_image_to(app: &mut App, out: &mut impl Write, id: u32) -> io::Result<()> {
    if app.terminal_images.contains(id) {
        kitty::delete_image_to(out, id)?;
        app.terminal_images.remove(id);
    }
    Ok(())
}

/// Delete the oldest images that no visible cell shows, until the stored bytes fit the budget.
/// A cell that shows one of them later transmits its thumbnail again from the cache.
pub(super) fn trim_to_budget(
    app: &mut App,
    out: &mut impl Write,
    shown: &HashSet<u32>,
) -> io::Result<()> {
    for id in app.terminal_images.over_budget(shown) {
        kitty::delete_image_to(out, id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::TerminalImages;

    #[test]
    fn the_terminal_images_count_their_bytes_newest_last() {
        let mut images = TerminalImages::new();
        assert_eq!(images.budget, super::TERMINAL_IMAGE_BUDGET);
        images.budget = 10;
        images.insert(1, 4);
        images.insert(2, 4);
        images.insert(3, 4);
        assert_eq!((images.len(), images.bytes()), (3, 12));
        images.touch(1);
        let keep = std::collections::HashSet::from([3]);
        assert_eq!(
            images.over_budget(&keep),
            vec![2],
            "the oldest one that no cell shows"
        );
        assert_eq!(images.bytes(), 8);
        images.insert(1, 6);
        assert_eq!(
            (images.len(), images.bytes()),
            (2, 10),
            "a second insert replaces"
        );
        images.remove(9);
        images.clear();
        assert!(images.is_empty() && images.bytes() == 0);
    }
}
