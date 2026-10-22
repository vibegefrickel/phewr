//! The ffmpeg process that decodes a video into raw RGBA frames, and its command line.
//!
//! ffmpeg decodes on the media engine of the Mac (`-hwaccel videotoolbox`), scales and turns
//! the frame on the GPU (`scale_vt`, `transpose_vt`), maps an HDR signal to SDR with the tone
//! mapping cube (`lut3d`) and writes raw RGBA frames into a pipe. A thread reads the pipe one
//! frame at a time, so the run loop never waits for ffmpeg.
//!
//! An animated image decodes in software without the hardware flags, because the AV1 decoder
//! of an AVIS stops with an error when `-hwaccel videotoolbox` is on. Its output is a constant
//! rate (`-fps_mode cfr -r`) at the rate of the probe, so a frame with a long delay goes out
//! more than one time and the pacing of the player stays exact.
//!
//! A video with a variable frame rate gets the same constant output rate at the rate of the
//! probe, see [`super::probe`]. A video with a constant rate gets no `-r`, and ffmpeg writes
//! its frames as they are.

use super::probe::VideoInfo;
use super::tone_map::Hdr;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;

/// How many decoded frames wait in the channel. Two frames keep the pacing exact and hold
/// ffmpeg back during a pause, because the pipe fills up after them.
const FRAME_QUEUE: usize = 2;

/// The filter chain of ffmpeg: on the GPU, or in software for a format that the GPU chain
/// cannot take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pipeline {
    Gpu,
    Software,
}

/// The ffmpeg command line that writes raw RGBA frames of `frame` pixels to stdout.
///
/// `seek` is the start position in seconds. ffmpeg seeks in the input, before the decode.
/// `cube` is the tone mapping cube of an HDR video, see [`super::tone_map::tone_map_file`].
pub(crate) fn args(
    video: &Path,
    info: &VideoInfo,
    frame: (u32, u32),
    cube: Option<&Path>,
    seek: f64,
    pipeline: Pipeline,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostats",
        "-nostdin",
        "-noautorotate",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    if !info.animation {
        args.extend(["-hwaccel", "videotoolbox"].map(OsString::from));
    }
    if pipeline == Pipeline::Gpu {
        args.extend(["-hwaccel_output_format", "videotoolbox_vld"].map(OsString::from));
    }
    if seek > 0.0 {
        args.extend([OsString::from("-ss"), OsString::from(format!("{seek:.3}"))]);
    }
    args.extend([OsString::from("-i"), video.as_os_str().to_owned()]);
    if info.animation {
        let stream = format!("0:{}", info.stream);
        args.extend(["-map", &stream].map(OsString::from));
    }
    if info.animation || info.variable_rate {
        let rate = format!("{:.4}", info.fps);
        args.extend(["-fps_mode", "cfr", "-r", &rate].map(OsString::from));
    }
    args.extend(
        [
            "-an",
            "-sn",
            "-vf",
            &filter_graph(info, frame, cube, pipeline),
        ]
        .map(OsString::from),
    );
    args.extend(["-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"].map(OsString::from));
    args
}

/// The filter graph: the scale, the rotation, the download from the GPU and the tone mapping.
///
/// `frame` is the size on the screen. The scale runs before the rotation, so a portrait clip
/// scales to the swapped size. `-noautorotate` is on, because ffmpeg turns no GPU frame itself.
fn filter_graph(
    info: &VideoInfo,
    frame: (u32, u32),
    cube: Option<&Path>,
    pipeline: Pipeline,
) -> String {
    let (width, height) = if info.rotation.is_multiple_of(180) {
        frame
    } else {
        (frame.1, frame.0)
    };
    let mut graph = match pipeline {
        Pipeline::Gpu => {
            let mut graph = format!("scale_vt=w={width}:h={height}");
            match info.rotation {
                90 => graph.push_str(",transpose_vt=dir=clock"),
                270 => graph.push_str(",transpose_vt=dir=cclock"),
                _ => {}
            }
            let download = if info.ten_bit { "p010le" } else { "nv12" };
            push(&mut graph, format_args!(",hwdownload,format={download}"));
            if info.rotation == 180 {
                graph.push_str(",hflip,vflip");
            }
            graph
        }
        Pipeline::Software => {
            let mut graph = format!("scale=w={width}:h={height}:flags=bilinear");
            match info.rotation {
                90 => graph.push_str(",transpose=clock"),
                180 => graph.push_str(",hflip,vflip"),
                270 => graph.push_str(",transpose=cclock"),
                _ => {}
            }
            graph
        }
    };
    if let (Some(_), Some(cube)) = (info.hdr, cube) {
        push(
            &mut graph,
            format_args!(
                ",format=rgb48le,lut3d=file={}:interp=tetrahedral",
                cube.display()
            ),
        );
    }
    graph
}

