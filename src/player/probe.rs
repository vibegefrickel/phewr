//! The facts about a video that the player needs before the first frame: the size after the
//! rotation, the frame rate, the duration, the HDR curve and the bit depth.

use super::tone_map::Hdr;
use std::io;
use std::path::Path;

/// The header facts of a video. The size is the size on the screen, after the rotation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VideoInfo {
    pub width: u32,
    pub height: u32,
    /// The clockwise rotation that the display matrix asks for: 0, 90, 180 or 270 degrees.
    pub rotation: u16,
    pub fps: f64,
    /// The length in seconds, or 0 when the container does not tell it.
    pub duration: f64,
    pub hdr: Option<Hdr>,
    /// A 10 bit stream. The frames that the Mac hardware decoder gives back are `p010le` then.
    pub ten_bit: bool,
}

impl VideoInfo {
    /// The frame interval at the frame rate of the video.
    pub(crate) fn frame_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(1.0 / self.fps)
    }
}

/// The frame rate when the container gives none, or a rate that makes no sense.
const FALLBACK_FPS: f64 = 24.0;

/// Read the header of `path` with the `FFmpeg` library. No frame is decoded.
pub(crate) fn probe(path: &Path) -> io::Result<VideoInfo> {
    let invalid = |text: String| io::Error::new(io::ErrorKind::InvalidData, text);
    let input = ffmpeg::format::input(path).map_err(|error| invalid(error.to_string()))?;
    let stream = input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| invalid("no video stream".to_owned()))?;
    let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .map_err(|error| invalid(error.to_string()))?;
    let decoder = context
        .decoder()
        .video()
        .map_err(|error| invalid(error.to_string()))?;
    let rate = stream.avg_frame_rate();
    let fps = if rate.denominator() > 0 {
        sanitize_fps(f64::from(rate.numerator()) / f64::from(rate.denominator()))
    } else {
        FALLBACK_FPS
    };
    let duration = if input.duration() > 0 {
        input.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE)
    } else {
        0.0
    };
    let rotation = stream
        .side_data()
        .find(|data| data.kind() == ffmpeg::codec::packet::side_data::Type::DisplayMatrix)
        .map_or(0, |data| rotation_of(data.data()));
    let (width, height) = if rotation % 180 == 0 {
        (decoder.width(), decoder.height())
    } else {
        (decoder.height(), decoder.width())
    };
    let pixel_format = format!("{:?}", decoder.format()).to_ascii_lowercase();
    Ok(VideoInfo {
        width,
        height,
        rotation,
        fps,
        duration,
        hdr: Hdr::of_transfer(decoder.color_transfer_characteristic()),
        ten_bit: pixel_format.contains("10") || pixel_format.contains("12"),
    })
}

/// A frame rate that the pacing can use.
fn sanitize_fps(fps: f64) -> f64 {
    if fps.is_finite() && fps > 0.0 {
        fps
    } else {
        FALLBACK_FPS
    }
}

/// The clockwise rotation in whole quarter turns from a display matrix of nine 16.16 fixed
/// point values, as `FFmpeg` reads it (`av_display_rotation_get`). iPhone portrait clips carry
/// a rotation of -90 in the matrix, and the player must turn them 90 degrees clockwise.
pub(crate) fn rotation_of(matrix: &[u8]) -> u16 {
    let value = |index: usize| -> f64 {
        matrix
            .get(index * 4..index * 4 + 4)
            .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
            .map_or(0.0, |bytes| f64::from(i32::from_ne_bytes(bytes)) / 65536.0)
    };
    let (m0, m1, m3, m4) = (value(0), value(1), value(3), value(4));
    let scale_x = m0.hypot(m3);
    let scale_y = m1.hypot(m4);
    if scale_x == 0.0 || scale_y == 0.0 {
        return 0;
    }
    let rotation = -(m1 / scale_y).atan2(m0 / scale_x).to_degrees();
    let clockwise = (-rotation).rem_euclid(360.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is 0, 90, 180 or 270"
    )]
    let quarter_turns = ((clockwise / 90.0).round() as u16) % 4;
    quarter_turns * 90
}

#[cfg(test)]
mod tests {
    use super::{VideoInfo, probe, rotation_of, sanitize_fps};
    use std::path::Path;
    use std::time::Duration;

    /// The display matrix of a rotation of `degrees`, as `av_display_rotation_set` writes it.
    fn matrix(degrees: f64) -> Vec<u8> {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let fixed = |value: f64| ((value * 65536.0).round() as i32).to_ne_bytes();
        [cos, -sin, 0.0, sin, cos, 0.0, 0.0, 0.0, 1.0]
            .iter()
            .flat_map(|&value| fixed(value))
            .collect()
    }

    #[test]
    fn the_display_matrix_gives_the_clockwise_rotation() {
        assert_eq!(rotation_of(&matrix(0.0)), 0);
        assert_eq!(rotation_of(&matrix(-90.0)), 90, "an iPhone portrait clip");
        assert_eq!(rotation_of(&matrix(90.0)), 270);
        assert_eq!(rotation_of(&matrix(180.0)), 180);
        assert_eq!(rotation_of(&matrix(-180.0)), 180);
        assert_eq!(rotation_of(&[0; 36]), 0, "an empty matrix");
        assert_eq!(rotation_of(&[1, 2, 3]), 0, "a short matrix");
    }

    #[test]
    fn invalid_frame_rates_use_the_fallback() {
        for fps in [0.0, -1.0, f64::INFINITY, f64::NAN] {
            assert_eq!(sanitize_fps(fps), 24.0);
        }
        assert_eq!(sanitize_fps(29.97), 29.97);
        let info = VideoInfo {
            width: 16,
            height: 9,
            rotation: 0,
            fps: 25.0,
            duration: 1.0,
            hdr: None,
            ten_bit: false,
        };
        assert_eq!(info.frame_interval(), Duration::from_millis(40));
    }

    #[test]
    fn a_file_that_is_no_video_does_not_probe() {
        ffmpeg::init().unwrap();
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "not a video").unwrap();
        assert!(probe(file.path()).is_err());
        assert!(probe(Path::new("/nonexistent/a.mov")).is_err());
    }

    /// Read the header of a real video: `VIEWR_TEST_HDR_VIDEO` or `VIEWR_TEST_SDR_VIDEO`.
    #[test]
    #[ignore = "needs VIEWR_TEST_HDR_VIDEO or VIEWR_TEST_SDR_VIDEO, real video files"]
    fn real_videos_report_their_size_and_rate() {
        ffmpeg::init().unwrap();
        for variable in ["VIEWR_TEST_HDR_VIDEO", "VIEWR_TEST_SDR_VIDEO"] {
            let Some(path) = crate::decode::real_test_file(variable) else {
                continue;
            };
            let info = probe(&path).unwrap();
            assert!(info.width > 0 && info.height > 0, "{info:?}");
            assert!(info.fps > 1.0 && info.duration > 0.0, "{info:?}");
            assert_eq!(info.hdr.is_some(), variable.contains("HDR"), "{info:?}");
        }
    }
}
