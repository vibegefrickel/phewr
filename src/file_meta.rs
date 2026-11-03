//! The facts of one file for the status row. A thread reads them, and a cursor move never
//! waits.

use crate::{decode, info};
use chrono::{DateTime, Local};
use std::path::Path;

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
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct VideoMeta {
    /// 0 when the container does not tell it.
    pub(crate) duration: f64,
    pub(crate) fps: f64,
    /// `HLG` or `PQ`, `None` for SDR.
    pub(crate) hdr: Option<&'static str>,
}

/// A missing part stays `None`.
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
    use super::{FileMeta, VideoMeta, format_time, read};
    use image::RgbImage;

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

    /// `PHEWR_TEST_HEIC=/path/to/photo.heic`.
    #[test]
    #[ignore = "needs PHEWR_TEST_HEIC, a real photo"]
    fn real_heic_has_size_and_date() {
        let Some(path) = crate::decode::real_test_file("PHEWR_TEST_HEIC") else {
            return;
        };
        let meta = read(&path);
        assert!(meta.size.is_some());
        let exif_date = crate::info::date_taken(&path);
        assert!(exif_date.is_some(), "the EXIF date of {}", path.display());
        assert_eq!(meta.date, exif_date);
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
