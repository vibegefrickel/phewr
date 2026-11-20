//! The loop jobs and the loop clock of the grid, on files that hold no video.

use crate::app::{App, ViewMode};
use crate::config::Config;
use crate::image_list::SharedImageList;
use crate::test_support::{test_dir, wait_until};
use image::RgbaImage;
use ratatui::layout::Rect;
use std::path::Path;
#[cfg(feature = "video")]
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tuilibr::pictures::jobs::thumbnail_cache::GridPicture;
use tuilibr::pictures::kitty::loops::player::LoopOutcome;
use tuilibr::pictures::resize::padding::pad_for_grid;

fn picture() -> GridPicture {
    RgbaImage::new(2, 2).into()
}

/// A grid app with the files `names` in `dir`, each with a sharp still, after its first grid
/// frame. The video files hold no video, and a decode fails.
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
    app.show_grid_loops(Instant::now());
    app
}

/// The frame count of the loop of `index`, one id per frame on the tick path.
fn loop_frames(app: &App, index: usize) -> usize {
    app.grid_loops.player.ids_of(&index).len()
}

/// Two frames of `picture()`.
#[cfg(feature = "video")]
fn two_frames() -> LoopOutcome {
    LoopOutcome::Frames(vec![picture(), picture()], vec![Duration::ZERO; 2])
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
    assert_eq!(loop_frames(&app, 1), 0);
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
    let ids = app.grid_loops.player.ids_of(&0).to_vec();
    assert_eq!(ids.len(), 2);
    assert!(app.loops_to_send());
    crate::kitty_output::play_grid_loops(&mut app, Instant::now(), &mut Vec::new()).unwrap();
    let written = crate::test_support::text(&app.graphics_out.take_bytes());
    assert!(
        written.contains(&format!("i={},p=1,c=20,r=6", ids[0])),
        "the cells of the still: {written}"
    );
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
    app.show_grid_loops(Instant::now());
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

    assert_eq!(loop_frames(&app, 0), 3);
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

/// A result of a job that the player stopped goes: the job of a hidden cell, or of a cell before
/// a forget.
#[cfg(feature = "video")]
#[test]
fn poll_drops_hidden_or_old_results() {
    let dir = test_dir();
    let names: Vec<String> = (0..40).map(|index| format!("{index}.mp4")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut app = grid_app(dir.path(), &names, &Config::default());
    app.mode = ViewMode::Fullscreen;
    let old = app.loop_ticket(0);
    app.forget_grid_loops();
    let tickets = [old, app.loop_ticket(1), app.loop_ticket(39)];
    app.show_grid_loops(Instant::now());
    for ticket in tickets {
        app.grid_loops.tx.send(ticket.finish(two_frames())).unwrap();
    }
    app.poll_grid_loops();
    assert_eq!(loop_frames(&app, 0), 0, "the forget stopped the job");
    assert_eq!(loop_frames(&app, 1), 2);
    assert_eq!(loop_frames(&app, 39), 0, "the hidden cell stopped its job");
}

#[cfg(feature = "video")]
#[test]
fn cancelled_result_leaves_cell_free() {
    let dir = test_dir();
    let mut app = grid_app(dir.path(), &["clip.mp4"], &Config::default());
    app.mode = ViewMode::Fullscreen;
    let ticket = app.loop_ticket(0);
    app.grid_loops
        .tx
        .send(ticket.finish(LoopOutcome::Cancelled))
        .unwrap();
    app.poll_grid_loops();
    assert_eq!(loop_frames(&app, 0), 0);
    assert!(!app.grid_loops_pending());
    app.mode = ViewMode::Gallery;
    app.poll_grid_loops();
    assert!(app.grid_loops_pending(), "the cell gets a new job later");
    finish_jobs(&mut app);
}

/// A loop needs 2 frames: a loop of 1 frame is a still.
#[test]
fn loop_of_one_frame_fails() {
    let dir = test_dir();
    let mut app = grid_app(dir.path(), &["clip.mp4"], &Config::default());
    app.mode = ViewMode::Fullscreen;
    let ticket = app.loop_ticket(0);
    let one_frame = LoopOutcome::Frames(vec![picture()], vec![Duration::ZERO]);
    app.grid_loops.tx.send(ticket.finish(one_frame)).unwrap();
    app.poll_grid_loops();
    assert_eq!(loop_frames(&app, 0), 0);
    app.mode = ViewMode::Gallery;
    app.poll_grid_loops();
    assert!(!app.grid_loops_pending(), "the failed cell gets no job");
}

#[cfg(feature = "video")]
#[test]
fn at_most_four_jobs_run() {
    let dir = test_dir();
    let names = ["a.mp4", "b.mp4", "c.mp4", "d.mp4", "e.mp4"];
    let mut app = grid_app(dir.path(), &names, &Config::default());
    let mut started = Vec::new();
    app.grid_loops.player.start_jobs(|ticket| {
        started.push(ticket.key);
        true
    });
    assert_eq!(started, vec![0, 1, 2, 3], "the fifth cell waits");
}

#[cfg(feature = "video")]
#[test]
fn stills_and_other_views_cancel_jobs() {
    let dir = test_dir();
    let mut app = grid_app(dir.path(), &["a.png", "clip.mp4"], &Config::default());
    let ticket = app.loop_ticket(1);
    let flag = || ticket.cancel().load(Ordering::Relaxed);
    app.poll_grid_loops();
    assert!(!flag(), "no still waits");

    app.mode = ViewMode::Fullscreen;
    app.poll_grid_loops();
    assert!(flag(), "the fullscreen view takes the cores");

    ticket.cancel().store(false, Ordering::Relaxed);
    app.mode = ViewMode::Gallery;
    app.thumb_cache.clear();
    app.spawn_thumb_decode(0, Rect::new(0, 0, 20, 6));
    assert!(app.thumbnails_pending());
    app.poll_grid_loops();
    assert!(flag(), "a new still waits");
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
    use std::sync::mpsc;

    let dir = test_dir();
    let mut app = grid_app(dir.path(), &["clip.mp4"], &Config::default());
    let ticket = app.loop_ticket(0);
    let (tx, rx) = mpsc::channel();
    let job = std::thread::spawn(move || {
        let decoder = fake_decoder(clip_info());
        let result = tuilibr::pictures::motion::loop_frames::decode_loop(
            decoder.as_ref(),
            Path::new("ffmpeg"),
            Path::new("clip.mp4"),
            (160, 96),
            &[],
            ticket.cancel(),
        );
        tx.send((result.err().map(|error| error.kind()), decoder.streaming()))
            .unwrap();
    });

    drop(app);

    let (error, streaming) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(error, Some(std::io::ErrorKind::Interrupted));
    assert!(!streaming, "the job killed its ffmpeg");
    job.join().unwrap();
}

#[cfg(feature = "video")]
#[test]
fn finished_job_goes_out_next_pass() {
    let dir = test_dir();
    let mut app = grid_app(dir.path(), &["clip.mp4"], &Config::default());
    let ticket = app.loop_ticket(0);
    app.grid_loops.tx.send(ticket.finish(two_frames())).unwrap();
    assert!(!app.loops_to_send(), "the result waits for the poll");
    app.poll_grid_loops();
    assert!(!app.grid_loops_pending());
    assert!(app.loops_to_send(), "the next pass writes the frames");
    assert_eq!(crate::run_loop::next_timeout(&app, &[]), Duration::ZERO);
}

/// Each grid frame stops the job of a cell that left the screen.
#[cfg(feature = "video")]
#[test]
fn hidden_cell_cancels_its_job() {
    let dir = test_dir();
    let names: Vec<String> = (0..40).map(|index| format!("{index}.mp4")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut app = grid_app(dir.path(), &names, &Config::default());
    let ticket = app.loop_ticket(0);
    app.show_grid_loops(Instant::now());
    assert!(!ticket.cancel().load(Ordering::Relaxed), "the cell shows");

    app.gallery.scroll_rows(1);
    app.show_grid_loops(Instant::now());
    assert!(ticket.cancel().load(Ordering::Relaxed), "the cell left");
}

/// A cell below an overlay keeps its still and starts no job.
#[cfg(feature = "video")]
#[test]
fn overlay_starts_no_job() {
    let dir = test_dir();
    let mut app = grid_app(dir.path(), &["clip.mp4"], &Config::default());
    app.help_visible = true;
    app.show_grid_loops(Instant::now());
    app.poll_grid_loops();
    assert!(!app.grid_loops_pending(), "the help popup covers the cell");

    app.help_visible = false;
    app.show_grid_loops(Instant::now());
    app.poll_grid_loops();
    assert!(app.grid_loops_pending(), "the grid shows the cell again");
    finish_jobs(&mut app);
}

/// A poll between a folder switch and the first grid frame of the new folder starts no job on
/// the cells of the old folder, so a clip at the same index in the new folder loops.
#[cfg(feature = "video")]
#[test]
fn folder_switch_keeps_new_cells_free() {
    let dir = test_dir();
    let mut app = grid_app(dir.path(), &["old.mp4"], &Config::default());
    let new_dir = test_dir();
    app.switch_to_dir(new_dir.path());
    app.poll_grid_loops();
    assert!(
        !app.grid_loops_pending(),
        "the new folder lists no cell yet"
    );

    let path = new_dir.path().join("new.mp4");
    std::fs::write(&path, b"no video").unwrap();
    app.images.push(path);
    app.filenames.push("new.mp4".to_owned());
    let still = RgbaImage::new(160, 96);
    app.thumb_cache
        .insert(0, pad_for_grid(still, (160, 96), (8, 16), true));
    app.gallery.filtered_indices = vec![0];
    app.gallery.update_grid(Rect::new(0, 0, 106, 29));
    app.scan_complete = true;
    app.poll_grid_loops();
    assert!(app.grid_loops_pending(), "the clip of the new folder loops");
    finish_jobs(&mut app);
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
    let start = Instant::now();
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

    app.show_grid_loops(Instant::now());
    app.poll_grid_loops();
    finish_jobs(&mut app);
    assert_eq!(loop_frames(&app, still), frames.frames.len());
    assert!(frames.frames.len() >= 2);
}

#[test]
fn panicked_loop_job_fails_and_keeps_its_text() {
    let app = grid_app(test_dir().path(), &["clip.mp4"], &Config::default());
    assert_eq!(app.last_loop_panic(), None);
    let outcome =
        super::jobs::run_noting_panic(&app.grid_loops.last_panic, || panic!("decoder {}", "broke"));
    assert!(matches!(outcome, LoopOutcome::Failed));
    assert_eq!(app.last_loop_panic().as_deref(), Some("decoder broke"));
}
