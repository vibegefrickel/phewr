//! The work of one thumbnail job: the disk cache, the decode, the preview and the animation
//! flag.

use super::AnimationFlag;
use image::RgbaImage;
use std::io;
use std::path::Path;
use tuilibr::pictures::decode::EmbeddedThumbnail;
#[cfg(all(feature = "sips", target_os = "macos"))]
use tuilibr::pictures::decode::embedded_jpeg;
#[cfg(feature = "heic")]
use tuilibr::pictures::decode::libheif::{decode_heif_preview, decodes_with_libheif};
use tuilibr::pictures::jobs::disk_cache::{CacheEntry, LazyEntry};
#[cfg(feature = "video")]
use tuilibr::pictures::motion::animation::{has_animation_extension, is_animated};
use tuilibr::pictures::resize::load_and_resize_px;

/// The animation flag of an animation format, `None` for the other formats.
///
/// The job reads the header, because the draw must read no file. A `known` flag with the same
/// file stamp opens no file.
pub(super) fn animation_flag(
    path: &Path,
    entry: &LazyEntry<'_>,
    known: Option<AnimationFlag>,
) -> Option<AnimationFlag> {
    if !has_animation_extension(path) {
        return None;
    }
    let stamp = entry.get().map(|entry| entry.stamp);
    if let Some(known) = known
        && stamp.is_some()
        && known.stamp == stamp
    {
        return Some(known);
    }
    #[cfg(all(test, feature = "video"))]
    HEADER_READS.add(path);
    Some(AnimationFlag {
        animated: is_animated(path),
        stamp,
    })
}

/// Without the player, no file is an animation.
#[cfg(not(feature = "video"))]
fn has_animation_extension(_path: &Path) -> bool {
    false
}

#[cfg(not(feature = "video"))]
fn is_animated(_path: &Path) -> bool {
    false
}

#[cfg(all(test, feature = "video"))]
static HEADER_READS: tuilibr::pictures::jobs::disk_cache::PathCounter =
    tuilibr::pictures::jobs::disk_cache::PathCounter::new();

/// True for a video or an animated image: its still gets the play stamp.
#[cfg(feature = "video")]
pub(super) fn is_playable(path: &Path, animated: Option<bool>) -> bool {
    tuilibr::pictures::motion::first_frame::is_video(path) || animated == Some(true)
}

#[cfg(not(feature = "video"))]
pub(super) fn is_playable(_path: &Path, _animated: Option<bool>) -> bool {
    false
}

/// Put the sharp thumbnail of `path` into the disk cache, as the sharp job of the grid does.
/// A cached file costs one key and one `stat`.
pub(in crate::app) fn warm_disk_cache(cache_dir: &Path, path: &Path, target: (u32, u32)) {
    let entry = LazyEntry::new(cache_dir, path, target);
    if entry.get().is_none_or(CacheEntry::exists) {
        return;
    }
    decode_into_cache(cache_dir, path, target, &entry);
}

pub(super) fn sharp_thumbnail(
    cache_dir: &Path,
    path: &Path,
    target: (u32, u32),
    entry: &LazyEntry<'_>,
) -> Option<RgbaImage> {
    entry
        .load()
        .or_else(|| decode_into_cache(cache_dir, path, target, entry))
}

pub(super) fn decode_into_cache(
    cache_dir: &Path,
    path: &Path,
    target: (u32, u32),
    entry: &LazyEntry<'_>,
) -> Option<RgbaImage> {
    let image = decode_thumbnail(cache_dir, path, target).ok()?;
    save_into(entry, &image);
    Some(image)
}

fn save_into(entry: &LazyEntry<'_>, image: &RgbaImage) {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "a thumbnail that misses the disk cache decodes again on the next visit"
    )]
    let _ = entry.save(image);
}

