//! The loop jobs and the loop clock of the grid, on files that hold no video.

use crate::app::{App, ViewMode};
use crate::config::Config;
use crate::image_list::SharedImageList;
use crate::test_support::{test_dir, wait_until};
use image::RgbaImage;
use ratatui::layout::Rect;
use std::path::Path;
use std::time::Duration;
use tuilibr::pictures::jobs::thumbnail_cache::GridPicture;
use tuilibr::pictures::resize::padding::pad_for_grid;

fn picture() -> GridPicture {
    RgbaImage::new(2, 2).into()
}

/// A grid app with the files `names` in `dir`, each with a sharp still. The video files hold
/// no video, and a decode fails.
fn grid_app(dir: &Path, names: &[&str], config: &Config) -> App {
    let mut app = App::new_with_config((8, 16), SharedImageList::new(), config);
    app.disk_cache_dir = dir.join("cache");
    for (index, name) in names.iter().enumerate() {
        let path = dir.join(name);
        std::fs::write(&path, b"no video").unwrap();
        app.images.push(path);
        app.filenames.push((*name).to_owned());
        let still = RgbaImage::new(160, 96);
        app.thumb_cache
            .insert(index, pad_for_grid(still, (160, 96), (8, 16), true));
    }
    app.gallery.filtered_indices = (0..names.len()).collect();
    app.gallery.zoom_rows = 4;
    app.gallery.update_grid(Rect::new(0, 0, 106, 29));
    app.scan_complete = true;
    app
}

fn finish_jobs(app: &mut App) {
    wait_until(|| {
        app.poll_grid_loops();
        !app.grid_loops_pending()
    });
}

#[cfg(feature = "video")]
#[test]
fn jobs_wait_for_stills() {
    let dir = test_dir();
    let mut app = grid_app(dir.path(), &["a.jpg", "clip.mp4"], &Config::default());
    let still = app.thumb_cache.peek(&1).unwrap().0.clone();
    app.thumb_cache.clear();
    app.thumb_cache.insert(1, still);
    app.spawn_thumb_decode(0, Rect::new(0, 0, 20, 6));
    assert!(app.thumbnails_pending());
    app.poll_grid_loops();
    assert!(
        !app.grid_loops_pending(),
        "the still of the photo comes first"
    );

    wait_until(|| {
        app.poll_thumbnails();
        !app.thumbnails_pending()
    });
    app.poll_grid_loops();
    assert!(app.grid_loops_pending(), "then the clip gets its job");
    finish_jobs(&mut app);
}

#[test]
fn no_ffmpeg_tries_once() {
    let dir = test_dir();
    let mut app = grid_app(dir.path(), &["a.jpg", "clip.mp4"], &Config::default());
    app.poll_grid_loops();
    assert_eq!(
        app.grid_loops_pending(),
        cfg!(feature = "video"),
        "a job for the clip, none for the photo"
    );
    assert_eq!(app.has_pending_work(), cfg!(feature = "video"));
    finish_jobs(&mut app);
    assert!(app.grid_loops.loops.is_empty());
    assert!(!app.loop_plays(1));
    app.poll_grid_loops();
    assert!(
        !app.grid_loops_pending(),
        "a failed cell does not try again"
    );
}

#[cfg(feature = "video")]
#[test]
fn warm_folder_reads_loops_from_cache() {
    let dir = test_dir();
    let mut app = grid_app(dir.path(), &["clip.mp4"], &Config::default());
    let target =
        tuilibr::pictures::resize::padding::thumb_target_px(app.gallery.thumb_box(), app.cell_px);
    let frames = tuilibr::pictures::motion::loop_frames::LoopFrames {
        frames: vec![RgbaImage::new(80, 48), RgbaImage::new(80, 48)],
        delays: vec![Duration::from_millis(333); 2],
        shown: (160, 96),
    };
    tuilibr::pictures::motion::loop_file::try_save_loop(
        &app.disk_cache_dir,
        &app.images[0],
        target,
        &frames,
    )
    .unwrap();

    app.poll_grid_loops();
    finish_jobs(&mut app);
    let cell = &app.grid_loops.loops[&0];
    assert_eq!(cell.frames.len(), 2);
    assert_eq!(cell.cells(), (20, 6), "the cells of the still");
    assert!(app.loops_to_send());
}

/// A grid app with the Live Photo still `IMG_0001.JPG` and its motion `IMG_0001.MOV`, which
/// holds no video.
#[cfg(feature = "video")]
fn live_grid_app(dir: &Path, config: &Config) -> (App, std::path::PathBuf) {
    let mut app = grid_app(dir, &["IMG_0001.JPG"], config);
    let motion = dir.join("IMG_0001.MOV");
    std::fs::write(&motion, b"no video").unwrap();
    app.set_live_photos(crate::scanner::LivePhotos::from_pairs([(
        app.images[0].clone(),
        motion.clone(),
    )]));
    (app, motion)
}

