//! The first frame of a video, for the thumbnails of the grid, and the video extensions.
//!
//! The terminal player is in `player`, the mpv window in `mpv`.

use image::RgbaImage;
use std::io;
use std::path::Path;

const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov", "mkv", "avi", "webm", "m4v"];

pub(crate) fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
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
    use super::{decode_first_frame, fit_aspect, is_video};
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use tempfile::TempDir;

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
        assert!(decode_first_frame(&text, (32, 48)).is_err());

        let audio = dir.path().join("tone.m4a");
        let audio_text = audio.to_str().unwrap();
        if run_ffmpeg(&["-f", "lavfi", "-i", "sine=duration=0.2", audio_text]) {
            let error = decode_first_frame(&audio, (32, 48)).unwrap_err();
            assert_eq!(error.to_string(), "no video stream");
        }
    }
}