/// The sharp thumbnail from the disk cache (`true`), or the embedded preview.
///
/// An embedded JPEG that covers the box is sharp too and goes into the disk cache. Without
/// `read_cache`, the job skips the cache: the cache read of the cell missed.
pub(super) fn preview_or_cached(
    path: &Path,
    target: (u32, u32),
    entry: &LazyEntry<'_>,
    read_cache: bool,
) -> (Option<RgbaImage>, bool) {
    if read_cache && let Some(image) = entry.load() {
        return (Some(image), true);
    }
    match decode_preview(path, target) {
        Ok((image, true)) => {
            save_into(entry, &image);
            (Some(image), true)
        }
        Ok((image, false)) => (Some(image), false),
        Err(_) => (None, false),
    }
}

/// The first frame of a video, or the fitted image. A video writes its color cube into
/// `cache_dir`.
fn decode_thumbnail(cache_dir: &Path, path: &Path, target: (u32, u32)) -> io::Result<RgbaImage> {
    #[cfg(feature = "video")]
    if tuilibr::pictures::motion::first_frame::is_video(path) {
        return crate::video::session_first_frame(path, target, cache_dir);
    }
    #[cfg(not(feature = "video"))]
    let _ = cache_dir;
    load_and_resize_px(path, target, EmbeddedThumbnail::Allow)
}

/// True for the libheif formats, camera RAW and PSD.
pub(super) fn has_embedded_preview(path: &Path) -> bool {
    #[cfg(not(any(feature = "heic", all(feature = "sips", target_os = "macos"))))]
    let _ = path;
    #[cfg(feature = "heic")]
    let heif = decodes_with_libheif(path);
    #[cfg(not(feature = "heic"))]
    let heif = false;
    #[cfg(all(feature = "sips", target_os = "macos"))]
    let jpeg = embedded_jpeg::may_have_embedded_jpeg(path);
    #[cfg(not(all(feature = "sips", target_os = "macos")))]
    let jpeg = false;
    heif || jpeg
}

/// The embedded preview of a cell, and whether it is sharp.
///
/// A HEIC preview is never sharp. The embedded JPEG of a RAW or PSD file is sharp when it
/// covers the box, and then fits the box like a sharp decode.
fn decode_preview(path: &Path, target: (u32, u32)) -> io::Result<(RgbaImage, bool)> {
    #[cfg(feature = "heic")]
    if decodes_with_libheif(path) {
        return decode_heif_preview(path, target).map(|image| (image, false));
    }
    #[cfg(all(feature = "sips", target_os = "macos"))]
    if let Some(jpeg) = embedded_jpeg::find(path) {
        let sharp = jpeg.covers(target);
        let image = embedded_jpeg::decode(path, &jpeg, Some(target))?;
        return Ok((image.fit_within(target.0, target.1), sharp));
    }
    #[cfg(not(any(feature = "heic", all(feature = "sips", target_os = "macos"))))]
    let _ = (path, target);
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no embedded preview in this file",
    ))
}

#[cfg(test)]
#[cfg(all(feature = "sips", target_os = "macos"))]
mod embedded_jpeg_tests {
    use crate::app::App;
    use crate::image_list::SharedImageList;
    use crate::test_support::{BLUE, RED, assert_close, test_dir, write_psd, write_tiff_raw};
    use image::RgbaImage;
    use ratatui::layout::Rect;
    use std::fs;
    use std::path::Path;

    /// An app with one file `path` in the cell `rect` of 8x16 px cells. The job starts now.
    fn grid_app(path: &Path, cache: &Path, rect: Rect) -> App {
        let mut app = App::new((8, 16), SharedImageList::new());
        app.images = vec![path.to_path_buf()];
        app.filenames = vec![path.file_name().unwrap().to_string_lossy().into_owned()];
        app.gallery.filtered_indices = vec![0];
        app.disk_cache_dir = cache.to_path_buf();
        app.scan_complete = true;
        app.spawn_thumb_decode(0, rect);
        app
    }

