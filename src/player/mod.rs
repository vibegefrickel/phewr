//! The terminal video player.
//!
//! ffmpeg decodes small frames into a pipe ([`decoder`]). The player paces them at the frame
//! rate of the video and hands the due frame to the Kitty write, which sends it through the
//! temp file medium with one image id. The terminal scales the frame to the window with `c=`
//! and `r=`. There is no sound: `p` opens the video in the mpv window.

pub(crate) mod animation;
mod decoder;
mod layout;
mod probe;
pub(crate) mod tone_map;

pub(crate) use animation::is_animated;
pub(crate) use decoder::{Decoder, FrameStream, Pipeline, SystemDecoder, args, cube_for};
pub(crate) use layout::{cell_box, frame_box, frame_limit};
pub(crate) use probe::VideoInfo;

use image::RgbaImage;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};

/// How far one seek key moves, in seconds.
pub(crate) const SEEK_STEP: f64 = 5.0;
/// The wait of the run loop while the player is paused.
const PAUSED_POLL: Duration = Duration::from_millis(250);
/// The wait of the run loop while the player waits for a frame from ffmpeg.
const DECODE_POLL: Duration = Duration::from_millis(10);
/// A seek never lands closer than this to the end, so that ffmpeg still has frames to give.
const END_MARGIN: f64 = 0.5;

/// Whether the player takes frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Playing,
    Paused,
    /// The clip without a repeat is over, or the decode failed.
    Finished,
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
    pending: Option<Vec<u8>>,
    /// The frame on the screen.
    pub(crate) current_frame: Option<RgbaImage>,
    state: State,
    /// The note of a failed decode.
    pub(crate) error: Option<String>,
    repeat: bool,
    /// The clip starts again at its end until this much time passed since the open, also
    /// without `repeat`. A slideshow sets it for a short animation.
    min_play: Option<Duration>,
    /// The time of the open.
    opened: Instant,
    interval: Duration,
    /// The time of the first frame of this run. `None` until it arrives.
    start: Option<Instant>,
    /// The frames due since `start`: the shown and the dropped ones.
    due_frames: u32,
    /// The position of the first frame of this run, in seconds.
    base: f64,
    /// True after the first frame of this run. A run without a frame failed.
    delivered: bool,
    /// The frames that came too late and never reached the screen.
    pub(crate) dropped: u32,
}

