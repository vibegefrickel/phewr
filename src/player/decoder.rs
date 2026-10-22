//! The ffmpeg process that decodes a video into raw RGB or RGBA frames, and its command line.
//!
//! ffmpeg decodes on the media engine of the Mac (`-hwaccel videotoolbox`), scales and turns
//! the frame on the GPU (`scale_vt`, `transpose_vt`), maps the colors into the color space of
//! the terminal with a cube (`lut3d`) and writes raw RGB frames into a pipe. A thread reads the pipe one
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
use super::tone_map::{Cube, cube_file};
use crate::decode::color::ColorTarget;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

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

/// The raw pixel format of the frames for ffmpeg, and its bytes per pixel.
///
/// An animation keeps the alpha channel, because a GIF, WebP or APNG frame can hold
/// transparent pixels. A video has none, so RGB saves a quarter of the bytes in the pipe and in
/// the temp file of each frame.
pub(crate) fn pixel_format(info: &VideoInfo) -> (&'static str, usize) {
    if info.animation {
        ("rgba", 4)
    } else {
        ("rgb24", 3)
    }
}

/// The ffmpeg command line that writes raw frames of `frame` pixels to stdout, in the format of
/// [`pixel_format`].
///
/// `seek` is the start position in seconds. ffmpeg seeks in the input, before the decode.
/// `cube` is the color cube of the video, see [`cube_for`].
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
    // The probe read the size of this stream, so ffmpeg must not pick another one.
    let stream = format!("0:{}", info.stream);
    args.extend(["-map", &stream].map(OsString::from));
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
    let (pixels, _) = pixel_format(info);
    args.extend(["-f", "rawvideo", "-pix_fmt", pixels, "pipe:1"].map(OsString::from));
    args
}

/// The command line of [`args`] that writes only the first frame, for a grid thumbnail. The
/// frame is RGBA also for a video, because the grid pads each thumbnail with transparent pixels.
pub(crate) fn first_frame_args(
    video: &Path,
    info: &VideoInfo,
    frame: (u32, u32),
    cube: Option<&Path>,
    pipeline: Pipeline,
) -> Vec<OsString> {
    let mut args = args(video, info, frame, cube, 0.0, pipeline);
    let output = args.pop();
    // [`args`] ends with `-pix_fmt <format> pipe:1`.
    if let Some(format) = args.last_mut() {
        *format = OsString::from("rgba");
    }
    args.extend(["-frames:v", "1"].map(OsString::from));
    args.extend(output);
    args
}

/// The filter graph: the scale, the rotation, the download from the GPU and the color cube.
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
    if let Some(cube) = cube {
        push(
            &mut graph,
            format_args!(
                ",format={},lut3d=file={}:interp=tetrahedral",
                cube_format(info),
                cube.display()
            ),
        );
    }
    graph
}

/// The pixel format in which the cube runs. An SDR cube runs in 8 bit, also for a 10 bit
/// stream: it is the cheapest step, about 1 ms more per 1920x1080 frame, and its values hold
/// the rounding of `lut3d` in 8 bit ([`Cube::runs_in_8_bit`]). An HDR cube runs in 16 bit,
/// because the tone curve spreads the dark values over more levels. An animation keeps its
/// alpha.
fn cube_format(info: &VideoInfo) -> &'static str {
    match (info.animation, info.hdr.is_some()) {
        (false, false) => "rgb24",
        (false, true) => "rgb48le",
        (true, false) => "rgba",
        (true, true) => "rgba64le",
    }
}

/// Append formatted text to the graph.
fn push(graph: &mut String, text: std::fmt::Arguments<'_>) {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "a write to a String cannot fail"
    )]
    let _ = graph.write_fmt(text);
}

