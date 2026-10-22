//! The facts about a video that the player needs before the first frame: the size after the
//! rotation, the frame rate, the duration, the HDR curve and the bit depth.
//!
//! An animated image (GIF, WebP, APNG, AVIS) gets its rate and its length from the packet
//! durations, because the containers tell no frame rate that fits the frame delays.

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
    /// An animated image. ffmpeg decodes it in software at the exact rate `fps`.
    pub animation: bool,
    /// A video with a variable frame rate. ffmpeg duplicates or drops frames to the constant
    /// rate `fps`, see [`variable_rate_fps`].
    pub variable_rate: bool,
    /// The index of the stream that ffmpeg maps. An AVIS file holds the still item as its
    /// first stream and the image sequence as its second.
    pub stream: usize,
}

impl VideoInfo {
    /// The frame interval at the frame rate of the video.
    pub(crate) fn frame_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(1.0 / self.fps)
    }
}

/// The frame rate when the container gives none, or a rate that makes no sense.
const FALLBACK_FPS: f64 = 24.0;

/// The highest rate of an animated image. The player shows the frames at the rate of the
/// shortest common delay, so that each frame holds for its own delay. Above this rate, the
/// player takes the mean rate of the frames instead.
const MAX_ANIMATION_FPS: f64 = 50.0;

/// The highest output rate of a video with a variable frame rate. The terminal player shows
/// no more frames than this.
const MAX_VARIABLE_FPS: f64 = 60.0;

/// The share by which the average rate can differ from the real base rate of a constant-rate
/// video. iPhone clips have an average rate of 29.99 at a base rate of 30 (0.03 %).
const VARIABLE_RATE_TOLERANCE: f64 = 0.01;

/// How many packets the timing walk reads. The walk runs on the UI thread before the first
/// frame: 16 frames of a 181 MB GIF of 1280x720 pixels cost about 15 ms, all 300 cost 250 ms.
const TIMING_PACKETS: usize = 16;

/// The packet durations of an animated image: the rate and, after a walk to the end, the length.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Timing {
    fps: f64,
    /// The sum of the durations, or `None` when the walk stopped before the end.
    duration: Option<f64>,
}

/// Read the header of `path` with the `FFmpeg` library. No frame is decoded.
pub(crate) fn probe(path: &Path) -> io::Result<VideoInfo> {
    let invalid = |text: String| io::Error::new(io::ErrorKind::InvalidData, text);
    let mut input = ffmpeg::format::input(path).map_err(|error| invalid(error.to_string()))?;
    let animation = super::animation::has_animation_extension(path);
    let stream = animation
        .then(|| animation_stream(&input))
        .flatten()
        .or_else(|| input.streams().best(ffmpeg::media::Type::Video))
        .ok_or_else(|| invalid("no video stream".to_owned()))?;
    let stream_index = stream.index();
    let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .map_err(|error| invalid(error.to_string()))?;
    let decoder = context
        .decoder()
        .video()
        .map_err(|error| invalid(error.to_string()))?;
    let (mut fps, variable_rate) = pacing_rate(
        rate_of(stream.rate()),
        rate_of(stream.avg_frame_rate()),
        animation,
    );
    let mut duration = if input.duration() > 0 {
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
    let hdr = Hdr::of_transfer(decoder.color_transfer_characteristic());
    let ten_bit = pixel_format.contains("10") || pixel_format.contains("12");
    let time_base = stream.time_base();
    if animation && let Some(timing) = animation_timing(&mut input, stream_index, time_base) {
        fps = timing.fps;
        if let Some(sum) = timing.duration {
            duration = sum;
        }
    }
    Ok(VideoInfo {
        width,
        height,
        rotation,
        fps,
        duration,
        hdr,
        ten_bit,
        animation,
        variable_rate,
        stream: stream_index,
    })
}

/// The value of a rate, or `None` when the container tells no rate.
fn rate_of(rate: ffmpeg::Rational) -> Option<f64> {
    (rate.numerator() > 0 && rate.denominator() > 0)
        .then(|| f64::from(rate.numerator()) / f64::from(rate.denominator()))
}

/// The rate that the player paces at, and `true` when ffmpeg must get it as `-r`, from
/// `r_frame_rate` (`real`) and `avg_frame_rate` (`average`).
///
/// A video with a variable frame rate gets [`variable_rate_fps`]. Another video paces at its
/// average rate. Without an average rate, it paces at `real`, because ffmpeg writes raw frames
/// at `real` then. An animation gets its rate from the packets later, see [`timing_of`].
fn pacing_rate(real: Option<f64>, average: Option<f64>, animation: bool) -> (f64, bool) {
    let variable = (!animation)
        .then(|| variable_rate_fps(real?, average?))
        .flatten();
    match variable {
        Some(rate) => (rate, true),
        None => (average.or(real).map_or(FALLBACK_FPS, sanitize_fps), false),
    }
}

/// The constant output rate of a video with a variable frame rate, or `None` for a video with
/// a constant rate.
///
/// `real` is `r_frame_rate`, the rate of the shortest frame spacing that `FFmpeg` finds in
/// the first packets. `average` is the frame count over the length. A video is variable when
/// the two differ by more than [`VARIABLE_RATE_TOLERANCE`]. Without `-r`, ffmpeg writes raw
/// frames at `real`, but the player paced them at `average`: a screen recording at 25 and
/// 7.7 played 3.2 times too slow. The output rate is `real` up to [`MAX_VARIABLE_FPS`], so no
/// frame drops and the still parts repeat their frame. Above it, `real` is a fast burst (an
/// iPhone slow motion clip at 120 and 29.6) or a time base guess, and the output rate is
/// `average`, capped at [`MAX_VARIABLE_FPS`].
fn variable_rate_fps(real: f64, average: f64) -> Option<f64> {
    let valid = |rate: f64| rate.is_finite() && rate > 0.0;
    if !valid(real) || !valid(average) || (real - average).abs() <= real * VARIABLE_RATE_TOLERANCE {
        return None;
    }
    let rate = if real <= MAX_VARIABLE_FPS {
        real
    } else {
        average
    };
    Some(rate.min(MAX_VARIABLE_FPS))
}

/// The video stream with the most frames, when an animated file holds more than one video
/// stream. An AVIS file lists its still item first and its sequence second. A normal video
/// never comes here: `best()` skips its cover art, and a fragmented MP4 counts 0 frames.
fn animation_stream(input: &ffmpeg::format::context::Input) -> Option<ffmpeg::Stream<'_>> {
    let videos: Vec<ffmpeg::Stream<'_>> = input
        .streams()
        .filter(|stream| stream.parameters().medium() == ffmpeg::media::Type::Video)
        .collect();
    if videos.len() < 2 {
        return None;
    }
    videos.into_iter().max_by_key(ffmpeg::Stream::frames)
}