#[cfg(feature = "video")]
#[test]
fn live_still_loops_its_motion() {
    let dir = test_dir();
    let (mut app, motion) = live_grid_app(dir.path(), &Config::default());
    assert_eq!(app.loop_source(0), Some(motion.clone()));
    let target =
        tuilibr::pictures::resize::padding::thumb_target_px(app.gallery.thumb_box(), app.cell_px);
    let frames = tuilibr::pictures::motion::loop_frames::LoopFrames {
        frames: vec![RgbaImage::new(80, 48); 3],
        delays: vec![Duration::from_millis(333); 3],
        shown: (160, 96),
    };
    // The loop of the motion sits in the cache under the path of the motion.
    tuilibr::pictures::motion::loop_file::try_save_loop(
        &app.disk_cache_dir,
        &motion,
        target,
        &frames,
    )
    .unwrap();

    app.poll_grid_loops();
    assert!(app.grid_loops_pending(), "the still gets a loop job");
    finish_jobs(&mut app);

    assert_eq!(app.grid_loops.loops[&0].frames.len(), 3);
    assert!(app.loops_to_send());
}

#[cfg(feature = "video")]
#[test]
fn live_still_without_animate_keeps_still() {
    let dir = test_dir();
    let config = Config {
        animate: false,
        ..Config::default()
    };
    let (mut app, _motion) = live_grid_app(dir.path(), &config);
    app.poll_grid_loops();
    assert!(!app.grid_loops_pending());
    assert!(!app.grid_loops_shown());
}

#[cfg(feature = "video")]
#[test]
fn config_key_turns_loops_off() {
    let dir = test_dir();
    let config = Config {
        animate: false,
        ..Config::default()
    };
    let mut app = grid_app(dir.path(), &["clip.mp4"], &config);
    app.poll_grid_loops();
    assert!(!app.grid_loops_pending());
    assert!(!app.grid_loops_shown());
}

