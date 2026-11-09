//! The scheduling of the thumbnail jobs and the HEIC previews, on synthetic libheif files.

use crate::app::App;
use crate::test_support::{app_with, app_with_files, test_dir, wait_until};

use super::{ThumbImage, ThumbJob, ThumbPhase, ThumbResult, preview_or_cached, warm_disk_cache};
use crate::image_list::SharedImageList;
use crate::pictures::decode::libheif::decode_heif_preview;
use crate::pictures::jobs::disk_cache::{
    KEY_COMPUTATIONS, LazyEntry, get_cache_file_path, try_load_from_disk_cache,
};
use crate::test_support::write_heif;
use crate::test_support::{BLUE, RED, assert_close};
use image::RgbaImage;
use libheif_rs::{CompressionFormat, ImageOrientation};
use ratatui::layout::Rect;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

fn result(generation: u32, index: usize, phase: ThumbPhase, sharp: bool) -> ThumbResult {
    ThumbResult {
        generation,
        index,
        phase,
        image: ThumbImage::of(Some(RgbaImage::new(1, 1).into()), sharp),
        animated: None,
    }
}

/// An app with one file `path` in a cell of 160x160 px. The 256x128 primary image fits as
/// 160x80.
fn grid_app(path: &Path, cache: &Path) -> App {
    let mut app = App::new((8, 16), SharedImageList::new());
    app.images = vec![path.to_path_buf()];
    app.filenames = vec!["photo.heic".to_owned()];
    app.gallery.filtered_indices = vec![0];
    app.disk_cache_dir = cache.to_path_buf();
    app.scan_complete = true;
    app.spawn_thumb_decode(0, Rect::new(0, 0, 20, 10));
    app
}

/// Poll until the cell 0 gets a new image, for at most 2 s.
fn wait_for_new_image(app: &mut App) -> RgbaImage {
    wait_until(|| app.poll_thumbnails() == vec![0]);
    app.thumb_cache.peek(&0).unwrap().0.image.clone()
}

fn rgba_center(image: &RgbaImage) -> [u8; 4] {
    image.get_pixel(image.width() / 2, image.height() / 2).0
}

#[test]
fn decode_once_then_read_cache() {
    let dir = test_dir();
    let cache = dir.path().join("cache");
    let mut app = app_with_files(dir.path(), &["a.png"]);
    app.disk_cache_dir = cache.clone();
    // 3x2 cells of 8x16 px: a box of 24x32 px.
    let rect = Rect::new(0, 0, 3, 2);
    let path = app.images[0].clone();

    app.spawn_thumb_decode(0, rect);
    // A second request while the first one runs starts nothing.
    app.spawn_thumb_decode(0, rect);
    let mut arrived = Vec::new();
    wait_until(|| {
        arrived.extend(app.poll_thumbnails());
        !arrived.is_empty()
    });
    assert_eq!(arrived, vec![0]);
    assert!(app.thumb_cache.contains(&0));
    let cached = get_cache_file_path(&cache, &path, (24, 32)).unwrap();
    assert!(cached.is_file());
    app.spawn_thumb_decode(0, rect);
    assert!(
        !app.thumb_jobs.sharp.running.contains(&0),
        "a cached thumbnail needs no decode"
    );

    let mut fresh = app_with(&[path.to_str().unwrap()]);
    fresh.disk_cache_dir = cache;
    fresh.spawn_thumb_decode(0, rect);
    let mut arrived = Vec::new();
    wait_until(|| {
        arrived.extend(fresh.poll_thumbnails());
        !arrived.is_empty()
    });
    assert_eq!(
        fresh.thumb_cache.peek(&0).unwrap().0.image.dimensions(),
        (8, 16),
        "the 8x8 image padded to one cell of 8x16 px"
    );
    let on_disk = try_load_from_disk_cache(&fresh.disk_cache_dir, &path, (24, 32)).unwrap();
    assert_eq!(
        on_disk.dimensions(),
        (8, 8),
        "the disk cache stays unpadded"
    );
}