/// Walk the first [`TIMING_PACKETS`] packets of `stream_index` and read the rate from their
/// durations, and the length when the walk reaches the end. See [`timing_of`].
fn animation_timing(
    input: &mut ffmpeg::format::context::Input,
    stream_index: usize,
    time_base: ffmpeg::Rational,
) -> Option<Timing> {
    let mut durations = Vec::with_capacity(TIMING_PACKETS);
    let mut complete = true;
    for (stream, packet) in input.packets() {
        if stream.index() != stream_index {
            continue;
        }
        if durations.len() == TIMING_PACKETS {
            complete = false;
            break;
        }
        durations.push(packet.duration().max(0));
    }
    if time_base.numerator() <= 0 || time_base.denominator() <= 0 {
        return None;
    }
    let seconds_per_unit = f64::from(time_base.numerator()) / f64::from(time_base.denominator());
    timing_of(&durations, seconds_per_unit, complete)
}

/// The rate and the length of the frame `durations`, in units of `seconds_per_unit`.
///
/// The rate is the inverse of the greatest common divisor of the delays, so a GIF with delays
/// of 100 ms and 500 ms plays at 10 frames per second, with each long frame sent 5 times.
/// Above [`MAX_ANIMATION_FPS`], the rate is the mean rate, so that each frame shows one time:
/// a GIF of 30 frames per second alternates delays of 30 and 40 ms (gcd 10 ms), and a cap
/// would round them to 20 and 40 ms. The length is the sum, when `complete`.
fn timing_of(durations: &[i64], seconds_per_unit: f64, complete: bool) -> Option<Timing> {
    let step = durations
        .iter()
        .fold(0, |step, &duration| gcd(step, duration));
    let total: i64 = durations.iter().sum();
    if step <= 0 || seconds_per_unit <= 0.0 {
        return None;
    }
    let step_fps = sanitize_fps(1.0 / (step as f64 * seconds_per_unit));
    let fps = if step_fps > MAX_ANIMATION_FPS {
        let mean = sanitize_fps(durations.len() as f64 / (total as f64 * seconds_per_unit));
        mean.min(MAX_ANIMATION_FPS)
    } else {
        step_fps
    };
    Some(Timing {
        fps,
        duration: complete.then_some(total as f64 * seconds_per_unit),
    })
}

