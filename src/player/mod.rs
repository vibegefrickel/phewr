//! The terminal video player.
//!
//! ffmpeg decodes small frames into a pipe ([`decoder`]). The player paces them at the frame
//! rate of the video and hands the due frame to the Kitty write. The terminal scales the frame
//! to the window with `c=` and `r=`.
//!
//! The player keeps real time: a slow decode or a slow terminal drops frames, and a clip never
//! plays in slow motion. It has no pause, no seek and no sound: `p` opens the video in the mpv
//! window.

pub(crate) mod animation;
mod decoder;
mod layout;
mod probe;
pub(crate) mod tone_map;

pub(crate) use animation::is_animated;
pub(crate) use decoder::{
    Decoder, FrameStream, LoopSampling, Pipeline, PooledFrame, SystemDecoder, args, cube_for,
    first_frame_args, loop_args, pixel_format, with_fallback,
};
pub(crate) use layout::{cell_box, frame_box, frame_limit};
#[cfg(test)]
pub(crate) use probe::Rate;
#[cfg(test)]
pub(crate) use probe::probe as probe_file;
pub(crate) use probe::{VideoInfo, probe_cached};

use crate::encoder::Pixels;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};

const IDLE_POLL: Duration = Duration::from_millis(250);
const DECODE_POLL: Duration = Duration::from_millis(10);
/// A restart never lands closer than this to the end of the clip.
const END_MARGIN: f64 = 0.5;
const MAX_LAG: Duration = Duration::from_millis(300);
/// The start of ffmpeg takes 100 ms to 250 ms.
const RESTART_LEAD: f64 = 0.3;
const FIRST_FRAME_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Playing,
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Repeat {
    Never,
    /// A slideshow repeats a short animation until its interval passed.
    Until(Duration),
    Always,
}

struct Run {
    started: Instant,
    /// The due time of the first frame. A restart keeps the clock of the run before.
    start: Option<Instant>,
    /// The shown and the dropped frames since `start`.
    due_frames: u32,
    /// The position of the first frame in seconds.
    base: f64,
    delivered: bool,
}

impl Run {
    fn new(base: f64, now: Instant) -> Self {
        Self {
            started: now,
            start: None,
            due_frames: 0,
            base,
            delivered: false,
        }
    }
}

/// RGB for a video, RGBA for an animation, see [`pixel_format`].
pub(crate) struct Frame {
    size: (u32, u32),
    alpha: bool,
    data: PooledFrame,
}

impl Frame {
    #[cfg(test)]
    /// `None` when `data` is shorter than the frame.
    pub(crate) fn new(size: (u32, u32), alpha: bool, data: Vec<u8>) -> Option<Self> {
        Self::pooled(size, alpha, PooledFrame::unpooled(data))
    }

    fn pooled(size: (u32, u32), alpha: bool, mut data: PooledFrame) -> Option<Self> {
        let bytes = size.0 as usize * size.1 as usize * if alpha { 4 } else { 3 };
        if data.bytes().len() < bytes {
            return None;
        }
        data.truncate(bytes);
        Some(Self { size, alpha, data })
    }

    pub(crate) fn dimensions(&self) -> (u32, u32) {
        self.size
    }

    pub(crate) fn pixels(&self) -> Pixels<'_> {
        Pixels {
            width: self.size.0,
            height: self.size.1,
            alpha: self.alpha,
            data: self.data.bytes(),
            keep_alpha: self.alpha,
        }
    }
}

pub(crate) struct Player {
    path: PathBuf,
    program: PathBuf,
    decoder: Rc<dyn Decoder>,
    info: VideoInfo,
    frame: (u32, u32),
    cube: Option<PathBuf>,
    stream: Option<FrameStream>,
    pipeline: Pipeline,
    pending: Option<PooledFrame>,
    pub(crate) current_frame: Option<Frame>,
    state: State,
    pub(crate) error: Option<String>,
    repeat: Repeat,
    opened: Instant,
    interval: Duration,
    run: Run,
    /// Late frames and the frames that a restart skipped.
    pub(crate) dropped: u32,
}

