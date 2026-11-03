//! The loops of the video and animated cells of the grid: the jobs, the frames and the clock.
//!
//! A job decodes the frames of a visible cell after the stills of the screen are out. One of two
//! paths ([`Loops`]) then plays the loop: one Kitty animation that the terminal plays, or one
//! image per frame that a tick of the shared clock swaps.

mod cell;
mod jobs;
#[cfg(test)]
mod tests;

pub(crate) use self::cell::CellLoop;
use self::cell::TICKS_PER_SECOND;
use super::{App, ViewMode};
use crate::encoder::Loops;
use crate::gallery::GridPicture;
use image::RgbaImage;
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// The decoded frames of one loop, before the padding. The disk cache keeps them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LoopFrames {
    pub(crate) frames: Vec<RgbaImage>,
    pub(crate) delays: Vec<Duration>,
    /// The pixel size of the still.
    pub(crate) shown: (u32, u32),
}

/// The thumbnails count up from 1 and the video frame is `u32::MAX - 1`.
const LOOP_ID_BASE: u32 = 1 << 31;

struct LoopResult {
    generation: u32,
    index: usize,
    outcome: LoopOutcome,
}

enum LoopOutcome {
    Frames(Vec<GridPicture>, Vec<Duration>),
    /// No ffmpeg, a broken file, a single frame or a panic. The cell keeps its still.
    Failed,
    /// The stills or another view needed the cores. The cell gets a new job later.
    Cancelled,
}

pub(crate) struct GridLoops {
    /// `[gallery] animate`.
    pub(crate) enabled: bool,
    pub(crate) loops: BTreeMap<usize, CellLoop>,
    running: BTreeMap<usize, Arc<AtomicBool>>,
    /// The cells without a loop in this generation.
    failed: HashSet<usize>,
    /// The cells above the terminal budget, until the visible cells change.
    over_budget: HashSet<usize>,
    last_visible: Vec<usize>,
    generation: u32,
    tx: mpsc::Sender<LoopResult>,
    rx: mpsc::Receiver<LoopResult>,
    next_id: u32,
    /// The ids of the hidden loops. The next grid write deletes them.
    pub(crate) stale_ids: Vec<u32>,
    epoch: Instant,
}

