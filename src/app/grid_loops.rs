//! The loops of the video and animated cells of the grid: the jobs, the frames and the clock.
//!
//! A job decodes the frames of a visible cell in the background, after the still thumbnails of
//! the screen are out (`grid_loop`, or the disk cache). Then one of two paths plays the loop
//! (`kitty_output::play_grid_loops`), as the probe at the start or the config picks it
//! ([`Loops`]):
//!
//! - Animation: the loop goes to the terminal one time as one Kitty animation under one id,
//!   and the terminal plays it. viewr sends nothing per tick, and the run loop does not wake.
//! - Ticks: each frame goes to the terminal one time as its own image. A tick then swaps the
//!   placement of one frame for the next one, about 75 bytes per cell. All loops share one
//!   clock of [`TICKS_PER_SECOND`], and each loop starts at a multiple of [`START_ALIGN`]
//!   ticks, so the frames of all cells change in the same pass of the run loop.

use super::disk_cache::{try_load_loop, try_save_loop};
use super::thumbnails::{pad_loop_frame, thumb_target_px};
use super::{App, ViewMode};
use crate::encoder::Loops;
use crate::gallery::GridPicture;
use image::RgbaImage;
use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// The decoded frames of one loop, before the padding for the grid. The disk cache keeps them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LoopFrames {
    pub(crate) frames: Vec<RgbaImage>,
    /// The time that each frame shows.
    pub(crate) delays: Vec<Duration>,
    /// The pixel size of the still thumbnail. The terminal scales each frame to its cells.
    pub(crate) shown: (u32, u32),
}

/// The loop jobs that run at once, as many as the thumbnail decodes. A job waits for ffmpeg
/// most of the time, so each job runs on a thread of its own and holds no core of the pool.
const MAX_LOOP_JOBS: usize = 4;

/// The ticks of the loop clock per second: the highest rate of an animation loop
/// (`grid_loop::MAX_ANIMATION_FPS`). A tick is the shortest time that a frame shows, so a
/// screen of loops wakes the run loop at most 12 times per second.
const TICKS_PER_SECOND: u64 = 12;

/// The ticks of a video frame (333 ms). Each loop starts at a multiple of it, so the frames of
/// all video cells change in the same tick: 3 wakeups per second for a screen of videos.
const START_ALIGN: u64 = 4;

/// The first Kitty id of a loop frame. The thumbnails count up from 1 and the video frame is
/// `u32::MAX - 1`, so the three ranges never meet.
const LOOP_ID_BASE: u32 = 1 << 31;

/// The result of a loop job.
struct LoopResult {
    generation: u32,
    index: usize,
    outcome: LoopOutcome,
}

/// What a loop job gives back.
enum LoopOutcome {
    /// The padded frames and their times.
    Frames(Vec<GridPicture>, Vec<Duration>),
    /// No loop: no ffmpeg, a broken file, a single frame, or a job that panicked. The cell
    /// keeps its still and its marker in this generation.
    Failed,
    /// The stills of the screen or another view needed the cores. The cell gets a new job
    /// after the stills.
    Cancelled,
}

/// Sends the result of one loop job. A job that ends without a result, also by a panic,
/// sends `Failed` from the drop, so that its cell never stays in the running jobs.
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

/// The loop of one visible cell: its padded frames, their Kitty ids and their ends on the clock.
pub(crate) struct CellLoop {
    pub(crate) frames: Vec<GridPicture>,
    /// The Kitty id of each frame. A Kitty animation has one id for all frames.
    pub(crate) ids: Vec<u32>,
    /// The time of each frame in milliseconds, at least 1, the gaps of a Kitty animation.
    gaps_ms: Vec<u32>,
    /// True when the terminal plays the loop as one Kitty animation under `ids[0]`.
    pub(crate) animation: bool,
    /// The tick at which each frame ends, counted from the start of the loop. The last one is
    /// the length of the loop.
    ends: Vec<u64>,
    /// The tick of the first placement, a multiple of [`START_ALIGN`].
    start: Option<u64>,
    /// The frame that the terminal shows in the cell. `None` while the cell shows its still.
    pub(crate) placed: Option<u32>,
}

