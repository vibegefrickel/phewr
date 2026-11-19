//! The loop jobs: one thread per cell reads the disk cache or decodes the loop.

use super::{LoopOutcome, LoopResult};
use crate::app::{App, ViewMode};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};
use tuilibr::pictures::motion::loop_file::{try_load_loop, try_save_loop};
use tuilibr::pictures::motion::loop_frames::LoopFrames;
use tuilibr::pictures::resize::padding::{pad_loop_frame, thumb_target_px};

/// As many as the thumbnail decodes. A job waits for ffmpeg most of the time and runs on a
/// thread of its own, outside the pool.
const MAX_LOOP_JOBS: usize = 4;

/// Sends the result of one loop job. A job that ends without a result, by a panic for example,
/// sends `Failed` from the drop. Its cell never stays in the running jobs.
struct ResultGuard {
    tx: Option<mpsc::Sender<LoopResult>>,
    generation: u32,
    index: usize,
}

impl ResultGuard {
    fn send(mut self, outcome: LoopOutcome) {
        self.send_once(outcome);
    }

    fn send_once(&mut self, outcome: LoopOutcome) {
        if let Some(tx) = self.tx.take() {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a closed receiver means that nobody waits for the result"
            )]
            let _ = tx.send(LoopResult {
                generation: self.generation,
                index: self.index,
                outcome,
            });
        }
    }
}

impl Drop for ResultGuard {
    fn drop(&mut self) {
        self.send_once(LoopOutcome::Failed);
    }
}

impl App {
    /// Take the finished loop jobs and start new ones. The poll drops the loop of a hidden cell.
    pub(crate) fn poll_grid_loops(&mut self) {
        while let Ok(result) = self.grid_loops.rx.try_recv() {
            // The forget cleared the flags of an old generation: the flag now belongs to a new job.
            if result.generation != self.grid_loops.generation {
                continue;
            }
            self.grid_loops.running.remove(&result.index);
            match result.outcome {
                LoopOutcome::Frames(frames, delays) if !frames.is_empty() => {
                    if self.gallery.visible_items().any(|(_, i)| i == result.index) {
                        let loops = self.graphics.loops();
                        self.grid_loops.insert(result.index, frames, &delays, loops);
                    }
                }
                // A loop without frames has nothing to place.
                LoopOutcome::Frames(..) | LoopOutcome::Failed => {
                    self.grid_loops.failed.insert(result.index);
                }
                LoopOutcome::Cancelled => {}
            }
        }
        if self.mode != ViewMode::Gallery || self.thumbnails_pending() {
            self.grid_loops.cancel_jobs();
        }
        self.start_loop_jobs();
    }

    /// Start the loop jobs of the visible cells in grid order, at most `MAX_LOOP_JOBS`.
    ///
    /// They start after every still of the screen is out, and `poll_grid_loops` cancels them when
    /// new stills wait. A loop never takes the cores from a still.
    fn start_loop_jobs(&mut self) {
        if !self.grid_loops.enabled
            || self.mode != ViewMode::Gallery
            || self.thumbnails_pending()
            || self.cell_shape_pending()
        {
            return;
        }
        let mut wanted = Vec::new();
        for (_, index) in self.gallery.visible_items() {
            if self.grid_loops.running.len() + wanted.len() >= MAX_LOOP_JOBS {
                break;
            }
            if self.wants_loop(index) {
                wanted.push(index);
            }
        }
        for index in wanted {
            self.spawn_loop_job(index);
        }
    }

    /// True for a video, animated or Live Photo cell with its sharp still and without a loop or
    /// a job.
    fn wants_loop(&self, index: usize) -> bool {
        let loops = &self.grid_loops;
        self.thumb_cache.is_sharp(&index)
            && !loops.loops.contains_key(&index)
            && !loops.running.contains_key(&index)
            && !loops.failed.contains(&index)
            && !loops.over_budget.contains(&index)
            && (self.is_video(index) || self.is_animated(index) || self.is_live(index))
    }

    /// Read the loop of `index` from the disk cache, or decode it, on a thread of its own. The job
    /// pads each frame off the UI thread. A Live Photo still loops its motion.
    fn spawn_loop_job(&mut self, index: usize) {
        let Some(path) = self.loop_source(index) else {
            return;
        };
        let guard = ResultGuard {
            tx: Some(self.grid_loops.tx.clone()),
            generation: self.grid_loops.generation,
            index,
        };
        let target = thumb_target_px(self.gallery.thumb_box(), self.cell_px);
        let cell_px = self.cell_px;
        let cache_dir = self.disk_cache_dir.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let job_cancel = Arc::clone(&cancel);
        let job = move || {
            let outcome = match loop_frames(&cache_dir, &path, target, &job_cancel) {
                Ok(frames) => {
                    let shown = frames.shown;
                    let padded = frames
                        .frames
                        .into_iter()
                        .map(|frame| pad_loop_frame(frame, shown, target, cell_px))
                        .collect();
                    LoopOutcome::Frames(padded, frames.delays)
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => LoopOutcome::Cancelled,
                Err(_) => LoopOutcome::Failed,
            };
            guard.send(outcome);
        };
        let started = std::thread::Builder::new()
            .name("phewr-loop".to_owned())
            .spawn(job);
        if started.is_ok() {
            self.grid_loops.running.insert(index, cancel);
        } else {
            self.grid_loops.failed.insert(index);
        }
    }
}

impl App {
    /// The file of the loop of `index`: the motion of a Live Photo still, else the file itself.
    pub(crate) fn loop_source(&self, index: usize) -> Option<PathBuf> {
        self.live_motion(index)
            .or_else(|| self.images.get(index).map(PathBuf::as_path))
            .map(Path::to_path_buf)
    }
}

/// The loop from the disk cache, or from a decode that fills the cache.
fn loop_frames(
    cache_dir: &Path,
    path: &Path,
    target: (u32, u32),
    cancel: &AtomicBool,
) -> io::Result<LoopFrames> {
    if let Some(frames) = try_load_loop(cache_dir, path, target) {
        return Ok(frames);
    }
    let frames = decode_session_loop(cache_dir, path, target, cancel)?;
    #[expect(
        clippy::let_underscore_must_use,
        reason = "a loop that misses the disk cache decodes again on the next visit"
    )]
    let _ = try_save_loop(cache_dir, path, target, &frames);
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

#[cfg(test)]
mod tests {
    #[test]
    fn guard_reports_one_failure() {
        let (tx, rx) = std::sync::mpsc::channel();
        let guard = |index| super::ResultGuard {
            tx: Some(tx.clone()),
            generation: 3,
            index,
        };
        let panicked = std::thread::spawn({
            let guard = guard(7);
            move || {
                let _guard = guard;
                panic!("a decode that panics");
            }
        })
        .join();
        assert!(panicked.is_err());
        let result = rx.try_recv().unwrap();
        assert_eq!((result.generation, result.index), (3, 7));
        assert!(matches!(result.outcome, super::LoopOutcome::Failed));

        guard(8).send(super::LoopOutcome::Cancelled);
        let result = rx.try_recv().unwrap();
        assert!(matches!(result.outcome, super::LoopOutcome::Cancelled));
        assert!(
            rx.try_recv().is_err(),
            "the drop after a send sends nothing"
        );
    }
}
