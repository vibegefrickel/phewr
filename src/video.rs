use image::RgbaImage;
use ratatui::layout::Rect;
use std::io;
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov", "mkv", "avi", "webm", "m4v"];
const FRAME_BUFFER_SIZE: usize = 3;
const MAX_DISPLAY_FPS: f64 = 10.0;

pub(crate) fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

struct VideoFrame {
    image: RgbaImage,
    pts: f64,
}

enum VideoCmd {
    Play,
    Pause,
    Stop,
}

pub(crate) struct VideoPlayback {
    frame_rx: mpsc::Receiver<VideoFrame>,
    cmd_tx: mpsc::Sender<VideoCmd>,
    pub playing: bool,
    /// The decoder thread ended: the clip without a repeat is over, or the decode failed.
    pub finished: bool,
    pub current_pts: f64,
    pub duration: f64,
    pub fps: f64,
    pub frame_interval: Duration,
    pub last_frame_time: Instant,
    pub current_frame: Option<RgbaImage>,
    _thread: Option<thread::JoinHandle<()>>,
}

impl VideoPlayback {
    /// Start the decoder thread. Without `repeat`, the clip plays one time and then `finished`
    /// turns true.
    pub(crate) fn open(
        path: &Path,
        rect: Rect,
        cell_px: (u32, u32),
        repeat: bool,
    ) -> io::Result<Self> {
        let target_w = (u32::from(rect.width) * cell_px.0).max(1);
        let target_h = (u32::from(rect.height) * cell_px.1).max(1);

        let input = ffmpeg::format::input(path)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no video stream found"))?;

        let reported_fps = {
            let rate = stream.avg_frame_rate();
            if rate.denominator() > 0 {
                f64::from(rate.numerator()) / f64::from(rate.denominator())
            } else {
                24.0
            }
        };
        let fps = sanitize_fps(reported_fps);
        let duration = if input.duration() > 0 {
            input.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE)
        } else {
            0.0
        };

        drop(input);

        let (frame_tx, frame_rx) = mpsc::sync_channel(FRAME_BUFFER_SIZE);
        let (cmd_tx, cmd_rx) = mpsc::channel();

        let path_owned = path.to_path_buf();
        let handle = thread::spawn(move || {
            decode_loop(
                &path_owned,
                (target_w, target_h),
                repeat,
                &frame_tx,
                &cmd_rx,
            );
        });

        let display_fps = fps.min(MAX_DISPLAY_FPS);
        let frame_interval = Duration::from_secs_f64(1.0 / display_fps);

        Ok(Self {
            frame_rx,
            cmd_tx,
            playing: true,
            finished: false,
            current_pts: 0.0,
            duration,
            fps,
            frame_interval,
            last_frame_time: Instant::now(),
            current_frame: None,
            _thread: Some(handle),
        })
    }

    pub(crate) fn poll_frame(&mut self) -> bool {
        if !self.playing {
            return false;
        }

        if self.last_frame_time.elapsed() < self.frame_interval {
            return false;
        }

        match self.frame_rx.try_recv() {
            Ok(frame) => {
                self.current_pts = frame.pts;
                self.current_frame = Some(frame.image);
                self.last_frame_time = Instant::now();
                true
            }
            Err(mpsc::TryRecvError::Empty) => false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.playing = false;
                self.finished = true;
                false
            }
        }
    }

    pub(crate) fn toggle_pause(&mut self) {
        self.playing = !self.playing;
        if self.playing {
            self.last_frame_time = Instant::now();
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a closed receiver means that nobody waits for the result"
            )]
            let _ = self.cmd_tx.send(VideoCmd::Play);
        } else {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a closed receiver means that nobody waits for the result"
            )]
            let _ = self.cmd_tx.send(VideoCmd::Pause);
        }
    }

    pub(crate) fn time_until_next_frame(&self) -> Duration {
        if !self.playing {
            return Duration::from_millis(250);
        }
        self.frame_interval
            .saturating_sub(self.last_frame_time.elapsed())
    }

    pub(crate) fn stop(&self) {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a closed receiver means that nobody waits for the result"
        )]
        let _ = self.cmd_tx.send(VideoCmd::Stop);
    }
}

fn sanitize_fps(fps: f64) -> f64 {
    if fps.is_finite() && fps > 0.0 {
        fps
    } else {
        24.0
    }
}

