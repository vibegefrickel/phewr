//! The first frame of a video, for the thumbnails of the grid, the video extensions and the
//! lookup of the `ffmpeg` and `ffprobe` programs.
//!
//! viewr links no `FFmpeg` library. It starts the programs when PATH has them. Without them, a
//! video cell keeps its play icon, and a video opens in the mpv window.
//!
//! The terminal player is in `player`, the mpv window in `mpv`.

use crate::player::{Decoder, Pipeline, SystemDecoder, cube_for, first_frame_args, frame_box};
use crate::yazi::find_on_path;
use image::RgbaImage;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov", "mkv", "avi", "webm", "m4v"];

/// The longest wait for the first frame of a thumbnail. A 4K HLG clip takes about 240 ms. A
/// file that hangs ffmpeg must not hold one of the 4 thumbnail slots for longer.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// The `ffmpeg` and `ffprobe` programs. `None` when PATH has no such program.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Tools {
    pub(crate) ffmpeg: Option<PathBuf>,
    pub(crate) ffprobe: Option<PathBuf>,
}

impl Tools {
    /// Find both programs in the folders of `path`, the value of PATH.
    pub(crate) fn find(path: Option<&OsStr>) -> Self {
        Self {
            ffmpeg: find_on_path("ffmpeg", path),
            ffprobe: find_on_path("ffprobe", path),
        }
    }
}

/// The programs of the session. Only [`find_session_tools`] fills it.
static SESSION_TOOLS: OnceLock<Tools> = OnceLock::new();

/// The value of [`session_tools`] before the lookup: no programs.
static NO_TOOLS: Tools = Tools {
    ffmpeg: None,
    ffprobe: None,
};

/// Look up `ffmpeg` and `ffprobe` on PATH one time, at the start of the session. `main` calls
/// it. The tests never do, so a test never starts a real ffmpeg through the session.
pub(crate) fn find_session_tools() {
    SESSION_TOOLS.get_or_init(|| Tools::find(std::env::var_os("PATH").as_deref()));
}

/// The programs that [`find_session_tools`] found, or no programs before the lookup.
pub(crate) fn session_tools() -> &'static Tools {
    SESSION_TOOLS.get().unwrap_or(&NO_TOOLS)
}

/// The first frame of `path` for a grid thumbnail, with the programs of the session. The tone
/// mapping cube goes into `cache_dir`, else into `$TMPDIR/viewr`. Without `ffmpeg` or
/// `ffprobe`, the error is `NotFound` and no process starts.
pub(crate) fn session_first_frame(
    path: &Path,
    target: (u32, u32),
    cache_dir: &Path,
) -> io::Result<RgbaImage> {
    let tools = session_tools();
    let (Some(ffmpeg), Some(ffprobe)) = (tools.ffmpeg.as_deref(), tools.ffprobe.as_ref()) else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "ffmpeg or ffprobe not found on PATH",
        ));
    };
    let temp = std::env::temp_dir().join("viewr");
    let decoder = SystemDecoder::new(Some(ffprobe.clone()));
    let cube_dirs = [cache_dir, temp.as_path()];
    decode_first_frame(
        &decoder,
        ffmpeg,
        path,
        target,
        &cube_dirs,
        FIRST_FRAME_TIMEOUT,
    )
}

/// The first frame of `path`, fitted into `target` pixels, for a grid thumbnail.
///
/// ffmpeg runs the filter chain of the terminal player: the rotation of the display matrix,
/// the scale and, for HLG and PQ, the tone mapping cube in the first of `cube_dirs` that works.
/// A GPU run without a frame runs again in software, as in the player. A run without a frame
/// after `timeout` is killed, and the error is `TimedOut`.
pub(crate) fn decode_first_frame(
    decoder: &dyn Decoder,
    ffmpeg: &Path,
    path: &Path,
    target: (u32, u32),
    cube_dirs: &[&Path],
    timeout: Duration,
) -> io::Result<RgbaImage> {
    let info = decoder.probe(path)?;
    let frame = frame_box(
        (info.width, info.height),
        (target.0.max(1), target.1.max(1)),
    );
    let cube = cube_for(&info, cube_dirs);
    let frame_bytes = frame.0 as usize * frame.1 as usize * 4;
    for pipeline in [Pipeline::Gpu, Pipeline::Software] {
        let args = first_frame_args(path, &info, frame, cube.as_deref(), pipeline);
        // The drop of the stream kills ffmpeg after a timeout.
        match decoder
            .start(ffmpeg, &args, frame_bytes)?
            .wait_next(timeout)
        {
            Ok(pixels) => {
                return RgbaImage::from_raw(frame.0, frame.1, pixels)
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid frame"));
            }
            Err(RecvTimeoutError::Timeout) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "ffmpeg gave no frame in time",
                ));
            }
            Err(RecvTimeoutError::Disconnected) => {}
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "no frames decoded",
    ))
}

