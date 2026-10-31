//! The terminal video player.
//!
//! ffmpeg decodes small frames into a pipe ([`decoder`]). The player paces them at the frame
//! rate of the video and hands the due frame to the Kitty write, which sends it through the
//! temp file medium with one image id. The terminal scales the frame to the window with `c=`
//! and `r=`. The player keeps real time: a slow decode or a slow terminal drops frames, and a
//! clip never plays in slow motion. It has no pause, no seek and no sound: `p` opens the video
//! in the mpv window.
//!
//! # From the AGENTS.md gotcha "Terminal player"
//!
//! ffmpeg runs as a subprocess (`player/decoder.rs`, behind the `Decoder` trait), with `-hwaccel
//! videotoolbox`, `-noautorotate`, `scale_vt` and `transpose_vt` on the GPU, `hflip,vflip` for 180
//! degrees, `hwdownload` as `p010le` (10 bit) or `nv12`, and
//! `format=rgb48le,lut3d=...:interp=tetrahedral` for HLG and PQ. A GPU run without a frame runs
//! again in software (`Pipeline::Software`). A video with a variable frame rate gets `-fps_mode
//! cfr -r` too: the probe calls it VFR when `r_frame_rate` and `avg_frame_rate` differ by more
//! than 1 %. The output rate is `r_frame_rate`, but a base rate above 1.5 times the average is a
//! burst, and then the output rate is the average, at least 24 fps and never above `r_frame_rate`.
//! Every rate stays at or below 60 fps (`pacing_fps`, `variable_fps` in `player/probe.rs`).
//! `VideoInfo.rate` is `Rate::Constant`, `Rate::Variable` or `Rate::Animation`. Without `-r`,
//! ffmpeg writes raw frames at `r_frame_rate`, so the pacing uses the same rate. iPhone clips (30
//! and 29.99) stay constant-rate. ffmpeg writes `-pix_fmt rgb24` for a video and `rgba` for an
//! animation, and `pixel_format` in `player/decoder.rs` is the one source for the frame layout
//! (`player::Frame`). One frame per temp file, image id `u32::MAX - 1` (the grid ids count up from
//! 1, the loop frames from 2^31), `c=` / `r=` to the cell box, no `X=` / `Y=`. The first frame of
//! a new video removes all other placements in the same synchronized update
//! (`App::clear_before_video`), so the photo of the view before does not show around the frame.
//! The frame box is the window in pixels, capped at 1920x1080 (`MAX_FRAME`). A resize moves the
//! cell box. The frame size stays until the next ffmpeg start. `encoder::MAX_PAYLOAD_BYTES` (64
//! MiB) caps one Kitty payload, because Ghostty 1.4.0 refuses 65 MiB. The fullscreen and prefetch
//! decode boxes shrink below it before the decode (`capped_box`, 3 bytes per pixel), so no opaque
//! decode runs the writer fallback. A transparent image above the cap and a zoom crop still
//! shrink in the writer with Catmull-Rom and get `c=` only, so the rows follow the aspect ratio.
//! The reader thread holds 2 frames (`FRAME_QUEUE`), so a slow terminal blocks ffmpeg on the pipe.
//! The reader of a run owns `FRAME_POOL` (5) frame buffers (`FramePool`), so no run allocates a
//! zeroed 6 MB `Vec` per frame. A `PooledFrame` returns its buffer on drop, also a frame that the
//! catch-up rule drops, and a frame of an old run returns to the pool of its own run.
//!
//! # The start ahead of Enter
//!
//! The first frame comes about 180 ms to 260 ms after the start of ffmpeg (the process start,
//! the `VideoToolbox` setup and, for HDR, the parse of the 33-point cube). When the gallery cursor
//! rests on a video for 200 ms and the grid has no work, `video_handoff/prestart.rs` opens the
//! player of that cell in the next pass of the run loop, about 270 ms into the rest on an idle
//! grid. ffmpeg decodes the first frames into the queue and then blocks on the pipe. Enter takes
//! the player ([`Player::begin_at`]). After a rest of about 450 ms or more, its first frame shows
//! 1 ms after Enter. A shorter rest gains less, and a rest below 270 ms gains nothing. The clock
//! of a run starts at its first frame, so the clip plays in real time from Enter and drops no
//! frame. A cursor move, a new frame box, the help popup, a slideshow and an mpv window drop the
//! player, and so does a wait of 60 s for Enter. The drop of a `FrameStream` kills ffmpeg and
//! reaps it on a thread, so the UI thread does not wait about 6 ms for the exit. A 17-point HDR
//! cube saves about 20 ms per start, but its colors differ by up to 39 levels from the 33-point
//! cube, so HDR keeps 33.

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