impl CellLoop {
    fn new(frames: Vec<GridPicture>, delays: &[Duration], first_id: u32, animation: bool) -> Self {
        let count = if animation { 1 } else { frames.len() };
        let ids = (first_id..).take(count).collect();
        let gaps_ms = delays
            .iter()
            .map(|delay| u32::try_from(delay.as_millis()).unwrap_or(u32::MAX).max(1))
            .collect();
        Self {
            frames,
            ids,
            gaps_ms,
            animation,
            ends: tick_ends(delays),
            start: None,
            placed: None,
        }
    }

    /// The RGBA bytes of all frames, the count of the terminal budget.
    pub(crate) fn bytes(&self) -> usize {
        self.frames
            .iter()
            .map(|frame| frame.image.as_raw().len())
            .sum()
    }

    /// The time of `frame` in milliseconds, the gap of the frame in a Kitty animation. A frame
    /// without a delay shows for one tick.
    pub(crate) fn gap_ms(&self, frame: usize) -> u32 {
        self.gaps_ms
            .get(frame)
            .copied()
            .unwrap_or_else(|| u32::try_from(1000 / TICKS_PER_SECOND).unwrap_or(1))
    }

    /// The whole cells of each frame: the cells of the still.
    pub(crate) fn cells(&self) -> (u16, u16) {
        self.frames.first().map_or((1, 1), |frame| frame.cells)
    }

    /// Start the clock of the loop at the next multiple of [`START_ALIGN`] after `tick`, unless
    /// it runs already. Until then, the first frame shows.
    pub(crate) fn start_at(&mut self, tick: u64) {
        if self.start.is_none() {
            self.start = Some(tick.div_ceil(START_ALIGN) * START_ALIGN);
        }
    }

    /// The Kitty id of the frame that shows at `tick`.
    pub(crate) fn id_at(&self, tick: u64) -> u32 {
        let (frame, _) = self.position(tick);
        self.ids[frame.min(self.ids.len() - 1)]
    }

    /// The next tick after `tick` at which another frame shows.
    fn next_change(&self, tick: u64) -> u64 {
        let (frame, cycle_start) = self.position(tick);
        cycle_start + self.ends.get(frame).copied().unwrap_or(1)
    }

    /// The frame at `tick` and the tick at which its round of the loop started.
    fn position(&self, tick: u64) -> (usize, u64) {
        let start = self.start.unwrap_or(tick);
        let length = self.ends.last().copied().unwrap_or(1).max(1);
        if tick < start {
            return (0, start);
        }
        let offset = (tick - start) % length;
        let frame = self.ends.partition_point(|&end| end <= offset);
        (frame, tick - offset)
    }
}

/// The ends of the frames on the clock. Each end rounds the sum of the delays to the nearest
/// tick, so a loop keeps its length, and each frame shows for at least one tick.
fn tick_ends(delays: &[Duration]) -> Vec<u64> {
    let mut total = 0_u128;
    let mut last = 0;
    delays
        .iter()
        .map(|delay| {
            total += delay.as_nanos();
            let ticks = (total * u128::from(TICKS_PER_SECOND) + 500_000_000) / 1_000_000_000;
            let rounded = u64::try_from(ticks).unwrap_or(u64::MAX);
            last = rounded.max(last + 1);
            last
        })
        .collect()
}

/// The loops of the grid and their jobs.
pub(crate) struct GridLoops {
    /// `[gallery] animate`. Off, no job starts and every cell keeps its still and its marker.
    pub(crate) enabled: bool,
    /// The loops of the visible cells, by image index.
    pub(crate) loops: BTreeMap<usize, CellLoop>,
    /// The running jobs by image index, each with its cancel flag.
    running: BTreeMap<usize, Arc<AtomicBool>>,
    /// The cells without a loop in this generation.
    failed: HashSet<usize>,
    /// The cells whose loop did not fit the terminal budget. They keep their stills until the
    /// visible cells change.
    over_budget: HashSet<usize>,
    /// The visible cells of the last render of the grid.
    last_visible: Vec<usize>,
    generation: u32,
    tx: mpsc::Sender<LoopResult>,
    rx: mpsc::Receiver<LoopResult>,
    next_id: u32,
    /// The ids of the frames of the loops that left the screen. The next grid write deletes
    /// them in the terminal.
    pub(crate) stale_ids: Vec<u32>,
    /// The tick 0 of the loop clock.
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