impl GridLoops {
    pub(crate) fn new(enabled: bool) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            enabled,
            loops: BTreeMap::new(),
            running: BTreeMap::new(),
            failed: HashSet::new(),
            over_budget: HashSet::new(),
            last_visible: Vec::new(),
            generation: 0,
            tx,
            rx,
            next_id: LOOP_ID_BASE,
            stale_ids: Vec::new(),
            epoch: Instant::now(),
        }
    }

    pub(crate) fn tick_at(&self, now: Instant) -> u64 {
        let ticks = now.saturating_duration_since(self.epoch).as_nanos()
            * u128::from(TICKS_PER_SECOND)
            / 1_000_000_000;
        u64::try_from(ticks).unwrap_or(u64::MAX)
    }

    /// The time of `tick`, rounded up to the nanosecond: `tick_at` of it is `tick` again.
    pub(crate) fn instant_of(&self, tick: u64) -> Instant {
        let nanos = (u128::from(tick) * 1_000_000_000).div_ceil(u128::from(TICKS_PER_SECOND));
        self.epoch + Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
    }

    /// Forget every placement after the terminal deleted them.
    pub(crate) fn forget_placements(&mut self) {
        for cell in self.loops.values_mut() {
            cell.placed = None;
        }
    }

    pub(crate) fn insert(
        &mut self,
        index: usize,
        frames: Vec<GridPicture>,
        delays: &[Duration],
        loops: Loops,
    ) {
        let animation = loops == Loops::Animation;
        let count = CellLoop::id_count(animation, frames.len());
        let first_id = self.allocate_ids(u32::try_from(count).unwrap_or(u32::MAX));
        let cell = CellLoop::new(frames, delays, first_id, animation);
        if let Some(old) = self.loops.insert(index, cell) {
            self.stale_ids.extend(old.ids);
        }
    }

    /// The first id of a free run of `count` ids. The run skips the ids of the loops in the map
    /// and of the loops that wait for their delete, which only matters after the counter wraps.
    fn allocate_ids(&mut self, count: u32) -> u32 {
        loop {
            let first = self.next_id;
            let end = first.checked_add(count).filter(|end| *end < u32::MAX);
            let Some(end) = end else {
                self.next_id = LOOP_ID_BASE;
                continue;
            };
            self.next_id = end;
            let in_run = |id: &u32| (first..end).contains(id);
            let used = self.loops.values().any(|cell| cell.ids.iter().any(in_run))
                || self.stale_ids.iter().any(in_run);
            if !used {
                return first;
            }
        }
    }

    /// The terminal budget has no room. The cell keeps its still.
    pub(crate) fn drop_over_budget(&mut self, index: usize) {
        self.loops.remove(&index);
        self.over_budget.insert(index);
    }

    fn cancel_jobs(&self) {
        for cancel in self.running.values() {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// A quit during a loop decode ends the ffmpeg runs within `CANCEL_POLL`.
impl Drop for GridLoops {
    fn drop(&mut self) {
        self.cancel_jobs();
    }
}

impl App {
    /// No overlay, no other view, `[gallery] animate` on.
    pub(crate) fn grid_loops_shown(&self) -> bool {
        self.grid_loops.enabled
            && self.mode == ViewMode::Gallery
            && !self.help_visible
            && !self.info_visible
    }

    #[cfg(test)]
    pub(crate) fn loop_plays(&self, index: usize) -> bool {
        self.grid_loops_shown()
            && self
                .grid_loops
                .loops
                .get(&index)
                .is_some_and(|cell| cell.placed.is_some())
    }

    #[cfg(test)]
    pub(crate) fn pretend_loop_job(&mut self, index: usize) {
        self.grid_loops
            .running
            .insert(index, Arc::new(AtomicBool::new(false)));
    }

    pub(crate) fn grid_loops_pending(&self) -> bool {
        !self.grid_loops.running.is_empty()
    }

    /// True when a visible loop waits for its frames or for its first placement.
    pub(crate) fn loops_to_send(&self) -> bool {
        self.grid_loops_shown()
            && self.gallery.visible_items().any(|(_, index)| {
                self.grid_loops
                    .loops
                    .get(&index)
                    .is_some_and(|cell| cell.placed.is_none())
            })
    }

    pub(crate) fn loop_frames_due(&self, now: Instant) -> bool {
        let tick = self.grid_loops.tick_at(now);
        self.grid_loops_shown()
            && self
                .grid_loops
                .loops
                .values()
                .any(|cell| !cell.animation && cell.placed.is_some_and(|id| id != cell.id_at(tick)))
    }

    /// The time until the next frame of a placed tick loop. A Kitty animation needs no wakeup.
    pub(crate) fn grid_loop_timeout(&self, now: Instant) -> Option<Duration> {
        if !self.grid_loops_shown() {
            return None;
        }
        let tick = self.grid_loops.tick_at(now);
        self.grid_loops
            .loops
            .values()
            .filter(|cell| !cell.animation && cell.placed.is_some())
            .map(|cell| cell.next_change(tick))
            .min()
            .map(|next| {
                self.grid_loops
                    .instant_of(next)
                    .saturating_duration_since(now)
            })
    }

    /// Drop every loop and every job. The caller deletes all images in the terminal.
    pub(super) fn forget_grid_loops(&mut self) {
        let loops = &mut self.grid_loops;
        loops.cancel_jobs();
        loops.loops.clear();
        loops.running.clear();
        loops.failed.clear();
        loops.over_budget.clear();
        loops.last_visible.clear();
        loops.stale_ids.clear();
        loops.generation += 1;
    }

    /// Drop the loops of the hidden cells. The cells above the budget try again on a new screen.
    pub(crate) fn drop_hidden_loops(&mut self) {
        let loops = &mut self.grid_loops;
        let shown = self.gallery.visible_items().map(|(_, index)| index);
        if shown.eq(loops.last_visible.iter().copied()) {
            return;
        }
        loops.last_visible.clear();
        loops
            .last_visible
            .extend(self.gallery.visible_items().map(|(_, index)| index));
        loops.over_budget.clear();
        let last_visible = &loops.last_visible;
        let mut stale = Vec::new();
        loops.loops.retain(|index, cell| {
            let keep = last_visible.contains(index);
            if !keep {
                stale.extend_from_slice(&cell.ids);
            }
            keep
        });
        loops.stale_ids.extend(stale);
    }
}