/// The wait of the run loop when the player takes no frames.
const IDLE_POLL: Duration = Duration::from_millis(250);
/// The wait of the run loop while the player waits for a frame from ffmpeg.
const DECODE_POLL: Duration = Duration::from_millis(10);
/// A restart never lands closer than this to the end, so that ffmpeg still has frames to give.
const END_MARGIN: f64 = 0.5;
/// The frames may lag the clock by this much. Behind it, ffmpeg starts again at the clock.
const MAX_LAG: Duration = Duration::from_millis(300);
/// A restart begins this far ahead of the clock, so that the first frame of the new ffmpeg
/// arrives in time. The start of ffmpeg takes about 100 ms to 250 ms.
const RESTART_LEAD: f64 = 0.3;
/// A run that writes no frame within this time ended, as the first-frame thumbnail does.
const FIRST_FRAME_DEADLINE: Duration = Duration::from_secs(5);

/// Whether the player takes frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Playing,
    /// The clip without a repeat is over, or the decode failed.
    Ended,
}

/// Whether the clip starts again at its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Repeat {
    /// The clip plays one time.
    Never,
    /// The clip starts again until this much time passed since the open. A slideshow sets it
    /// for a short animation.
    Until(Duration),
    /// The clip starts again for ever.
    Always,
}

/// One ffmpeg run: its clock and its start position.
struct Run {
    /// The time at which the ffmpeg run started.
    started: Instant,
    /// The time at which the first frame of this run is due. The first frame sets it, or the
    /// restart keeps the clock of the run before.
    start: Option<Instant>,
    /// The frames due since `start`: the shown and the dropped ones.
    due_frames: u32,
    /// The position of the first frame of this run, in seconds.
    base: f64,
    /// True after the first frame of this run. A run without a frame failed.
    delivered: bool,
}

impl Run {
    /// A run that starts at `now` at `base` seconds.
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

/// A decoded frame: RGB for a video, RGBA for an animation, see [`pixel_format`].
pub(crate) struct Frame {
    size: (u32, u32),
    alpha: bool,
    data: PooledFrame,
}

impl Frame {
    #[cfg(test)]
    /// The frame of `size` pixels in `data`. `None` when `data` is shorter than such a frame.
    pub(crate) fn new(size: (u32, u32), alpha: bool, data: Vec<u8>) -> Option<Self> {
        Self::pooled(size, alpha, PooledFrame::unpooled(data))
    }

    /// The frame of `size` pixels in a buffer of the pool. The drop of the frame returns it.
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

/// A video that plays in the terminal: the ffmpeg run, the pacing and the current frame.
pub(crate) struct Player {
    path: PathBuf,
    program: PathBuf,
    decoder: Rc<dyn Decoder>,
    info: VideoInfo,
    /// The pixel size of each frame.
    frame: (u32, u32),
    cube: Option<PathBuf>,
    stream: Option<FrameStream>,
    pipeline: Pipeline,
    /// The decoded frame that waits for its due time.
    pending: Option<PooledFrame>,
    /// The frame on the screen.
    pub(crate) current_frame: Option<Frame>,
    state: State,
    /// The note of a failed decode.
    pub(crate) error: Option<String>,
    repeat: Repeat,
    /// The time of the open.
    opened: Instant,
    interval: Duration,
    run: Run,
    /// The frames that came too late and never reached the screen, also the frames that a
    /// restart skipped.
    pub(crate) dropped: u32,
}

impl Player {
    /// Start ffmpeg for `path`. `limit` is the pixel box of the frame, `cube_dirs` the places
    /// for the color cube. Without `repeat`, the clip plays one time and then
    /// `playing` turns false.
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

    /// Count the open and the deadline of the first frame from `now`, so that a run that
    /// started ahead of Enter plays as if it started at Enter. The clock of the run starts at
    /// its first frame anyway, so a frame decoded ahead shows at once and nothing drops.
    pub(crate) fn begin_at(&mut self, now: Instant) {
        self.opened = now;
        self.run.started = now;
    }