#[test]
fn poll_drops_hidden_or_old_results() {
    let dir = test_dir();
    let names: Vec<String> = (0..40).map(|index| format!("{index}.mp4")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut app = grid_app(dir.path(), &names, &Config::default());
    app.mode = ViewMode::Fullscreen;
    let send = |app: &App, generation: u32, index: usize| {
        app.grid_loops
            .tx
            .send(super::LoopResult {
                generation,
                index,
                outcome: super::LoopOutcome::Frames(
                    vec![picture(), picture()],
                    vec![Duration::ZERO; 2],
                ),
            })
            .unwrap();
    };
    send(&app, 0, 39);
    send(&app, 1, 0);
    send(&app, 0, 1);
    app.poll_grid_loops();
    assert_eq!(
        app.grid_loops.loops.keys().copied().collect::<Vec<_>>(),
        vec![1]
    );
}

#[test]
fn cancelled_result_leaves_cell_free() {
    let dir = test_dir();
    let mut app = grid_app(dir.path(), &["clip.mp4"], &Config::default());
    app.mode = ViewMode::Fullscreen;
    app.grid_loops
        .tx
        .send(super::LoopResult {
            generation: 0,
            index: 0,
            outcome: super::LoopOutcome::Cancelled,
        })
        .unwrap();
    app.poll_grid_loops();
    assert!(app.grid_loops.loops.is_empty());
    assert!(
        !app.grid_loops.failed.contains(&0),
        "the cell gets a new job later"
    );
}

/// A late result of the old generation must not take the cancel flag of the new job.
#[test]
fn old_result_keeps_new_cancel_flag() {
    let dir = test_dir();
    let mut app = grid_app(dir.path(), &["clip.mp4"], &Config::default());
    app.pretend_loop_job(0);
    let old_generation = app.grid_loops.generation;
    app.forget_grid_loops();
    app.pretend_loop_job(0);
    let flag = std::sync::Arc::clone(&app.grid_loops.running[&0]);
    app.grid_loops
        .tx
        .send(super::LoopResult {
            generation: old_generation,
            index: 0,
            outcome: super::LoopOutcome::Cancelled,
        })
        .unwrap();
    app.poll_grid_loops();
    assert!(app.grid_loops.running.contains_key(&0), "the new job runs");
    app.grid_loops.cancel_jobs();
    assert!(flag.load(std::sync::atomic::Ordering::Relaxed));
}

#[test]
fn loop_without_frames_fails() {
    let dir = test_dir();
    let mut app = grid_app(dir.path(), &["clip.mp4"], &Config::default());
    app.mode = ViewMode::Fullscreen;
    app.grid_loops
        .tx
        .send(super::LoopResult {
            generation: app.grid_loops.generation,
            index: 0,
            outcome: super::LoopOutcome::Frames(Vec::new(), Vec::new()),
        })
        .unwrap();
    app.poll_grid_loops();
    assert!(app.grid_loops.loops.is_empty());
    assert!(app.grid_loops.failed.contains(&0));
}

#[test]
fn at_most_four_jobs_run() {
    let dir = test_dir();
    let names = ["a.mp4", "b.mp4", "c.mp4", "d.mp4", "e.mp4"];
    let mut app = grid_app(dir.path(), &names, &Config::default());
    for index in 0..4 {
        app.pretend_loop_job(index);
    }
    app.poll_grid_loops();
    assert_eq!(app.grid_loops.running.len(), 4);
    assert!(
        !app.grid_loops.running.contains_key(&4),
        "the fifth cell waits"
    );
}

#[test]
fn stills_and_other_views_cancel_jobs() {
    let dir = test_dir();
    let mut app = grid_app(dir.path(), &["a.png", "clip.mp4"], &Config::default());
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    app.grid_loops
        .running
        .insert(1, std::sync::Arc::clone(&cancel));
    let flag = || cancel.load(std::sync::atomic::Ordering::Relaxed);
    app.poll_grid_loops();
    assert!(!flag(), "no still waits");

    app.mode = ViewMode::Fullscreen;
    app.poll_grid_loops();
    assert!(flag(), "the fullscreen view takes the cores");

    cancel.store(false, std::sync::atomic::Ordering::Relaxed);
    app.mode = ViewMode::Gallery;
    app.thumb_cache.clear();
    app.spawn_thumb_decode(0, Rect::new(0, 0, 20, 6));
    assert!(app.thumbnails_pending());
    app.poll_grid_loops();
    assert!(flag(), "a new still waits");
    app.grid_loops.running.clear();
    wait_until(|| {
        app.poll_thumbnails();
        !app.thumbnails_pending()
    });
}

/// The job with the fake ffmpeg of the `loop_frames` tests of tuilibr. Without a queued run,
/// ffmpeg gives no frame, and the decode waits until the cancel.
#[cfg(feature = "video")]
#[test]
fn quit_ends_running_ffmpeg() {
    use crate::test_support::video::{clip_info, fake_decoder};
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, mpsc};

    let dir = test_dir();
    let mut app = grid_app(dir.path(), &["clip.mp4"], &Config::default());
    let cancel = Arc::new(AtomicBool::new(false));
    let job_cancel = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    let job = std::thread::spawn(move || {
        let decoder = fake_decoder(clip_info());
        let result = tuilibr::pictures::motion::loop_frames::decode_loop(
            decoder.as_ref(),
            Path::new("ffmpeg"),
            Path::new("clip.mp4"),
            (160, 96),
            &[],
            &job_cancel,
        );
        tx.send((result.err().map(|error| error.kind()), decoder.streaming()))
            .unwrap();
    });
    app.grid_loops.running.insert(0, cancel);

    drop(app);

    let (error, streaming) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(error, Some(std::io::ErrorKind::Interrupted));
    assert!(!streaming, "the job killed its ffmpeg");
    job.join().unwrap();
}