    /// The tick of the loop clock at `now`.
    pub(crate) fn tick_at(&self, now: Instant) -> u64 {
        let ticks = now.saturating_duration_since(self.epoch).as_nanos()
            * u128::from(TICKS_PER_SECOND)
            / 1_000_000_000;
        u64::try_from(ticks).unwrap_or(u64::MAX)
    }

    /// The time of the tick `tick`, rounded up to the nanosecond, so that `tick_at` of it is
    /// `tick` again.
    pub(crate) fn instant_of(&self, tick: u64) -> Instant {
        let nanos = (u128::from(tick) * 1_000_000_000).div_ceil(u128::from(TICKS_PER_SECOND));
        self.epoch + Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
    }

    /// Forget every placement, because the terminal deleted them. The next write of the grid
    /// places the stills or the frames again.
    pub(crate) fn forget_placements(&mut self) {
        for cell in self.loops.values_mut() {
            cell.placed = None;
        }
    }

    /// Store the loop of the cell `index` with new Kitty ids: one per frame for the ticks, one
    /// for a Kitty animation.
    pub(crate) fn insert(
        &mut self,
        index: usize,
        frames: Vec<GridPicture>,
        delays: &[Duration],
        loops: Loops,
    ) {
        let cell = CellLoop::new(frames, delays, self.next_id, loops == Loops::Animation);
        let count = u32::try_from(cell.ids.len()).unwrap_or(u32::MAX);
        self.next_id = self.next_id.wrapping_add(count).max(LOOP_ID_BASE);
        if let Some(old) = self.loops.insert(index, cell) {
            self.stale_ids.extend(old.ids);
        }
    }

    /// Drop the loop of `index`, because the terminal budget has no room for it. The cell
    /// keeps its still until the visible cells change.
    pub(crate) fn drop_over_budget(&mut self, index: usize) {
        self.loops.remove(&index);
        self.over_budget.insert(index);
    }

