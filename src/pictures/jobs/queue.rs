//! The queues of the thumbnail jobs and the slots of each phase.

use ratatui::layout::Rect;
use std::collections::{HashMap, HashSet, VecDeque};

/// A thumbnail job: the image index and its grid cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ThumbJob {
    pub(crate) index: usize,
    pub(crate) rect: Rect,
}

/// The waiting jobs of one phase in grid order, with a count per image index.
///
/// `holds` runs per visible cell per draw, and the membership test must not scan the queue.
#[derive(Debug, Default)]
pub(crate) struct JobQueue {
    jobs: VecDeque<ThumbJob>,
    members: HashMap<usize, usize>,
}

impl JobQueue {
    pub(crate) fn push_back(&mut self, job: ThumbJob) {
        *self.members.entry(job.index).or_default() += 1;
        self.jobs.push_back(job);
    }

    pub(crate) fn pop_front(&mut self) -> Option<ThumbJob> {
        let job = self.jobs.pop_front()?;
        self.forget(job.index);
        Some(job)
    }

    pub(crate) fn remove(&mut self, position: usize) -> Option<ThumbJob> {
        let job = self.jobs.remove(position)?;
        self.forget(job.index);
        Some(job)
    }

    /// Drop every waiting job of the image `index`.
    pub(crate) fn remove_index(&mut self, index: usize) {
        if self.members.remove(&index).is_some() {
            self.jobs.retain(|job| job.index != index);
        }
    }

    pub(crate) fn contains(&self, index: usize) -> bool {
        self.members.contains_key(&index)
    }

    pub(crate) fn iter(&self) -> std::collections::vec_deque::Iter<'_, ThumbJob> {
        self.jobs.iter()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.jobs.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.jobs.clear();
        self.members.clear();
    }

    fn forget(&mut self, index: usize) {
        if let Some(count) = self.members.get_mut(&index) {
            *count -= 1;
            if *count == 0 {
                self.members.remove(&index);
            }
        }
    }
}

#[cfg(test)]
impl<const N: usize> PartialEq<[ThumbJob; N]> for JobQueue {
    fn eq(&self, other: &[ThumbJob; N]) -> bool {
        self.jobs == *other
    }
}

/// The slots of one decode phase: at most `limit` running jobs, and the waiting jobs.
pub(crate) struct JobSlots {
    pub(crate) running: HashSet<usize>,
    pub(crate) waiting: JobQueue,
    limit: usize,
}

impl JobSlots {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            running: HashSet::new(),
            waiting: JobQueue::default(),
            limit,
        }
    }

    pub(crate) fn has_free_slot(&self) -> bool {
        self.running.len() < self.limit
    }

    pub(crate) fn holds(&self, index: usize) -> bool {
        self.running.contains(&index) || self.waiting.contains(index)
    }

    /// The running and waiting images as one set, for many lookups.
    pub(crate) fn indices(&self) -> HashSet<usize> {
        let waiting = self.waiting.members.keys().copied();
        self.running.iter().copied().chain(waiting).collect()
    }

    pub(crate) fn is_idle(&self) -> bool {
        self.running.is_empty() && self.waiting.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.running.clear();
        self.waiting.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{JobQueue, ThumbJob};
    use ratatui::layout::Rect;

    /// 3x2 cells of 8x16 px: a box of 24x32 px.
    const RECT: Rect = Rect::new(0, 0, 3, 2);

    #[test]
    fn queue_tracks_members_and_order() {
        let job = |index| ThumbJob { index, rect: RECT };
        let mut queue = JobQueue::default();
        for index in [3, 1, 4, 1, 5] {
            queue.push_back(job(index));
        }
        assert!([3, 1, 4, 5].into_iter().all(|index| queue.contains(index)));
        assert!(!queue.contains(2));

        assert_eq!(queue.pop_front(), Some(job(3)));
        assert!(!queue.contains(3));
        assert_eq!(queue.remove(0), Some(job(1)));
        assert!(queue.contains(1), "the second job of 1 still waits");
        queue.remove_index(1);
        assert!(!queue.contains(1));
        assert_eq!(queue, [job(4), job(5)]);

        queue.clear();
        assert!(queue.is_empty() && !queue.contains(4));
    }
}