impl Drop for VideoPlayback {
    fn drop(&mut self) {
        self.stop();
    }
}

fn fit_aspect(src_w: u32, src_h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if src_w == 0 || src_h == 0 {
        return (max_w, max_h);
    }
    let scale_w = f64::from(max_w) / f64::from(src_w);
    let scale_h = f64::from(max_h) / f64::from(src_h);
    let scale = scale_w.min(scale_h);
    let w = (f64::from(src_w) * scale).round() as u32;
    let h = (f64::from(src_h) * scale).round() as u32;
    (w.max(1), h.max(1))
}

/// Decode the clip into `frame_tx`, fitted into `target`. With `repeat`, the clip starts
/// again at its end. The thread ends at a stop, at the end without `repeat`, or at an error.
fn decode_loop(
    path: &Path,
    target: (u32, u32),
    repeat: bool,
    frame_tx: &mpsc::SyncSender<VideoFrame>,
    cmd_rx: &mpsc::Receiver<VideoCmd>,
) {
    let (target_w, target_h) = target;
    let Ok(mut input) = ffmpeg::format::input(path) else {
        return;
    };
    let Some(stream) = input.streams().best(ffmpeg::media::Type::Video) else {
        return;
    };
    let stream_index = stream.index();
    let time_base = stream.time_base();

    let Ok(context) = ffmpeg::codec::context::Context::from_parameters(stream.parameters()) else {
        return;
    };
    let Ok(mut decoder) = context.decoder().video() else {
        return;
    };

    let (fit_w, fit_h) = fit_aspect(decoder.width(), decoder.height(), target_w, target_h);

    let Ok(mut scaler) = ffmpeg::software::scaling::Context::get(
        decoder.format(),
        decoder.width(),
        decoder.height(),
        ffmpeg::util::format::pixel::Pixel::RGBA,
        fit_w,
        fit_h,
        ffmpeg::software::scaling::Flags::BILINEAR,
    ) else {
        return;
    };

    loop {
        // A pass reads the whole clip, because a pause waits inside the packet loop.
        let mut got_frame = false;
        for (stream, packet) in input.packets() {
            if !follow_commands(cmd_rx) {
                return;
            }

            if stream.index() != stream_index || decoder.send_packet(&packet).is_err() {
                continue;
            }

            if !send_ready_frames(
                &mut decoder,
                &mut scaler,
                time_base,
                frame_tx,
                &mut got_frame,
            ) {
                return;
            }
        }

        #[expect(
            clippy::let_underscore_must_use,
            reason = "a failed flush only means that no frames are left; the loop below ends"
        )]
        let _ = decoder.send_eof();
        if !send_ready_frames(
            &mut decoder,
            &mut scaler,
            time_base,
            frame_tx,
            &mut got_frame,
        ) {
            return;
        }

        if !repeat || input.seek(0, ..).is_err() {
            return;
        }
        decoder.flush();

        if !got_frame {
            return;
        }
    }
}

/// Apply the queued commands before the next packet. A pause blocks here, so it loses no
/// packet. `false` means stop.
fn follow_commands(cmd_rx: &mpsc::Receiver<VideoCmd>) -> bool {
    let mut paused = false;
    loop {
        let cmd = if paused {
            cmd_rx.recv().ok()
        } else {
            cmd_rx.try_recv().ok()
        };
        match cmd {
            Some(VideoCmd::Stop) => return false,
            Some(VideoCmd::Pause) => paused = true,
            Some(VideoCmd::Play) => paused = false,
            None => return !paused,
        }
    }
}

/// Scale and send each frame that the decoder has ready. `got_frame` counts only sent frames,
/// so a clip without a usable frame ends the loop. `false` means that the receiver is gone.
fn send_ready_frames(
    decoder: &mut ffmpeg::decoder::Video,
    scaler: &mut ffmpeg::software::scaling::Context,
    time_base: ffmpeg::Rational,
    frame_tx: &mpsc::SyncSender<VideoFrame>,
    got_frame: &mut bool,
) -> bool {
    let mut decoded = ffmpeg::util::frame::video::Video::empty();
    while decoder.receive_frame(&mut decoded).is_ok() {
        let mut rgba_frame = ffmpeg::util::frame::video::Video::empty();
        if scaler.run(&decoded, &mut rgba_frame).is_err() {
            continue;
        }
        let Some(frame) = extract_frame(&rgba_frame, &decoded, time_base) else {
            continue;
        };
        if frame_tx.send(frame).is_err() {
            return false;
        }
        *got_frame = true;
    }
    true
}