    /// Kill the ffmpeg runs of the running jobs. Their cells get new jobs later.
    fn cancel_jobs(&self) {
        for cancel in self.running.values() {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// The jobs run on detached threads. A quit during a loop decode ends their ffmpeg runs within
/// `CANCEL_POLL`, so that no ffmpeg outlives viewr.
impl Drop for GridLoops {
    fn drop(&mut self) {
        self.cancel_jobs();
    }
}

impl App {
    /// True when the grid shows its loops: no overlay, no other view, `[gallery] animate` on.
    /// The delete question is a prompt in the status row, so the loops play on.
    pub(crate) fn grid_loops_shown(&self) -> bool {
        self.grid_loops.enabled
            && self.mode == ViewMode::Gallery
            && !self.help_visible
            && !self.info_visible
    }

    /// True when the cell `index` plays its loop: a frame of it is placed. The tests of the
    /// Kitty output read it.
    #[cfg(test)]
    pub(crate) fn loop_plays(&self, index: usize) -> bool {
        self.grid_loops_shown()
            && self
                .grid_loops
                .loops
                .get(&index)
                .is_some_and(|cell| cell.placed.is_some())
    }

    /// Count a loop job for `index` as running, without a thread. The tests of the run loop use
    /// it.
    #[cfg(test)]
    pub(crate) fn pretend_loop_job(&mut self, index: usize) {
        self.grid_loops
            .running
            .insert(index, Arc::new(AtomicBool::new(false)));
    }

    /// True while a loop job runs.
    pub(crate) fn grid_loops_pending(&self) -> bool {
        !self.grid_loops.running.is_empty()
    }

    /// True when a visible loop waits for its frames in the terminal or for its first
    /// placement. The run loop then writes them in the next pass.
    pub(crate) fn loops_to_send(&self) -> bool {
        self.grid_loops_shown()
            && self.gallery.visible_items().any(|(_, index)| {
                self.grid_loops
                    .loops
                    .get(&index)
                    .is_some_and(|cell| cell.placed.is_none())
            })
    }

    /// True when a placed loop of the ticks shows another frame at `now`. The terminal plays a
    /// Kitty animation itself.
    pub(crate) fn loop_frames_due(&self, now: Instant) -> bool {
        let tick = self.grid_loops.tick_at(now);
        self.grid_loops_shown()
            && self
                .grid_loops
                .loops
                .values()
                .any(|cell| !cell.animation && cell.placed.is_some_and(|id| id != cell.id_at(tick)))
    }

    /// The time until the next frame of a placed loop of the ticks, `None` without one. The
    /// run loop sleeps until then. A Kitty animation needs no wakeup.
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

    /// Drop every loop and every job, so that a new list or a new size starts clean. The caller
    /// deletes all images in the terminal.
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

    /// Drop the loops of the cells that left the screen, and name their ids for the delete.
    /// When the visible cells change, the cells above the budget try again. The render of the
    /// grid calls this, also after a cursor move without a scroll, which changes nothing here.
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

    /// Take the finished loop jobs and start new ones. A loop of a cell that left the screen
    /// goes: the disk cache keeps its frames.
    pub(crate) fn poll_grid_loops(&mut self) {
        while let Ok(result) = self.grid_loops.rx.try_recv() {
            self.grid_loops.running.remove(&result.index);
            if result.generation != self.grid_loops.generation {
                continue;
            }
            match result.outcome {
                LoopOutcome::Frames(frames, delays) => {
                    if self.gallery.visible_items().any(|(_, i)| i == result.index) {
                        let loops = self.graphics.loops();
                        self.grid_loops.insert(result.index, frames, &delays, loops);
                    }
                }
                LoopOutcome::Failed => {
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

    /// Start the loop jobs of the visible cells in grid order, at most `MAX_LOOP_JOBS`. They
    /// start only after every still of the screen is out, and `poll_grid_loops` cancels them
    /// when new stills wait, so a loop never holds the cores from a still.
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

    /// True for a video or animated cell with its sharp still and without a loop or a job.
    fn wants_loop(&self, index: usize) -> bool {
        let loops = &self.grid_loops;
        self.thumb_cache.is_sharp(index)
            && !loops.loops.contains_key(&index)
            && !loops.running.contains_key(&index)
            && !loops.failed.contains(&index)
            && !loops.over_budget.contains(&index)
            && (self.is_video(index) || self.is_animated(index))
    }

    /// Read the loop of `index` from the disk cache, or decode it, on a thread of its own. The
    /// job pads each frame for the grid, so the UI thread copies no pixels.
    fn spawn_loop_job(&mut self, index: usize) {
        let Some(path) = self.images.get(index).cloned() else {
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
            .name("viewr-loop".to_owned())
            .spawn(job);
        if started.is_ok() {
            self.grid_loops.running.insert(index, cancel);
        } else {
            self.grid_loops.failed.insert(index);
        }
    }
}

/// The loop of `path` from the disk cache, or from a decode that then fills the cache.
fn loop_frames(
    cache_dir: &Path,
    path: &Path,
    target: (u32, u32),
    cancel: &AtomicBool,
) -> io::Result<LoopFrames> {
    if let Some(frames) = try_load_loop(cache_dir, path, target) {
        return Ok(frames);
    }
    let frames = decode_loop(cache_dir, path, target, cancel)?;
    #[expect(
        clippy::let_underscore_must_use,
        reason = "a loop that misses the disk cache decodes again on the next visit"
    )]
    let _ = try_save_loop(cache_dir, path, target, &frames);
    Ok(frames)
}

/// The loop of a video or an animated image, with the programs of the session.
#[cfg(feature = "video")]
fn decode_loop(
    cache_dir: &Path,
    path: &Path,
    target: (u32, u32),
    cancel: &AtomicBool,
) -> io::Result<LoopFrames> {
    crate::grid_loop::session_loop(path, target, cache_dir, cancel)
}

/// Without the `video` feature, no cell has a loop.
#[cfg(not(feature = "video"))]
fn decode_loop(
    _cache_dir: &Path,
    _path: &Path,
    _target: (u32, u32),
    _cancel: &AtomicBool,
) -> io::Result<LoopFrames> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "viewr has no video feature",
    ))
}

#[cfg(test)]
mod tests {
    use super::{CellLoop, START_ALIGN, tick_ends};
    use crate::app::thumbnails::pad_for_grid;
    use crate::app::{App, ViewMode};
    use crate::config::Config;
    use crate::gallery::GridPicture;
    use crate::image_list::SharedImageList;
    use crate::test_support::{test_dir, wait_until};
    use image::RgbaImage;
    use ratatui::layout::Rect;
    use std::path::Path;
    use std::time::Duration;

    fn picture() -> GridPicture {
        RgbaImage::new(2, 2).into()
    }

    /// A grid app with the files `names` in `dir`, a sharp still for each one, and its disk
    /// cache in `dir`. The video files hold no video, so a decode fails.
    fn grid_app(dir: &Path, names: &[&str], config: &Config) -> App {
        let mut app = App::new_with_config((8, 16), SharedImageList::new(), config);
        app.disk_cache_dir = dir.join("cache");
        for (index, name) in names.iter().enumerate() {
            let path = dir.join(name);
            std::fs::write(&path, b"no video").unwrap();
            app.images.push(path);
            app.filenames.push((*name).to_owned());
            let still = RgbaImage::new(160, 96);
            app.thumb_cache
                .insert(index, pad_for_grid(still, (160, 96), (8, 16), true));
        }
        app.gallery.filtered_indices = (0..names.len()).collect();
        app.gallery.zoom_rows = 4;
        app.gallery.update_grid(Rect::new(0, 0, 106, 29));
        app.scan_complete = true;
        app
    }

    /// Poll the loop jobs until none runs.
    fn finish_jobs(app: &mut App) {
        wait_until(|| {
            app.poll_grid_loops();
            !app.grid_loops_pending()
        });
    }

    #[test]
    fn the_frame_ends_round_to_the_ticks_and_keep_the_length() {
        assert_eq!(tick_ends(&[Duration::from_millis(333); 3]), vec![4, 8, 12]);
        // Ten frames of 100 ms: each ends on the nearest tick, one second in all.
        let ends = tick_ends(&[Duration::from_millis(100); 10]);
        assert_eq!(ends, vec![1, 2, 4, 5, 6, 7, 8, 10, 11, 12]);
        // A frame below one tick still shows for one tick.
        assert_eq!(tick_ends(&[Duration::from_millis(5); 3]), vec![1, 2, 3]);
    }

    #[test]
    fn a_loop_starts_on_the_shared_clock_and_wraps_around() {
        let delays = [Duration::from_millis(333); 3];
        let mut cell = CellLoop::new(vec![picture(), picture(), picture()], &delays, 100, false);
        assert_eq!(cell.id_at(3), 100, "before the start, the first frame");
        cell.start_at(3);
        assert_eq!(cell.start, Some(START_ALIGN), "the next multiple of 4");
        assert_eq!(cell.id_at(3), 100);
        assert_eq!(cell.next_change(3), 8);
        assert_eq!(cell.id_at(8), 101);
        assert_eq!(cell.id_at(15), 102);
        assert_eq!(cell.id_at(16), 100, "the loop starts again");
        assert_eq!(cell.next_change(16), 20);
        cell.start_at(100);
        assert_eq!(cell.start, Some(START_ALIGN), "a running clock stays");
    }

    #[cfg(feature = "video")]
    #[test]
    fn the_jobs_wait_for_the_stills_of_the_screen() {
        let dir = test_dir();
        let mut app = grid_app(dir.path(), &["a.jpg", "clip.mp4"], &Config::default());
        let still = app.thumb_cache.peek(1).unwrap().0.clone();
        app.thumb_cache.clear();
        app.thumb_cache.insert(1, still);
        app.spawn_thumb_decode(0, Rect::new(0, 0, 20, 6));
        assert!(app.thumbnails_pending());
        app.poll_grid_loops();
        assert!(
            !app.grid_loops_pending(),
            "the still of the photo comes first"
        );

        wait_until(|| {
            app.poll_thumbnails();
            !app.thumbnails_pending()
        });
        app.poll_grid_loops();
        assert!(app.grid_loops_pending(), "then the clip gets its job");
        finish_jobs(&mut app);
    }

    #[test]
    fn without_ffmpeg_a_video_cell_keeps_its_still_and_tries_once() {
        let dir = test_dir();
        let mut app = grid_app(dir.path(), &["a.jpg", "clip.mp4"], &Config::default());
        app.poll_grid_loops();
        assert_eq!(
            app.grid_loops_pending(),
            cfg!(feature = "video"),
            "a job for the clip, none for the photo"
        );
        assert_eq!(app.has_pending_work(), cfg!(feature = "video"));
        finish_jobs(&mut app);
        assert!(app.grid_loops.loops.is_empty());
        assert!(!app.loop_plays(1));
        app.poll_grid_loops();
        assert!(
            !app.grid_loops_pending(),
            "a failed cell does not try again"
        );
    }

    #[cfg(feature = "video")]
    #[test]
    fn a_warm_folder_reads_its_loops_from_the_disk_cache_without_ffmpeg() {
        let dir = test_dir();
        let mut app = grid_app(dir.path(), &["clip.mp4"], &Config::default());
        let target = crate::app::thumbnails::thumb_target_px(app.gallery.thumb_box(), app.cell_px);
        let frames = super::LoopFrames {
            frames: vec![RgbaImage::new(80, 48), RgbaImage::new(80, 48)],
            delays: vec![Duration::from_millis(333); 2],
            shown: (160, 96),
        };
        crate::app::disk_cache::try_save_loop(&app.disk_cache_dir, &app.images[0], target, &frames)
            .unwrap();

        app.poll_grid_loops();
        finish_jobs(&mut app);
        let cell = &app.grid_loops.loops[&0];
        assert_eq!(cell.frames.len(), 2);
        assert_eq!(cell.cells(), (20, 6), "the cells of the still");
        assert!(app.loops_to_send());
    }

    #[cfg(feature = "video")]
    #[test]
    fn the_config_key_turns_the_loops_off() {
        let dir = test_dir();
        let config = Config {
            animate: false,
            ..Config::default()
        };
        let mut app = grid_app(dir.path(), &["clip.mp4"], &config);
        app.poll_grid_loops();
        assert!(!app.grid_loops_pending());
        assert!(!app.grid_loops_shown());
    }

    #[test]
    fn a_result_for_a_cell_that_left_the_screen_or_an_old_generation_goes() {
        let dir = test_dir();
        let names: Vec<String> = (0..40).map(|index| format!("{index}.mp4")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut app = grid_app(dir.path(), &names, &Config::default());
        app.mode = ViewMode::Fullscreen;
        let send = |app: &App, generation: u32, index: usize| {
            app.grid_loops
                .tx
                .send(super::LoopResult {
                    generation,
                    index,
                    outcome: super::LoopOutcome::Frames(
                        vec![picture(), picture()],
                        vec![Duration::ZERO; 2],
                    ),
                })
                .unwrap();
        };
        send(&app, 0, 39);
        send(&app, 1, 0);
        send(&app, 0, 1);
        app.poll_grid_loops();
        assert_eq!(
            app.grid_loops.loops.keys().copied().collect::<Vec<_>>(),
            vec![1]
        );
    }

    #[test]
    fn a_job_that_ends_without_a_result_reports_a_failure_once() {
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

    #[test]
    fn new_stills_and_other_views_cancel_the_running_jobs() {
        let dir = test_dir();
        let mut app = grid_app(dir.path(), &["a.png", "clip.mp4"], &Config::default());
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        app.grid_loops
            .running
            .insert(1, std::sync::Arc::clone(&cancel));
        let flag = || cancel.load(std::sync::atomic::Ordering::Relaxed);
        app.poll_grid_loops();
        assert!(!flag(), "no still waits");

        app.mode = ViewMode::Fullscreen;
        app.poll_grid_loops();
        assert!(flag(), "the fullscreen view takes the cores");

        cancel.store(false, std::sync::atomic::Ordering::Relaxed);
        app.mode = ViewMode::Gallery;
        app.thumb_cache.clear();
        app.spawn_thumb_decode(0, Rect::new(0, 0, 20, 6));
        assert!(app.thumbnails_pending());
        app.poll_grid_loops();
        assert!(flag(), "a new still waits");
        app.grid_loops.running.clear();
        wait_until(|| {
            app.poll_thumbnails();
            !app.thumbnails_pending()
        });
    }

    /// The job of `spawn_loop_job` with the fake ffmpeg of the `grid_loop` tests: without a
    /// queued run, ffmpeg gives no frame, so the decode waits until the cancel.
    #[cfg(feature = "video")]
    #[test]
    fn a_quit_ends_the_ffmpeg_run_of_a_running_job() {
        use crate::test_support::video::{clip_info, fake_decoder};
        use std::sync::atomic::AtomicBool;
        use std::sync::{Arc, mpsc};

        let dir = test_dir();
        let mut app = grid_app(dir.path(), &["clip.mp4"], &Config::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let job_cancel = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();
        let job = std::thread::spawn(move || {
            let decoder = fake_decoder(clip_info());
            let result = crate::grid_loop::decode_loop(
                decoder.as_ref(),
                Path::new("ffmpeg"),
                Path::new("clip.mp4"),
                (160, 96),
                &[],
                &job_cancel,
            );
            tx.send((result.err().map(|error| error.kind()), decoder.streaming()))
                .unwrap();
        });
        app.grid_loops.running.insert(0, cancel);

        drop(app);

        let (error, streaming) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(error, Some(std::io::ErrorKind::Interrupted));
        assert!(!streaming, "the job killed its ffmpeg");
        job.join().unwrap();
    }

    #[cfg(feature = "video")]
    #[test]
    fn a_cell_above_the_budget_tries_again_only_when_the_visible_cells_change() {
        let dir = test_dir();
        let names: Vec<String> = (0..40).map(|index| format!("{index}.mp4")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut app = grid_app(dir.path(), &names, &Config::default());
        app.drop_hidden_loops();
        // Every job fails without ffmpeg, except the one cell that waits above the budget.
        app.grid_loops.failed.extend(1..40);
        app.grid_loops.drop_over_budget(0);

        // A render without a scroll, after a cursor move or a meta result.
        app.drop_hidden_loops();
        app.poll_grid_loops();
        assert!(
            !app.grid_loops_pending(),
            "no job decodes the loop for nothing"
        );

        app.gallery.scroll_rows(1);
        app.drop_hidden_loops();
        app.gallery.scroll_rows(-1);
        app.drop_hidden_loops();
        app.poll_grid_loops();
        assert!(app.grid_loops_pending(), "another screen tries again");
        finish_jobs(&mut app);
    }

    #[test]
    fn a_finished_loop_job_goes_out_on_the_next_pass() {
        let dir = test_dir();
        let mut app = grid_app(dir.path(), &["clip.mp4"], &Config::default());
        app.pretend_loop_job(0);
        app.grid_loops
            .tx
            .send(super::LoopResult {
                generation: 0,
                index: 0,
                outcome: super::LoopOutcome::Frames(
                    vec![picture(), picture()],
                    vec![Duration::ZERO; 2],
                ),
            })
            .unwrap();
        assert!(!app.loops_to_send(), "the result waits for the poll");
        app.poll_grid_loops();
        assert!(!app.grid_loops_pending());
        assert!(app.loops_to_send(), "the next pass writes the frames");
        assert_eq!(crate::run_loop::next_timeout(&app, &[]), Duration::ZERO);
    }

    #[test]
    fn the_hidden_loop_check_keeps_its_list_while_the_visible_cells_stay() {
        let dir = test_dir();
        let names: Vec<String> = (0..40).map(|index| format!("{index}.mp4")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut app = grid_app(dir.path(), &names, &Config::default());
        app.drop_hidden_loops();
        let list = app.grid_loops.last_visible.as_ptr();
        let shown = app.grid_loops.last_visible.len();
        assert!(shown > 0);
        app.drop_hidden_loops();
        assert_eq!(app.grid_loops.last_visible.as_ptr(), list);
        app.gallery.scroll_rows(1);
        app.drop_hidden_loops();
        assert_ne!(
            app.grid_loops.last_visible.first(),
            Some(&0),
            "the first row left"
        );
        assert_eq!(app.grid_loops.last_visible.len(), shown);
        assert_eq!(
            app.grid_loops.last_visible.as_ptr(),
            list,
            "a change with the same count reuses the buffer"
        );
    }
}