impl Player {
    /// Start ffmpeg for `path` within the pixel box `limit`. Without `repeat`, the clip plays once.
    pub(crate) fn open(
        decoder: Rc<dyn Decoder>,
        program: &Path,
        path: &Path,
        limit: (u32, u32),
        cube_dirs: &[&Path],
        repeat: bool,
    ) -> std::io::Result<Self> {
        let info = decoder.probe(path)?;
        let frame = frame_box((info.width, info.height), limit);
        let cube = cube_for(&info, crate::decode::color::target(), cube_dirs);
        let interval = info.frame_interval();
        let pipeline = Pipeline::first(&info);
        let opened = Instant::now();
        let mut player = Self {
            path: path.to_path_buf(),
            program: program.to_path_buf(),
            decoder,
            info,
            frame,
            cube,
            stream: None,
            pipeline,
            pending: None,
            current_frame: None,
            state: State::Playing,
            error: None,
            repeat: if repeat {
                Repeat::Always
            } else {
                Repeat::Never
            },
            opened,
            interval,
            run: Run::new(0.0, opened),
            dropped: 0,
        };
        player.start_run(0.0, opened)?;
        Ok(player)
    }

    /// Restart the open time and the first-frame deadline at Enter, for a player that started
    /// ahead.
    pub(crate) fn begin_at(&mut self, now: Instant) {
        self.opened = now;
        self.run.started = now;
    }

    fn start_run(&mut self, position: f64, now: Instant) -> std::io::Result<()> {
        self.run = Run::new(position, now);
        self.stream = None;
        self.pending = None;
        let args = args(
            &self.path,
            &self.info,
            self.frame,
            self.cube.as_deref(),
            position,
            self.pipeline,
        );
        let bytes = self.frame.0 as usize * self.frame.1 as usize * pixel_format(&self.info).1;
        self.stream = Some(self.decoder.start(&self.program, &args, bytes)?);
        Ok(())
    }

    /// Take the frame that is due at `now`. `true` means a new `current_frame`.
    ///
    /// The clock runs at the wall time and never waits for a frame. A frame more than one
    /// interval late drops when a newer frame waits. Else it shows late, and the clock stays.
    pub(crate) fn poll_at(&mut self, now: Instant) -> bool {
        if self.state != State::Playing {
            return false;
        }
        loop {
            if self.pending.is_none() {
                match self.stream.as_mut().map(FrameStream::try_next_pooled) {
                    Some(Ok(frame)) => self.pending = Some(frame),
                    Some(Err(TryRecvError::Empty)) => {
                        if !self.run.delivered
                            && now.saturating_duration_since(self.run.started)
                                > FIRST_FRAME_DEADLINE
                        {
                            self.end_of_run(now);
                            return false;
                        }
                        self.catch_up(now);
                        return false;
                    }
                    Some(Err(TryRecvError::Disconnected)) | None => {
                        self.end_of_run(now);
                        return false;
                    }
                }
            }
            let start = *self.run.start.get_or_insert(now);
            let due = start + self.interval.mul_f64(f64::from(self.run.due_frames));
            if now < due {
                return false;
            }
            self.run.delivered = true;
            if now > due + self.interval
                && let Some(Ok(newer)) = self.stream.as_mut().map(FrameStream::try_next_pooled)
            {
                self.pending = Some(newer);
                self.run.due_frames += 1;
                self.dropped += 1;
                continue;
            }
            self.run.due_frames += 1;
            let Some(frame) = self.pending.take() else {
                return false;
            };
            let (_, bytes_per_pixel) = pixel_format(&self.info);
            let shown = Frame::pooled(self.frame, bytes_per_pixel == 4, frame);
            self.catch_up(now);
            // A short frame drops, and the screen keeps the last frame.
            let Some(shown) = shown else {
                self.dropped += 1;
                return false;
            };
            self.current_frame = Some(shown);
            return true;
        }
    }

