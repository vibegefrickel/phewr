//! Tests of the player start during a rest of the gallery cursor.

use super::{PRESTART_DELAY, PRESTART_LIFETIME, prestart_timeout, take_prestart, tick_prestart};
use crate::app::{App, ViewMode};
use crate::config::VideoPlayer;
use crate::test_support::video::{
    FakeDecoder, clip_info, fake_decoder, state_with, with_temp_file_medium,
};
use crate::test_support::{app_with, press};
use crate::video_handoff::play_videos;
use crossterm::event::KeyCode;
use ratatui::layout::Rect;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// 640x480 RGB.
const FRAME: usize = 640 * 480 * 3;

/// A gallery of a photo and a video with the cursor on the video, after the scan.
fn gallery_on_video() -> App {
    let mut app = with_temp_file_medium(app_with(&["/videos/a.jpg", "/videos/clip.mp4"]));
    app.scan_complete = true;
    app.gallery.cursor = 1;
    app.fullscreen_box = Rect::new(0, 0, 100, 40);
    app
}

/// The rest begins at `start`, and the player starts at the end of the delay.
fn rest_until_started(app: &mut App, decoder: &Rc<FakeDecoder>, start: Instant) -> Instant {
    let state = state_with(VideoPlayer::Terminal, Rc::clone(decoder));
    tick_prestart(app, &state, start);
    let due = start + PRESTART_DELAY;
    tick_prestart(app, &state, due);
    assert!(app.video_prestart.running());
    due
}

/// Not for a photo and not after the start.
#[test]
fn run_loop_wakes_after_delay() {
    let decoder = fake_decoder(clip_info());
    let state = state_with(VideoPlayer::Terminal, Rc::clone(&decoder));
    let mut app = gallery_on_video();
    let start = Instant::now();
    assert_eq!(prestart_timeout(&app, start), None, "no rest yet");
    tick_prestart(&mut app, &state, start);
    assert_eq!(
        prestart_timeout(&app, start + PRESTART_DELAY / 2),
        Some(PRESTART_DELAY / 2)
    );
    assert_eq!(
        prestart_timeout(&app, start + PRESTART_DELAY * 3),
        Some(Duration::from_millis(10)),
        "a held start waits with the busy poll"
    );
    tick_prestart(&mut app, &state, start + PRESTART_DELAY);
    assert_eq!(
        prestart_timeout(&app, start + PRESTART_DELAY),
        None,
        "the start ran"
    );

    app.gallery.cursor = 0;
    tick_prestart(&mut app, &state, start);
    assert_eq!(prestart_timeout(&app, start), None, "a photo needs no wake");
}

#[test]
fn rest_starts_player_after_delay() {
    let decoder = fake_decoder(clip_info());
    let state = state_with(VideoPlayer::Terminal, Rc::clone(&decoder));
    let mut app = gallery_on_video();
    let start = Instant::now();

    tick_prestart(&mut app, &state, start);
    tick_prestart(&mut app, &state, start + PRESTART_DELAY / 2);
    assert!(decoder.starts().is_empty(), "no start before the delay");

    tick_prestart(&mut app, &state, start + PRESTART_DELAY);
    assert_eq!(decoder.starts().len(), 1);
    assert!(decoder.last_args().contains("scale_vt=w=640:h=480,"));
    tick_prestart(&mut app, &state, start + PRESTART_DELAY * 10);
    assert_eq!(decoder.starts().len(), 1, "at most one start per rest");
    assert_eq!(app.mode, ViewMode::Gallery, "the view stays");
    assert!(app.video.is_none());
}

#[test]
fn photo_or_no_terminal_starts_nothing() {
    let decoder = fake_decoder(clip_info());
    let mut app = gallery_on_video();
    app.gallery.cursor = 0;
    let start = Instant::now();
    let state = state_with(VideoPlayer::Terminal, Rc::clone(&decoder));
    tick_prestart(&mut app, &state, start);
    tick_prestart(&mut app, &state, start + PRESTART_DELAY);
    assert!(decoder.starts().is_empty(), "a photo plays nothing");

    app.gallery.cursor = 1;
    let window = state_with(VideoPlayer::Window, Rc::clone(&decoder));
    tick_prestart(&mut app, &window, start);
    tick_prestart(&mut app, &window, start + PRESTART_DELAY);
    assert!(
        decoder.starts().is_empty(),
        "the mpv window starts on Enter"
    );
}

#[test]
fn cursor_move_drops_player() {
    let decoder = fake_decoder(clip_info());
    let mut app = gallery_on_video();
    let due = rest_until_started(&mut app, &decoder, Instant::now());
    assert!(decoder.streaming());

    app.gallery.cursor = 0;
    let state = state_with(VideoPlayer::Terminal, Rc::clone(&decoder));
    tick_prestart(&mut app, &state, due + Duration::from_millis(1));
    assert!(!app.video_prestart.running());
    assert!(!decoder.streaming(), "the drop ends the ffmpeg run");

    // Back on the video, the rest begins again.
    app.gallery.cursor = 1;
    let back = due + Duration::from_millis(2);
    tick_prestart(&mut app, &state, back);
    tick_prestart(&mut app, &state, back + PRESTART_DELAY);
    assert_eq!(decoder.starts().len(), 2);
}

