//! The facts about one file for the status row: the size on the screen, the file size, the
//! format and the date. A video adds its length, its frame rate and its HDR curve from the
//! probe cache. The run loop reads them on a thread, so a cursor move never waits.

use crate::{decode, info};
use chrono::{DateTime, Local};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct FileMeta {
    /// Width and height as the viewer shows them, after the orientation.
    pub(crate) size: Option<(u32, u32)>,
    pub(crate) bytes: Option<u64>,
    /// The extension in capitals.
    pub(crate) format: String,
    /// The EXIF date of the shot, else the modified time of the file.
    pub(crate) date: Option<String>,
    pub(crate) video: Option<VideoMeta>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct VideoMeta {
    /// The length in seconds, 0 when the container does not tell it.
    pub(crate) duration: f64,
    pub(crate) fps: f64,
    /// `HLG` or `PQ`, `None` for SDR.
    pub(crate) hdr: Option<&'static str>,
}

/// Read the facts of `path`. A missing part stays `None`.
pub(crate) fn read(path: &Path) -> FileMeta {
    let metadata = std::fs::metadata(path).ok();
    let modified = metadata
        .as_ref()
        .and_then(|m| m.modified().ok())
        .map(|time| {
            DateTime::<Local>::from(time)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        });
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
    };
    if read_video(path, &mut meta) {
        meta.date = modified;
        return meta;
    }
    meta.size = decode::shape::shown_size(path).or_else(|| info::dimensions(path));
    meta.date = info::date_taken(path).or(modified);
    meta
}

/// Fill the video facts from the probe cache. False for a file that is not a video, or
/// without `ffprobe`.
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

impl FileMeta {
    /// One line for the status row, the parts split by a middle dot.
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

/// Seconds as `mm:ss`, or `h:mm:ss` from one hour on.
pub(crate) fn format_time(seconds: f64) -> String {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a position is never negative, and a clip never runs for 2^64 seconds"
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
    use super::{FileMeta, VideoMeta, format_time, read};
    use image::RgbImage;

    #[test]
    fn an_image_has_its_size_bytes_format_and_date() {
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
    fn a_video_without_the_tools_has_no_video_facts_and_no_size() {
        // The tests never look up ffprobe, so the session has no tools.
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
    fn a_missing_file_has_only_its_format() {
        let meta = read(std::path::Path::new("/nonexistent/viewr/a.jpg"));
        assert_eq!(meta.size, None);
        assert_eq!(meta.bytes, None);
        assert_eq!(meta.format, "JPG");
    }

    #[test]
    fn the_line_of_an_image_and_of_a_video() {
        let image = FileMeta {
            size: Some((4032, 3024)),
            bytes: Some(2_200_000),
            format: "HEIC".to_owned(),
            date: Some("2026-04-25 14:32".to_owned()),
            video: None,
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
        };
        assert_eq!(
            video.describe(),
            "01:23 \u{b7} 3840\u{d7}2160 \u{b7} 30 fps \u{b7} HLG \u{b7} MOV"
        );
    }

    /// A real iPhone photo carries the date of the shot in its EXIF block:
    /// `VIEWR_TEST_HEIC=/path/to/photo.heic`.
    #[test]
    #[ignore = "needs VIEWR_TEST_HEIC, a real photo"]
    fn a_real_heic_has_its_size_and_the_date_of_the_shot() {
        let Some(path) = crate::decode::real_test_file("VIEWR_TEST_HEIC") else {
            return;
        };
        let meta = read(&path);
        assert!(meta.size.is_some());
        let exif_date = crate::info::date_taken(&path);
        assert!(exif_date.is_some(), "the EXIF date of {}", path.display());
        assert_eq!(meta.date, exif_date);
    }

    #[test]
    fn the_time_gets_hours_from_one_hour_on() {
        assert_eq!(format_time(59.9), "00:59");
        assert_eq!(format_time(3725.0), "1:02:05");
        assert_eq!(format_time(-1.0), "00:00");
    }
}