    /// Poll until the cell 0 gets a new image, for at most 5 s. A cold sips start can take more
    /// than the 2 s of `wait_until`.
    fn wait_for_new_image(app: &mut App) -> RgbaImage {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.poll_thumbnails() != vec![0] {
            assert!(std::time::Instant::now() < deadline, "no thumbnail arrived");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        app.thumb_cache.peek(&0).unwrap().0.image.clone()
    }

    fn left_right(image: &RgbaImage) -> ([u8; 4], [u8; 4]) {
        let y = image.height() / 2;
        (
            image.get_pixel(1, y).0,
            image.get_pixel(image.width() - 2, y).0,
        )
    }

    #[test]
    fn raw_takes_embedded_jpeg_as_sharp() {
        let dir = test_dir();
        let path = dir.path().join("photo.dng");
        write_tiff_raw(&path, 1);
        let cache = dir.path().join("cache");
        // A box of 32x32 px: the 64x32 JPEG covers it. The preview phase gives the sharp thumbnail,
        // and the poll drops the sharp job before it runs sips.
        let mut app = grid_app(&path, &cache, Rect::new(0, 0, 4, 2));
        assert!(app.thumb_jobs.previews.running.contains(&0));
        assert_eq!(
            app.thumb_jobs.sharp.waiting.len(),
            1,
            "the sips job waits for the preview"
        );

        let sharp = wait_for_new_image(&mut app);
        assert!(app.thumb_cache.is_sharp(&0));
        assert_eq!(
            sharp.dimensions(),
            (32, 16),
            "fitted into the box, 4x1 cells"
        );
        assert_eq!(app.thumb_cache.peek(&0).unwrap().0.cells, (4, 1));
        let (left, right) = left_right(&sharp);
        assert_close(left, RED);
        assert_close(right, BLUE);
        assert!(app.thumb_jobs.sharp.waiting.is_empty(), "no sips decode");
        assert!(app.thumb_jobs.sharp.running.is_empty());
        assert!(!app.has_pending_work());
        assert_eq!(
            fs::read_dir(&cache).unwrap().count(),
            1,
            "the embedded JPEG went into the disk cache"
        );
    }

    #[test]
    fn psd_shows_thumbnail_then_sips_decode() {
        let dir = test_dir();
        let path = dir.path().join("design.psd");
        write_psd(&path, true);
        let cache = dir.path().join("cache");
        // A box of 160x160 px: the 64x32 thumbnail is too small and stays the preview.
        let mut app = grid_app(&path, &cache, Rect::new(0, 0, 20, 10));

        let preview = wait_for_new_image(&mut app);
        assert!(!app.thumb_cache.is_sharp(&0));
        assert_eq!(preview.dimensions(), (64, 32), "its own resolution");
        assert_eq!(app.thumb_cache.peek(&0).unwrap().0.cells, (20, 5));
        let (left, right) = left_right(&preview);
        assert_close(left, RED);
        assert_close(right, BLUE);
        assert!(fs::read_dir(&cache).is_err(), "no preview on disk");
        let (_, preview_id) = app.thumb_cache.peek(&0).unwrap();

        let sharp = wait_for_new_image(&mut app);
        assert!(app.thumb_cache.is_sharp(&0));
        assert_eq!(
            sharp.dimensions(),
            (64, 32),
            "sips does not enlarge the 64x32 PSD"
        );
        assert_eq!(app.thumb_cache.peek(&0).unwrap().0.cells, (8, 2));
        let (left, right) = left_right(&sharp);
        assert_close(left, RED);
        assert_close(right, BLUE);
        assert_eq!(app.stale_image_ids, vec![(0, preview_id)]);
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 1);
        assert!(!app.has_pending_work());
    }

    #[test]
    fn psd_without_thumbnail_uses_sips() {
        let dir = test_dir();
        let path = dir.path().join("plain.psd");
        write_psd(&path, false);
        let mut app = grid_app(&path, &dir.path().join("cache"), Rect::new(0, 0, 20, 10));

        let first = wait_for_new_image(&mut app);
        assert!(
            app.thumb_cache.is_sharp(&0),
            "the first image is the sharp one"
        );
        assert_eq!(first.dimensions(), (64, 32));
        assert!(app.stale_image_ids.is_empty());
        assert!(!app.has_pending_work(), "the failed preview freed its slot");
    }
}