impl Player {
    /// Start ffmpeg for `path`. `limit` is the pixel box of the frame, `cube_dirs` the places
    /// for the tone mapping cube. Without `repeat`, the clip plays one time and then
    /// `finished` turns true.
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
        let cube = cube_for(&info, cube_dirs);
        let interval = info.frame_interval();
        let pipeline = if info.animation {
            Pipeline::Software
        } else {
            Pipeline::Gpu
        };
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
            repeat,
            min_play: None,
            opened: Instant::now(),
            interval,
            start: None,
            due_frames: 0,
            base: 0.0,
            delivered: false,
            dropped: 0,
        };
        player.start_run(0.0)?;
        Ok(player)
    }

    /// Start ffmpeg at `position` seconds with the current pipeline.
    fn start_run(&mut self, position: f64) -> std::io::Result<()> {
        self.stream = None;
        self.pending = None;
        self.start = None;
        self.due_frames = 0;
        self.base = position;
        self.delivered = false;
        let args = args(
            &self.path,
            &self.info,
            self.frame,
            self.cube.as_deref(),
            position,
            self.pipeline,
        );
        let bytes = self.frame.0 as usize * self.frame.1 as usize * 4;
        self.stream = Some(self.decoder.start(&self.program, &args, bytes)?);
        Ok(())
    }

    /// Take the frame that is due at `now`. `true` means that a new frame is on `current_frame`.
    ///
    /// A frame that is more than one interval late drops when a newer frame already waits, so
    /// a slow terminal never falls behind the clock. Without a newer frame, the decode is slow:
    /// the clock moves to the frame and the clip plays slower instead of frozen. The end of
    /// the run starts the clip again with `repeat`, or ends the playback. A GPU run without a
    /// single frame runs again in software.
    pub(crate) fn poll_at(&mut self, now: Instant) -> bool {
        if self.state != State::Playing {
            return false;
        }
        loop {
            if self.pending.is_none() {
                match self.stream.as_mut().map(FrameStream::try_next) {
                    Some(Ok(frame)) => self.pending = Some(frame),
                    Some(Err(TryRecvError::Empty)) => return false,
                    Some(Err(TryRecvError::Disconnected)) | None => {
                        self.end_of_run(now);
                        return false;
                    }
                }
            }
            let start = *self.start.get_or_insert(now);
            let due = start + self.interval.mul_f64(f64::from(self.due_frames));
            if now < due {
                return false;
            }
            self.delivered = true;
            if now > due + self.interval {
                if let Some(Ok(newer)) = self.stream.as_mut().map(FrameStream::try_next) {
                    self.pending = Some(newer);
                    self.due_frames += 1;
                    self.dropped += 1;
                    continue;
                }
                // The clock moves to this frame. An `Instant` before the start of time stays
                // at the old clock, which cannot happen on a running machine.
                self.start = now
                    .checked_sub(self.interval.mul_f64(f64::from(self.due_frames)))
                    .or(self.start);
            }
            self.due_frames += 1;
            let Some(frame) = self.pending.take() else {
                return false;
            };
            self.current_frame = RgbaImage::from_raw(self.frame.0, self.frame.1, frame);
            return true;
        }
    }

    /// Take the due frame now, see [`Player::poll_at`].
    pub(crate) fn poll(&mut self) -> bool {
        self.poll_at(Instant::now())
    }

    /// Play the clip again at its end until `duration` passed since the open. A slideshow
    /// holds a short animation for its interval this way, instead of a flash of one loop.
    pub(crate) fn play_at_least(&mut self, duration: Duration) {
        self.min_play = Some(duration);
    }

    /// True when the clip starts again at its end: always with `repeat`, and until
    /// `min_play` passed at `now` without it.
    fn repeats_at(&self, now: Instant) -> bool {
        self.repeat
            || self
                .min_play
                .is_some_and(|min_play| now.saturating_duration_since(self.opened) < min_play)
    }

    /// ffmpeg ended at `now`: the clip is over, or the decode failed before its first frame.
    fn end_of_run(&mut self, now: Instant) {
        // A run from the start without a frame failed. A run after a seek without a frame
        // began past the end of the stream, which can be shorter than the container says.
        let failed = !self.delivered && self.base == 0.0;
        if failed && self.pipeline == Pipeline::Gpu {
            // A format that the GPU chain cannot take decodes in software.
            self.pipeline = Pipeline::Software;
            let position = self.base;
            let started = self.start_run(position);
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
            let started = self.start_run(0.0);
            self.fail_if_error(started);
        } else {
            self.stream = None;
            self.state = State::Finished;
        }
    }

    /// True while the player takes frames.
    pub(crate) fn playing(&self) -> bool {
        self.state == State::Playing
    }

    /// True when the clip without a repeat is over, or the decode failed.
    pub(crate) fn finished(&self) -> bool {
        self.state == State::Finished
    }

    /// Stop with `note` as the error of the player.
    fn fail(&mut self, note: String) {
        self.stream = None;
        self.error = Some(note);
        self.state = State::Finished;
    }

    /// Stop when a start of ffmpeg failed.
    fn fail_if_error(&mut self, started: std::io::Result<()>) {
        if let Err(error) = started {
            self.fail(format!("ffmpeg did not start ({error})"));
        }
    }

    /// Pause, or play on with a fresh clock, so that the frames of the pause do not drop.
    pub(crate) fn toggle_pause(&mut self) {
        self.state = match self.state {
            State::Playing => {
                self.base = self.position();
                self.start = None;
                self.due_frames = 0;
                State::Paused
            }
            State::Paused => State::Playing,
            State::Finished => State::Finished,
        };
    }

    /// Move the position by `seconds` and start ffmpeg there. The end of the clip wraps to the
    /// start with `repeat`, and ends the playback without it.
    pub(crate) fn seek_by(&mut self, seconds: f64) {
        if self.error.is_some() {
            return;
        }
        let mut target = (self.position() + seconds).max(0.0);
        let end = (self.info.duration - END_MARGIN).max(0.0);
        if self.info.duration > 0.0 && target > end {
            if !self.repeats_at(Instant::now()) {
                self.stream = None;
                self.state = State::Finished;
                return;
            }
            target = 0.0;
        }
        self.state = State::Playing;
        let started = self.start_run(target);
        self.fail_if_error(started);
    }

    /// The position in seconds: the time of the last due frame.
    pub(crate) fn position(&self) -> f64 {
        self.base + f64::from(self.due_frames) / self.info.fps
    }

    /// True for an animated image, which a slideshow holds for its interval.
    pub(crate) fn is_animation(&self) -> bool {
        self.info.animation
    }

    /// The length of the clip in seconds, or 0 when the container does not tell it.
    pub(crate) fn duration(&self) -> f64 {
        self.info.duration
    }

    /// How long the run loop can wait before the next frame is due at `now`.
    pub(crate) fn time_until_next_frame_at(&self, now: Instant) -> Duration {
        if self.state != State::Playing {
            return PAUSED_POLL;
        }
        match (self.pending.as_ref(), self.start) {
            (Some(_), Some(start)) => (start + self.interval.mul_f64(f64::from(self.due_frames)))
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
    use super::{DECODE_POLL, PAUSED_POLL, Player, SEEK_STEP, VideoInfo};
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
        vec![value; 64 * 48 * 4]
    }

    #[test]
    fn the_open_probes_and_starts_ffmpeg_with_the_frame_box() {
        let decoder = fake_decoder(info_of(640, 480, 10.0));
        let player = player(&decoder, true);
        assert_eq!(decoder.starts().len(), 1);
        let args = decoder.last_args();
        assert!(args.contains("scale_vt=w=64:h=48,"), "{args}");
        assert!(args.contains(" -i /videos/clip.mp4 "), "{args}");
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
    fn frames_show_at_their_due_time_and_a_late_frame_drops() {
        let decoder = fake_decoder(info_of(640, 480, 10.0));
        let mut player = player(&decoder, true);
        let clock = Instant::now();
        assert!(!player.poll_at(clock), "no frame from ffmpeg yet");
        decoder.feed(frame(1));
        decoder.feed(frame(2));
        assert!(player.poll_at(clock), "the first frame starts the clock");
        assert_eq!(player.current_frame.as_ref().unwrap().as_raw()[0], 1);
        assert!(
            !player.poll_at(clock + FRAME / 2),
            "the second frame is not due"
        );
        assert_eq!(
            player.time_until_next_frame_at(clock + FRAME / 2),
            FRAME / 2
        );
        assert!(player.poll_at(clock + FRAME));
        assert_eq!(player.current_frame.as_ref().unwrap().as_raw()[0], 2);
        assert!((player.position() - 0.2).abs() < 1e-9);

        // Frame 3 is due at 200 ms. At 350 ms it is more than one interval late and drops.
        decoder.feed(frame(3));
        decoder.feed(frame(4));
        assert!(
            player.poll_at(clock + FRAME * 7 / 2),
            "frame 4 shows instead"
        );
        assert_eq!(player.current_frame.as_ref().unwrap().as_raw()[0], 4);
        assert_eq!(player.dropped, 1);
        assert!((player.position() - 0.4).abs() < 1e-9);
    }

    #[test]
    fn a_slow_decode_moves_the_clock_and_plays_slower_instead_of_frozen() {
        let decoder = fake_decoder(info_of(640, 480, 10.0));
        let mut player = player(&decoder, true);
        let clock = Instant::now();
        decoder.feed(frame(1));
        assert!(player.poll_at(clock));
        decoder.feed(frame(2));
        assert!(player.poll_at(clock + FRAME));
        // Frame 3 is due at 200 ms and comes alone at 350 ms: no newer frame waits.
        decoder.feed(frame(3));
        assert!(
            player.poll_at(clock + FRAME * 7 / 2),
            "the late frame shows"
        );
        assert_eq!(player.current_frame.as_ref().unwrap().as_raw()[0], 3);
        assert_eq!(player.dropped, 0);
        // The clock moved: frame 4 is due one interval after frame 3, at 450 ms.
        decoder.feed(frame(4));
        assert!(!player.poll_at(clock + FRAME * 4));
        assert_eq!(
            player.time_until_next_frame_at(clock + FRAME * 4),
            FRAME / 2
        );
        assert!(player.poll_at(clock + FRAME * 9 / 2));
        assert_eq!(player.current_frame.as_ref().unwrap().as_raw()[0], 4);
        assert!((player.position() - 0.4).abs() < 1e-9);
        // Every later frame is on time again at the new clock.
        for value in 5..15 {
            decoder.feed(frame(value));
            assert!(
                player.poll_at(clock + FRAME * 9 / 2 + FRAME * u32::from(value - 4)),
                "frame {value}"
            );
        }
        assert_eq!(player.dropped, 0);
    }

    #[test]
    fn a_seek_past_the_end_of_the_stream_ends_the_clip_without_an_error() {
        for repeat in [false, true] {
            let mut info = info_of(640, 480, 10.0);
            info.duration = 20.0;
            let decoder = fake_decoder(info);
            let mut player = player(&decoder, repeat);
            player.seek_by(SEEK_STEP);
            assert_eq!(decoder.starts().len(), 2);
            decoder.end();
            assert!(!player.poll_at(Instant::now()));
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
    fn a_pause_holds_the_frames_and_the_resume_starts_a_fresh_clock() {
        let decoder = fake_decoder(info_of(640, 480, 10.0));
        let mut player = player(&decoder, true);
        let clock = Instant::now();
        decoder.feed(frame(1));
        assert!(player.poll_at(clock));
        player.toggle_pause();
        assert!(!player.playing());
        decoder.feed(frame(2));
        assert!(!player.poll_at(clock + FRAME * 10));
        assert_eq!(player.time_until_next_frame_at(clock), PAUSED_POLL);
        assert!((player.position() - 0.1).abs() < 1e-9, "the position stays");

        player.toggle_pause();
        assert!(player.playing());
        assert!(
            player.poll_at(clock + FRAME * 20),
            "the first frame after the pause is on time"
        );
        assert_eq!(player.current_frame.as_ref().unwrap().as_raw()[0], 2);
        assert_eq!(player.dropped, 0);
        assert!((player.position() - 0.2).abs() < 1e-9);
    }

    #[test]
    fn a_seek_restarts_ffmpeg_at_the_new_position() {
        let mut info = info_of(640, 480, 10.0);
        info.duration = 20.0;
        let decoder = fake_decoder(info);
        let mut player = player(&decoder, true);
        let clock = Instant::now();
        decoder.feed(frame(1));
        assert!(player.poll_at(clock));
        player.seek_by(SEEK_STEP);
        assert_eq!(decoder.starts().len(), 2);
        assert!(
            decoder.last_args().contains(" -ss 5.100 -i "),
            "{}",
            decoder.last_args()
        );
        assert!((player.position() - 5.1).abs() < 1e-9);
        assert!(player.playing());
        player.seek_by(-SEEK_STEP);
        assert!(
            decoder.last_args().contains(" -ss 0.100 -i "),
            "{}",
            decoder.last_args()
        );
        player.seek_by(-SEEK_STEP);
        assert!(
            !decoder.last_args().contains("-ss"),
            "a seek to 0 needs no -ss"
        );
        assert_eq!(player.position(), 0.0);
        // Past the end with a repeat: the clip starts again.
        for _ in 0..4 {
            player.seek_by(SEEK_STEP);
        }
        assert_eq!(player.position(), 0.0);
        assert_eq!(decoder.starts().len(), 8);
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

        player.play_at_least(Duration::from_millis(300));
        let clock = Instant::now();
        decoder.feed(frame(1));
        assert!(player.poll_at(clock));
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

        decoder.feed(frame(2));
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
            variable_rate: true,
            ..info_of(640, 480, 20.0)
        });
        let mut player = player(&decoder, true);
        let args = decoder.last_args();
        assert!(
            args.contains(" -i /videos/clip.mp4 -fps_mode cfr -r 20.0000 "),
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

        player.toggle_pause();
        player.toggle_pause();
        player.seek_by(1.0);
        let args = decoder.last_args();
        assert!(
            args.contains("-ss 1.500 -i /videos/clip.mp4 -fps_mode cfr -r 20.0000 "),
            "{args}"
        );
        decoder.feed(frame(11));
        assert!(player.poll_at(clock + step * 20));
        assert!((player.position() - 1.55).abs() < 1e-9);
    }

    /// The played clips of the real tests: the name, the player at the end, the frames that
    /// ffmpeg wrote and the played time in seconds. Empty without ffmpeg or libx264.
    fn play_real_clips(real_clock: bool) -> Vec<(&'static str, Player, u32, f64)> {
        use crate::test_support::video::{real_ffmpeg, write_clip};
        ffmpeg::init().unwrap();
        let Some(program) = real_ffmpeg() else {
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
            let (player, frames, seconds) = play_to_end(&program, &path, real_clock);
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

    /// Play `path` to its end with the real ffmpeg. With `real_clock`, the player runs at the
    /// real clock. Else a virtual clock jumps to each due frame, so the scheduler cannot change
    /// the result. Gives the player, the frames that ffmpeg wrote and the played seconds.
    fn play_to_end(program: &Path, path: &Path, real_clock: bool) -> (Player, u32, f64) {
        let decoder: Rc<dyn super::Decoder> = Rc::new(super::SystemDecoder);
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
    fn a_seek_past_the_end_without_repeat_ends_the_clip() {
        let decoder = fake_decoder(info_of(640, 480, 10.0));
        let mut player = player(&decoder, false);
        player.seek_by(5.0);
        assert!(player.finished());
        assert!(!player.playing());
        assert_eq!(decoder.starts().len(), 1, "no new ffmpeg");
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
        player.seek_by(1.0);
        assert_eq!(
            decoder.starts().len(),
            2,
            "a seek on a failed clip starts nothing"
        );
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
