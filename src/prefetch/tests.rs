//! The tests of the prefetch: the jobs, the cache, the generations and the neighbor order.

use super::{Prefetcher, capped_box};
use image::RgbaImage;
use ratatui::layout::Rect;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;

fn synthetic_dir() -> (TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    (temp, root)
}

fn save_image(dir: &Path, name: &str, width: u32, height: u32) -> PathBuf {
    let path = dir.join(name);
    RgbaImage::new(width, height).save(&path).unwrap();
    path
}

fn wait_until_idle(prefetcher: &mut Prefetcher) {
    for _ in 0..200 {
        prefetcher.poll();
        if prefetcher.pending_count() == 0 {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("prefetch jobs did not finish in time");
}

#[test]
fn target_hint_stays_below_cap() {
    let mut prefetcher = Prefetcher::new();
    // 480x270 cells of 16 px: a box of 7680x4320 px.
    prefetcher.set_target_hint(Rect::new(0, 0, 480, 270), (16, 16));
    assert_eq!(prefetcher.target_hint, Some(capped_box((7680, 4320))));
    assert!(prefetcher.target_hint.is_some_and(|(w, _)| w < 7680));
    prefetcher.set_target_hint(Rect::new(0, 0, 240, 135), (16, 16));
    assert_eq!(prefetcher.target_hint, Some((3840, 2160)));
}

#[test]
fn decodes_neighbors_async() {
    let (_temp, dir) = synthetic_dir();
    let paths: Vec<_> = (0..5)
        .map(|i| save_image(&dir, &format!("{i}.png"), 8, 8))
        .collect();

    let mut prefetcher = Prefetcher::new();
    prefetcher.set_target_hint(Rect::new(0, 0, 2, 2), (2, 2));
    prefetcher.kick(2, 0, &paths);
    wait_until_idle(&mut prefetcher);

    for i in [0, 1, 3, 4] {
        let img = prefetcher
            .take_resized(i, Rect::default(), (1, 1))
            .expect("neighbor should be cached");
        assert_eq!(img.dimensions(), (4, 4));
    }
    assert!(
        prefetcher
            .take_resized(2, Rect::default(), (1, 1))
            .is_none()
    );
}

#[test]
fn gallery_decodes_selected() {
    let (_temp, dir) = synthetic_dir();
    let paths: Vec<_> = (0..3)
        .map(|i| save_image(&dir, &format!("{i}.png"), 4, 4))
        .collect();

    let mut prefetcher = Prefetcher::new();
    prefetcher.kick_gallery(1, &paths);
    wait_until_idle(&mut prefetcher);

    let img = prefetcher
        .take_resized(1, Rect::default(), (1, 1))
        .expect("gallery target should be cached");
    assert_eq!(img.dimensions(), (4, 4));
}

#[test]
fn zero_hint_keeps_size() {
    let (_temp, dir) = synthetic_dir();
    let paths = vec![save_image(&dir, "0.png", 8, 8)];

    let mut prefetcher = Prefetcher::new();
    prefetcher.set_target_hint(Rect::new(0, 0, 0, 5), (1, 1));
    prefetcher.kick_gallery(0, &paths);
    wait_until_idle(&mut prefetcher);

    let img = prefetcher
        .take_resized(0, Rect::default(), (1, 1))
        .expect("image should be cached");
    assert_eq!(img.dimensions(), (8, 8));
}

#[test]
fn invalidate_clears_cache() {
    let (_temp, dir) = synthetic_dir();
    let paths: Vec<_> = (0..3)
        .map(|i| save_image(&dir, &format!("{i}.png"), 4, 4))
        .collect();

    let mut prefetcher = Prefetcher::new();
    prefetcher.kick(1, 0, &paths);
    wait_until_idle(&mut prefetcher);
    prefetcher.kick(1, 0, &paths);
    wait_until_idle(&mut prefetcher);
    prefetcher.invalidate();

    for i in 0..3 {
        assert!(
            prefetcher
                .take_resized(i, Rect::default(), (1, 1))
                .is_none()
        );
    }
}

#[test]
fn lru_evicts_oldest() {
    let (_temp, dir) = synthetic_dir();
    let paths: Vec<_> = (0..10)
        .map(|i| save_image(&dir, &format!("{i}.png"), 4, 4))
        .collect();

    let mut prefetcher = Prefetcher::new();
    for i in 0..10 {
        prefetcher.kick_gallery(i, &paths);
        wait_until_idle(&mut prefetcher);
    }

    assert!(
        prefetcher
            .take_resized(0, Rect::default(), (1, 1))
            .is_none()
    );
    assert!(
        prefetcher
            .take_resized(1, Rect::default(), (1, 1))
            .is_none()
    );
    for i in 2..10 {
        assert!(
            prefetcher
                .take_resized(i, Rect::default(), (1, 1))
                .is_some()
        );
    }
}

#[test]
fn drops_result_after_invalidate() {
    let (_temp, dir) = synthetic_dir();
    let paths = vec![save_image(&dir, "0.png", 4, 4)];

    let mut prefetcher = Prefetcher::new();
    prefetcher.kick_gallery(0, &paths);
    // The job of the old list is still in flight.
    prefetcher.invalidate();
    assert!(
        prefetcher.accept_next(Duration::from_secs(10)),
        "the stale job did not send its result in time"
    );

    assert_eq!(prefetcher.pending_count(), 0);
    assert!(
        prefetcher
            .take_resized(0, Rect::default(), (1, 1))
            .is_none(),
        "a result of an older generation must not enter the cache"
    );

    prefetcher.kick_gallery(0, &paths);
    assert_eq!(prefetcher.pending_count(), 1, "the new list decodes again");
    wait_until_idle(&mut prefetcher);
    assert!(
        prefetcher
            .take_resized(0, Rect::default(), (1, 1))
            .is_some()
    );
}

#[test]
fn remembers_failure_until_invalidate() {
    let (_temp, dir) = synthetic_dir();
    let path = dir.join("0.png");
    fs::write(&path, b"this is not a PNG file").unwrap();
    let paths = vec![path];

    let mut prefetcher = Prefetcher::new();
    prefetcher.kick_gallery(0, &paths);
    wait_until_idle(&mut prefetcher);
    assert!(
        prefetcher
            .take_resized(0, Rect::default(), (1, 1))
            .is_none()
    );

    prefetcher.kick_gallery(0, &paths);
    assert_eq!(
        prefetcher.pending_count(),
        0,
        "a corrupt file must not decode again on each kick"
    );

    prefetcher.invalidate();
    prefetcher.kick_gallery(0, &paths);
    assert_eq!(prefetcher.pending_count(), 1, "invalidate allows a new try");
    wait_until_idle(&mut prefetcher);
}

#[test]
fn unhinted_decode_fits_box() {
    let (_temp, dir) = synthetic_dir();
    let paths = vec![save_image(&dir, "0.png", 64, 32)];

    let mut prefetcher = Prefetcher::new();
    prefetcher.kick_gallery(0, &paths);
    wait_until_idle(&mut prefetcher);

    let img = prefetcher
        .take_resized(0, Rect::new(0, 0, 2, 2), (8, 8))
        .expect("image should be cached");
    assert_eq!(img.dimensions(), (16, 8));
}

#[test]
fn neighbor_order_follows_direction() {
    use super::neighbor_order;
    assert_eq!(neighbor_order(4, 0..9, None), vec![5, 3, 6, 2, 7, 1]);
    assert_eq!(neighbor_order(4, 0..9, Some(true)), vec![5, 6, 7, 3, 2, 1]);
    assert_eq!(neighbor_order(4, 0..9, Some(false)), vec![3, 2, 1, 5, 6, 7]);
    assert_eq!(neighbor_order(0, 0..2, None), vec![1]);
    assert_eq!(neighbor_order(1, 0..2, Some(true)), vec![0]);
    assert!(neighbor_order(0, 0..1, Some(false)).is_empty());
    // The folders before the first file are never a neighbor.
    assert_eq!(neighbor_order(3, 2..6, None), vec![4, 2, 5]);
    assert_eq!(neighbor_order(2, 2..4, Some(false)), vec![3]);
}

#[test]
fn step_forward_queues_next_first() {
    let (_temp, dir) = synthetic_dir();
    let paths: Vec<_> = (0..12)
        .map(|i| save_image(&dir, &format!("{i}.png"), 4, 4))
        .collect();

    let mut prefetcher = Prefetcher::new();
    prefetcher.kick(4, 0, &paths);
    assert_eq!(prefetcher.running_count(), 2, "5 and 3 run");

    // Only `poll` frees a job. The next kicks queue everything that does not run.
    prefetcher.kick(5, 0, &paths);
    let waiting: Vec<usize> = prefetcher.queue.iter().map(|job| job.0).collect();
    assert_eq!(
        waiting,
        vec![6, 7, 8, 4, 2],
        "forward: the next images first"
    );

    prefetcher.kick(4, 0, &paths);
    let waiting: Vec<usize> = prefetcher.queue.iter().map(|job| job.0).collect();
    assert_eq!(waiting, vec![2, 1, 6, 7], "back: the previous images first");

    // A redraw keeps the direction of the step.
    prefetcher.kick(4, 0, &paths);
    let waiting: Vec<usize> = prefetcher.queue.iter().map(|job| job.0).collect();
    assert_eq!(
        waiting,
        vec![2, 1, 6, 7],
        "the same position keeps the order"
    );
    assert!(prefetcher.has_jobs());
    wait_until_idle(&mut prefetcher);
    assert!(!prefetcher.has_jobs());
}

#[test]
fn opaque_unhinted_decode_is_rgb() {
    let (_temp, dir) = synthetic_dir();
    let path = dir.join("0.png");
    image::RgbImage::new(8, 4).save(&path).unwrap();
    let paths = vec![path];
    for rect in [Rect::default(), Rect::new(0, 0, 1, 1)] {
        let mut prefetcher = Prefetcher::new();
        prefetcher.kick_gallery(0, &paths);
        wait_until_idle(&mut prefetcher);
        let image = prefetcher.take_resized(0, rect, (4, 4)).unwrap();
        assert!(
            matches!(image, crate::decode::ScreenImage::Rgb(_)),
            "{rect:?}"
        );
    }
}

#[test]
fn stored_image_returns_for_box() {
    let mut prefetcher = Prefetcher::new();
    let rect = Rect::new(0, 0, 4, 2);
    prefetcher.store(3, RgbaImage::new(16, 8).into(), rect, (4, 4));
    assert!(
        prefetcher
            .take_resized(3, Rect::new(0, 0, 8, 4), (4, 4))
            .is_none(),
        "a larger box needs a new decode"
    );
    prefetcher.store(3, RgbaImage::new(16, 8).into(), rect, (4, 4));
    let image = prefetcher.take_resized(3, rect, (4, 4)).unwrap();
    assert_eq!(image.dimensions(), (16, 8));
    assert!(prefetcher.take_resized(3, rect, (4, 4)).is_none());
}

#[test]
fn two_jobs_run_rest_wait() {
    let (_temp, dir) = synthetic_dir();
    let paths: Vec<_> = (0..9)
        .map(|i| save_image(&dir, &format!("{i}.png"), 4, 4))
        .collect();

    let mut prefetcher = Prefetcher::new();
    prefetcher.kick(4, 0, &paths);
    assert_eq!(
        prefetcher.running_count(),
        2,
        "the two nearest neighbors decode"
    );
    assert_eq!(prefetcher.pending_count(), 6);
    let waiting: Vec<usize> = prefetcher.queue.iter().map(|job| job.0).collect();
    assert_eq!(waiting, vec![6, 2, 7, 1], "the others wait, nearest first");

    prefetcher.kick(4, 0, &paths);
    assert_eq!(prefetcher.pending_count(), 6);

    wait_until_idle(&mut prefetcher);
    for i in [1, 2, 3, 5, 6, 7] {
        assert!(
            prefetcher
                .take_resized(i, Rect::default(), (1, 1))
                .is_some(),
            "neighbor {i} should be cached"
        );
    }

    let mut prefetcher = Prefetcher::new();
    prefetcher.kick(4, 0, &paths);
    prefetcher.kick_gallery(8, &paths);
    let waiting: Vec<usize> = prefetcher.queue.iter().map(|job| job.0).collect();
    assert_eq!(waiting, vec![8]);
    assert_eq!(prefetcher.running_count(), 2);
    wait_until_idle(&mut prefetcher);
}

#[test]
fn smaller_box_decode_unused() {
    let (_temp, dir) = synthetic_dir();
    let paths = vec![
        save_image(&dir, "0.png", 64, 32),
        save_image(&dir, "1.png", 64, 32),
    ];

    let mut prefetcher = Prefetcher::new();
    prefetcher.set_target_hint(Rect::new(0, 0, 2, 2), (8, 8));
    prefetcher.kick_gallery(0, &paths);
    prefetcher.kick_gallery(1, &paths);
    wait_until_idle(&mut prefetcher);

    assert!(
        prefetcher
            .take_resized(0, Rect::new(0, 0, 4, 4), (8, 8))
            .is_none(),
        "a 16x8 decode must not fill a 32x32 box"
    );
    let img = prefetcher
        .take_resized(1, Rect::new(0, 0, 1, 1), (8, 8))
        .expect("a larger decode fits a smaller box");
    assert_eq!(img.dimensions(), (8, 4));
}
