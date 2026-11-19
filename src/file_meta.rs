//! The facts of one file for the status row. A thread reads them, and a cursor move never
//! waits.

use crate::info;
use std::path::Path;
use tuilibr::pictures::decode;

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct FileMeta {
    /// The size after the orientation.
    pub(crate) size: Option<(u32, u32)>,
    pub(crate) bytes: Option<u64>,
    /// The extension in capitals.
    pub(crate) format: String,
    /// The EXIF date of the shot, else the modified time.
    pub(crate) date: Option<String>,
    pub(crate) video: Option<VideoMeta>,
    /// The length of the motion of a Live Photo still, in seconds. `describe` leaves it out.
    pub(crate) motion_seconds: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct VideoMeta {
    /// 0 when the container does not tell it.
    pub(crate) duration: f64,
    pub(crate) fps: f64,
    /// `HLG` or `PQ`, `None` for SDR.
    pub(crate) hdr: Option<&'static str>,
}

/// One formatter for both dates, in the system time zone. `None` when jiff cannot convert the time.
pub(crate) fn format_local(time: std::time::SystemTime, pattern: &str) -> Option<String> {
    format_in(time, &jiff::tz::TimeZone::system(), pattern)
}

/// The time zone is a parameter, so a test can fix it without the `TZ` variable.
fn format_in(
    time: std::time::SystemTime,
    zone: &jiff::tz::TimeZone,
    pattern: &str,
) -> Option<String> {
    let timestamp = jiff::Timestamp::try_from(time).ok()?;
    Some(
        timestamp
            .to_zoned_datetime(zone.clone())
            .strftime(pattern)
            .to_string(),
    )
}

/// A missing part stays `None`.
pub(crate) fn read(path: &Path) -> FileMeta {
    let metadata = std::fs::metadata(path).ok();
    let modified = metadata
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|time| format_local(time, "%Y-%m-%d %H:%M"));
    let format = path
        .extension()
        .map(|ext| ext.to_string_lossy().to_ascii_uppercase())
        .unwrap_or_default();
    let mut meta = FileMeta {
        size: None,
        bytes: metadata.map(|m| m.len()),
        format,
        date: None,
        video: None,
        motion_seconds: None,
    };
    if read_video(path, &mut meta) {
        meta.date = modified;
        return meta;
    }
    meta.size = tuilibr::pictures::shape::shown_size(path).or_else(|| decode::dimensions(path));
    meta.date = info::date_taken(path).or(modified);
    meta
}

/// True for every video, with or without `ffprobe` and a probe.
#[cfg(feature = "video")]
fn read_video(path: &Path, meta: &mut FileMeta) -> bool {
    use crate::player::tone_map::Hdr;
    if !crate::video::is_video(path) {
        return false;
    }
    let Some(ffprobe) = crate::video::session_tools().ffprobe.as_deref() else {
        return true;
    };
    let Ok(probe) = crate::player::probe_cached(ffprobe, path) else {
        return true;
    };
    meta.size = Some((probe.width, probe.height));
    meta.video = Some(VideoMeta {
        duration: probe.duration,
        fps: probe.fps,
        hdr: probe.hdr.map(|hdr| match hdr {
            Hdr::Hlg => "HLG",
            Hdr::Pq => "PQ",
        }),
    });
    true
}

#[cfg(not(feature = "video"))]
fn read_video(_path: &Path, _meta: &mut FileMeta) -> bool {
    false
}

/// The probe gives 0 when the container does not tell the length.
#[cfg(feature = "video")]
fn known_length(seconds: f64) -> Option<f64> {
    (seconds > 0.0).then_some(seconds)
}

/// The length of a motion from the probe cache, without a probe.
#[cfg(feature = "video")]
pub(crate) fn motion_length_cached(motion: &Path) -> Option<f64> {
    crate::player::probe_peek(motion).and_then(|probe| known_length(probe.duration))
}

#[cfg(not(feature = "video"))]
pub(crate) fn motion_length_cached(_motion: &Path) -> Option<f64> {
    None
}

/// The length of a motion through one `ffprobe` of about 31 ms. Call it on a thread only.
#[cfg(feature = "video")]
pub(crate) fn motion_length_probed(motion: &Path) -> Option<f64> {
    let ffprobe = crate::video::session_tools().ffprobe.as_deref()?;
    crate::player::probe_cached(ffprobe, motion)
        .ok()
        .and_then(|probe| known_length(probe.duration))
}

#[cfg(not(feature = "video"))]
pub(crate) fn motion_length_probed(_motion: &Path) -> Option<f64> {
    None
}

/// `2.8 s`, the length of a Live Photo motion.
pub(crate) fn format_motion(seconds: f64) -> String {
    format!("{:.1} s", seconds.max(0.0))
}

impl FileMeta {
    /// The parts split by a middle dot.
    pub(crate) fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(video) = &self.video {
            parts.push(format_time(video.duration));
        }
        if let Some((width, height)) = self.size {
            parts.push(format!("{width}\u{d7}{height}"));
            if self.video.is_none() {
                parts.push(info::megapixels(width, height));
            }
        }
        if let Some(video) = &self.video {
            parts.push(format!("{:.0} fps", video.fps));
            parts.push(video.hdr.unwrap_or("SDR").to_owned());
        }
        if let Some(bytes) = self.bytes {
            parts.push(info::human_size(bytes));
        }
        if !self.format.is_empty() {
            parts.push(self.format.clone());
        }
        if let Some(date) = &self.date {
            parts.push(date.clone());
        }
        parts.join(" \u{b7} ")
    }
}

