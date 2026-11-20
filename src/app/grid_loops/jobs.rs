//! The loop jobs: one thread per cell reads the disk cache or decodes the loop.

use crate::app::{App, ViewMode};
use crate::scanner::LivePhotos;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::Instant;
use tuilibr::pictures::kitty::loops::player::{
    LoopOutcome, LoopResult, LoopTicket, run_caught_with,
};
use tuilibr::pictures::motion::loop_file::load_or_decode;
use tuilibr::pictures::motion::loop_frames::LoopFrames;
use tuilibr::pictures::resize::padding::{pad_loop_frame, thumb_target_px};

impl App {
    /// The text of the last panic of a loop job, if one panicked.
    #[cfg(test)]
    pub(crate) fn last_loop_panic(&self) -> Option<String> {
        let last = self.grid_loops.last_panic.lock();
        last.unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Give the finished loop jobs to the player and start new ones.
    pub(crate) fn poll_grid_loops(&mut self) {
        let loops = self.graphics.loops();
        while let Ok(result) = self.grid_loops.rx.try_recv() {
            self.grid_loops.player.ready(result, loops);
        }
        if self.mode != ViewMode::Gallery || self.thumbnails_pending() {
            self.grid_loops.player.cancel_jobs();
        }
        self.start_loop_jobs();
    }

    /// Start the loop jobs of the visible cells, in grid order.
    ///
    /// They start after every still of the screen is out, and `poll_grid_loops` cancels them when
    /// new stills wait. A loop never takes the cores from a still. The cells come from the grid
    /// now, not from the last grid frame: a folder switch, a zoom or a scroll before the next
    /// draw starts no job for a cell of the old screen.
    fn start_loop_jobs(&mut self) {
        if !self.grid_loops_shown() || self.thumbnails_pending() || self.cell_shape_pending() {
            return;
        }
        self.show_grid_loops(Instant::now());
        let spawner = LoopSpawner {
            images: &self.images,
            live: &self.live,
            tx: &self.grid_loops.tx,
            last_panic: &self.grid_loops.last_panic,
            cache_dir: &self.disk_cache_dir,
            target: thumb_target_px(self.gallery.thumb_box(), self.cell_px),
            cell_px: self.cell_px,
        };
        self.grid_loops
            .player
            .start_jobs(|ticket| spawner.spawn(ticket));
    }

    /// The file of the loop of `index`: the motion of a Live Photo still, else the file itself.
    #[cfg(all(test, feature = "video"))]
    pub(crate) fn loop_source(&self, index: usize) -> Option<PathBuf> {
        loop_source(&self.images, &self.live, index)
    }
}

/// The parts of the app that a loop job needs, apart from the player that starts the jobs.
struct LoopSpawner<'a> {
    images: &'a [PathBuf],
    live: &'a LivePhotos,
    tx: &'a mpsc::Sender<LoopResult<usize>>,
    last_panic: &'a Arc<Mutex<Option<String>>>,
    cache_dir: &'a Path,
    target: (u32, u32),
    cell_px: (u32, u32),
}

impl LoopSpawner<'_> {
    /// Read the loop of the ticket from the disk cache, or decode it, on a thread of its own.
    /// The job pads each frame off the UI thread. False without a file or a thread, and the
    /// player marks the cell failed.
    fn spawn(&self, ticket: LoopTicket<usize>) -> bool {
        let Some(path) = loop_source(self.images, self.live, ticket.key) else {
            return false;
        };
        let tx = self.tx.clone();
        let last_panic = Arc::clone(self.last_panic);
        let cache_dir = self.cache_dir.to_path_buf();
        let (target, cell_px) = (self.target, self.cell_px);
        let job = move || {
            let outcome = run_noting_panic(&last_panic, || {
                let frames = loop_frames(&cache_dir, &path, target, ticket.cancel());
                LoopOutcome::of(frames, |frame, shown| {
                    pad_loop_frame(frame, shown, target, cell_px)
                })
            });
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a closed receiver means that nobody waits for the result"
            )]
            let _ = tx.send(ticket.finish(outcome));
        };
        std::thread::Builder::new()
            .name("phewr-loop".to_owned())
            .spawn(job)
            .is_ok()
    }
}

/// Run a loop job. A panic gives `Failed`, and its text goes to `last_panic` for a test.
pub(super) fn run_noting_panic(
    last_panic: &Mutex<Option<String>>,
    job: impl FnOnce() -> LoopOutcome,
) -> LoopOutcome {
    run_caught_with(job, |text| {
        *last_panic.lock().unwrap_or_else(PoisonError::into_inner) = Some(text.to_owned());
    })
}

/// The file of the loop of `index`: the motion of a Live Photo still, else the file itself.
fn loop_source(images: &[PathBuf], live: &LivePhotos, index: usize) -> Option<PathBuf> {
    let image = images.get(index)?;
    Some(live.motion(image).unwrap_or(image).to_path_buf())
}

/// The loop from the disk cache, or from a decode that fills the cache.
fn loop_frames(
    cache_dir: &Path,
    path: &Path,
    target: (u32, u32),
    cancel: &AtomicBool,
) -> io::Result<LoopFrames> {
    let (frames, saved) = load_or_decode(cache_dir, path, target, || {
        decode_session_loop(cache_dir, path, target, cancel)
    })?;
    #[expect(
        clippy::let_underscore_must_use,
        reason = "a loop that misses the disk cache decodes again on the next visit"
    )]
    let _ = saved;
    Ok(frames)
}

#[cfg(feature = "video")]
fn decode_session_loop(
    cache_dir: &Path,
    path: &Path,
    target: (u32, u32),
    cancel: &AtomicBool,
) -> io::Result<LoopFrames> {
    crate::video::session_loop(path, target, cache_dir, cancel)
}

#[cfg(not(feature = "video"))]
fn decode_session_loop(
    _cache_dir: &Path,
    _path: &Path,
    _target: (u32, u32),
    _cancel: &AtomicBool,
) -> io::Result<LoopFrames> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "phewr has no video feature",
    ))
}