#[test]
fn at_most_four_decodes_run() {
    let dir = test_dir();
    let names = ["a.png", "b.png", "c.png", "d.png", "e.png", "f.png"];
    let mut app = app_with_files(dir.path(), &names);
    app.scan_complete = true;
    let rect = Rect::new(0, 0, 1, 1);
    assert!(!app.has_pending_work());

    for index in 0..names.len() {
        app.spawn_thumb_decode(index, rect);
    }
    assert_eq!(app.thumb_jobs.sharp.running.len(), 4, "four jobs run");
    assert_eq!(app.thumb_jobs.sharp.waiting.len(), 2, "two jobs wait");
    assert_eq!(
        app.thumb_jobs.cache_reads.running.len(),
        2,
        "the waiting cells read the disk cache"
    );
    assert!(app.has_pending_work());
    // A second request for a waiting thumbnail queues nothing.
    app.spawn_thumb_decode(5, rect);
    assert_eq!(app.thumb_jobs.sharp.waiting.len(), 2);

    let mut arrived = Vec::new();
    wait_until(|| {
        arrived.extend(app.poll_thumbnails());
        arrived.len() == names.len()
    });
    arrived.sort_unstable();
    assert_eq!(arrived, (0..names.len()).collect::<Vec<_>>());
    assert!(app.thumb_jobs.sharp.running.is_empty() && app.thumb_jobs.sharp.waiting.is_empty());
    assert!(!app.has_pending_work());

    // The running jobs finish after a render drops the waiting jobs.
    app.mark_dirty();
    for index in 0..names.len() {
        app.spawn_thumb_decode(index, rect);
    }
    app.clear_thumb_queue();
    assert_eq!(app.thumb_jobs.sharp.running.len(), 4);
    assert!(app.thumb_jobs.sharp.waiting.is_empty());
    let mut arrived = Vec::new();
    wait_until(|| {
        arrived.extend(app.poll_thumbnails());
        arrived.len() == 4
    });
}

#[test]
fn cache_hit_skips_decode_slots() {
    let dir = test_dir();
    let cache = dir.path().join("cache");
    let names = ["a.png", "b.png", "c.png", "d.png", "warm.png", "cold.png"];
    let mut app = app_with_files(dir.path(), &names);
    app.disk_cache_dir = cache.clone();
    // 3x2 cells of 8x16 px: a box of 24x32 px, as the folder prefetch fills it.
    let rect = Rect::new(0, 0, 3, 2);
    warm_disk_cache(&cache, &app.images[4], (24, 32));
    // Four slow decodes that never finish hold every decode slot.
    app.thumb_jobs.sharp.running.extend(0..4);

    app.spawn_thumb_decode(4, rect);
    app.spawn_thumb_decode(5, rect);
    assert_eq!(app.thumb_jobs.sharp.waiting.len(), 2, "both decodes wait");
    assert!(
        app.thumb_jobs.cache_reads.running.contains(&4)
            && app.thumb_jobs.cache_reads.running.contains(&5)
    );
    assert!(app.cache_reads_pending());
    let mut arrived = Vec::new();
    wait_until(|| {
        arrived.extend(app.poll_thumbnails());
        !app.cache_reads_pending()
    });

    assert_eq!(arrived, vec![4], "the hit comes while the slots are full");
    assert!(app.thumb_cache.is_sharp(&4));
    assert_eq!(app.thumb_jobs.sharp.running.len(), 4, "the cold cap holds");
    assert_eq!(
        app.thumb_jobs.sharp.waiting,
        [ThumbJob::new(5, rect)],
        "the miss waits for a decode slot, the hit needs none"
    );
    // A new render of the grid reads the missed cell again only after its decode.
    app.clear_thumb_queue();
    app.spawn_thumb_decode(5, rect);
    assert!(!app.cache_reads_pending(), "the miss is known");
    assert_eq!(app.thumb_jobs.sharp.waiting.len(), 1);
}