    /// Start ffmpeg at `position` seconds with the current pipeline. `now` starts the deadline
    /// of the first frame.
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

    /// Take the frame that is due at `now`. `true` means that a new frame is on `current_frame`.
    ///
    /// The clock runs at the wall time and never waits for a frame, so a clip plays for its
    /// length. A frame that is more than one interval late drops when a newer frame already
    /// waits. A late frame without a newer one shows, and the clock stays. When the frames lag
    /// the clock by more than `MAX_LAG`, ffmpeg starts again ahead of the clock
    /// ([`Player::catch_up`]). The end of the run starts the clip again with `repeat`, or ends
    /// the playback. A GPU run without a single frame runs again in software.
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
            self.current_frame = Frame::pooled(self.frame, bytes_per_pixel == 4, frame);
            self.catch_up(now);
            return true;
        }
    }

    /// Start ffmpeg again ahead of the clock when the frames lag it by more than `MAX_LAG`.
    /// The new run keeps the clock, so the time of the restart counts, and the frames in
    /// between count as dropped. Close to the end, the run plays out instead, and a run whose
    /// clock passed the length of the clip while its frames lag ends at once.
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

    /// Take the due frame now, see [`Player::poll_at`].
    pub(crate) fn poll(&mut self) -> bool {
        self.poll_at(Instant::now())
    }

    /// Play the clip again at its end until `duration` passed since the open. A slideshow
    /// holds a short animation for its interval this way, instead of a flash of one loop.
    pub(crate) fn play_at_least(&mut self, duration: Duration) {
        if matches!(self.repeat, Repeat::Never | Repeat::Until(_)) {
            self.repeat = Repeat::Until(duration);
        }
    }

    /// True when the clip starts again at its end at `now`.
    fn repeats_at(&self, now: Instant) -> bool {
        match self.repeat {
            Repeat::Never => false,
            Repeat::Until(duration) => now.saturating_duration_since(self.opened) < duration,
            Repeat::Always => true,
        }
    }

