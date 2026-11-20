//! The loops of the video, animated and Live cells of the grid, on the loop player of tuilibr.
//!
//! The player keeps the loops, their jobs and their clock (`tuilibr/docs/pictures.md`, "The
//! loop player"). phewr names the loop cells of each grid frame, runs each job on a thread of its
//! own and gives the results back to the player.

mod jobs;
#[cfg(test)]
mod tests;

use super::{App, ViewMode};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};
use tuilibr::pictures::kitty::loops::player::{LoopLimits, LoopPlayer, LoopResult};
use tuilibr::pictures::motion::clock::LoopClock;

/// The thumbnails count up from 1 and the video frame is `u32::MAX - 1`.
const LOOP_ID_BASE: u32 = 1 << 31;

pub(crate) struct GridLoops {
    /// `[gallery] animate`.
    pub(crate) enabled: bool,
    /// The loops of the cells, keyed by the image index.
    pub(crate) player: LoopPlayer<usize>,
    tx: mpsc::Sender<LoopResult<usize>>,
    rx: mpsc::Receiver<LoopResult<usize>>,
    /// The text of the last panic of a loop job. phewr writes nothing to the terminal while the
    /// TUI runs and has no log file, so the text stays here for a test and a debugger.
    last_panic: Arc<Mutex<Option<String>>>,
    /// The clock of the player, so a test can name the time of a tick.
    #[cfg(all(test, feature = "video"))]
    pub(crate) clock: LoopClock,
}

impl GridLoops {
    pub(crate) fn new(enabled: bool) -> Self {
        let (tx, rx) = mpsc::channel();
        let clock = LoopClock::starting_at(Instant::now());
        Self {
            enabled,
            player: LoopPlayer::new(LOOP_ID_BASE..u32::MAX - 1, LoopLimits::default())
                .with_clock(clock),
            tx,
            rx,
            last_panic: Arc::default(),
            #[cfg(all(test, feature = "video"))]
            clock,
        }
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

    /// Name the loop cells of the grid frame at `now` to the player, in grid order. The loops
    /// and the jobs of the hidden cells go.
    pub(crate) fn show_grid_loops(&mut self, now: Instant) {
        let play = self.grid_loops_shown();
        self.grid_loops.player.begin_frame(now);
        for (_, index) in self.gallery.visible_items() {
            if self.can_loop(index) {
                self.grid_loops.player.show(index, play);
            }
        }
        self.grid_loops.player.drop_hidden();
    }

    /// True for a video, animated or Live Photo cell with its sharp still.
    fn can_loop(&self, index: usize) -> bool {
        self.thumb_cache.is_sharp(&index)
            && (self.is_video(index) || self.is_animated(index) || self.is_live(index))
    }

    #[cfg(test)]
    pub(crate) fn loop_plays(&self, index: usize) -> bool {
        self.grid_loops_shown() && self.grid_loops.player.is_playing(&index)
    }

    /// The ticket of a job for `index` that runs on no thread. The frame of the player then
    /// shows only `index`.
    #[cfg(test)]
    pub(crate) fn loop_ticket(
        &mut self,
        index: usize,
    ) -> tuilibr::pictures::kitty::loops::player::LoopTicket<usize> {
        let player = &mut self.grid_loops.player;
        player.begin_frame(Instant::now());
        player.show(index, true);
        let mut ticket = None;
        player.start_jobs(|new| {
            ticket = Some(new);
            true
        });
        ticket.expect("the cell has no loop, no job and no failure")
    }

    pub(crate) fn grid_loops_pending(&self) -> bool {
        self.grid_loops.player.pending()
    }

    /// True when a visible loop waits for its frames or for its first placement.
    pub(crate) fn loops_to_send(&self) -> bool {
        self.grid_loops_shown() && self.grid_loops.player.needs_send()
    }

    pub(crate) fn loop_frames_due(&self, now: Instant) -> bool {
        self.grid_loops_shown() && self.grid_loops.player.due(now)
    }

    /// The time until the next frame of a placed tick loop. A Kitty animation needs no wakeup.
    pub(crate) fn grid_loop_timeout(&self, now: Instant) -> Option<Duration> {
        if !self.grid_loops_shown() {
            return None;
        }
        self.grid_loops.player.next_wake(now)
    }

    /// Drop every loop and every job. The caller deletes all images in the terminal, and the
    /// grid write after the wipe drops the stale ids.
    pub(super) fn forget_grid_loops(&mut self) {
        self.grid_loops.player.clear();
    }
}
