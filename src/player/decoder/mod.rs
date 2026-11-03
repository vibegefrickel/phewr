//! The ffmpeg process that decodes a video into raw RGB or RGBA frames.
//!
//! [`command`] builds the command line. [`process`] reads the frames from the pipe on a
//! thread, and the run loop never waits for ffmpeg. A GPU run without a frame runs again in
//! software ([`Pipeline`]).

mod command;
mod process;

pub(crate) use command::{LoopSampling, args, cube_for, first_frame_args, loop_args, pixel_format};
pub(crate) use process::{FrameStream, PooledFrame, SystemDecoder};

use super::probe::VideoInfo;
use std::ffi::OsString;
use std::io;
use std::path::Path;

/// The ffmpeg filter chain: on the GPU, or in software for a format that the GPU cannot take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pipeline {
    Gpu,
    Software,
}

impl Pipeline {
    /// An animation decodes in software only.
    pub(crate) fn first(info: &VideoInfo) -> Self {
        if info.is_animation() {
            Self::Software
        } else {
            Self::Gpu
        }
    }

    /// The pipeline after a run without a frame, or `None` after the last one.
    pub(crate) fn next(self, info: &VideoInfo) -> Option<Self> {
        (self == Self::Gpu && !info.is_animation()).then_some(Self::Software)
    }
}

/// The result of `attempt` on the first pipeline that gives a frame. An error stops the
/// fallback.
pub(crate) fn with_fallback<T>(
    info: &VideoInfo,
    mut attempt: impl FnMut(Pipeline) -> io::Result<Option<T>>,
) -> io::Result<T> {
    let mut pipeline = Some(Pipeline::first(info));
    while let Some(current) = pipeline {
        if let Some(result) = attempt(current)? {
            return Ok(result);
        }
        pipeline = current.next(info);
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "no frames decoded",
    ))
}

/// Starts ffmpeg and reads the video header with ffprobe. The tests use a fake.
pub(crate) trait Decoder {
    fn probe(&self, video: &Path) -> io::Result<VideoInfo>;
    /// False when ffprobe is missing. Each probe then fails.
    fn has_ffprobe(&self) -> bool {
        true
    }
    /// Start `program` with `args`. Each frame is `frame_bytes` long.
    fn start(
        &self,
        program: &Path,
        args: &[OsString],
        frame_bytes: usize,
    ) -> io::Result<FrameStream>;
}

#[cfg(test)]
mod tests {
    use super::{Decoder, Pipeline, SystemDecoder, args, cube_for, first_frame_args};
    use crate::decode::color::{self, ColorTarget, SourceColor};
    use crate::player::tone_map::Primaries;
    use std::ffi::OsString;
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::Duration;

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

    /// The largest channel difference between `cubed` and the still conversion of `plain` into
    /// Display P3.
    fn largest_difference_to_the_still_conversion(plain: &[u8], cubed: &[u8]) -> u8 {
        let pixels = u32::try_from(plain.len() / 4).unwrap();
        let image = image::RgbaImage::from_raw(pixels, 1, plain.to_vec()).unwrap();
        let converted = color::convert(
            image,
            &SourceColor::Srgb,
            ColorTarget::DisplayP3,
            crate::decode::hdr::Peak::DEFAULT,
        );
        converted
            .as_raw()
            .iter()
            .zip(cubed)
            .map(|(still, video)| still.abs_diff(*video))
            .max()
            .unwrap_or(0)
    }

    /// The animated fixtures, without a cube and with the Display P3 cube.
    #[test]
    fn real_ffmpeg_decodes_animated_fixtures() {
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
            assert!(info.is_animation(), "{name}: {info:?}");
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

    /// A synthetic SDR clip with a red patch, on the GPU and in software. The hardware decoder
    /// rejects an H.264 stream smaller than 128x96. The cube output matches the still conversion
    /// within 2 levels.
    #[test]
    fn real_sdr_clip_converts_like_still() {
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
}