/// Append formatted text to the graph.
fn push(graph: &mut String, text: std::fmt::Arguments<'_>) {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "a write to a String cannot fail"
    )]
    let _ = graph.write_fmt(text);
}

/// The tone mapping cube for `info`, in the first of `dirs` that works. `None` for SDR.
pub(crate) fn cube_for(info: &VideoInfo, dirs: &[&Path]) -> Option<PathBuf> {
    let hdr: Hdr = info.hdr?;
    super::tone_map::tone_map_file(dirs, hdr)
}

/// The frames of one ffmpeg run. The channel ends when ffmpeg ends or the stream stops.
pub(crate) struct FrameStream {
    frames: mpsc::Receiver<Vec<u8>>,
    child: Option<Child>,
}

impl FrameStream {
    /// A stream over `frames`. `child` is the ffmpeg process, which the drop kills.
    pub(crate) fn new(frames: mpsc::Receiver<Vec<u8>>, child: Option<Child>) -> Self {
        Self { frames, child }
    }

    /// The next decoded frame, if one waits. `Disconnected` is the end of the run.
    pub(crate) fn try_next(&mut self) -> Result<Vec<u8>, mpsc::TryRecvError> {
        self.frames.try_recv()
    }
}

impl Drop for FrameStream {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a process that already ended cannot be killed, and that is fine"
            )]
            let _ = child.kill();
            #[expect(
                clippy::let_underscore_must_use,
                reason = "the wait only reaps the process; its exit code does not matter"
            )]
            let _ = child.wait();
        }
    }
}

/// Starts ffmpeg and reads the header of a video. The tests swap in a fake that feeds
/// synthetic frames and never starts ffmpeg.
pub(crate) trait Decoder {
    /// The header facts of `video`, see [`super::probe::probe`].
    fn probe(&self, video: &Path) -> io::Result<VideoInfo>;
    /// Start `program` with `args` and return its frames, each `frame_bytes` long.
    fn start(
        &self,
        program: &Path,
        args: &[OsString],
        frame_bytes: usize,
    ) -> io::Result<FrameStream>;
}

/// The [`Decoder`] that starts the real ffmpeg.
pub(crate) struct SystemDecoder;

impl Decoder for SystemDecoder {
    fn probe(&self, video: &Path) -> io::Result<VideoInfo> {
        super::probe::probe(video)
    }

    fn start(
        &self,
        program: &Path,
        args: &[OsString],
        frame_bytes: usize,
    ) -> io::Result<FrameStream> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("ffmpeg has no stdout"))?;
        let (frame_tx, frame_rx) = mpsc::sync_channel(FRAME_QUEUE);
        thread::spawn(move || read_frames(stdout, frame_bytes, &frame_tx));
        Ok(FrameStream::new(frame_rx, Some(child)))
    }
}