#[test]
fn overlays_drop_player() {
    let decoder = fake_decoder(clip_info());
    let state = state_with(VideoPlayer::Terminal, Rc::clone(&decoder));
    let mut app = gallery_on_video();
    let due = rest_until_started(&mut app, &decoder, Instant::now());
    app.help_visible = true;
    tick_prestart(&mut app, &state, due);
    assert!(!app.video_prestart.running(), "the help popup");
    tick_prestart(&mut app, &state, due + PRESTART_DELAY);
    assert_eq!(decoder.starts().len(), 1, "no start under the help popup");

    app.help_visible = false;
    let due = rest_until_started(&mut app, &decoder, due);
    app.start_slideshow();
    tick_prestart(&mut app, &state, due);
    assert!(!app.video_prestart.running(), "the slideshow");

    let mut app = gallery_on_video();
    rest_until_started(&mut app, &decoder, due);
    press(&mut app, KeyCode::Char('p'));
    let (_log, mut runner) = crate::test_support::video::fake_runner();
    let mut state = state_with(VideoPlayer::Terminal, Rc::clone(&decoder));
    play_videos(&mut app, &mut state, &mut runner);
    assert!(!app.video_prestart.running(), "the mpv window");
    assert!(app.video.is_none());
}

#[test]
fn grid_work_holds_start() {
    let decoder = fake_decoder(clip_info());
    let state = state_with(VideoPlayer::Terminal, Rc::clone(&decoder));
    let mut app = gallery_on_video();
    app.scan_complete = false;
    let start = Instant::now();
    tick_prestart(&mut app, &state, start);
    tick_prestart(&mut app, &state, start + PRESTART_DELAY * 4);
    assert!(decoder.starts().is_empty(), "the scan runs");

    app.scan_complete = true;
    tick_prestart(&mut app, &state, start + PRESTART_DELAY * 5);
    assert_eq!(decoder.starts().len(), 1, "the idle grid starts it");
}

#[test]
fn loop_job_holds_start() {
    use crate::app::thumbnails::pad_for_grid;
    use crate::config::Config;
    use crate::image_list::SharedImageList;
    use crate::test_support::wait_until;

    let decoder = fake_decoder(clip_info());
    let state = state_with(VideoPlayer::Terminal, Rc::clone(&decoder));
    let dir = tempfile::tempdir().unwrap();
    let mut app = with_temp_file_medium(App::new_with_config(
        (8, 16),
        SharedImageList::new(),
        &Config::default(),
    ));
    app.disk_cache_dir = dir.path().join("cache");
    for (index, name) in ["a.jpg", "clip.mp4"].iter().enumerate() {
        let path = dir.path().join(name);
        std::fs::write(&path, b"no video").unwrap();
        app.images.push(path);
        app.filenames.push((*name).to_owned());
        let still = image::RgbaImage::new(160, 96);
        app.thumb_cache
            .insert(index, pad_for_grid(still, (160, 96), (8, 16), true));
    }
    app.gallery.filtered_indices = vec![0, 1];
    app.gallery.update_grid(Rect::new(0, 0, 106, 29));
    app.scan_complete = true;
    app.gallery.cursor = 1;
    app.fullscreen_box = Rect::new(0, 0, 100, 40);
    app.poll_grid_loops();
    assert!(app.grid_loops_pending(), "the clip gets its loop job");
    assert!(!app.has_pending_work_but_loops(), "only the loop job runs");

    let start = Instant::now();
    tick_prestart(&mut app, &state, start);
    tick_prestart(&mut app, &state, start + PRESTART_DELAY * 4);
    assert!(decoder.starts().is_empty(), "the loop job runs");

    wait_until(|| {
        app.poll_grid_loops();
        !app.grid_loops_pending()
    });
    tick_prestart(&mut app, &state, start + PRESTART_DELAY * 5);
    assert_eq!(decoder.starts().len(), 1, "the idle grid starts it");
}

#[test]
fn new_frame_box_restarts_player() {
    let decoder = fake_decoder(clip_info());
    let state = state_with(VideoPlayer::Terminal, Rc::clone(&decoder));
    let mut app = gallery_on_video();
    let due = rest_until_started(&mut app, &decoder, Instant::now());
    assert!(decoder.last_args().contains("scale_vt=w=640:h=480,"));

    // A resize or the info panel: the draw sets the new box.
    app.fullscreen_box = Rect::new(0, 0, 50, 20);
    let later = due + Duration::from_millis(1);
    tick_prestart(&mut app, &state, later);
    assert_eq!(decoder.starts().len(), 2, "a start for the new box");
    assert!(decoder.last_args().contains("scale_vt=w=400:h=300,"));
    let limit = crate::player::frame_limit(app.fullscreen_box, app.cell_px);
    let path = app.images[1].clone();
    assert!(take_prestart(&mut app, &path, limit, later).is_some());
}