    /// Start ffmpeg again ahead of the clock when the frames lag it by more than `MAX_LAG`.
    ///
    /// The new run keeps the clock, and the skipped frames count as dropped. Close to the end,
    /// the run plays out. A lagging run past the clip length ends.
    fn catch_up(&mut self, now: Instant) {
        let Some(start) = self.run.start else {
            return;
        };
        if !self.run.delivered {
            return;
        }
        let next_due = start + self.interval.mul_f64(f64::from(self.run.due_frames));
        let lag = now.saturating_duration_since(next_due);
        let clock = self.run.base + now.saturating_duration_since(start).as_secs_f64();
        if lag > self.interval && self.info.duration > 0.0 && clock >= self.info.duration {
            self.stream = None;
            self.pending = None;
            self.end_of_run(now);
            return;
        }
        if lag <= MAX_LAG {
            return;
        }
        let target = clock + RESTART_LEAD;
        if self.info.duration > 0.0 && target > self.info.duration - END_MARGIN {
            return;
        }
        let skipped = (target - self.position()) * self.info.fps;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the skipped frames of one restart are few and never negative"
        )]
        let skipped = skipped.max(0.0).round() as u32;
        let run_start = start + Duration::from_secs_f64(target - self.run.base);
        let started = self.start_run(target, now);
        self.run.start = Some(run_start);
        self.dropped += skipped;
        self.fail_if_error(started);
    }

    pub(crate) fn poll(&mut self) -> bool {
        self.poll_at(Instant::now())
    }

    /// Repeat the clip until `duration` passed since the open.
    pub(crate) fn play_at_least(&mut self, duration: Duration) {
        if matches!(self.repeat, Repeat::Never | Repeat::Until(_)) {
            self.repeat = Repeat::Until(duration);
        }
    }

    fn repeats_at(&self, now: Instant) -> bool {
        match self.repeat {
            Repeat::Never => false,
            Repeat::Until(duration) => now.saturating_duration_since(self.opened) < duration,
            Repeat::Always => true,
        }
    }

    /// Repeat, fall back to software or stop after ffmpeg ended.
    fn end_of_run(&mut self, now: Instant) {
        // A run after a seek without a frame began past the end of the stream. The stream can be
        // shorter than the container says.
        let failed = !self.run.delivered && self.run.base == 0.0;
        if failed && let Some(next) = self.pipeline.next(&self.info) {
            self.pipeline = next;
            let position = self.run.base;
            let started = self.start_run(position, now);
            self.fail_if_error(started);
            return;
        }
        if failed {
            self.fail(format!(
                "ffmpeg could not decode {}",
                crate::mpv::file_name(&self.path)
            ));
            return;
        }
        if self.repeats_at(now) {
            let started = self.start_run(0.0, now);
            self.fail_if_error(started);
        } else {
            self.stream = None;
            self.state = State::Ended;
        }
    }

    pub(crate) fn playing(&self) -> bool {
        self.state == State::Playing
    }

    /// True when the clip is over or the decode failed.
    pub(crate) fn finished(&self) -> bool {
        !self.playing()
    }

    fn fail(&mut self, note: String) {
        self.stream = None;
        self.error = Some(note);
        self.state = State::Ended;
    }

    fn fail_if_error(&mut self, started: std::io::Result<()>) {
        if let Err(error) = started {
            self.fail(format!("ffmpeg did not start ({error})"));
        }
    }

    /// The time of the last due frame, in seconds.
    pub(crate) fn position(&self) -> f64 {
        self.run.base + f64::from(self.run.due_frames) / self.info.fps
    }

    pub(crate) fn is_animation(&self) -> bool {
        self.info.is_animation()
    }

    /// The clip length in seconds, or 0 when the container does not tell it.
    pub(crate) fn duration(&self) -> f64 {
        self.info.duration
    }

    /// The wait of the run loop until the next frame is due.
    pub(crate) fn time_until_next_frame_at(&self, now: Instant) -> Duration {
        if self.state != State::Playing {
            return IDLE_POLL;
        }
        match (self.pending.as_ref(), self.run.start) {
            (Some(_), Some(start)) => (start
                + self.interval.mul_f64(f64::from(self.run.due_frames)))
            .saturating_duration_since(now),
            _ => DECODE_POLL,
        }
    }

    pub(crate) fn time_until_next_frame(&self) -> Duration {
        self.time_until_next_frame_at(Instant::now())
    }
}

#[cfg(test)]
mod tests;
