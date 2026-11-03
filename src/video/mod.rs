//! The first frame of a video for the grid thumbnails, the video extensions and the lookup of
//! the `ffmpeg` and `ffprobe` programs.
//!
//! phewr links no `FFmpeg` library. It starts the programs when PATH has them. Without them, a
//! video cell keeps its play icon, and a video opens in the mpv window.

use crate::decode::color::{self, ColorTarget};
use crate::player::{
    Decoder, SystemDecoder, VideoInfo, cube_for, first_frame_args, frame_box, with_fallback,
};
use crate::yazi::find_on_path;
use image::RgbaImage;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

/// The longest wait for a thumbnail's first frame and for each grid loop frame. A 4K HLG clip
/// takes about 240 ms. A file that hangs ffmpeg must not hold a thumbnail slot for longer.
pub(crate) const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn is_video(path: &Path) -> bool {
    crate::scanner::has_extension(path, crate::scanner::VIDEO_EXTENSIONS)
}

/// `None` when PATH has no such program.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Tools {
    pub(crate) ffmpeg: Option<PathBuf>,
    pub(crate) ffprobe: Option<PathBuf>,
}

impl Tools {
    /// Find both programs in the folders of the PATH value `path`.
    pub(crate) fn find(path: Option<&OsStr>) -> Self {
        Self {
            ffmpeg: find_on_path("ffmpeg", path),
            ffprobe: find_on_path("ffprobe", path),
        }
    }
}

/// Only [`find_session_tools`] fills it.
static SESSION_TOOLS: OnceLock<Tools> = OnceLock::new();

static NO_TOOLS: Tools = Tools {
    ffmpeg: None,
    ffprobe: None,
};

/// Look up `ffmpeg` and `ffprobe` once at the start. `main` calls it, and the tests never do.
pub(crate) fn find_session_tools() {
    SESSION_TOOLS.get_or_init(|| Tools::find(std::env::var_os("PATH").as_deref()));
}

/// No programs before the lookup.
pub(crate) fn session_tools() -> &'static Tools {
    SESSION_TOOLS.get().unwrap_or(&NO_TOOLS)
}

/// `cache_dir`, else `$TMPDIR/phewr`.
pub(crate) struct CubeDirs([PathBuf; 2]);

impl CubeDirs {
    pub(crate) fn paths(&self) -> [&Path; 2] {
        [self.0[0].as_path(), self.0[1].as_path()]
    }
}

pub(crate) fn cube_dirs(cache_dir: &Path) -> CubeDirs {
    CubeDirs([cache_dir.to_path_buf(), std::env::temp_dir().join("phewr")])
}

/// Without one of the session programs, the error is `NotFound` and no process starts.
pub(crate) fn session_decoder() -> io::Result<(&'static Path, SystemDecoder)> {
    decoder_of(session_tools())
}

/// The twin of [`session_decoder`] for any `tools`: a test reaches the path with both programs.
fn decoder_of(tools: &Tools) -> io::Result<(&Path, SystemDecoder)> {
    let (Some(ffmpeg), Some(_)) = (tools.ffmpeg.as_deref(), tools.ffprobe.as_ref()) else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "ffmpeg or ffprobe not found on PATH",
        ));
    };
    Ok((ffmpeg, SystemDecoder::new(tools.ffprobe.clone())))
}

/// The first frame of `path` for a grid thumbnail, with the programs of the session.
pub(crate) fn session_first_frame(
    path: &Path,
    target: (u32, u32),
    cache_dir: &Path,
) -> io::Result<RgbaImage> {
    first_frame_of(session_tools(), path, target, cache_dir)
}

fn first_frame_of(
    tools: &Tools,
    path: &Path,
    target: (u32, u32),
    cache_dir: &Path,
) -> io::Result<RgbaImage> {
    let (ffmpeg, decoder) = decoder_of(tools)?;
    decode_first_frame(
        &decoder,
        ffmpeg,
        path,
        target,
        &cube_dirs(cache_dir).paths(),
        FIRST_FRAME_TIMEOUT,
    )
}

/// Only an HLG or PQ first frame gets a cube. ffmpeg parses a cube at each start (about 16 ms
/// for an SDR cube), and the conversion of one small SDR frame costs well below 1 ms.
pub(crate) fn first_frame_cube(
    info: &VideoInfo,
    target: ColorTarget,
    dirs: &[&Path],
) -> Option<PathBuf> {
    info.hdr.and_then(|_| cube_for(info, target, dirs))
}

/// An SDR frame converts as a still image. An HDR frame left ffmpeg in the target already.
pub(crate) fn convert_first_frame(
    image: RgbaImage,
    info: &VideoInfo,
    target: ColorTarget,
) -> RgbaImage {
    if info.hdr.is_some() {
        image
    } else {
        color::convert(image, &info.primaries.source_color(), target)
    }
}

/// The first frame of `path`, fitted into `target` pixels, for a grid thumbnail.
///
/// ffmpeg runs the filter chain of the terminal player, with its software retry. A run
/// without a frame after `timeout` is killed, and the error is `TimedOut`.
pub(crate) fn decode_first_frame(
    decoder: &dyn Decoder,
    ffmpeg: &Path,
    path: &Path,
    target: (u32, u32),
    cube_dirs: &[&Path],
    timeout: Duration,
) -> io::Result<RgbaImage> {
    let info = decoder.probe(path)?;
    let frame = frame_box((info.width, info.height), target);
    let color_target = color::target();
    let cube = first_frame_cube(&info, color_target, cube_dirs);
    let frame_bytes = frame.0 as usize * frame.1 as usize * 4;
    let pixels = with_fallback(&info, |pipeline| {
        let args = first_frame_args(path, &info, frame, cube.as_deref(), pipeline);
        // The drop of the stream kills ffmpeg after a timeout.
        match decoder
            .start(ffmpeg, &args, frame_bytes)?
            .wait_next(timeout)
        {
            Ok(pixels) => Ok(Some(pixels)),
            Err(RecvTimeoutError::Timeout) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "ffmpeg gave no frame in time",
            )),
            Err(RecvTimeoutError::Disconnected) => Ok(None),
        }
    })?;
    RgbaImage::from_raw(frame.0, frame.1, pixels)
        .map(|image| convert_first_frame(image, &info, color_target))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid frame"))
}

#[cfg(test)]
mod tests;
