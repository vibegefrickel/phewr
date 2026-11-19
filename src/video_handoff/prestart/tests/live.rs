//! Tests of the motion start during a rest of the gallery cursor on a Live Photo cell.

use super::FRAME;
use crate::app::{App, ViewMode};
use crate::config::VideoPlayer;
use crate::run_loop::{LoopState, poll_background};
use crate::test_support::live::{LIVE_ID, grid_on_still, select};
use crate::test_support::video::{
    FakeDecoder, clip_info, fake_decoder, fake_runner, last_start, state_with,
};
use crate::test_support::{char_key, press, wait_until};
use crate::video_handoff::play_videos;
use crate::video_handoff::prestart::{
    PRESTART_DELAY, PRESTART_LIFETIME, prestart_timeout, take_prestart, tick_prestart,
};
use crossterm::event::KeyCode;
use ratatui::layout::Rect;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};
use tuilibr::pictures::motion::frame_box::frame_limit;

fn terminal(decoder: &Rc<FakeDecoder>) -> LoopState {
    state_with(VideoPlayer::Terminal, Rc::clone(decoder))
}

/// The grid with the cursor on the still `IMG_0001.JPG`, after the background work.
fn idle_on_still(dir: &Path) -> App {
    let mut app = grid_on_still(dir, LIVE_ID);
    wait_idle(&mut app);
    app
}

/// The background work ends. Only the rest target then holds a start.
fn wait_idle(app: &mut App) {
    let mut pending = Vec::new();
    wait_until(|| {
        poll_background(app, &mut pending);
        !app.has_pending_work()
    });
}

/// The rest begins at `start`, and the motion starts at the end of the delay.
fn rest_until_started(app: &mut App, decoder: &Rc<FakeDecoder>, start: Instant) -> Instant {
    let state = terminal(decoder);
    let before = decoder.starts().len();
    tick_prestart(app, &state, start);
    tick_prestart(app, &state, start + PRESTART_DELAY / 2);
    assert_eq!(decoder.starts().len(), before, "no start before the delay");
    let due = start + PRESTART_DELAY;
    tick_prestart(app, &state, due);
    assert!(app.video_prestart.running());
    due
}

#[test]
fn rest_on_live_cell_starts_motion() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = idle_on_still(dir.path());
    let decoder = fake_decoder(clip_info());
    let due = rest_until_started(&mut app, &decoder, Instant::now());

    assert_eq!(decoder.starts().len(), 1);
    let args = decoder.last_args();
    assert!(args.contains("IMG_0001.MOV"), "{args}");
    assert!(args.contains("scale_vt=w=640:h=480,"), "{args}");
    assert_eq!(app.mode, ViewMode::Gallery, "the grid stays");
    assert!(app.video.is_none());
    tick_prestart(&mut app, &terminal(&decoder), due + PRESTART_DELAY * 10);
    assert_eq!(decoder.starts().len(), 1, "at most one start per rest");
}

#[test]
fn enter_takes_motion_loop() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = idle_on_still(dir.path());
    let still = app.gallery.selected_index().unwrap();
    let decoder = fake_decoder(clip_info());
    rest_until_started(&mut app, &decoder, Instant::now());
    // ffmpeg decodes two frames during the rest.
    decoder.feed(vec![0; FRAME]);
    decoder.feed(vec![0; FRAME]);

    press(&mut app, KeyCode::Enter);
    let mut state = terminal(&decoder);
    let (log, mut runner) = fake_runner();
    play_videos(&mut app, &mut state, &mut runner);
    let enter = Instant::now();
    assert!(log.borrow().is_empty(), "no mpv");
    assert_eq!(decoder.starts().len(), 1, "Enter starts no second ffmpeg");
    assert!(!app.video_prestart.running());
    assert!(app.plays_motion());
    assert_eq!(app.current, still, "the cursor stays on the still");

    let player = app.video.as_mut().unwrap();
    let interval = clip_info().frame_interval();
    assert!(
        player.poll_at(enter),
        "the frame decoded ahead shows at once"
    );
    assert!(player.poll_at(enter + interval));
    assert_eq!(player.dropped, 0);
    decoder.end();
    assert!(!player.poll_at(enter + interval * 2));
    assert!(!player.finished(), "the prestarted motion loops");
    assert_eq!(decoder.starts().len(), 2, "the motion starts again");
    assert_eq!(app.mode, ViewMode::Video);
}

#[test]
fn cursor_move_drops_motion_player() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = idle_on_still(dir.path());
    let decoder = fake_decoder(clip_info());
    let due = rest_until_started(&mut app, &decoder, Instant::now());
    assert!(decoder.streaming());

    select(&mut app, "z.png");
    let state = terminal(&decoder);
    tick_prestart(&mut app, &state, due + Duration::from_millis(1));
    assert!(!app.video_prestart.running());
    assert!(!decoder.streaming(), "the drop ends the ffmpeg run");
    tick_prestart(&mut app, &state, due + PRESTART_DELAY * 2);
    assert_eq!(prestart_timeout(&app, due), None, "z.png plays nothing");
    assert_eq!(decoder.starts().len(), 1);
}