/// `mm:ss`, or `h:mm:ss` from one hour on.
pub(crate) fn format_time(seconds: f64) -> String {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a clip length is never negative or near 2^64 seconds"
    )]
    let whole = seconds.max(0.0) as u64;
    let (hours, minutes, seconds) = (whole / 3600, whole / 60 % 60, whole % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::{FileMeta, VideoMeta, format_in, format_local, format_motion, format_time, read};
    use image::RgbImage;

    #[test]
    fn format_in_pads_and_matches_the_old_text() {
        let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        let utc = jiff::tz::TimeZone::UTC;
        assert_eq!(
            format_in(time, &utc, "%Y-%m-%d %H:%M:%S").as_deref(),
            Some("2001-09-09 01:46:40")
        );
        assert_eq!(
            format_in(time, &utc, "%Y-%m-%d %H:%M").as_deref(),
            Some("2001-09-09 01:46")
        );
    }

    #[test]
    fn format_local_formats_now() {
        assert!(format_local(std::time::SystemTime::now(), "%Y").is_some());
    }

    #[test]
    fn image_has_size_bytes_format_date() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.png");
        RgbImage::new(40, 30).save(&path).unwrap();

        let meta = read(&path);
        assert_eq!(meta.size, Some((40, 30)));
        assert!(meta.bytes.is_some_and(|bytes| bytes > 0));
        assert_eq!(meta.format, "PNG");
        assert!(
            meta.date.is_some(),
            "the modified time stands in for the EXIF date"
        );
        assert_eq!(meta.video, None);
    }

    #[cfg(feature = "video")]
    #[test]
    fn video_without_tools_has_no_facts() {
        // The tests never look up ffprobe.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clip.mp4");
        std::fs::write(&path, b"synthetic").unwrap();

        let meta = read(&path);
        assert_eq!(meta.video, None);
        assert_eq!(meta.size, None);
        assert_eq!(meta.bytes, Some(9));
        assert_eq!(meta.format, "MP4");
    }

    #[test]
    fn missing_file_has_only_format() {
        let meta = read(std::path::Path::new("/nonexistent/phewr/a.jpg"));
        assert_eq!(meta.size, None);
        assert_eq!(meta.bytes, None);
        assert_eq!(meta.format, "JPG");
    }

    #[test]
    fn describes_image_and_video() {
        let image = FileMeta {
            size: Some((4032, 3024)),
            bytes: Some(2_200_000),
            format: "HEIC".to_owned(),
            date: Some("2026-04-25 14:32".to_owned()),
            video: None,
            motion_seconds: None,
        };
        assert_eq!(
            image.describe(),
            "4032\u{d7}3024 \u{b7} 12 MP \u{b7} 2.1 MB \u{b7} HEIC \u{b7} 2026-04-25 14:32"
        );
        let video = FileMeta {
            size: Some((3840, 2160)),
            bytes: None,
            format: "MOV".to_owned(),
            date: None,
            video: Some(VideoMeta {
                duration: 83.4,
                fps: 29.97,
                hdr: Some("HLG"),
            }),
            motion_seconds: None,
        };
        assert_eq!(
            video.describe(),
            "01:23 \u{b7} 3840\u{d7}2160 \u{b7} 30 fps \u{b7} HLG \u{b7} MOV"
        );
    }

    /// `PHEWR_TEST_HEIC=/path/to/photo.heic`.
    #[test]
    #[ignore = "needs PHEWR_TEST_HEIC, a real photo"]
    fn real_heic_has_size_and_date() {
        let Some(path) = tuilibr::pictures::decode::real_test_file("PHEWR_TEST_HEIC") else {
            return;
        };
        let meta = read(&path);
        assert!(meta.size.is_some());
        let exif_date = crate::info::date_taken(&path);
        assert!(exif_date.is_some(), "the EXIF date of {}", path.display());
        assert_eq!(meta.date, exif_date);
    }

    #[test]
    #[cfg(feature = "video")]
    fn unknown_motion_length_is_none() {
        assert_eq!(super::known_length(0.0), None);
        assert_eq!(super::known_length(-1.0), None);
        assert_eq!(super::known_length(2.8), Some(2.8));
    }

    /// `PHEWR_TEST_LIVE_DIR` with `IMG_1050.MOV` (2.771667 s) and the real ffprobe.
    #[test]
    #[cfg(feature = "video")]
    #[ignore = "needs PHEWR_TEST_LIVE_DIR and ffprobe"]
    fn real_motion_length_rounds_to_2_8() {
        let Some(dir) = tuilibr::pictures::decode::real_test_file("PHEWR_TEST_LIVE_DIR") else {
            return;
        };
        crate::video::find_session_tools();
        if crate::video::session_tools().ffprobe.is_none() {
            println!("note: skipped, because ffprobe is not on PATH");
            return;
        }
        let seconds = super::motion_length_probed(&dir.join("IMG_1050.MOV")).unwrap();
        assert_eq!(format_motion(seconds), "2.8 s");
    }

    #[test]
    fn motion_has_one_decimal() {
        assert_eq!(format_motion(2.83), "2.8 s");
        assert_eq!(format_motion(-1.0), "0.0 s");
    }

    #[test]
    fn time_gets_hours_from_one_hour() {
        assert_eq!(format_time(59.9), "00:59");
        assert_eq!(format_time(3725.0), "1:02:05");
        assert_eq!(format_time(-1.0), "00:00");
    }

    #[test]
    fn sdr_video_line_without_size() {
        let video = FileMeta {
            bytes: Some(1024),
            video: Some(VideoMeta {
                duration: 3725.0,
                fps: 25.0,
                hdr: None,
            }),
            ..FileMeta::default()
        };
        assert_eq!(
            video.describe(),
            "1:02:05 \u{b7} 25 fps \u{b7} SDR \u{b7} 1.0 KB"
        );
        assert_eq!(FileMeta::default().describe(), "");
    }
}