    /// ffmpeg ended at `now`: the clip is over, or the decode failed before its first frame.
    fn end_of_run(&mut self, now: Instant) {
        // A run from the start without a frame failed. A run after a seek without a frame
        // began past the end of the stream, which can be shorter than the container says.
        let failed = !self.run.delivered && self.run.base == 0.0;
        if failed && let Some(next) = self.pipeline.next(&self.info) {
            // A format that the GPU chain cannot take decodes in software.
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

    /// True while the player takes frames.
    pub(crate) fn playing(&self) -> bool {
        self.state == State::Playing
    }

    /// True when the clip without a repeat is over, or the decode failed.
    pub(crate) fn finished(&self) -> bool {
        !self.playing()
    }

    /// Stop with `note` as the error of the player.
    fn fail(&mut self, note: String) {
        self.stream = None;
        self.error = Some(note);
        self.state = State::Ended;
    }

    /// Stop when a start of ffmpeg failed.
    fn fail_if_error(&mut self, started: std::io::Result<()>) {
        if let Err(error) = started {
            self.fail(format!("ffmpeg did not start ({error})"));
        }
    }

    /// The position in seconds: the time of the last due frame.
    pub(crate) fn position(&self) -> f64 {
        self.run.base + f64::from(self.run.due_frames) / self.info.fps
    }

    /// True for an animated image, which a slideshow holds for its interval.
    pub(crate) fn is_animation(&self) -> bool {
        self.info.is_animation()
    }

    /// The length of the clip in seconds, or 0 when the container does not tell it.
    pub(crate) fn duration(&self) -> f64 {
        self.info.duration
    }

    /// How long the run loop can wait before the next frame is due at `now`.
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

    /// How long the run loop can wait before the next frame is due.
    pub(crate) fn time_until_next_frame(&self) -> Duration {
        self.time_until_next_frame_at(Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::{DECODE_POLL, Frame, IDLE_POLL, Player, VideoInfo};
    use crate::test_support::video::{FakeDecoder, fake_decoder, info_of, shared};
    use std::path::Path;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    const FRAME: Duration = Duration::from_millis(100);

    /// A player on a 64x48 clip at 10 frames per second, with a frame box of 64x48.
    fn player(decoder: &Rc<FakeDecoder>, repeat: bool) -> Player {
        Player::open(
            shared(decoder),
            Path::new("/bin/ffmpeg"),
            Path::new("/videos/clip.mp4"),
            (64, 48),
            &[],
            repeat,
        )
        .unwrap()
    }

    /// A frame of the clip with every byte at `value`.
    fn frame(value: u8) -> Vec<u8> {
        vec![value; 64 * 48 * 3]
    }

    #[test]
    fn the_open_probes_and_starts_ffmpeg_with_the_frame_box() {
        let decoder = fake_decoder(info_of(640, 480, 10.0));
        let player = player(&decoder, true);
        assert_eq!(decoder.starts().len(), 1);
        let args = decoder.last_args();
        assert!(args.contains("scale_vt=w=64:h=48,"), "{args}");
        assert!(args.contains(" -i /videos/clip.mp4 "), "{args}");
        assert!(args.ends_with(" -pix_fmt rgb24 pipe:1"), "{args}");
        assert!(player.playing());
        assert_eq!(player.position(), 0.0);
        assert_eq!(player.duration(), 2.0);
        assert_eq!(
            player.time_until_next_frame_at(Instant::now()),
            DECODE_POLL,
            "no frame yet: a short poll"
        );
    }

    #[test]
    fn a_run_without_a_frame_ends_after_the_deadline() {
        let decoder = fake_decoder(info_of(640, 480, 10.0));
        let mut player = player(&decoder, false);
        let clock = Instant::now();
        assert!(!player.poll_at(clock + Duration::from_secs(4)));
        assert!(player.playing(), "within the deadline");
        // The GPU run falls back to software and gets its own deadline.
        assert!(!player.poll_at(clock + Duration::from_secs(6)));
        assert_eq!(decoder.starts().len(), 2);
        assert!(player.playing());
        assert!(!player.poll_at(clock + Duration::from_secs(12)));
        assert!(player.finished());
        assert!(player.error.is_some());
    }

    #[test]
    fn a_run_with_a_frame_at_4_seconds_plays() {
        let decoder = fake_decoder(info_of(640, 480, 10.0));
        let mut player = player(&decoder, false);
        let clock = Instant::now();
        assert!(!player.poll_at(clock + Duration::from_secs(4)));
        for value in 0..25 {
            decoder.feed(frame(value));
        }
        let first = clock + Duration::from_secs(4);
        assert!(player.poll_at(first));
        // The frames keep coming past the 5 s mark of the open: the deadline is for the first
        // frame only.
        for step in 1..=21 {
            player.poll_at(first + FRAME * step);
        }
        assert!(player.playing());
        assert!(player.error.is_none());
        assert_eq!(decoder.starts().len(), 1);
    }

    #[test]
    fn a_video_frame_has_3_bytes_per_pixel_and_a_short_one_is_no_frame() {
        let frame = Frame::new((4, 2), false, vec![5; 4 * 2 * 3 + 1]).unwrap();
        let pixels = frame.pixels();
        assert_eq!(frame.dimensions(), (4, 2));
        assert!(!pixels.alpha);
        assert_eq!(pixels.data, [5; 4 * 2 * 3], "the tail is cut");
        assert!(Frame::new((4, 2), true, vec![5; 4 * 2 * 3]).is_none());
    }

    #[test]
    fn frames_show_at_their_due_time_and_a_late_frame_drops() {
        let decoder = fake_decoder(info_of(640, 480, 10.0));
        let mut player = player(&decoder, true);
        let clock = Instant::now();
        assert!(!player.poll_at(clock), "no frame from ffmpeg yet");
        decoder.feed(frame(1));
        decoder.feed(frame(2));
        assert!(player.poll_at(clock), "the first frame starts the clock");
        assert_eq!(player.current_frame.as_ref().unwrap().pixels().data[0], 1);
        assert!(
            !player.poll_at(clock + FRAME / 2),
            "the second frame is not due"
        );
        assert_eq!(
            player.time_until_next_frame_at(clock + FRAME / 2),
            FRAME / 2
        );
        assert!(player.poll_at(clock + FRAME));
        assert_eq!(player.current_frame.as_ref().unwrap().pixels().data[0], 2);
        assert!((player.position() - 0.2).abs() < 1e-9);

        // Frame 3 is due at 200 ms. At 350 ms it is more than one interval late and drops.
        decoder.feed(frame(3));
        decoder.feed(frame(4));
        assert!(
            player.poll_at(clock + FRAME * 7 / 2),
            "frame 4 shows instead"
        );
        assert_eq!(player.current_frame.as_ref().unwrap().pixels().data[0], 4);
        assert_eq!(player.dropped, 1);
        assert!((player.position() - 0.4).abs() < 1e-9);
    }

    #[test]
    fn a_slow_decode_keeps_the_clock_and_shows_the_late_frame() {
        let decoder = fake_decoder(info_of(640, 480, 10.0));
        let mut player = player(&decoder, true);
        let clock = Instant::now();
        decoder.feed(frame(1));
        assert!(player.poll_at(clock));
        decoder.feed(frame(2));
        assert!(player.poll_at(clock + FRAME));
        // Frame 3 is due at 200 ms and comes at 250 ms, with no newer frame: it shows.
        decoder.feed(frame(3));
        assert!(
            player.poll_at(clock + FRAME * 5 / 2),
            "the late frame shows"
        );
        assert_eq!(player.current_frame.as_ref().unwrap().pixels().data[0], 3);
        assert_eq!(player.dropped, 0);
        // The clock stays: frame 4 is due at 300 ms, not one interval after the late frame.
        decoder.feed(frame(4));
        assert!(!player.poll_at(clock + FRAME * 29 / 10));
        assert_eq!(
            player.time_until_next_frame_at(clock + FRAME * 5 / 2),
            FRAME / 2
        );
        assert!(player.poll_at(clock + FRAME * 3));
        assert_eq!(
            decoder.starts().len(),
            1,
            "a small lag starts no new ffmpeg"
        );
    }

    #[test]
    fn a_restart_past_the_end_of_the_stream_ends_the_clip_without_an_error() {
        for repeat in [false, true] {
            let mut info = info_of(640, 480, 10.0);
            info.duration = 20.0;
            let decoder = fake_decoder(info);
            let mut player = player(&decoder, repeat);
            let clock = Instant::now();
            decoder.feed(frame(1));
            assert!(player.poll_at(clock));
            // 1 s without a frame: ffmpeg starts again ahead of the clock.
            assert!(!player.poll_at(clock + FRAME * 10));
            assert_eq!(decoder.starts().len(), 2);
            // The stream is shorter than the container says: the new run has no frame.
            decoder.end();
            assert!(!player.poll_at(clock + FRAME * 11));
            assert_eq!(player.error, None, "repeat {repeat}");
            assert_eq!(player.finished(), !repeat, "repeat {repeat}");
            if repeat {
                assert_eq!(decoder.starts().len(), 3, "the clip starts again");
                assert!(!decoder.last_args().contains("-ss"));
            } else {
                assert_eq!(decoder.starts().len(), 2, "no retry");
            }
        }
    }

    #[test]
    fn a_lag_beyond_the_limit_restarts_ffmpeg_ahead_of_the_clock() {
        let mut info = info_of(640, 480, 10.0);
        info.duration = 20.0;
        let decoder = fake_decoder(info);
        let mut player = player(&decoder, true);
        let clock = Instant::now();
        decoder.feed(frame(1));
        assert!(player.poll_at(clock));
        // Frame 2 is due at 100 ms. At 400 ms it lags by exactly the limit: no restart yet.
        assert!(!player.poll_at(clock + FRAME * 4));
        assert_eq!(decoder.starts().len(), 1);
        // At 500 ms the lag passes the limit: ffmpeg starts at the clock plus the lead.
        assert!(!player.poll_at(clock + FRAME * 5));
        assert_eq!(decoder.starts().len(), 2);
        assert!(
            decoder.last_args().contains(" -ss 0.800 -i "),
            "{}",
            decoder.last_args()
        );
        assert_eq!(player.dropped, 7, "the frames from 100 ms to 700 ms");
        // The new run keeps the clock: its first frame is due at 800 ms.
        decoder.feed(frame(9));
        assert!(!player.poll_at(clock + FRAME * 5));
        assert_eq!(
            player.time_until_next_frame_at(clock + FRAME * 5),
            FRAME * 3
        );
        assert!(!player.poll_at(clock + FRAME * 7));
        assert!(player.poll_at(clock + FRAME * 8));
        assert!((player.position() - 0.9).abs() < 1e-9);
        // Close to the end, the run plays out and starts nothing.
        let mut info = info_of(640, 480, 10.0);
        info.duration = 1.0;
        let short_decoder = fake_decoder(info);
        let mut short = self::player(&short_decoder, true);
        short_decoder.feed(frame(1));
        assert!(short.poll_at(clock));
        assert!(!short.poll_at(clock + FRAME * 5));
        assert_eq!(short_decoder.starts().len(), 1);
    }

    #[test]
    fn a_clip_without_a_known_length_restarts_at_a_lag_and_has_no_end_margin() {
        let mut info = info_of(640, 480, 10.0);
        info.duration = 0.0;
        let decoder = fake_decoder(info);
        let mut player = player(&decoder, true);
        assert_eq!(player.duration(), 0.0);
        let clock = Instant::now();
        decoder.feed(frame(1));
        assert!(player.poll_at(clock));
        // A lag past the limit restarts ffmpeg, and no clip length ends the run or stops it.
        assert!(!player.poll_at(clock + FRAME * 5));
        assert_eq!(decoder.starts().len(), 2);
        assert!(player.playing(), "the unknown length ends nothing");
        assert!(
            decoder.last_args().contains(" -ss 0.800 -i "),
            "{}",
            decoder.last_args()
        );
    }

    /// The rule of the player: a slow terminal drops frames, and the clip ends on time. Here
    /// each frame write takes 3 intervals, and ffmpeg keeps two frames ready, as the queue of
    /// the reader thread does.
    #[test]
    fn a_slow_terminal_drops_frames_and_the_clip_ends_on_time() {
        const CLIP_FRAMES: u32 = 60;
        let mut info = info_of(640, 480, 10.0);
        info.duration = 6.0;
        let decoder = fake_decoder(info);
        let mut player = player(&decoder, false);
        let clock = Instant::now();
        let mut now = clock;
        let (mut runs, mut fed, mut shown) = (1, 0, 0);
        while !player.finished() && now < clock + FRAME * 200 {
            if decoder.starts().len() > runs {
                (runs, fed) = (decoder.starts().len(), 0);
            }
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the start of a run of the 6 s test clip, in frames"
            )]
            let first = (player.run.base * 10.0).round() as u32;
            let taken = player.run.due_frames + u32::from(player.pending.is_some());
            while fed < taken + 2 && first + fed < CLIP_FRAMES {
                decoder.feed(frame(1));
                fed += 1;
            }
            if first + fed >= CLIP_FRAMES && taken >= fed {
                decoder.end();
            }
            if player.poll_at(now) {
                shown += 1;
                now += FRAME * 3;
            } else {
                now += FRAME / 10;
            }
        }
        assert!(player.finished());
        let played = now.duration_since(clock);
        // The last frame shows at most one slow write after the 6 s, and the loop adds the
        // time of that write: 6.6 s at the most, against 18 s at the old moving clock.
        assert!(
            played <= FRAME * 66,
            "the 6 s clip ended after {played:?}, not in slow motion"
        );
        assert!(player.dropped > 0, "frames dropped instead");
        assert!(shown >= 15, "{shown} frames shown");
    }

    #[test]
    fn an_animation_decodes_in_software_and_holds_for_the_minimum_play_time() {
        use crate::test_support::video::animation_info;
        let decoder = fake_decoder(animation_info());
        let mut player = player(&decoder, false);
        let args = decoder.last_args();
        assert!(
            args.contains(" -map 0:0 -fps_mode cfr -r 10.0000 "),
            "{args}"
        );
        assert!(args.contains("scale=w=64:h=48:flags=bilinear"), "{args}");
        assert!(!args.contains("-hwaccel"), "{args}");
        assert!(args.ends_with(" -pix_fmt rgba pipe:1"), "{args}");

        player.play_at_least(Duration::from_millis(300));
        let clock = Instant::now();
        decoder.feed(vec![1; 64 * 48 * 4]);
        assert!(player.poll_at(clock));
        let pixels = player.current_frame.as_ref().unwrap().pixels();
        assert!(pixels.alpha, "an animation keeps its transparent pixels");
        assert_eq!(pixels.data.len(), 64 * 48 * 4);
        decoder.end();
        assert!(!player.poll_at(clock + FRAME));
        assert!(
            player.playing(),
            "the loop starts again before the hold passed"
        );
        assert_eq!(decoder.starts().len(), 2);
        assert!(
            !decoder.last_args().contains("hwaccel_output_format"),
            "no GPU retry for an animation"
        );

        decoder.feed(vec![2; 64 * 48 * 4]);
        assert!(player.poll_at(clock + 2 * FRAME));
        decoder.end();
        assert!(!player.poll_at(clock + Duration::from_millis(400)));
        assert!(
            player.finished(),
            "the end of the loop after the hold ends the playback"
        );
        assert_eq!(decoder.starts().len(), 2);
        assert_eq!(player.error, None);
    }

    #[test]
    fn a_variable_rate_clip_paces_at_the_constant_rate_that_ffmpeg_writes() {
        let decoder = fake_decoder(VideoInfo {
            rate: super::Rate::Variable,
            ..info_of(640, 480, 20.0)
        });
        let mut player = player(&decoder, true);
        let args = decoder.last_args();
        assert!(
            args.contains(" -i /videos/clip.mp4 -map 0:0 -fps_mode cfr -r 20.0000 "),
            "{args}"
        );
        assert!(args.contains("-hwaccel videotoolbox"), "{args}");
        let step = Duration::from_millis(50);
        let clock = Instant::now();
        for value in 1..=10 {
            decoder.feed(frame(value));
            let due = clock + step * u32::from(value - 1);
            if value > 1 {
                assert!(
                    !player.poll_at(due.checked_sub(step / 2).unwrap()),
                    "frame {value} is not due"
                );
            }
            assert!(player.poll_at(due), "frame {value}");
        }
        assert_eq!(player.dropped, 0);
        assert!((player.position() - 0.5).abs() < 1e-9);
    }

    /// The played clips of the real tests: the name, the player at the end, the frames that
    /// ffmpeg wrote and the played time in seconds. Empty without ffmpeg or libx264.
    fn play_real_clips(real_clock: bool) -> Vec<(&'static str, Player, u32, f64)> {
        use crate::test_support::video::{real_ffmpeg, real_ffprobe, write_clip};
        let (Some(program), Some(ffprobe)) = (real_ffmpeg(), real_ffprobe()) else {
            return Vec::new();
        };
        let dir = tempfile::tempdir().unwrap();
        let mut played = Vec::new();
        for (name, variable) in [
            ("vfr.mp4", true),
            ("vfr.mov", true),
            ("cfr.mp4", false),
            ("cfr.mov", false),
        ] {
            let Some(path) = write_clip(&program, &dir.path().join(name), variable) else {
                return Vec::new();
            };
            let (player, frames, seconds) = play_to_end(&program, &ffprobe, &path, real_clock);
            println!(
                "{name}: {frames} frames at {:.2} fps ({} dropped): played {seconds:.3} s, \
                 container {:.3} s",
                player.info.fps,
                player.dropped,
                player.duration()
            );
            played.push((name, player, frames, seconds));
        }
        played
    }

    /// Play `path` to its end with the real ffmpeg and ffprobe. With `real_clock`, the player
    /// runs at the real clock. Else a virtual clock jumps to each due frame, so the scheduler
    /// cannot change the result. Gives the player, the frames that ffmpeg wrote and the played
    /// seconds.
    fn play_to_end(
        program: &Path,
        ffprobe: &Path,
        path: &Path,
        real_clock: bool,
    ) -> (Player, u32, f64) {
        let decoder: Rc<dyn super::Decoder> =
            Rc::new(super::SystemDecoder::new(Some(ffprobe.to_path_buf())));
        let mut player = Player::open(decoder, program, path, (32, 24), &[], false).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut clock = Instant::now();
        let (mut first, mut last, mut shown) = (None, None, 0_u32);
        while !player.finished() {
            assert!(
                Instant::now() < deadline,
                "{}: the player hangs",
                path.display()
            );
            if real_clock {
                clock = Instant::now();
            }
            if player.poll_at(clock) {
                first.get_or_insert(clock);
                last = Some(clock);
                shown += 1;
            } else if !real_clock && player.pending.is_some() {
                clock += player.time_until_next_frame_at(clock);
            } else {
                let wait = player.time_until_next_frame_at(clock);
                std::thread::sleep(wait.min(Duration::from_millis(2)));
            }
        }
        assert_eq!(player.error, None, "{}", path.display());
        let (Some(first), Some(last)) = (first, last) else {
            panic!("{}: no frame", path.display());
        };
        let seconds = (last - first + player.interval).as_secs_f64();
        let frames = shown + player.dropped;
        (player, frames, seconds)
    }

    /// Play a variable and a constant-rate clip that the real ffmpeg writes, at a virtual
    /// clock. The frames times the interval must match the container length. Without ffmpeg
    /// on PATH, the test skips with a note.
    #[test]
    fn the_real_ffmpeg_plays_variable_and_constant_rate_clips_at_their_length() {
        for (name, player, frames, seconds) in play_real_clips(false) {
            let paced = (player.interval * frames).as_secs_f64();
            let length = player.duration();
            assert!(
                (paced - length).abs() < 0.1 * length,
                "{name}: paced {paced}"
            );
            assert!((seconds - paced).abs() < 1e-6, "{name}: played {seconds}");
            assert_eq!(player.dropped, 0, "{name}: the virtual clock is never late");
        }
    }

    /// The same clips at the real clock: the wall time must match the container length. It
    /// takes about 7 s and depends on the scheduler, so it runs only in `just test-real`.
    #[test]
    #[ignore = "plays 4 clips at the real clock for about 7 s"]
    fn the_real_ffmpeg_plays_variable_rate_clips_at_their_length_in_wall_time() {
        for (name, player, _, seconds) in play_real_clips(true) {
            let length = player.duration();
            assert!(
                (seconds - length).abs() < 0.25 * length,
                "{name}: wall {seconds}"
            );
        }
    }

    #[test]
    fn the_end_of_the_clip_starts_again_with_repeat_and_ends_without() {
        for repeat in [true, false] {
            let decoder = fake_decoder(info_of(640, 480, 10.0));
            let mut player = player(&decoder, repeat);
            let clock = Instant::now();
            decoder.feed(frame(1));
            assert!(player.poll_at(clock));
            decoder.end();
            assert!(!player.poll_at(clock + FRAME));
            assert_eq!(player.finished(), !repeat, "repeat {repeat}");
            assert_eq!(decoder.starts().len(), if repeat { 2 } else { 1 });
            if repeat {
                assert!(player.playing());
                decoder.feed(frame(2));
                assert!(player.poll_at(clock + FRAME * 2));
                assert!(
                    (player.position() - 0.1).abs() < 1e-9,
                    "the position starts at 0 again"
                );
            }
        }
    }

    #[test]
    fn a_run_without_a_frame_retries_in_software_and_then_fails() {
        let decoder = fake_decoder(info_of(640, 480, 10.0));
        let mut player = player(&decoder, true);
        let clock = Instant::now();
        decoder.end();
        assert!(!player.poll_at(clock));
        assert_eq!(decoder.starts().len(), 2);
        let args = decoder.last_args();
        assert!(!args.contains("videotoolbox_vld"), "{args}");
        assert!(args.contains("scale=w=64:h=48:flags=bilinear"), "{args}");
        assert!(player.playing() && !player.finished());
        decoder.end();
        assert!(!player.poll_at(clock));
        assert!(player.finished());
        assert_eq!(
            player.error.as_deref(),
            Some("ffmpeg could not decode clip.mp4")
        );
        assert_eq!(decoder.starts().len(), 2, "no third try");
        assert_eq!(player.time_until_next_frame_at(clock), IDLE_POLL);
    }

    #[test]
    fn a_failed_probe_or_start_is_an_error_of_the_open() {
        let decoder = fake_decoder(info_of(640, 480, 10.0));
        decoder.fail_probe.set(true);
        assert!(
            Player::open(
                shared(&decoder),
                Path::new("/bin/ffmpeg"),
                Path::new("/videos/clip.mp4"),
                (64, 48),
                &[],
                true,
            )
            .is_err()
        );
        decoder.fail_probe.set(false);
        decoder.fail_start.set(true);
        assert!(
            Player::open(
                shared(&decoder),
                Path::new("/bin/ffmpeg"),
                Path::new("/videos/clip.mp4"),
                (64, 48),
                &[],
                true,
            )
            .is_err()
        );
    }

    #[test]
    fn a_dropped_player_stops_its_stream() {
        let decoder = fake_decoder(info_of(640, 480, 10.0));
        let player = player(&decoder, true);
        assert!(decoder.streaming());
        drop(player);
        assert!(!decoder.streaming(), "the receiver is gone");
    }
}