/// Read whole frames from `pipe` into `frames`, until the pipe or the receiver ends.
fn read_frames(mut pipe: impl Read, frame_bytes: usize, frames: &mpsc::SyncSender<Vec<u8>>) {
    loop {
        let mut frame = vec![0; frame_bytes];
        if pipe.read_exact(&mut frame).is_err() || frames.send(frame).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Decoder, Pipeline, SystemDecoder, VideoInfo, args, cube_for, read_frames};
    use crate::player::tone_map::Hdr;
    use std::ffi::OsString;
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::Duration;

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn info(rotation: u16, hdr: Option<Hdr>, ten_bit: bool) -> VideoInfo {
        let (width, height) = if rotation.is_multiple_of(180) {
            (3840, 2160)
        } else {
            (2160, 3840)
        };
        VideoInfo {
            width,
            height,
            rotation,
            fps: 30.0,
            duration: 10.0,
            hdr,
            ten_bit,
            animation: false,
            variable_rate: false,
            stream: 0,
        }
    }

    #[test]
    fn an_animated_image_maps_its_stream_and_decodes_in_software_at_a_constant_rate() {
        let mut info = info(0, None, false);
        info.animation = true;
        info.stream = 1;
        info.fps = 12.5;
        let args = args(
            Path::new("/v/anim.avif"),
            &info,
            (640, 360),
            None,
            0.0,
            Pipeline::Software,
        );
        assert_eq!(
            strings(&args),
            [
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostats",
                "-nostdin",
                "-noautorotate",
                "-i",
                "/v/anim.avif",
                "-map",
                "0:1",
                "-fps_mode",
                "cfr",
                "-r",
                "12.5000",
                "-an",
                "-sn",
                "-vf",
                "scale=w=640:h=360:flags=bilinear",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "pipe:1",
            ]
        );
    }

    #[test]
    fn a_variable_rate_video_decodes_on_the_gpu_at_a_constant_rate() {
        let mut info = info(0, None, false);
        info.variable_rate = true;
        info.fps = 60.0;
        let args = strings(&args(
            Path::new("/v/screen.mov"),
            &info,
            (1280, 720),
            None,
            0.0,
            Pipeline::Gpu,
        ));
        let input = args.iter().position(|arg| arg == "-i").unwrap();
        assert_eq!(
            args[input..input + 6],
            ["-i", "/v/screen.mov", "-fps_mode", "cfr", "-r", "60.0000"]
        );
        assert!(args.contains(&"videotoolbox".to_owned()), "{args:?}");
        assert!(!args.contains(&"-map".to_owned()), "{args:?}");
        // A constant-rate video gets no `-r`: see the exact line of the SDR test below.
    }

    /// The `-vf` graph of `args`.
    fn graph(args: &[OsString]) -> String {
        let args = strings(args);
        let position = args.iter().position(|arg| arg == "-vf").unwrap();
        args[position + 1].clone()
    }

    /// True when the ffmpeg `program` lists the decoder `name`.
    fn has_decoder(program: &Path, name: &str) -> bool {
        std::process::Command::new(program)
            .args(["-hide_banner", "-decoders"])
            .output()
            .is_ok_and(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .split_whitespace()
                    .any(|word| word == name)
            })
    }

    /// Decode the animated fixtures with the real ffmpeg. Without ffmpeg on PATH, the test
    /// skips with a note.
    #[test]
    fn the_real_ffmpeg_decodes_every_frame_of_the_animated_fixtures() {
        use crate::test_support::fixture;
        ffmpeg::init().unwrap();
        let Some(program) = crate::test_support::video::real_ffmpeg() else {
            return;
        };
        // The GIF holds 3 frames with delays of 100, 500 and 200 ms: 8 frames at 10 per second.
        for (name, expected) in [
            ("anim.gif", 8),
            ("anim.webp", 3),
            ("anim.png", 3),
            ("anim.avif", 3),
        ] {
            if name == "anim.webp" && !has_decoder(&program, "webp_anim") {
                println!("note: skipped {name}, because ffmpeg below 9.0 has no webp_anim decoder");
                continue;
            }
            let path = fixture(name);
            let info = SystemDecoder.probe(&path).unwrap();
            assert!(info.animation, "{name}: {info:?}");
            let args = args(&path, &info, (16, 12), None, 0.0, Pipeline::Software);
            let mut stream = SystemDecoder.start(&program, &args, 16 * 12 * 4).unwrap();
            let mut frames = Vec::new();
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            loop {
                match stream.try_next() {
                    Ok(frame) => frames.push(frame),
                    Err(mpsc::TryRecvError::Empty) => {
                        assert!(std::time::Instant::now() < deadline, "{name}: ffmpeg hangs");
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(mpsc::TryRecvError::Disconnected) => break,
                }
            }
            assert_eq!(frames.len(), expected, "{name}");
            assert!(
                frames
                    .first()
                    .is_some_and(|frame| frame.iter().any(|&byte| byte != 0)),
                "{name}: the first frame is black"
            );
        }
    }

    #[test]
    fn an_sdr_video_decodes_on_the_gpu_without_a_cube() {
        let args = args(
            Path::new("/v/clip.mp4"),
            &info(0, None, false),
            (1280, 720),
            None,
            0.0,
            Pipeline::Gpu,
        );
        assert_eq!(
            strings(&args),
            [
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostats",
                "-nostdin",
                "-noautorotate",
                "-hwaccel",
                "videotoolbox",
                "-hwaccel_output_format",
                "videotoolbox_vld",
                "-i",
                "/v/clip.mp4",
                "-an",
                "-sn",
                "-vf",
                "scale_vt=w=1280:h=720,hwdownload,format=nv12",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "pipe:1",
            ]
        );
    }

    #[test]
    fn hlg_and_pq_videos_get_the_cube_after_the_download_in_16_bit() {
        for hdr in [Hdr::Hlg, Hdr::Pq] {
            let args = args(
                Path::new("a.mov"),
                &info(0, Some(hdr), true),
                (1920, 1080),
                Some(Path::new("/cache/tonemap.cube")),
                0.0,
                Pipeline::Gpu,
            );
            assert_eq!(
                graph(&args),
                "scale_vt=w=1920:h=1080,hwdownload,format=p010le,\
                 format=rgb48le,lut3d=file=/cache/tonemap.cube:interp=tetrahedral"
            );
        }
        // Without a cube file, an HDR video plays without the tone mapping.
        let args = args(
            Path::new("a.mov"),
            &info(0, Some(Hdr::Hlg), true),
            (1920, 1080),
            None,
            0.0,
            Pipeline::Gpu,
        );
        assert_eq!(
            graph(&args),
            "scale_vt=w=1920:h=1080,hwdownload,format=p010le"
        );
    }

    #[test]
    fn a_rotated_video_scales_to_the_swapped_size_and_turns_on_the_gpu() {
        let portrait = args(
            Path::new("a.mov"),
            &info(90, None, true),
            (608, 1080),
            None,
            0.0,
            Pipeline::Gpu,
        );
        assert_eq!(
            graph(&portrait),
            "scale_vt=w=1080:h=608,transpose_vt=dir=clock,hwdownload,format=p010le"
        );
        let other_way = args(
            Path::new("a.mov"),
            &info(270, None, false),
            (608, 1080),
            None,
            0.0,
            Pipeline::Gpu,
        );
        assert_eq!(
            graph(&other_way),
            "scale_vt=w=1080:h=608,transpose_vt=dir=cclock,hwdownload,format=nv12"
        );
        let upside_down = args(
            Path::new("a.mov"),
            &info(180, None, false),
            (1920, 1080),
            None,
            0.0,
            Pipeline::Gpu,
        );
        assert_eq!(
            graph(&upside_down),
            "scale_vt=w=1920:h=1080,hwdownload,format=nv12,hflip,vflip"
        );
    }

    #[test]
    fn the_software_chain_scales_and_turns_without_the_gpu_filters() {
        let args = args(
            Path::new("a.mov"),
            &info(90, Some(Hdr::Pq), true),
            (608, 1080),
            Some(Path::new("/cache/pq.cube")),
            0.0,
            Pipeline::Software,
        );
        let text = strings(&args);
        assert!(
            !text.contains(&"-hwaccel_output_format".to_owned()),
            "{text:?}"
        );
        assert!(
            text.contains(&"videotoolbox".to_owned()),
            "the decode stays on the media engine"
        );
        assert_eq!(
            graph(&args),
            "scale=w=1080:h=608:flags=bilinear,transpose=clock,\
             format=rgb48le,lut3d=file=/cache/pq.cube:interp=tetrahedral"
        );
    }

    #[test]
    fn a_seek_goes_before_the_input() {
        let args = strings(&args(
            Path::new("a.mov"),
            &info(0, None, false),
            (1280, 720),
            None,
            12.5,
            Pipeline::Gpu,
        ));
        let seek = args.iter().position(|arg| arg == "-ss").unwrap();
        let input = args.iter().position(|arg| arg == "-i").unwrap();
        assert_eq!(args[seek + 1], "12.500");
        assert!(seek < input, "{args:?}");
    }

    #[test]
    fn only_an_hdr_video_gets_a_cube() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(cube_for(&info(0, None, false), &[dir.path()]), None);
        let cube = cube_for(&info(0, Some(Hdr::Hlg), true), &[dir.path()]).unwrap();
        assert_eq!(cube, dir.path().join("tonemap-hlg-v2.cube"));
    }

    #[test]
    fn the_reader_cuts_the_pipe_into_frames_and_drops_a_short_tail() {
        let (frame_tx, frame_rx) = mpsc::sync_channel(8);
        let pipe: &[u8] = &[1, 1, 1, 1, 2, 2, 2, 2, 3, 3];
        read_frames(pipe, 4, &frame_tx);
        drop(frame_tx);
        assert_eq!(frame_rx.recv(), Ok(vec![1, 1, 1, 1]));
        assert_eq!(frame_rx.recv(), Ok(vec![2, 2, 2, 2]));
        assert!(frame_rx.recv().is_err(), "the tail of 2 bytes is no frame");
    }

    #[test]
    fn the_system_decoder_streams_the_output_of_a_program_and_ends_with_it() {
        let shell = Path::new("/bin/sh");
        let args = [OsString::from("-c"), OsString::from("printf 'abcdefgh'")];
        let mut stream = SystemDecoder.start(shell, &args, 4).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut frames = Vec::new();
        loop {
            match stream.try_next() {
                Ok(frame) => frames.push(frame),
                Err(mpsc::TryRecvError::Empty) => {
                    assert!(std::time::Instant::now() < deadline, "no end of the stream");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        }
        assert_eq!(frames, [b"abcd".to_vec(), b"efgh".to_vec()]);
        assert!(
            SystemDecoder
                .start(Path::new("/nonexistent/ffmpeg"), &[], 4)
                .is_err()
        );
    }

    #[test]
    fn a_dropped_stream_kills_its_process() {
        let shell = Path::new("/bin/sh");
        let args = [OsString::from("-c"), OsString::from("sleep 30")];
        let stream = SystemDecoder.start(shell, &args, 4).unwrap();
        let started = std::time::Instant::now();
        drop(stream);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the drop waits for the kill, not for the sleep"
        );
    }
}