#[test]
fn waiting_decode_follows_cache_read() {
    let mut app = app_with(&["a.png", "b.png"]);
    let rect = Rect::new(0, 0, 1, 1);
    app.thumb_jobs.sharp.waiting.push(ThumbJob::new(0, rect));
    app.thumb_jobs.sharp.waiting.push(ThumbJob::new(1, rect));
    app.thumb_jobs.cache_reads.running.insert(0);

    app.poll_thumbnails();
    assert_eq!(
        app.thumb_jobs.sharp.waiting,
        [ThumbJob::new(0, rect)],
        "the cell without a cache read starts first"
    );
    assert!(app.thumb_jobs.sharp.running.contains(&1));

    let generation = app.thumb_jobs.generation;
    app.thumb_jobs
        .tx
        .send(ThumbResult {
            generation,
            index: 0,
            phase: ThumbPhase::CacheRead,
            image: ThumbImage::Miss,
            animated: None,
        })
        .unwrap();
    assert!(app.poll_thumbnails().is_empty());
    assert!(app.thumb_jobs.cache_misses.contains(&0));
    assert!(app.thumb_jobs.sharp.running.contains(&0) && app.thumb_jobs.sharp.waiting.is_empty());
}

#[test]
fn missed_cache_read_is_not_repeated() {
    let dir = test_dir();
    let cache = dir.path().join("cache");
    let app = app_with_files(dir.path(), &["a.png"]);
    let path = &app.images[0];
    warm_disk_cache(&cache, path, (24, 32));
    let entry = LazyEntry::new(&cache, path, (24, 32));

    let (image, sharp) = preview_or_cached(path, (24, 32), &entry, true);
    assert!(
        image.is_some() && sharp,
        "a job without a known miss reads the cache"
    );
    let (image, sharp) = preview_or_cached(path, (24, 32), &entry, false);
    assert!(
        image.is_none() && !sharp,
        "after a miss, only the embedded preview counts, and a PNG has none"
    );
}

#[test]
fn sharp_drops_waiting_preview_and_read() {
    let mut app = app_with(&["a.heic", "b.heic"]);
    let rect = Rect::new(0, 0, 1, 1);
    app.thumb_jobs.previews.waiting.push(ThumbJob::new(0, rect));
    app.thumb_jobs.previews.waiting.push(ThumbJob::new(1, rect));
    app.thumb_jobs.cache_misses.insert(0);

    app.thumb_jobs
        .cache_reads
        .waiting
        .push(ThumbJob::new(0, rect));

    assert!(app.store_thumbnail(0, RgbaImage::new(1, 1), true));
    assert_eq!(app.thumb_jobs.previews.waiting, [ThumbJob::new(1, rect)]);
    assert!(
        app.thumb_jobs.cache_reads.waiting.is_empty(),
        "the read is not needed"
    );
    assert!(
        !app.thumb_jobs.cache_misses.contains(&0),
        "a cell that leaves the cache reads it again"
    );
}

#[test]
fn drops_old_generation_results() {
    let mut app = app_with(&["a.png", "b.png"]);
    app.mark_dirty();
    let current = app.thumb_jobs.generation;
    app.thumb_jobs
        .tx
        .send(result(current - 1, 0, ThumbPhase::Sharp, true))
        .unwrap();
    app.thumb_jobs
        .tx
        .send(result(current, 1, ThumbPhase::Sharp, true))
        .unwrap();

    assert_eq!(app.poll_thumbnails(), vec![1]);
    assert!(!app.thumb_cache.contains(&0));
    assert!(app.thumb_cache.contains(&1));
}