#[cfg(test)]
mod read_count_tests {
    use super::warm_disk_cache;
    use crate::app::App;
    use crate::test_support::{app_with_files, test_dir, wait_until};
    use ratatui::layout::Rect;
    use std::path::PathBuf;
    use tempfile::TempDir;
    use tuilibr::pictures::jobs::disk_cache::KEY_COMPUTATIONS;

    /// 3x2 cells of 8x16 px: a box of 24x32 px.
    const RECT: Rect = Rect::new(0, 0, 3, 2);

    fn png_app() -> (TempDir, App, PathBuf) {
        let dir = test_dir();
        let mut app = app_with_files(dir.path(), &["a.png"]);
        app.disk_cache_dir = dir.path().join("cache");
        let path = app.images[0].clone();
        (dir, app, path)
    }

    fn decode_sharp(app: &mut App) {
        app.spawn_thumb_decode(0, RECT);
        wait_until(|| {
            app.poll_thumbnails();
            app.thumb_cache.is_sharp(&0)
        });
    }

    /// A cold cell through a missed cache read and its decode, then a warm cell after a zoom.
    fn cold_then_warm(app: &mut App) {
        // Four other decodes hold the slots. The cell waits and reads the cache ahead.
        app.thumb_jobs.sharp.running.extend(100..104);
        app.spawn_thumb_decode(0, RECT);
        wait_until(|| {
            app.poll_thumbnails();
            app.thumb_jobs.cache_misses.contains(&0)
        });
        app.thumb_jobs.sharp.running.clear();
        wait_until(|| {
            app.poll_thumbnails();
            app.thumb_cache.is_sharp(&0)
        });
        app.forget_thumbnails();
        decode_sharp(app);
    }

    #[test]
    fn each_job_computes_key_once() {
        let (_dir, mut app, path) = png_app();

        cold_then_warm(&mut app);

        assert_eq!(
            KEY_COMPUTATIONS.of(&path),
            3,
            "the missed cache read, the sharp decode with its save, the warm read"
        );
    }

    #[test]
    fn prefetch_computes_one_key_per_file() {
        let (dir, _app, path) = png_app();
        let cache = dir.path().join("cache");

        warm_disk_cache(&cache, &path, (24, 32));
        assert_eq!(
            KEY_COMPUTATIONS.of(&path),
            1,
            "the check, the decode and the save"
        );
        warm_disk_cache(&cache, &path, (24, 32));
        assert_eq!(KEY_COMPUTATIONS.of(&path), 2, "a warm file only checks");
    }

    #[test]
    #[cfg(feature = "video")]
    fn header_read_once_until_file_changes() {
        let (_dir, mut app, path) = png_app();

        cold_then_warm(&mut app);
        assert_eq!(
            super::HEADER_READS.of(&path),
            1,
            "the sharp decode reads the header, the miss and the warm read do not"
        );
        assert_eq!(app.animated.get(&path), Some(&false));

        image::RgbaImage::new(9, 9).save(&path).unwrap();
        app.forget_thumbnails();
        decode_sharp(&mut app);
        assert_eq!(
            super::HEADER_READS.of(&path),
            2,
            "a changed file reads its header again"
        );
    }

    /// A cold AVIF cell runs a preview job and a sharp job. The second takes the flag of the first.
    #[test]
    #[cfg(all(feature = "video", feature = "heic"))]
    fn cold_avif_reads_header_once() {
        let dir = test_dir();
        let path = dir.path().join("still.avif");
        std::fs::copy(crate::test_support::module_fixture("still.avif"), &path).unwrap();
        let mut app = crate::test_support::app();
        app.images = vec![path.clone()];
        app.filenames = vec!["still.avif".to_owned()];
        app.gallery.filtered_indices = vec![0];
        app.disk_cache_dir = dir.path().join("cache");

        decode_sharp(&mut app);

        assert_eq!(
            KEY_COMPUTATIONS.of(&path),
            2,
            "the preview job and the sharp job"
        );
        assert_eq!(super::HEADER_READS.of(&path), 1);
        assert_eq!(app.animated.get(&path), Some(&false));
    }
}