#[cfg(test)]
mod tests {
    use super::{Tools, decode_first_frame, is_video, session_first_frame};
    use crate::player::SystemDecoder;
    use crate::player::tone_map::Hdr;
    use crate::test_support::video::{fake_decoder, info_of, real_ffmpeg, real_ffprobe};
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Duration;
    use tempfile::TempDir;

    /// The timeout of the first frame in the tests. A fake run that never ends waits this long.
    const WAIT: Duration = Duration::from_secs(10);

    /// Give each test its own directory. It goes away on drop, also after a panic.
    fn test_dir() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    /// Run the ffmpeg `program` with `args`. `false` means that ffmpeg failed.
    fn run_ffmpeg(program: &Path, args: &[&str]) -> bool {
        let status = Command::new(program)
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(args)
            .status();
        if status.as_ref().is_ok_and(std::process::ExitStatus::success) {
            return true;
        }
        println!("note: skipped, because ffmpeg failed: {status:?}");
        false
    }

    /// Make a 64x48 clip of `seconds` at 10 frames per second, or `None` when ffmpeg fails.
    fn test_clip(program: &Path, dir: &Path, seconds: &str) -> Option<PathBuf> {
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
        run_ffmpeg(program, &args).then_some(path)
    }

    /// The real ffmpeg and a decoder with the real ffprobe, or `None` with a note.
    fn real_programs() -> Option<(PathBuf, SystemDecoder)> {
        let (Some(ffmpeg), Some(ffprobe)) = (real_ffmpeg(), real_ffprobe()) else {
            return None;
        };
        Some((ffmpeg, SystemDecoder::new(Some(ffprobe))))
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
    fn the_lookup_finds_ffmpeg_and_ffprobe_on_path() {
        let dir = test_dir();
        for name in ["ffmpeg", "ffprobe"] {
            let program = dir.path().join(name);
            std::fs::write(&program, "#!/bin/sh\n").unwrap();
            let mut permissions = std::fs::metadata(&program).unwrap().permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
            std::fs::set_permissions(&program, permissions).unwrap();
        }
        let path = OsString::from(dir.path());
        let tools = Tools::find(Some(&path));
        assert_eq!(tools.ffmpeg, Some(dir.path().join("ffmpeg")));
        assert_eq!(tools.ffprobe, Some(dir.path().join("ffprobe")));
        assert_eq!(Tools::find(None), Tools::default());
    }

    #[test]
    fn without_the_session_lookup_a_first_frame_starts_no_process() {
        let error = session_first_frame(Path::new("clip.mp4"), (32, 24), Path::new("/dev/null"))
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn the_first_frame_turns_scales_and_tone_maps_as_the_player_does() {
        let dir = test_dir();
        let decoder = fake_decoder(crate::player::VideoInfo {
            rotation: 90,
            hdr: Some(Hdr::Hlg),
            ten_bit: true,
            ..info_of(1080, 1920, 30.0)
        });
        decoder.queue_run(vec![vec![7; 18 * 32 * 4]]);
        let frame = decode_first_frame(
            &*decoder,
            Path::new("/bin/ffmpeg"),
            Path::new("/v/portrait.mov"),
            (64, 32),
            &[dir.path()],
            WAIT,
        )
        .unwrap();
        assert_eq!(frame.dimensions(), (18, 32));
        assert_eq!(frame.get_pixel(0, 0).0, [7; 4]);
        let args = decoder.last_args();
        assert!(
            args.contains("scale_vt=w=32:h=18,transpose_vt=dir=clock"),
            "{args}"
        );
        assert!(args.contains("tonemap-hlg-v2.cube"), "{args}");
        assert!(
            args.ends_with("-f rawvideo -pix_fmt rgba -frames:v 1 pipe:1"),
            "{args}"
        );
    }

    #[test]
    fn a_gpu_run_without_a_frame_runs_again_in_software() {
        let decoder = fake_decoder(info_of(640, 480, 25.0));
        decoder.queue_run(Vec::new());
        decoder.queue_run(vec![vec![1; 32 * 24 * 4]]);
        let frame = decode_first_frame(
            &*decoder,
            Path::new("/bin/ffmpeg"),
            Path::new("/v/clip.mp4"),
            (32, 48),
            &[],
            WAIT,
        )
        .unwrap();
        assert_eq!(frame.dimensions(), (32, 24));
        let starts = decoder.starts();
        assert_eq!(starts.len(), 2);
        assert!(starts[0].contains("scale_vt="), "{}", starts[0]);
        assert!(starts[1].contains("scale=w=32:h=24"), "{}", starts[1]);

        decoder.queue_run(Vec::new());
        decoder.queue_run(Vec::new());
        let error = decode_first_frame(
            &*decoder,
            Path::new("/bin/ffmpeg"),
            Path::new("/v/clip.mp4"),
            (32, 48),
            &[],
            WAIT,
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "no frames decoded");
    }

    #[test]
    fn a_run_without_a_frame_in_time_is_a_timeout_and_runs_no_software_retry() {
        let decoder = fake_decoder(info_of(640, 480, 25.0));
        let error = decode_first_frame(
            &*decoder,
            Path::new("/bin/ffmpeg"),
            Path::new("/v/clip.mp4"),
            (32, 48),
            &[],
            Duration::from_millis(20),
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert_eq!(decoder.starts().len(), 1);
    }

    #[test]
    fn a_failed_probe_or_start_gives_an_error() {
        let decoder = fake_decoder(info_of(640, 480, 25.0));
        decoder.fail_probe.set(true);
        let target = (32, 48);
        let ffmpeg = Path::new("/bin/ffmpeg");
        let path = Path::new("/v/clip.mp4");
        assert!(decode_first_frame(&*decoder, ffmpeg, path, target, &[], WAIT).is_err());
        assert!(decoder.starts().is_empty(), "no ffmpeg without a probe");
        decoder.fail_probe.set(false);
        decoder.fail_start.set(true);
        assert!(decode_first_frame(&*decoder, ffmpeg, path, target, &[], WAIT).is_err());
    }

    /// Decode the first frame of a clip that the real ffmpeg writes. Without ffmpeg or ffprobe
    /// on PATH, the test skips with a note.
    #[test]
    fn the_real_first_frame_fits_the_target_rectangle() {
        let Some((ffmpeg, decoder)) = real_programs() else {
            return;
        };
        let dir = test_dir();
        let Some(clip) = test_clip(&ffmpeg, dir.path(), "0.5") else {
            return;
        };
        let frame = decode_first_frame(&decoder, &ffmpeg, &clip, (32, 48), &[], WAIT).unwrap();
        let tiny = decode_first_frame(&decoder, &ffmpeg, &clip, (0, 0), &[], WAIT).unwrap();
        assert_eq!(frame.dimensions(), (32, 24));
        assert_eq!(tiny.dimensions(), (2, 2), "the smallest even frame");
    }

    /// A clip with a display rotation of 90 degrees counterclockwise gives a portrait frame.
    #[test]
    fn the_real_first_frame_of_a_rotated_clip_is_turned() {
        let Some((ffmpeg, decoder)) = real_programs() else {
            return;
        };
        let dir = test_dir();
        let Some(clip) = test_clip(&ffmpeg, dir.path(), "0.5") else {
            return;
        };
        let rotated = dir.path().join("rotated.mp4");
        let (clip_text, rotated_text) = (clip.to_string_lossy(), rotated.to_string_lossy());
        let args = [
            "-display_rotation",
            "90",
            "-i",
            &clip_text,
            "-c",
            "copy",
            &rotated_text,
        ];
        if !run_ffmpeg(&ffmpeg, &args) {
            return;
        }
        let frame = decode_first_frame(&decoder, &ffmpeg, &rotated, (64, 64), &[], WAIT).unwrap();
        assert_eq!(frame.dimensions(), (48, 64));
    }

    #[test]
    fn files_without_a_video_stream_give_an_error() {
        let Some((ffmpeg, decoder)) = real_programs() else {
            return;
        };
        let dir = test_dir();
        let text = dir.path().join("notes.mp4");
        std::fs::write(&text, "not a video").unwrap();
        assert!(decode_first_frame(&decoder, &ffmpeg, &text, (32, 48), &[], WAIT).is_err());

        let audio = dir.path().join("tone.m4a");
        let audio_text = audio.to_str().unwrap();
        if run_ffmpeg(
            &ffmpeg,
            &["-f", "lavfi", "-i", "sine=duration=0.2", audio_text],
        ) {
            let error =
                decode_first_frame(&decoder, &ffmpeg, &audio, (32, 48), &[], WAIT).unwrap_err();
            assert_eq!(error.to_string(), "no video stream");
        }
    }
}