#[test]
fn old_preview_dropped_and_slot_freed() {
    let mut app = app_with(&["a.heic", "b.heic"]);
    app.scan_complete = true;
    app.gallery.update_grid(Rect::new(0, 0, 100, 40));
    let before = app.thumb_jobs.generation;
    app.thumb_jobs.previews.running.insert(0);
    app.thumb_jobs.sharp.running.insert(1);
    app.thumb_zoom_in();
    let current = app.thumb_jobs.generation;
    assert_ne!(current, before, "a grid zoom starts a new generation");
    assert!(app.thumb_jobs.previews.running.is_empty() && app.thumb_jobs.sharp.running.is_empty());

    // The preview of the old zoom, then a preview and a failed sharp decode of the new zoom.
    app.thumb_jobs
        .tx
        .send(result(before, 0, ThumbPhase::Preview, false))
        .unwrap();
    app.thumb_jobs
        .tx
        .send(result(current, 1, ThumbPhase::Preview, false))
        .unwrap();
    app.thumb_jobs.previews.running.insert(1);
    app.thumb_jobs.sharp.running.insert(1);
    app.thumb_jobs
        .tx
        .send(ThumbResult {
            generation: current,
            index: 1,
            phase: ThumbPhase::Sharp,
            image: ThumbImage::Miss,
            animated: None,
        })
        .unwrap();

    assert_eq!(app.poll_thumbnails(), vec![1]);
    assert!(
        !app.thumb_cache.contains(&0),
        "the stale preview is dropped"
    );
    assert!(app.thumb_cache.contains(&1) && !app.thumb_cache.is_sharp(&1));
    assert!(!app.has_pending_work(), "a failed decode frees its slot");
}

/// The tests never look up ffmpeg and ffprobe, as a session without them.
#[test]
#[cfg(feature = "video")]
fn no_ffmpeg_video_frees_slot() {
    let dir = test_dir();
    let clip = dir.path().join("clip.mp4");
    fs::write(&clip, "not decoded").unwrap();
    let mut app = grid_app(&clip, dir.path());
    wait_until(|| {
        app.poll_thumbnails();
        !app.has_pending_work()
    });
    assert!(!app.thumb_cache.contains(&0));
    assert!(app.is_video(0), "the cell shows the play icon");
}

#[test]
fn only_libheif_sharp_jobs_wait() {
    let dir = test_dir();
    let mut app = app_with_files(dir.path(), &["b.png"]);
    app.images.push(PathBuf::from("a.heic"));
    app.filenames.push("a.heic".to_owned());
    app.gallery.filtered_indices.push(1);
    let rect = Rect::new(0, 0, 1, 1);
    // A preview job runs. The HEIC cell waits, and the PNG cell starts now.
    app.thumb_jobs.previews.running.insert(1);
    app.spawn_thumb_decode(1, rect);
    app.spawn_thumb_decode(0, rect);

    assert_eq!(
        app.thumb_jobs.sharp.waiting.len(),
        1,
        "the HEIC sharp job waits"
    );
    assert!(
        app.thumb_jobs.sharp.running.contains(&0),
        "the PNG decodes at once"
    );
    assert_eq!(app.poll_thumbnails(), Vec::<usize>::new());
    assert_eq!(
        app.thumb_jobs.sharp.waiting.len(),
        1,
        "the poll keeps the HEIC job waiting"
    );
}

#[test]
fn sharp_result_replaces_preview() {
    let mut app = app_with(&["a.heic", "b.heic"]);
    let rect = Rect::new(0, 0, 1, 1);
    assert!(app.store_thumbnail(0, RgbaImage::new(1, 1), false));
    let (_, preview_id) = app.thumb_cache.peek(&0).unwrap();
    app.thumb_jobs.sharp.waiting.push(ThumbJob::new(0, rect));
    app.thumb_jobs.sharp.waiting.push(ThumbJob::new(1, rect));

    assert!(
        !app.store_thumbnail(0, RgbaImage::new(1, 1), false),
        "a second preview changes nothing"
    );
    assert!(app.store_thumbnail(0, RgbaImage::new(2, 2), true));
    assert!(app.thumb_cache.is_sharp(&0));
    assert_eq!(app.stale_image_ids, vec![(0, preview_id)]);
    assert_eq!(
        app.thumb_jobs.sharp.waiting.len(),
        1,
        "the disk cache hit made the job unnecessary"
    );
    assert!(
        !app.store_thumbnail(0, RgbaImage::new(3, 3), true),
        "a second sharp image changes nothing"
    );
    assert_eq!(
        app.thumb_cache.peek(&0).unwrap().0.image.dimensions(),
        (2, 2)
    );
}