#[test]
fn overlays_drop_motion_player() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = idle_on_still(dir.path());
    let decoder = fake_decoder(clip_info());
    let state = terminal(&decoder);
    let due = rest_until_started(&mut app, &decoder, Instant::now());
    app.help_visible = true;
    tick_prestart(&mut app, &state, due);
    assert!(!app.video_prestart.running(), "the help popup");

    app.help_visible = false;
    let due = rest_until_started(&mut app, &decoder, due);
    char_key(&mut app, 's');
    tick_prestart(&mut app, &state, due);
    assert!(!app.video_prestart.running(), "the slideshow");
}

#[test]
fn long_wait_ends_motion_player() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = idle_on_still(dir.path());
    let decoder = fake_decoder(clip_info());
    let state = terminal(&decoder);
    let due = rest_until_started(&mut app, &decoder, Instant::now());
    tick_prestart(&mut app, &state, due + PRESTART_LIFETIME / 2);
    assert!(app.video_prestart.running());

    tick_prestart(&mut app, &state, due + PRESTART_LIFETIME);
    assert!(!app.video_prestart.running());
    assert!(!decoder.streaming(), "ffmpeg ends");

    // Enter after the end starts the motion again.
    press(&mut app, KeyCode::Enter);
    let mut state = terminal(&decoder);
    let (_log, mut runner) = fake_runner();
    play_videos(&mut app, &mut state, &mut runner);
    assert_eq!(decoder.starts().len(), 2);
    assert!(app.video.is_some());
    assert!(app.plays_motion());
}

#[test]
fn new_frame_box_restarts_motion_player() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = idle_on_still(dir.path());
    let decoder = fake_decoder(clip_info());
    let due = rest_until_started(&mut app, &decoder, Instant::now());

    // The info panel or a resize: the draw sets the new box.
    app.fullscreen_box = Rect::new(0, 0, 50, 20);
    let later = due + Duration::from_millis(1);
    tick_prestart(&mut app, &terminal(&decoder), later);
    assert_eq!(decoder.starts().len(), 2, "a start for the new box");
    let args = decoder.last_args();
    assert!(args.contains("IMG_0001.MOV"), "{args}");
    assert!(args.contains("scale_vt=w=400:h=300,"), "{args}");
    let limit = frame_limit(app.fullscreen_box, app.cell_px);
    let still = app.gallery.selected_index().unwrap();
    let still_path = app.images[still].clone();
    assert!(
        take_prestart(&mut app, &still_path, limit, later).is_none(),
        "the player plays the motion, not the still"
    );
    let motion = app.live_motion(still).unwrap().to_path_buf();
    assert!(take_prestart(&mut app, &motion, limit, later).is_some());
}

#[test]
fn mpv_window_drops_motion_player() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = idle_on_still(dir.path());
    let decoder = fake_decoder(clip_info());
    rest_until_started(&mut app, &decoder, Instant::now());

    char_key(&mut app, 'p');
    let mut state = terminal(&decoder);
    let (log, mut runner) = fake_runner();
    play_videos(&mut app, &mut state, &mut runner);
    assert!(last_start(&log).contains("IMG_0001.MOV"));
    assert_eq!(app.mode, ViewMode::Gallery, "the grid stays the grid");
    assert!(app.watches_video_window());
    assert!(!app.video_prestart.running());
    assert!(!decoder.streaming(), "the drop ends the ffmpeg run");
}

#[test]
fn cold_motion_probes_on_a_thread() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = idle_on_still(dir.path());
    let still = app.gallery.selected_index().unwrap();
    let motion = app.live_motion(still).unwrap().to_path_buf();
    let decoder = fake_decoder(clip_info());
    decoder.cold.set(true);
    let state = terminal(&decoder);
    let start = Instant::now();
    tick_prestart(&mut app, &state, start);
    tick_prestart(&mut app, &state, start + PRESTART_DELAY);
    assert_eq!(decoder.prepared(), [motion], "the probe runs on a thread");
    assert!(decoder.starts().is_empty());
    assert_eq!(
        prestart_timeout(&app, start + PRESTART_DELAY),
        Some(Duration::from_millis(10)),
        "the run loop polls for the end of the probe"
    );
    tick_prestart(&mut app, &state, start + PRESTART_DELAY * 2);
    assert_eq!(decoder.prepared().len(), 1, "one probe per rest");

    decoder.finish_prepare(true);
    tick_prestart(&mut app, &state, start + PRESTART_DELAY * 3);
    assert!(app.video_prestart.running());
    assert_eq!(decoder.starts().len(), 1);
    assert_eq!(decoder.cold_probes(), 0, "no ffprobe on the UI thread");
}

#[test]
fn failed_thread_probe_starts_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = idle_on_still(dir.path());
    let decoder = fake_decoder(clip_info());
    decoder.cold.set(true);
    let state = terminal(&decoder);
    let start = Instant::now();
    tick_prestart(&mut app, &state, start);
    tick_prestart(&mut app, &state, start + PRESTART_DELAY);
    decoder.finish_prepare(false);
    tick_prestart(&mut app, &state, start + PRESTART_DELAY * 2);
    assert!(!app.video_prestart.running());
    assert_eq!(prestart_timeout(&app, start), None, "no second try");
    tick_prestart(&mut app, &state, start + PRESTART_DELAY * 3);
    assert_eq!(decoder.prepared().len(), 1);
    assert!(decoder.starts().is_empty());
    assert_eq!(decoder.cold_probes(), 0);
}