fn extract_frame(
    rgba_frame: &ffmpeg::util::frame::video::Video,
    decoded: &ffmpeg::util::frame::video::Video,
    time_base: ffmpeg::Rational,
) -> Option<VideoFrame> {
    let w = rgba_frame.width();
    let h = rgba_frame.height();
    let stride = rgba_frame.stride(0);
    let data = rgba_frame.data(0);

    let pixels = if stride == w as usize * 4 {
        data[..w as usize * h as usize * 4].to_vec()
    } else {
        let mut buf = Vec::with_capacity(w as usize * h as usize * 4);
        for row in 0..h as usize {
            let start = row * stride;
            buf.extend_from_slice(&data[start..start + w as usize * 4]);
        }
        buf
    };

    let pts = decoded.pts().map_or(0.0, |p| {
        p as f64 * f64::from(time_base.numerator()) / f64::from(time_base.denominator())
    });

    RgbaImage::from_raw(w, h, pixels).map(|image| VideoFrame { image, pts })
}

/// The first frame of `path`, fitted into `target` pixels, for a grid thumbnail.
pub(crate) fn decode_first_frame(path: &Path, target: (u32, u32)) -> io::Result<RgbaImage> {
    let target_w = target.0.max(1);
    let target_h = target.1.max(1);

    let mut input = ffmpeg::format::input(path)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    let stream = input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no video stream"))?;

    let stream_index = stream.index();

    let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    let mut decoder = context
        .decoder()
        .video()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    let (fit_w, fit_h) = fit_aspect(decoder.width(), decoder.height(), target_w, target_h);

    let mut scaler = ffmpeg::software::scaling::Context::get(
        decoder.format(),
        decoder.width(),
        decoder.height(),
        ffmpeg::util::format::pixel::Pixel::RGBA,
        fit_w,
        fit_h,
        ffmpeg::software::scaling::Flags::BILINEAR,
    )
    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    for (stream, packet) in input.packets() {
        if stream.index() != stream_index {
            continue;
        }
        if decoder.send_packet(&packet).is_err() {
            continue;
        }
        let mut decoded = ffmpeg::util::frame::video::Video::empty();
        if decoder.receive_frame(&mut decoded).is_ok() {
            let mut rgba_frame = ffmpeg::util::frame::video::Video::empty();
            scaler
                .run(&decoded, &mut rgba_frame)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

            let w = rgba_frame.width();
            let h = rgba_frame.height();
            let stride = rgba_frame.stride(0);
            let data = rgba_frame.data(0);

            let pixels = if stride == w as usize * 4 {
                data[..w as usize * h as usize * 4].to_vec()
            } else {
                let mut buf = Vec::with_capacity(w as usize * h as usize * 4);
                for row in 0..h as usize {
                    let start = row * stride;
                    buf.extend_from_slice(&data[start..start + w as usize * 4]);
                }
                buf
            };

            return RgbaImage::from_raw(w, h, pixels)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid frame"));
        }
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "no frames decoded",
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        VideoCmd, VideoFrame, VideoPlayback, decode_first_frame, decode_loop, fit_aspect, is_video,
        sanitize_fps,
    };
    use crate::app::{App, ViewMode};
    use crate::image_list::SharedImageList;
    use ratatui::layout::Rect;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::mpsc;
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    /// The longest wait for a frame of the decoder thread.
    const DEADLINE: Duration = Duration::from_secs(10);

    /// Give each test its own directory. It goes away on drop, also after a panic.
    fn test_dir() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    /// Run the ffmpeg program with `args`. `false` means that ffmpeg is missing or failed.
    fn run_ffmpeg(args: &[&str]) -> bool {
        ffmpeg::init().unwrap();
        let status = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(args)
            .status();
        if status.as_ref().is_ok_and(std::process::ExitStatus::success) {
            return true;
        }
        println!("note: skipped, because the ffmpeg program is missing or failed: {status:?}");
        false
    }

    /// Make a 64x48 clip of `seconds` at 10 frames per second, or `None` without ffmpeg.
    fn test_clip(dir: &Path, seconds: &str) -> Option<PathBuf> {
        let path = dir.join("clip.mp4");
        let path_text = path.to_string_lossy();
        let args = [
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=64x48:rate=10",
            "-t",
            seconds,
            "-c:v",
            "mpeg4",
            "-pix_fmt",
            "yuv420p",
            &path_text,
        ];
        run_ffmpeg(&args).then_some(path)
    }

    /// Poll until a frame arrives, or fail after [`DEADLINE`].
    fn wait_for_frame(playback: &mut VideoPlayback) {
        let start = Instant::now();
        while !playback.poll_frame() {
            assert!(start.elapsed() < DEADLINE, "no frame within {DEADLINE:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn invalid_frame_rates_use_the_fallback() {
        for fps in [0.0, -1.0, f64::INFINITY, f64::NAN] {
            assert_eq!(sanitize_fps(fps), 24.0);
        }
        assert_eq!(sanitize_fps(29.97), 29.97);
    }

    #[test]
    fn aspect_ratio_fits_within_the_target() {
        assert_eq!(fit_aspect(1920, 1080, 800, 600), (800, 450));
        assert_eq!(fit_aspect(1080, 1920, 800, 600), (338, 600));
        assert_eq!(fit_aspect(0, 1080, 800, 600), (800, 600));
        assert_eq!(fit_aspect(10_000, 1, 10, 10), (10, 1));
    }

    #[test]
    fn video_extensions_ignore_the_case() {
        for name in ["a.mp4", "b.MOV", "c.mkv", "d.avi", "e.webm", "f.M4V"] {
            assert!(is_video(Path::new(name)), "{name}");
        }
        for name in ["a.jpg", "mp4", "clip.mp4.txt", ""] {
            assert!(!is_video(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn first_frame_fits_the_target_rectangle() {
        let dir = test_dir();
        let Some(clip) = test_clip(dir.path(), "0.5") else {
            return;
        };
        let frame = decode_first_frame(&clip, (32, 48)).unwrap();
        let tiny = decode_first_frame(&clip, (0, 0)).unwrap();
        assert_eq!(frame.dimensions(), (32, 24));
        assert_eq!(tiny.dimensions(), (1, 1));
    }

    #[test]
    fn files_without_a_video_stream_give_an_error() {
        let dir = test_dir();
        let text = dir.path().join("notes.mp4");
        std::fs::write(&text, "not a video").unwrap();
        let rect = Rect::new(0, 0, 4, 3);
        assert!(decode_first_frame(&text, (32, 48)).is_err());
        assert!(VideoPlayback::open(&text, rect, (8, 16), true).is_err());

        let audio = dir.path().join("tone.m4a");
        let audio_text = audio.to_str().unwrap();
        if run_ffmpeg(&["-f", "lavfi", "-i", "sine=duration=0.2", audio_text]) {
            let error = decode_first_frame(&audio, (32, 48)).unwrap_err();
            assert_eq!(error.to_string(), "no video stream");
            let error = VideoPlayback::open(&audio, rect, (8, 16), true)
                .err()
                .unwrap();
            assert_eq!(error.to_string(), "no video stream found");
        }
    }

    /// Send the stop command and poll until the decoder thread ends, or fail after [`DEADLINE`].
    fn stop_and_drain(playback: &mut VideoPlayback) {
        playback.stop();
        let start = Instant::now();
        while playback.playing {
            assert!(start.elapsed() < DEADLINE, "the decoder did not stop");
            playback.poll_frame();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn player_delivers_a_fitted_frame_and_pauses() {
        let dir = test_dir();
        let Some(clip) = test_clip(dir.path(), "0.5") else {
            return;
        };
        let mut playback =
            VideoPlayback::open(&clip, Rect::new(0, 0, 8, 6), (8, 16), true).unwrap();
        assert!(playback.playing);
        assert!((playback.fps - 10.0).abs() < 0.01, "fps: {}", playback.fps);
        assert!(playback.duration > 0.4, "duration: {}", playback.duration);
        assert_eq!(playback.frame_interval, Duration::from_millis(100));

        wait_for_frame(&mut playback);
        let frame = playback.current_frame.as_ref().unwrap();
        assert_eq!(frame.dimensions(), (64, 48));
        // A long interval keeps the next assertions free of the scheduler.
        playback.frame_interval = Duration::from_secs(60);
        assert!(playback.time_until_next_frame() > Duration::from_secs(59));
        assert!(
            !playback.poll_frame(),
            "the next frame waits for the interval"
        );

        playback.toggle_pause();
        assert!(!playback.playing);
        assert!(!playback.poll_frame());
        assert_eq!(playback.time_until_next_frame(), Duration::from_millis(250));
        playback.last_frame_time -= Duration::from_secs(30);
        playback.toggle_pause();
        assert!(playback.playing);
        assert!(
            playback.time_until_next_frame() > Duration::from_secs(59),
            "a resume starts the interval again"
        );

        playback.frame_interval = Duration::ZERO;
        stop_and_drain(&mut playback);
    }

    #[test]
    fn player_starts_the_clip_again_at_the_end() {
        let dir = test_dir();
        let Some(clip) = test_clip(dir.path(), "0.5") else {
            return;
        };
        let mut playback =
            VideoPlayback::open(&clip, Rect::new(0, 0, 8, 6), (8, 16), true).unwrap();
        // Without the interval, the frames arrive as fast as the decoder makes them.
        playback.frame_interval = Duration::ZERO;
        wait_for_frame(&mut playback);
        let mut last_pts = playback.current_pts;
        let mut looped = false;
        while !looped {
            wait_for_frame(&mut playback);
            looped = playback.current_pts < last_pts;
            last_pts = playback.current_pts;
        }
        stop_and_drain(&mut playback);
    }

    /// Run `decode_loop` with a repeat in a thread. The frame channel has no buffer, so the
    /// decoder waits at each frame until the test receives it. After the test receives a frame,
    /// the decoder reads a command before the next packet or before the packet after it.
    fn start_decoder(
        clip: &Path,
    ) -> (
        mpsc::Receiver<VideoFrame>,
        mpsc::Sender<VideoCmd>,
        JoinHandle<()>,
    ) {
        start_decoder_with(clip, true)
    }

    /// Run `decode_loop` in a thread, with or without `repeat`.
    fn start_decoder_with(
        clip: &Path,
        repeat: bool,
    ) -> (
        mpsc::Receiver<VideoFrame>,
        mpsc::Sender<VideoCmd>,
        JoinHandle<()>,
    ) {
        let (frame_tx, frame_rx) = mpsc::sync_channel(0);
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let clip = clip.to_path_buf();
        let handle =
            std::thread::spawn(move || decode_loop(&clip, (64, 48), repeat, &frame_tx, &cmd_rx));
        (frame_rx, cmd_tx, handle)
    }

    /// Receive the next frame as its frame number at 10 frames per second, or fail after
    /// [`DEADLINE`].
    fn next_frame(frames: &mpsc::Receiver<VideoFrame>) -> i64 {
        let frame = frames
            .recv_timeout(DEADLINE)
            .unwrap_or_else(|error| panic!("no frame within {DEADLINE:?}: {error}"));
        (frame.pts * 10.0).round() as i64
    }

    /// Pause and resume at once. The test never waits for a frame while the decoder waits for
    /// the resume, and the decoder reads both commands before the same packet.
    fn pause_and_resume(commands: &mpsc::Sender<VideoCmd>) {
        commands.send(VideoCmd::Pause).unwrap();
        commands.send(VideoCmd::Play).unwrap();
    }

    /// Stop the decoder thread and wait until it ends.
    fn stop_decoder(
        frames: mpsc::Receiver<VideoFrame>,
        commands: &mpsc::Sender<VideoCmd>,
        decoder: JoinHandle<()>,
    ) {
        commands.send(VideoCmd::Stop).unwrap();
        // The dropped receiver also ends a decoder that waits to send a frame.
        drop(frames);
        decoder.join().unwrap();
    }

    #[test]
    fn a_pause_at_the_last_packet_keeps_the_loop() {
        let dir = test_dir();
        let Some(clip) = test_clip(dir.path(), "0.5") else {
            return;
        };
        let (frames, commands, decoder) = start_decoder(&clip);
        // Each pass pauses before packet 3 or before packet 4, the last packet. The test cannot
        // select the packet. Only a pause before packet 4 can end a pass without frames, so
        // three passes give that position more chances.
        for _ in 0..3 {
            for expected in 0..3 {
                assert_eq!(next_frame(&frames), expected);
            }
            pause_and_resume(&commands);
            for expected in 3..5 {
                assert_eq!(
                    next_frame(&frames),
                    expected,
                    "the paused packet is not lost"
                );
            }
        }
        assert_eq!(next_frame(&frames), 0, "the clip starts again");
        stop_decoder(frames, &commands, decoder);
    }

    #[test]
    fn a_pause_in_the_middle_loses_no_frame() {
        let dir = test_dir();
        let Some(clip) = test_clip(dir.path(), "1") else {
            return;
        };
        let (frames, commands, decoder) = start_decoder(&clip);
        let mut first_loop: Vec<_> = (0..4).map(|_| next_frame(&frames)).collect();
        // The decoder reads the commands before packet 4 or packet 5 of 10.
        pause_and_resume(&commands);
        let next_loop = loop {
            let frame = next_frame(&frames);
            if frame == 0 {
                break frame;
            }
            first_loop.push(frame);
        };
        assert_eq!(first_loop, (0..10).collect::<Vec<_>>());
        assert_eq!(next_loop, 0);
        stop_decoder(frames, &commands, decoder);
    }

    #[test]
    fn a_pause_holds_the_decoder_until_the_resume() {
        let dir = test_dir();
        let Some(clip) = test_clip(dir.path(), "1") else {
            return;
        };
        let (frames, commands, decoder) = start_decoder(&clip);
        for expected in 0..4 {
            assert_eq!(next_frame(&frames), expected);
        }
        // The decoder reads the pause before packet 4, or it sends frame 4 and reads the pause
        // before packet 5. A third frame shows that the pause has no effect. A slow decoder can
        // only hide that error. It cannot make a correct decoder fail.
        commands.send(VideoCmd::Pause).unwrap();
        let mut held = Vec::new();
        while held.len() < 2
            && let Ok(frame) = frames.recv_timeout(Duration::from_millis(200))
        {
            held.push((frame.pts * 10.0).round() as i64);
        }
        assert!(held.len() < 2, "frames during the pause: {held:?}");
        assert!(held.iter().all(|&frame| frame == 4), "{held:?}");
        commands.send(VideoCmd::Play).unwrap();
        let expected = if held.is_empty() { 4 } else { 5 };
        assert_eq!(next_frame(&frames), expected, "the resume continues");
        stop_decoder(frames, &commands, decoder);
    }

    #[test]
    fn a_clip_without_repeat_ends_after_its_last_frame() {
        let dir = test_dir();
        let Some(clip) = test_clip(dir.path(), "0.5") else {
            return;
        };
        let (frames, commands, decoder) = start_decoder_with(&clip, false);
        for expected in 0..5 {
            assert_eq!(next_frame(&frames), expected);
        }
        assert!(
            frames.recv_timeout(DEADLINE).is_err(),
            "no second pass: the decoder ends"
        );
        decoder.join().unwrap();
        drop(commands);
    }

    #[test]
    fn a_slideshow_moves_on_at_the_end_of_the_clip_in_the_builtin_player() {
        let dir = test_dir();
        let Some(clip) = test_clip(dir.path(), "0.5") else {
            return;
        };
        let photo = dir.path().join("photo.png");
        image::RgbaImage::new(4, 4).save(&photo).unwrap();
        let mut app = App::new((8, 16), SharedImageList::new());
        app.images = vec![clip, photo];
        app.filenames = vec!["clip.mp4".to_owned(), "photo.png".to_owned()];
        app.gallery.filtered_indices = vec![0, 1];
        app.enter_fullscreen_at(0);
        app.start_slideshow();
        app.open_video_if_needed();
        app.video.as_mut().unwrap().frame_interval = Duration::ZERO;

        let start = Instant::now();
        while app.mode == ViewMode::Video {
            assert!(start.elapsed() < DEADLINE, "the clip did not end");
            app.video.as_mut().unwrap().poll_frame();
            app.slideshow_tick();
            std::thread::sleep(Duration::from_millis(5));
        }

        assert_eq!(app.current, 1, "no second pass of the clip");
        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert!(app.slideshow_running());
    }

    #[test]
    fn a_player_without_repeat_reports_the_end_of_the_clip() {
        let dir = test_dir();
        let Some(clip) = test_clip(dir.path(), "0.5") else {
            return;
        };
        let mut playback =
            VideoPlayback::open(&clip, Rect::new(0, 0, 8, 6), (8, 16), false).unwrap();
        playback.frame_interval = Duration::ZERO;
        let start = Instant::now();
        let mut frames = 0;
        while !playback.finished {
            assert!(start.elapsed() < DEADLINE, "the clip did not end");
            frames += usize::from(playback.poll_frame());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(frames, 5, "each frame shows one time");
        assert!(!playback.playing);
    }
}