#[test]
fn heic_shows_preview_then_sharp() {
    let dir = test_dir();
    let path = dir.path().join("photo.heic");
    let normal = ImageOrientation::Normal;
    write_heif(&path, CompressionFormat::Hevc, normal, Some(normal));
    let cache = dir.path().join("cache");
    fs::create_dir(&cache).unwrap();
    let mut app = grid_app(&path, &cache);
    assert_eq!(
        app.thumb_jobs.previews.running.len(),
        1,
        "the preview starts at once"
    );
    assert_eq!(
        app.thumb_jobs.sharp.waiting.len(),
        1,
        "the sharp decode waits for the preview"
    );

    let preview = wait_for_new_image(&mut app);
    assert!(!app.thumb_cache.is_sharp(&0));
    assert_eq!(
        preview.dimensions(),
        (64, 32),
        "the 64x32 thumbnail has the shape of the 160x80 sharp image: no padding"
    );
    assert_eq!(app.thumb_cache.peek(&0).unwrap().0.cells, (20, 5));
    assert_close(rgba_center(&preview), BLUE);
    assert_eq!(
        fs::read_dir(&cache).unwrap().count(),
        0,
        "no preview on disk"
    );
    let (_, preview_id) = app.thumb_cache.peek(&0).unwrap();

    let sharp = wait_for_new_image(&mut app);
    assert!(app.thumb_cache.is_sharp(&0));
    assert_eq!(
        sharp.dimensions(),
        (160, 80),
        "20x5 whole cells, no padding"
    );
    assert_eq!(app.thumb_cache.peek(&0).unwrap().0.cells, (20, 5));
    assert_close(rgba_center(&sharp), RED);
    assert_eq!(
        fs::read_dir(&cache).unwrap().count(),
        1,
        "the sharp image on disk"
    );
    assert_eq!(app.stale_image_ids, vec![(0, preview_id)]);
    assert!(!app.has_pending_work());

    let mut warm = grid_app(&path, &cache);
    let first = wait_for_new_image(&mut warm);
    assert!(warm.thumb_cache.is_sharp(&0));
    assert_close(rgba_center(&first), RED);
    assert!(warm.stale_image_ids.is_empty());
    wait_until(|| {
        assert!(warm.poll_thumbnails().is_empty(), "no second image");
        !warm.has_pending_work()
    });
}

/// A HEIC cell that waits for a preview slot misses the cache ahead. Its preview job then
/// reads no cache and saves nothing: it computes no cache key.
#[test]
fn heic_preview_skips_cache_key() {
    let dir = test_dir();
    let path = dir.path().join("photo.heic");
    let normal = ImageOrientation::Normal;
    write_heif(&path, CompressionFormat::Hevc, normal, Some(normal));
    let mut app = App::new((8, 16), SharedImageList::new());
    app.images = vec![path.clone()];
    app.filenames = vec!["photo.heic".to_owned()];
    app.gallery.filtered_indices = vec![0];
    app.disk_cache_dir = dir.path().join("cache");
    app.scan_complete = true;
    // Four other previews hold the slots. The cell waits and reads the cache ahead.
    app.thumb_jobs.previews.running.extend(100..104);

    app.spawn_thumb_decode(0, Rect::new(0, 0, 20, 10));
    wait_until(|| {
        app.poll_thumbnails();
        app.thumb_jobs.cache_misses.contains(&0)
    });
    assert_eq!(KEY_COMPUTATIONS.of(&path), 1, "the missed cache read");
    app.thumb_jobs.previews.running.clear();
    wait_until(|| app.poll_thumbnails() == vec![0]);
    assert!(!app.thumb_cache.is_sharp(&0), "the preview comes first");
    wait_until(|| {
        app.poll_thumbnails();
        app.thumb_cache.is_sharp(&0)
    });

    assert_eq!(
        KEY_COMPUTATIONS.of(&path),
        2,
        "the preview computes no key, the sharp decode computes one for its save"
    );
}

