//! Tests of the motion start during a rest of the fullscreen view on a Live Photo still.

use super::FRAME;
use crate::app::{App, ViewMode};
use crate::config::VideoPlayer;
use crate::player::frame_limit;
use crate::run_loop::{LoopState, poll_background};
use crate::test_support::live::{LIVE_ID, fullscreen_on_still};
use crate::test_support::video::{
    FakeDecoder, clip_info, fake_decoder, fake_runner, fullscreen_video_app, last_start, state_with,
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

fn terminal(decoder: &Rc<FakeDecoder>) -> LoopState {
    state_with(VideoPlayer::Terminal, Rc::clone(decoder))
}

/// The fullscreen view of the still `IMG_0001.JPG`, after the decode of the still and the
/// prefetch of its neighbors.
fn idle_on_still(dir: &Path) -> App {
    let mut app = fullscreen_on_still(dir, LIVE_ID);
    wait_idle(&mut app);
    app
}

/// The decode of the shown file and the prefetch end. Only the rest target then holds a start.
fn wait_idle(app: &mut App) {
    let mut pending = Vec::new();
    wait_until(|| {
        app.load_if_needed();
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
fn rest_on_still_starts_motion_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = idle_on_still(dir.path());
    let decoder = fake_decoder(clip_info());
    let due = rest_until_started(&mut app, &decoder, Instant::now());

    assert_eq!(decoder.starts().len(), 1);
    let args = decoder.last_args();
    assert!(args.contains("IMG_0001.MOV"), "{args}");
    assert!(args.contains("scale_vt=w=640:h=480,"), "{args}");
    assert_eq!(app.mode, ViewMode::Fullscreen, "the still stays");
    assert!(app.video.is_none());
    tick_prestart(&mut app, &terminal(&decoder), due + PRESTART_DELAY * 10);
    assert_eq!(decoder.starts().len(), 1, "at most one start per rest");
}

#[test]
fn enter_takes_motion_player() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = idle_on_still(dir.path());
    let still = app.current;
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
    assert!(player.finished(), "the motion plays one time");

    play_videos(&mut app, &mut state, &mut runner);
    assert_eq!(decoder.starts().len(), 1, "no second run of the motion");
    assert_eq!(app.mode, ViewMode::Fullscreen);
    assert_eq!(app.current, still, "back on the still");
}

#[test]
fn step_drops_motion_player() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = idle_on_still(dir.path());
    let still = app.current;
    let decoder = fake_decoder(clip_info());
    let due = rest_until_started(&mut app, &decoder, Instant::now());
    assert!(decoder.streaming());

    char_key(&mut app, 'l');
    assert_eq!(app.current, still + 1);
    let state = terminal(&decoder);
    tick_prestart(&mut app, &state, due + Duration::from_millis(1));
    assert!(!app.video_prestart.running());
    assert!(!decoder.streaming(), "the drop ends the ffmpeg run");
    wait_idle(&mut app);
    tick_prestart(&mut app, &state, due + PRESTART_DELAY * 2);
    assert!(!app.video_prestart.rests(), "z.png has no motion");
    assert_eq!(prestart_timeout(&app, due), None);
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
    app.start_slideshow();
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
fn plain_still_and_placeholder_start_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = idle_on_still(dir.path());
    char_key(&mut app, 'l');
    assert!(!app.fullscreen_on_live(), "z.png");
    let decoder = fake_decoder(clip_info());
    let state = terminal(&decoder);
    wait_idle(&mut app);
    let start = Instant::now();
    tick_prestart(&mut app, &state, start);
    tick_prestart(&mut app, &state, start + PRESTART_DELAY * 2);
    assert!(!app.video_prestart.rests(), "a plain still plays nothing");
    assert!(decoder.starts().is_empty());

    let mut app = fullscreen_video_app();
    app.scan_complete = true;
    app.return_from_video();
    assert!(app.shows_video_placeholder());
    tick_prestart(&mut app, &state, start);
    tick_prestart(&mut app, &state, start + PRESTART_DELAY * 2);
    assert!(
        !app.video_prestart.rests(),
        "the placeholder starts on Enter"
    );
    assert!(decoder.starts().is_empty());
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
    let motion = app.live_motion(app.current).unwrap().to_path_buf();
    assert!(
        take_prestart(&mut app, &motion, true, limit, later).is_none(),
        "a loop"
    );
    assert!(take_prestart(&mut app, &motion, false, limit, later).is_some());
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
    assert!(app.watches_video_window());
    assert!(!app.video_prestart.running());
    assert!(!decoder.streaming(), "the drop ends the ffmpeg run");
}

#[test]
fn cold_motion_probes_on_a_thread() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = idle_on_still(dir.path());
    let motion = app.live_motion(app.current).unwrap().to_path_buf();
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

/// Back on the still after the motion, the rest starts nothing until a step to another file.
#[test]
fn played_motion_starts_no_second_player() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = idle_on_still(dir.path());
    let decoder = fake_decoder(clip_info());
    let mut state = terminal(&decoder);
    let (_log, mut runner) = fake_runner();
    rest_until_started(&mut app, &decoder, Instant::now());
    decoder.feed(vec![0; FRAME]);
    press(&mut app, KeyCode::Enter);
    play_videos(&mut app, &mut state, &mut runner);
    let player = app.video.as_mut().unwrap();
    assert!(player.poll());
    decoder.end();
    assert!(!player.poll());
    play_videos(&mut app, &mut state, &mut runner);
    assert_eq!(app.mode, ViewMode::Fullscreen, "the motion ended");
    rest_without_start(&mut app, &state, "after the end");

    press(&mut app, KeyCode::Enter);
    play_videos(&mut app, &mut state, &mut runner);
    assert_eq!(decoder.starts().len(), 2, "Enter plays it again");
    assert!(!char_key(&mut app, 'q'));
    assert_eq!(app.mode, ViewMode::Fullscreen);
    rest_without_start(&mut app, &state, "after q");

    char_key(&mut app, 'l');
    tick_prestart(&mut app, &state, Instant::now());
    char_key(&mut app, 'h');
    wait_idle(&mut app);
    rest_until_started(&mut app, &decoder, Instant::now());
    assert_eq!(decoder.starts().len(), 3, "a step back rests again");
}

/// A long rest on the still in the fullscreen view starts no player.
fn rest_without_start(app: &mut App, state: &LoopState, case: &str) {
    wait_idle(app);
    let start = Instant::now();
    let before = app.video_prestart.running();
    tick_prestart(app, state, start);
    tick_prestart(app, state, start + PRESTART_DELAY * 2);
    assert!(!before && !app.video_prestart.running(), "{case}");
    assert!(!app.video_prestart.rests(), "{case}");
}