#[cfg(feature = "video")]
#[test]
fn over_budget_retries_on_new_screen() {
    let dir = test_dir();
    let names: Vec<String> = (0..40).map(|index| format!("{index}.mp4")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut app = grid_app(dir.path(), &names, &Config::default());
    app.drop_hidden_loops();
    // Every job fails without ffmpeg, except the cell that waits above the budget.
    app.grid_loops.failed.extend(1..40);
    app.grid_loops.drop_over_budget(0);

    // A render without a scroll, after a cursor move or a meta result.
    app.drop_hidden_loops();
    app.poll_grid_loops();
    assert!(
        !app.grid_loops_pending(),
        "no job decodes the loop for nothing"
    );

    app.gallery.scroll_rows(1);
    app.drop_hidden_loops();
    app.gallery.scroll_rows(-1);
    app.drop_hidden_loops();
    app.poll_grid_loops();
    assert!(app.grid_loops_pending(), "another screen tries again");
    finish_jobs(&mut app);
}

#[test]
fn finished_job_goes_out_next_pass() {
    let dir = test_dir();
    let mut app = grid_app(dir.path(), &["clip.mp4"], &Config::default());
    app.pretend_loop_job(0);
    app.grid_loops
        .tx
        .send(super::LoopResult {
            generation: 0,
            index: 0,
            outcome: super::LoopOutcome::Frames(
                vec![picture(), picture()],
                vec![Duration::ZERO; 2],
            ),
        })
        .unwrap();
    assert!(!app.loops_to_send(), "the result waits for the poll");
    app.poll_grid_loops();
    assert!(!app.grid_loops_pending());
    assert!(app.loops_to_send(), "the next pass writes the frames");
    assert_eq!(crate::run_loop::next_timeout(&app, &[]), Duration::ZERO);
}

#[test]
fn same_screen_keeps_hidden_list() {
    let dir = test_dir();
    let names: Vec<String> = (0..40).map(|index| format!("{index}.mp4")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut app = grid_app(dir.path(), &names, &Config::default());
    app.drop_hidden_loops();
    let list = app.grid_loops.last_visible.as_ptr();
    let shown = app.grid_loops.last_visible.len();
    assert!(shown > 0);
    app.drop_hidden_loops();
    assert_eq!(app.grid_loops.last_visible.as_ptr(), list);
    app.gallery.scroll_rows(1);
    app.drop_hidden_loops();
    assert_ne!(
        app.grid_loops.last_visible.first(),
        Some(&0),
        "the first row left"
    );
    assert_eq!(app.grid_loops.last_visible.len(), shown);
    assert_eq!(
        app.grid_loops.last_visible.as_ptr(),
        list,
        "a change with the same count reuses the buffer"
    );
}

/// A range of 9 ids: the allocator wraps once, skips the live loop, the other loop and the
/// stale id, and finds the last free run. The allocator itself has its tests in tuilibr.
#[test]
fn wrapped_ids_skip_live_loops() {
    use tuilibr::pictures::kitty::Loops;
    use tuilibr::pictures::motion::loop_ids::LoopIds;
    let mut loops = super::GridLoops::new(true);
    let base = super::LOOP_ID_BASE;
    loops.ids = LoopIds::new(base..base + 9);
    let delays = [Duration::from_millis(100); 3];
    loops.insert(
        0,
        vec![picture(), picture(), picture()],
        &delays,
        Loops::Ticks,
    );
    let live = loops.loops[&0].ids.clone();
    loops.insert(1, vec![picture()], &delays[..1], Loops::Ticks);
    loops.stale_ids.push(live[live.len() - 1] + 2);
    let frames = vec![picture(), picture(), picture()];
    loops.insert(2, frames, &delays, Loops::Ticks);
    let fresh = &loops.loops[&2].ids;
    assert_eq!(fresh, &[base + 6, base + 7, base + 8]);
    for id in fresh {
        assert!(!live.contains(id), "id {id} belongs to a live loop");
        assert!(!loops.loops[&1].ids.contains(id));
        assert!(!loops.stale_ids.contains(id));
    }
}

/// The real motion of `IMG_1043.HEIC`, the first Live still of `PHEWR_TEST_LIVE_DIR`, decodes
/// into a loop, and the grid cell of the still plays it.
#[cfg(feature = "video")]
#[test]
#[ignore = "needs PHEWR_TEST_LIVE_DIR, ffmpeg and ffprobe"]
fn real_live_still_loops_its_motion() {
    use crate::test_support::live::{index_of, real_live_dir, scanned_app};
    use crate::test_support::video::{real_ffmpeg, real_ffprobe};
    use std::sync::atomic::AtomicBool;
    use tuilibr::pictures::motion::decoder::SystemDecoder;
    let (Some(dir), Some(ffmpeg), Some(ffprobe)) = (real_live_dir(), real_ffmpeg(), real_ffprobe())
    else {
        return;
    };
    let cache = test_dir();
    let mut app = scanned_app(&dir, &Config::default());
    app.disk_cache_dir = cache.path().to_path_buf();
    let still = index_of(&app, "IMG_1043.HEIC");
    let motion = app.loop_source(still).unwrap();
    assert_eq!(motion, dir.join("IMG_1043.MOV"));
    let target =
        tuilibr::pictures::resize::padding::thumb_target_px(app.gallery.thumb_box(), app.cell_px);

    let decoder = SystemDecoder::new(Some(ffprobe));
    let start = std::time::Instant::now();
    let frames = tuilibr::pictures::motion::loop_frames::decode_loop(
        &decoder,
        &ffmpeg,
        &motion,
        target,
        &[cache.path()],
        &AtomicBool::new(false),
    )
    .unwrap();
    println!(
        "IMG_1043.MOV: {} loop frames of {:?} for a target of {target:?} in {:?}",
        frames.frames.len(),
        frames.frames[0].dimensions(),
        start.elapsed()
    );
    tuilibr::pictures::motion::loop_file::try_save_loop(
        &app.disk_cache_dir,
        &motion,
        target,
        &frames,
    )
    .unwrap();
    app.thumb_cache.insert(
        still,
        pad_for_grid(
            RgbaImage::new(target.0, target.1),
            target,
            app.cell_px,
            true,
        ),
    );

    app.poll_grid_loops();
    finish_jobs(&mut app);
    assert_eq!(
        app.grid_loops.loops[&still].frames.len(),
        frames.frames.len()
    );
    assert!(frames.frames.len() >= 2);
}