#[test]
fn other_transform_gives_no_preview() {
    let dir = test_dir();
    let path = dir.path().join("rotated.heic");
    write_heif(
        &path,
        CompressionFormat::Hevc,
        ImageOrientation::Rotate90Cw,
        Some(ImageOrientation::Normal),
    );
    let error = decode_heif_preview(&path, (160, 160)).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);

    let mut app = grid_app(&path, dir.path());
    let first = wait_for_new_image(&mut app);
    assert!(
        app.thumb_cache.is_sharp(&0),
        "the first image is the sharp one"
    );
    assert_eq!(
        first.dimensions(),
        (80, 160),
        "10x10 whole cells, no padding"
    );
    assert_eq!(app.thumb_cache.peek(&0).unwrap().0.cells, (10, 10));
    assert_close(rgba_center(&first), RED);
    assert!(app.stale_image_ids.is_empty());
}

#[test]
fn heic_preview_keeps_rotation() {
    let dir = test_dir();
    let path = dir.path().join("rotated.heic");
    let rotation = ImageOrientation::Rotate90Cw;
    write_heif(&path, CompressionFormat::Hevc, rotation, Some(rotation));

    let preview = decode_heif_preview(&path, (160, 160)).unwrap();
    assert_eq!(
        preview.dimensions(),
        (32, 64),
        "portrait like the primary image, at its own size"
    );
    assert_close(rgba_center(&preview), BLUE);

    // A box smaller than the thumbnail scales it down to the size of the sharp image.
    let small = decode_heif_preview(&path, (40, 40)).unwrap();
    assert_eq!(small.dimensions(), (20, 40));
}

#[test]
fn empty_cells_start_before_heic() {
    let mut app = app_with(&["a.heic", "b.jpg", "c.heic", "d.png"]);
    let rect = Rect::new(0, 0, 1, 1);
    assert!(app.store_thumbnail(0, RgbaImage::new(1, 1), false));
    assert!(app.store_thumbnail(2, RgbaImage::new(1, 1), false));
    for index in 0..4 {
        app.thumb_jobs
            .sharp
            .waiting
            .push(ThumbJob::new(index, rect));
    }
    app.thumb_jobs.sharp.running.extend(10..13);

    assert_eq!(
        app.next_sharp_job(&HashSet::new()),
        Some(1),
        "b.jpg shows nothing yet"
    );
    app.thumb_jobs.sharp.waiting.remove(1);
    assert_eq!(
        app.next_sharp_job(&HashSet::new()),
        Some(2),
        "d.png, now at position 2"
    );
    app.thumb_jobs.sharp.waiting.remove(2);
    assert_eq!(
        app.next_sharp_job(&HashSet::new()),
        Some(0),
        "then the HEIC cells in grid order"
    );

    app.thumb_jobs.previews.running.insert(2);
    assert_eq!(
        app.next_sharp_job(&HashSet::new()),
        None,
        "every HEIC sharp job waits"
    );
    app.thumb_jobs.previews.running.clear();
    assert_eq!(
        app.next_sharp_job(&HashSet::from([0])),
        Some(1),
        "cell 0 reads the cache, so cell 2 at position 1 goes first"
    );
}
