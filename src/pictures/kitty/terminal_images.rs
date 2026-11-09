//! The images that the terminal stores for phewr, and the deletes that keep them in budget.

use super::protocol;
use lru::LruCache;
use std::collections::HashSet;
use std::io::{self, Write};

/// The RGBA bytes that phewr leaves in the terminal at most. Ghostty evicts at 320 MB, and herdr
/// stores each image twice.
pub(crate) const TERMINAL_IMAGE_BUDGET: usize = 128 * 1024 * 1024;

/// The images that the terminal stores, newest last, with their pixel bytes. A stored image
/// can be placed again by its id (`a=p`).
pub(crate) struct TerminalImages {
    stored: LruCache<u32, usize>,
    bytes: usize,
    /// A test sets a small budget.
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

    /// Record a transmitted image as the newest one.
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

    /// The pixel bytes of the stored image `id`, or 0.
    pub(crate) fn bytes_of(&self, id: u32) -> usize {
        self.stored.peek(&id).copied().unwrap_or(0)
    }

    /// Make `id` the newest image.
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

    /// The oldest ids that must go to fit the budget. An id in `keep` stays, and the search goes
    /// on past it, because a shown image can be older than a hidden one.
    fn over_budget(&mut self, keep: &HashSet<u32>) -> Vec<u32> {
        let mut excess = self.bytes.saturating_sub(self.budget);
        let mut dropped = Vec::new();
        for (&id, &bytes) in self.stored.iter().rev() {
            if excess == 0 {
                break;
            }
            if !keep.contains(&id) {
                excess = excess.saturating_sub(bytes);
                dropped.push(id);
            }
        }
        for &id in &dropped {
            self.remove(id);
        }
        dropped
    }
}

/// Delete the image `id` if the terminal stores it.
pub(crate) fn delete_image_to(
    images: &mut TerminalImages,
    out: &mut impl Write,
    id: u32,
) -> io::Result<()> {
    if images.contains(id) {
        protocol::delete_image_to(out, id)?;
        images.remove(id);
    }
    Ok(())
}

/// Delete the oldest hidden images until the stored bytes fit the budget.
pub(crate) fn trim_to_budget(
    images: &mut TerminalImages,
    out: &mut impl Write,
    shown: &HashSet<u32>,
) -> io::Result<()> {
    for id in images.over_budget(shown) {
        protocol::delete_image_to(out, id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::TerminalImages;

    #[test]
    fn counts_bytes_newest_last() {
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

    #[test]
    fn trim_skips_older_shown_image() {
        let mut images = TerminalImages::new();
        images.budget = 10;
        images.insert(1, 4);
        images.insert(2, 4);
        images.insert(3, 4);
        let keep = std::collections::HashSet::from([1]);
        assert_eq!(images.over_budget(&keep), vec![2]);
        assert!(images.contains(1));
        assert_eq!(images.bytes(), 8);
    }
}