/// The greatest common divisor of two durations. A zero does not change the other value.
fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a } else { gcd(b, a % b) }
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
    use super::{
        VideoInfo, gcd, pacing_rate, probe, rotation_of, sanitize_fps, timing_of, variable_rate_fps,
    };
    use crate::test_support::fixture;
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
            animation: false,
            variable_rate: false,
            stream: 0,
        };
        assert_eq!(info.frame_interval(), Duration::from_millis(40));
    }

    #[test]
    fn a_variable_rate_plays_at_its_base_rate_up_to_60_and_else_at_its_average() {
        assert_eq!(variable_rate_fps(30.0, 30.0), None, "a constant rate");
        assert_eq!(
            variable_rate_fps(30.0, 100_080.0 / 3337.0),
            None,
            "an iPhone clip at 29.99 is constant"
        );
        assert_eq!(
            variable_rate_fps(25.0, 7.7),
            Some(25.0),
            "a screen recording"
        );
        assert_eq!(variable_rate_fps(60.0, 24.0), Some(60.0));
        assert_eq!(
            variable_rate_fps(120.0, 29.6),
            Some(29.6),
            "an iPhone slow motion clip"
        );
        assert_eq!(
            variable_rate_fps(90_000.0, 75.0),
            Some(60.0),
            "a time base guess, capped"
        );
        for (real, average) in [
            (0.0, 24.0),
            (24.0, 0.0),
            (f64::NAN, 24.0),
            (24.0, f64::INFINITY),
        ] {
            assert_eq!(variable_rate_fps(real, average), None);
        }
    }

    #[test]
    fn the_pacing_rate_follows_the_rate_that_ffmpeg_writes() {
        assert_eq!(pacing_rate(Some(30.0), Some(30.0), false), (30.0, false));
        assert_eq!(pacing_rate(Some(25.0), Some(7.7), false), (25.0, true));
        assert_eq!(
            pacing_rate(Some(25.0), Some(7.7), true),
            (7.7, false),
            "an animation gets no variable rate"
        );
        assert_eq!(
            pacing_rate(Some(30.0), None, false),
            (30.0, false),
            "without an average rate, ffmpeg writes at the real rate"
        );
        assert_eq!(pacing_rate(None, Some(25.0), false), (25.0, false));
        assert_eq!(pacing_rate(None, None, false), (24.0, false));
    }

    /// Probe a variable and a constant-rate clip that the real ffmpeg writes. Without ffmpeg
    /// on PATH, the test skips with a note.
    #[test]
    fn a_variable_rate_clip_probes_as_variable_and_a_constant_one_does_not() {
        ffmpeg::init().unwrap();
        let Some(program) = crate::test_support::video::real_ffmpeg() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        for (name, variable) in [
            ("vfr.mp4", true),
            ("vfr.mov", true),
            ("cfr.mp4", false),
            ("cfr.mov", false),
        ] {
            let path = dir.path().join(name);
            let Some(path) = crate::test_support::video::write_clip(&program, &path, variable)
            else {
                return;
            };
            let info = probe(&path).unwrap();
            assert_eq!(info.variable_rate, variable, "{name}: {info:?}");
            let fps = if variable { 60.0 } else { 24.0 };
            assert!((info.fps - fps).abs() < 1e-9, "{name}: {info:?}");
            assert!(!info.animation, "{name}");
        }
    }

    #[test]
    fn a_fast_animation_plays_at_its_mean_rate_and_a_cut_walk_has_no_length() {
        // Delays of 30 and 40 ms in 10 ms units: gcd 10 ms is 100 fps, above the cap.
        let fast = timing_of(&[3, 4, 3, 4, 3, 4], 0.01, true).unwrap();
        assert!((fast.fps - 6.0 / 0.21).abs() < 1e-9, "{fast:?}");
        assert!((fast.duration.unwrap() - 0.21).abs() < 1e-9);
        // Delays of 100 and 500 ms: the gcd rate of 10 fps.
        let slow = timing_of(&[10, 50, 20], 0.01, false).unwrap();
        assert_eq!(slow.fps, 10.0);
        assert_eq!(slow.duration, None, "the walk stopped before the end");
        // A mean above the cap stays at the cap. No duration gives no timing.
        assert_eq!(timing_of(&[1, 1, 1], 0.001, true).unwrap().fps, 50.0);
        assert_eq!(timing_of(&[0, 0], 0.01, true), None);
        assert_eq!(timing_of(&[], 0.01, true), None);
    }

    #[test]
    fn the_shortest_common_delay_gives_the_rate() {
        assert_eq!(gcd(10, 50), 10);
        assert_eq!(gcd(0, 20), 20);
        assert_eq!(gcd(20, 0), 20);
        assert_eq!(gcd(3, 5), 1);
    }

    /// The animated fixtures: 3 frames of 16x12 pixels each.
    #[test]
    fn animated_images_report_their_rate_and_length_from_the_packets() {
        ffmpeg::init().unwrap();
        // The GIF has delays of 100, 500 and 200 ms: the rate is 10 and the length 0.8.
        let gif = probe(&fixture("anim.gif")).unwrap();
        assert!(gif.animation, "{gif:?}");
        assert_eq!((gif.width, gif.height, gif.stream), (16, 12, 0));
        assert_eq!(gif.fps, 10.0);
        assert!((gif.duration - 0.8).abs() < 1e-9, "{gif:?}");
        // The others hold 3 frames of 200 ms.
        for name in ["anim.webp", "anim.png"] {
            let info = probe(&fixture(name)).unwrap();
            assert!(info.animation, "{name}: {info:?}");
            assert_eq!((info.fps, info.stream), (5.0, 0), "{name}");
            assert!((info.duration - 0.6).abs() < 1e-9, "{name}: {info:?}");
        }
        // The AVIS lists its still item first; the sequence is the second stream.
        let avis = probe(&fixture("anim.avif")).unwrap();
        assert!(avis.animation, "{avis:?}");
        assert_eq!((avis.fps, avis.stream), (5.0, 1));
        assert!((avis.duration - 0.6).abs() < 1e-9, "{avis:?}");
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