/// The color cube that maps `info` into `target`, in the first of `dirs` that works. `None`
/// for an SDR video in the target space.
pub(crate) fn cube_for(info: &VideoInfo, target: ColorTarget, dirs: &[&Path]) -> Option<PathBuf> {
    let cube = Cube::for_video(info.hdr, info.primaries, target)?;
    cube_file(dirs, cube)
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

    /// Wait for the next decoded frame, at most `timeout`. `Disconnected` is the end of the run.
    pub(crate) fn wait_next(&self, timeout: Duration) -> Result<Vec<u8>, mpsc::RecvTimeoutError> {
        self.frames.recv_timeout(timeout)
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

/// Starts ffmpeg and reads the header of a video with ffprobe. The tests swap in a fake that
/// feeds synthetic frames and never starts a process.
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

/// The [`Decoder`] that starts the real ffmpeg and ffprobe.
pub(crate) struct SystemDecoder {
    /// The `ffprobe` program, or `None` when PATH has none.
    ffprobe: Option<PathBuf>,
}

impl SystemDecoder {
    /// A decoder with the `ffprobe` of the lookup, so that a missing program fails the probe
    /// instead of the start of ffmpeg.
    pub(crate) fn new(ffprobe: Option<PathBuf>) -> Self {
        Self { ffprobe }
    }
}

impl Decoder for SystemDecoder {
    fn probe(&self, video: &Path) -> io::Result<VideoInfo> {
        let Some(program) = self.ffprobe.as_deref() else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "ffprobe not found on PATH",
            ));
        };
        super::probe::probe_cached(program, video)
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
    use super::{
        Decoder, Pipeline, SystemDecoder, VideoInfo, args, cube_for, first_frame_args, read_frames,
    };
    use crate::decode::color::{self, ColorTarget, SourceColor};
    use crate::player::tone_map::{Hdr, Primaries};
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
            primaries: if hdr.is_some() {
                Primaries::Bt2020
            } else {
                Primaries::Bt709
            },
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
            args[input..input + 8],
            [
                "-i",
                "/v/screen.mov",
                "-map",
                "0:0",
                "-fps_mode",
                "cfr",
                "-r",
                "60.0000"
            ]
        );
        assert!(args.contains(&"videotoolbox".to_owned()), "{args:?}");
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

    /// Every frame of one real ffmpeg run, within 10 seconds.
    fn all_frames(
        decoder: &SystemDecoder,
        program: &Path,
        args: &[OsString],
        frame_bytes: usize,
    ) -> Vec<Vec<u8>> {
        let mut stream = decoder.start(program, args, frame_bytes).unwrap();
        let mut frames = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            match stream.try_next() {
                Ok(frame) => frames.push(frame),
                Err(mpsc::TryRecvError::Empty) => {
                    assert!(std::time::Instant::now() < deadline, "ffmpeg hangs");
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(mpsc::TryRecvError::Disconnected) => return frames,
            }
        }
    }

    /// The largest difference of one channel between `cubed` and the still conversion of
    /// `plain` from sRGB into Display P3. Both hold RGBA pixels of the same size.
    fn largest_difference_to_the_still_conversion(plain: &[u8], cubed: &[u8]) -> u8 {
        let pixels = u32::try_from(plain.len() / 4).unwrap();
        let image = image::RgbaImage::from_raw(pixels, 1, plain.to_vec()).unwrap();
        let converted = color::convert(image, &SourceColor::Srgb, ColorTarget::DisplayP3);
        converted
            .as_raw()
            .iter()
            .zip(cubed)
            .map(|(still, video)| still.abs_diff(*video))
            .max()
            .unwrap_or(0)
    }

    /// Decode the animated fixtures with the real ffmpeg, without a cube and with the cube into
    /// Display P3. Without ffmpeg on PATH, the test skips with a note.
    #[test]
    fn the_real_ffmpeg_decodes_every_frame_of_the_animated_fixtures() {
        use crate::test_support::fixture;
        use crate::test_support::video::{real_ffmpeg, real_ffprobe};
        let (Some(program), Some(ffprobe)) = (real_ffmpeg(), real_ffprobe()) else {
            return;
        };
        let decoder = SystemDecoder::new(Some(ffprobe));
        let dir = tempfile::tempdir().unwrap();
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
            let info = decoder.probe(&path).unwrap();
            assert!(info.animation, "{name}: {info:?}");
            assert_eq!(info.primaries, Primaries::Bt709, "{name}: sRGB or untagged");
            let plain = args(&path, &info, (16, 12), None, 0.0, Pipeline::Software);
            let frames = all_frames(&decoder, &program, &plain, 16 * 12 * 4);
            assert_eq!(frames.len(), expected, "{name}");
            assert!(
                frames
                    .first()
                    .is_some_and(|frame| frame.iter().any(|&byte| byte != 0)),
                "{name}: the first frame is black"
            );
            let cube = cube_for(&info, ColorTarget::DisplayP3, &[dir.path()]);
            let cubed = args(
                &path,
                &info,
                (16, 12),
                cube.as_deref(),
                0.0,
                Pipeline::Software,
            );
            let converted = all_frames(&decoder, &program, &cubed, 16 * 12 * 4);
            assert_eq!(converted.len(), expected, "{name}");
            for (plain, cubed) in frames.iter().zip(&converted) {
                let difference = largest_difference_to_the_still_conversion(plain, cubed);
                assert!(difference <= 2, "{name}: {difference} levels off the still");
            }
        }
    }

    /// Decode the first frame of a synthetic SDR clip with a red patch, without a cube and with
    /// the cube into Display P3, on the GPU and in software. The clip has 128x96 pixels, because
    /// the hardware decoder rejects a smaller H.264 stream. The cube output matches the still
    /// conversion of the plain frame within 2 levels. Without ffmpeg or libx264, the test skips
    /// with a note.
    #[test]
    fn the_real_ffmpeg_converts_an_sdr_clip_like_a_still_image() {
        use crate::test_support::video::{real_ffmpeg, real_ffprobe};
        let (Some(program), Some(ffprobe)) = (real_ffmpeg(), real_ffprobe()) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let clip = dir.path().join("red.mp4");
        let made = std::process::Command::new(&program)
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("color=c=0x808080:s=128x96:r=10:d=1,drawbox=x=0:y=0:w=64:h=96:color=red:t=fill")
            .args([
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-color_primaries",
                "bt709",
            ])
            .arg(&clip)
            .status()
            .is_ok_and(|status| status.success());
        if !made {
            println!("note: skipped, because ffmpeg cannot encode with libx264");
            return;
        }
        let decoder = SystemDecoder::new(Some(ffprobe));
        let info = decoder.probe(&clip).unwrap();
        assert_eq!((info.primaries, info.hdr), (Primaries::Bt709, None));
        let cube = cube_for(&info, ColorTarget::DisplayP3, &[dir.path()]);
        assert!(cube.is_some());
        let first = |cube: Option<&Path>, pipeline| {
            let args = first_frame_args(&clip, &info, (64, 48), cube, pipeline);
            decoder
                .start(&program, &args, 64 * 48 * 4)
                .unwrap()
                .wait_next(Duration::from_secs(10))
                .ok()
        };
        for pipeline in [Pipeline::Gpu, Pipeline::Software] {
            let (Some(plain), Some(cubed)) =
                (first(None, pipeline), first(cube.as_deref(), pipeline))
            else {
                assert_eq!(pipeline, Pipeline::Gpu, "the software chain gives a frame");
                println!("note: skipped the GPU chain, because it gave no frame");
                continue;
            };
            // The center of the red patch is red before the cube and P3 red after it.
            let red = (24 * 64 + 16) * 4;
            assert!(
                plain[red] > 240 && plain[red + 1] < 15,
                "{:?}",
                &plain[red..red + 3]
            );
            assert!(
                cubed[red] < 240 && cubed[red + 1] > 35,
                "{:?}",
                &cubed[red..red + 3]
            );
            let difference = largest_difference_to_the_still_conversion(&plain, &cubed);
            assert!(
                difference <= 2,
                "{pipeline:?}: {difference} levels off the still"
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
                "-map",
                "0:0",
                "-an",
                "-sn",
                "-vf",
                "scale_vt=w=1280:h=720,hwdownload,format=nv12",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
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
    fn a_video_outside_the_target_space_gets_a_cube() {
        let dir = tempfile::tempdir().unwrap();
        let cube = |info: &VideoInfo, target| cube_for(info, target, &[dir.path()]);
        let sdr = info(0, None, false);
        assert_eq!(cube(&sdr, ColorTarget::Srgb), None, "sRGB needs no step");
        assert_eq!(
            cube(&sdr, ColorTarget::DisplayP3),
            Some(dir.path().join("color-bt709-p3-v3.cube"))
        );
        let p3 = VideoInfo {
            primaries: Primaries::DisplayP3,
            ..sdr
        };
        assert_eq!(cube(&p3, ColorTarget::DisplayP3), None);
        assert_eq!(
            cube(&p3, ColorTarget::Srgb),
            Some(dir.path().join("color-p3-srgb-v3.cube"))
        );
        for (target, name) in [
            (ColorTarget::Srgb, "tonemap-hlg-srgb-v3.cube"),
            (ColorTarget::DisplayP3, "tonemap-hlg-p3-v3.cube"),
        ] {
            assert_eq!(
                cube(&info(0, Some(Hdr::Hlg), true), target),
                Some(dir.path().join(name))
            );
        }
    }

    #[test]
    fn an_sdr_cube_runs_in_8_bit_and_keeps_the_alpha_of_an_animation() {
        let cube = Some(Path::new("/cache/color.cube"));
        let lut = "lut3d=file=/cache/color.cube:interp=tetrahedral";
        let sdr = info(0, None, false);
        let video = args(
            Path::new("a.mp4"),
            &sdr,
            (1280, 720),
            cube,
            0.0,
            Pipeline::Gpu,
        );
        assert_eq!(
            graph(&video),
            format!("scale_vt=w=1280:h=720,hwdownload,format=nv12,format=rgb24,{lut}")
        );
        let ten_bit = info(0, None, true);
        let video = args(
            Path::new("a.mov"),
            &ten_bit,
            (1280, 720),
            cube,
            0.0,
            Pipeline::Gpu,
        );
        assert_eq!(
            graph(&video),
            format!("scale_vt=w=1280:h=720,hwdownload,format=p010le,format=rgb24,{lut}")
        );
        let animation = VideoInfo {
            animation: true,
            ..sdr
        };
        let frames = args(
            Path::new("a.gif"),
            &animation,
            (64, 48),
            cube,
            0.0,
            Pipeline::Software,
        );
        assert_eq!(
            graph(&frames),
            format!("scale=w=64:h=48:flags=bilinear,format=rgba,{lut}")
        );
        let hdr_animation = VideoInfo {
            hdr: Some(Hdr::Pq),
            ..animation
        };
        let frames = args(
            Path::new("a.avif"),
            &hdr_animation,
            (64, 48),
            cube,
            0.0,
            Pipeline::Software,
        );
        assert!(graph(&frames).ends_with(&format!(",format=rgba64le,{lut}")));
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
        let mut stream = SystemDecoder::new(None).start(shell, &args, 4).unwrap();
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
            SystemDecoder::new(None)
                .start(Path::new("/nonexistent/ffmpeg"), &[], 4)
                .is_err()
        );
    }

    #[test]
    fn the_wait_for_a_frame_gives_the_first_frame_and_then_the_end() {
        let shell = Path::new("/bin/sh");
        let args = [OsString::from("-c"), OsString::from("printf 'abcd'")];
        let stream = SystemDecoder::new(None).start(shell, &args, 4).unwrap();
        let wait = Duration::from_secs(10);
        assert_eq!(stream.wait_next(wait), Ok(b"abcd".to_vec()));
        assert_eq!(
            stream.wait_next(wait),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
    }

    #[test]
    fn the_probe_without_ffprobe_is_a_not_found_error() {
        let error = SystemDecoder::new(None)
            .probe(Path::new("clip.mp4"))
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(error.to_string(), "ffprobe not found on PATH");
    }

    #[test]
    fn the_first_frame_command_line_stops_after_one_frame() {
        let args = strings(&first_frame_args(
            Path::new("/v/clip.mov"),
            &info(90, Some(Hdr::Hlg), true),
            (320, 180),
            Some(Path::new("/c/hlg.cube")),
            Pipeline::Gpu,
        ));
        assert_eq!(
            args[args.len() - 7..],
            [
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "-frames:v",
                "1",
                "pipe:1"
            ]
        );
        let player = strings(&super::args(
            Path::new("/v/clip.mov"),
            &info(90, Some(Hdr::Hlg), true),
            (320, 180),
            Some(Path::new("/c/hlg.cube")),
            0.0,
            Pipeline::Gpu,
        ));
        assert_eq!(args[..args.len() - 7], player[..player.len() - 5]);
        assert_eq!(
            player[player.len() - 2],
            "rgb24",
            "the player writes RGB for a video"
        );
    }

    #[test]
    fn a_dropped_stream_kills_its_process() {
        let shell = Path::new("/bin/sh");
        let args = [OsString::from("-c"), OsString::from("sleep 30")];
        let stream = SystemDecoder::new(None).start(shell, &args, 4).unwrap();
        let started = std::time::Instant::now();
        drop(stream);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the drop waits for the kill, not for the sleep"
        );
    }
}