#[test]
fn long_wait_ends_player() {
    let decoder = fake_decoder(clip_info());
    let state = state_with(VideoPlayer::Terminal, Rc::clone(&decoder));
    let mut app = gallery_on_video();
    let due = rest_until_started(&mut app, &decoder, Instant::now());
    tick_prestart(&mut app, &state, due + PRESTART_LIFETIME / 2);
    assert!(app.video_prestart.running());

    tick_prestart(&mut app, &state, due + PRESTART_LIFETIME);
    assert!(!app.video_prestart.running());
    assert!(!decoder.streaming(), "ffmpeg ends");
    tick_prestart(&mut app, &state, due + PRESTART_LIFETIME * 2);
    assert_eq!(
        decoder.starts().len(),
        1,
        "no new start during the same rest"
    );
}

#[test]
fn failed_probe_is_not_retried() {
    let decoder = fake_decoder(clip_info());
    decoder.fail_probe.set(true);
    let state = state_with(VideoPlayer::Terminal, Rc::clone(&decoder));
    let mut app = gallery_on_video();
    let start = Instant::now();
    tick_prestart(&mut app, &state, start);
    tick_prestart(&mut app, &state, start + PRESTART_DELAY);
    assert!(!app.video_prestart.running());
    decoder.fail_probe.set(false);
    tick_prestart(&mut app, &state, start + PRESTART_DELAY * 2);
    assert!(decoder.starts().is_empty(), "one try per rest");
}

#[test]
fn enter_takes_player() {
    let decoder = fake_decoder(clip_info());
    let mut app = gallery_on_video();
    let rest = Instant::now();
    rest_until_started(&mut app, &decoder, rest);
    // ffmpeg decodes two frames during the rest. The rest lasts far longer than the first-frame
    // deadline.
    decoder.feed(vec![0; FRAME]);
    decoder.feed(vec![0; FRAME]);

    press(&mut app, KeyCode::Enter);
    let (_log, mut runner) = crate::test_support::video::fake_runner();
    let mut state = state_with(VideoPlayer::Terminal, Rc::clone(&decoder));
    play_videos(&mut app, &mut state, &mut runner);
    let enter = Instant::now();
    assert_eq!(decoder.starts().len(), 1, "Enter starts no second ffmpeg");
    assert!(!app.video_prestart.running());

    let player = app.video.as_mut().unwrap();
    let interval = clip_info().frame_interval();
    assert!(
        player.poll_at(enter),
        "the frame decoded ahead shows at once"
    );
    assert!(!player.poll_at(enter + interval / 2));
    assert!(
        player.poll_at(enter + interval),
        "the second frame is due one interval after Enter"
    );
    assert_eq!(player.dropped, 0);
    let two_frames = 2.0 * interval.as_secs_f64();
    assert!(
        (player.position() - two_frames).abs() < 1e-6,
        "the clip starts at 0"
    );
}

#[test]
fn deadline_counts_from_enter() {
    let decoder = fake_decoder(clip_info());
    let mut app = gallery_on_video();
    let rest = Instant::now();
    rest_until_started(&mut app, &decoder, rest);
    let path = app.images[1].clone();
    let limit = crate::player::frame_limit(app.fullscreen_box, app.cell_px);
    let mut player = take_prestart(&mut app, &path, limit, rest + Duration::from_secs(9)).unwrap();
    // 9 s after the start of ffmpeg, past the first-frame deadline, but 1 s after Enter: the
    // run still waits.
    assert!(!player.poll_at(rest + Duration::from_secs(10)));
    assert!(player.playing());
    assert_eq!(decoder.starts().len(), 1, "no software fallback yet");
}

#[test]
fn enter_restarts_stale_player() {
    let decoder = fake_decoder(clip_info());
    let mut app = gallery_on_video();
    let due = rest_until_started(&mut app, &decoder, Instant::now());
    let path = app.images[1].clone();
    let other = crate::player::frame_limit(Rect::new(0, 0, 50, 20), app.cell_px);
    assert!(take_prestart(&mut app, &path, other, due).is_none());
    assert!(
        !app.video_prestart.running(),
        "a resize in the pass of Enter drops the player"
    );

    let mut app = gallery_on_video();
    rest_until_started(&mut app, &decoder, Instant::now());
    let limit = crate::player::frame_limit(app.fullscreen_box, app.cell_px);
    let photo = app.images[0].clone();
    assert!(take_prestart(&mut app, &photo, limit, due).is_none());
    assert!(app.video_prestart.running(), "another file keeps the rest");
}
