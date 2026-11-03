//! Tests of the start of a video in the terminal player or the mpv window.

use super::{MPV_MISSING_NOTE, play_videos};
use crate::app::ViewMode;
use crate::config::VideoPlayer;
use crate::encoder::{KittyBackend, Transfer};
use crate::keys::fullscreen::handle_fullscreen_key;
use crate::mpv;
use crate::player::SystemDecoder;
use crate::test_support::video::{
    clip_info, fake_decoder, fake_runner, fullscreen_video_app, gallery_app_on_video, last_start,
    slideshow_video_app, starts, state_with, terminal_state, video_app, window_state,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[test]
fn terminal_player_starts_ffmpeg() {
    let decoder = fake_decoder(clip_info());
    let mut app = fullscreen_video_app();
    app.fullscreen_box = ratatui::layout::Rect::new(0, 0, 100, 40);
    let mut state = state_with(VideoPlayer::Terminal, std::rc::Rc::clone(&decoder));
    let (log, mut runner) = fake_runner();

    play_videos(&mut app, &mut state, &mut runner);

    assert!(app.video.is_some());
    assert_eq!(app.mode, ViewMode::Video);
    assert!(log.borrow().is_empty(), "no mpv");
    let args = decoder.last_args();
    assert!(args.starts_with("/bin/ffmpeg -hide_banner"), "{args}");
    // 640x480 into 100x40 cells of 8x16 pixels: 800x640 pixels. The frame stays 640x480.
    assert!(args.contains("scale_vt=w=640:h=480,"), "{args}");
    assert_eq!(state.status, None);

    play_videos(&mut app, &mut state, &mut runner);
    assert_eq!(decoder.starts().len(), 1, "a running player starts nothing");
}

#[test]
fn slideshow_plays_video_once() {
    let decoder = fake_decoder(clip_info());
    let mut app = slideshow_video_app();
    let mut state = state_with(VideoPlayer::Terminal, std::rc::Rc::clone(&decoder));
    let (_log, mut runner) = fake_runner();
    play_videos(&mut app, &mut state, &mut runner);

    decoder.feed(vec![0; 640 * 480 * 4]);
    assert!(app.video.as_mut().unwrap().poll());
    decoder.end();
    assert!(!app.video.as_mut().unwrap().poll());
    assert!(app.video.as_ref().unwrap().finished());
    assert_eq!(decoder.starts().len(), 1, "no repeat in a slideshow");

    app.slideshow_tick();
    assert_eq!(app.current, 2, "the end of the clip moves on");
    assert_eq!(app.mode, ViewMode::Fullscreen);
    assert!(app.video.is_none());
    assert!(app.slideshow_running());
    let interval = app.slideshow.as_ref().unwrap().interval;
    assert!(
        app.slideshow_timeout().unwrap() > interval / 2,
        "the next slide gets the full interval"
    );
}

#[test]
fn slideshow_holds_animation() {
    use crate::test_support::video::{animation_info, with_temp_file_medium};
    use crate::test_support::{app_with, fixture};
    let decoder = fake_decoder(animation_info());
    let still = fixture("still.png");
    let anim = fixture("anim.gif");
    let mut app =
        with_temp_file_medium(app_with(&[still.to_str().unwrap(), anim.to_str().unwrap()]));
    app.enter_fullscreen_at(0);
    app.start_slideshow();
    app.next();
    assert_eq!(app.mode, ViewMode::Video);
    let mut state = state_with(VideoPlayer::Terminal, std::rc::Rc::clone(&decoder));
    let (_log, mut runner) = fake_runner();
    play_videos(&mut app, &mut state, &mut runner);

    decoder.feed(vec![0; 64 * 48 * 4]);
    assert!(app.video.as_mut().unwrap().poll());
    decoder.end();
    assert!(!app.video.as_mut().unwrap().poll());
    assert!(
        app.video.as_ref().unwrap().playing(),
        "a loop of 0.3 s starts again inside the slideshow interval"
    );
    assert_eq!(decoder.starts().len(), 2);
    app.slideshow_tick();
    assert_eq!(app.mode, ViewMode::Video, "the animation holds the slide");
}

#[test]
fn clip_repeats_outside_slideshow() {
    let decoder = fake_decoder(clip_info());
    let mut app = fullscreen_video_app();
    let mut state = state_with(VideoPlayer::Terminal, std::rc::Rc::clone(&decoder));
    let (_log, mut runner) = fake_runner();
    play_videos(&mut app, &mut state, &mut runner);

    decoder.feed(vec![0; 640 * 480 * 4]);
    assert!(app.video.as_mut().unwrap().poll());
    decoder.end();
    assert!(!app.video.as_mut().unwrap().poll());
    assert_eq!(decoder.starts().len(), 2, "the clip starts again");
    assert!(!app.video.as_ref().unwrap().finished());
    assert_eq!(app.mode, ViewMode::Video);
}

#[test]
fn paused_slideshow_loops_video() {
    let decoder = fake_decoder(clip_info());
    let mut app = slideshow_video_app();
    app.slideshow_toggle_pause();
    let mut state = state_with(VideoPlayer::Terminal, std::rc::Rc::clone(&decoder));
    let (_log, mut runner) = fake_runner();
    play_videos(&mut app, &mut state, &mut runner);

    decoder.feed(vec![0; 640 * 480 * 4]);
    assert!(app.video.as_mut().unwrap().poll());
    decoder.end();
    assert!(!app.video.as_mut().unwrap().poll());
    assert_eq!(
        decoder.starts().len(),
        2,
        "a paused slideshow loops the clip"
    );
}

#[test]
fn failed_open_shows_error() {
    let decoder = fake_decoder(clip_info());
    decoder.fail_probe.set(true);
    let mut app = fullscreen_video_app();
    let mut state = state_with(VideoPlayer::Terminal, decoder);
    let (_log, mut runner) = fake_runner();

    play_videos(&mut app, &mut state, &mut runner);

    assert_eq!(app.mode, ViewMode::Fullscreen);
    assert_eq!(app.error.as_deref(), Some("no video stream"));
    assert!(!app.shows_video_placeholder());
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    assert!(!handle_fullscreen_key(&mut app, enter).unwrap());
    assert_eq!(
        app.mode,
        ViewMode::Fullscreen,
        "Enter does not retry the open"
    );
}

#[test]
fn failed_start_goes_back() {
    let decoder = fake_decoder(clip_info());
    decoder.fail_start.set(true);
    let mut app = fullscreen_video_app();
    let mut state = state_with(VideoPlayer::Terminal, decoder);
    let (log, mut runner) = fake_runner();

    play_videos(&mut app, &mut state, &mut runner);

    assert_eq!(app.mode, ViewMode::Fullscreen);
    assert!(
        app.error
            .as_ref()
            .is_some_and(|error| error.contains("not found")),
        "{:?}",
        app.error
    );
    assert!(!app.shows_video_placeholder());
    assert!(log.borrow().is_empty(), "no mpv start");
}

#[test]
fn window_returns_to_placeholder() {
    let mut app = fullscreen_video_app();
    let mut state = window_state();
    let (log, mut runner) = fake_runner();

    play_videos(&mut app, &mut state, &mut runner);

    let start = last_start(&log);
    assert!(start.starts_with("spawn /bin/mpv --no-terminal"), "{start}");
    assert!(start.contains(" --loop-file=inf "), "{start}");
    assert!(start.ends_with(" -- /videos/clip.mp4"), "{start}");
    assert_eq!(app.mode, ViewMode::Fullscreen);
    assert_eq!(app.current, 1, "fullscreen stays on the video");
    assert!(app.shows_video_placeholder());
    assert!(app.watches_video_window());
    assert_eq!(
        state.status.as_deref(),
        Some("clip.mp4 plays in an mpv window")
    );
}

#[test]
fn window_returns_to_gallery() {
    let mut app = gallery_app_on_video(&["a.jpg", "clip.mp4", "b.jpg"], 1);
    app.gallery.cursor = 0;
    let mut state = window_state();
    let (_log, mut runner) = fake_runner();

    play_videos(&mut app, &mut state, &mut runner);

    assert_eq!(app.mode, ViewMode::Gallery, "the gallery opened this video");
    assert_eq!(app.gallery.cursor, 1);
}

#[test]
fn no_temp_file_uses_window() {
    for transfer in [Transfer::Direct, Transfer::Png] {
        let mut app = video_app();
        app.graphics = KittyBackend::new(transfer);
        let mut state = terminal_state();
        let (log, mut runner) = fake_runner();

        play_videos(&mut app, &mut state, &mut runner);

        assert_eq!(starts(&log).len(), 1, "{transfer:?}");
        assert_eq!(app.mode, ViewMode::Gallery);
        assert!(app.video.is_none());
        assert_eq!(
            state.status.as_deref(),
            Some("the terminal has no temp file medium: clip.mp4 plays in an mpv window")
        );
    }
}

#[test]
fn no_ffmpeg_uses_window() {
    let mut app = video_app();
    let mut state = terminal_state();
    state.video.ffmpeg = None;
    let (log, mut runner) = fake_runner();

    play_videos(&mut app, &mut state, &mut runner);

    assert_eq!(starts(&log).len(), 1);
    assert_eq!(
        state.status.as_deref(),
        Some("ffmpeg not found on PATH: clip.mp4 plays in an mpv window")
    );
}

#[test]
fn no_ffprobe_uses_window() {
    let mut app = video_app();
    let mut state = terminal_state();
    state.video.decoder = std::rc::Rc::new(SystemDecoder::new(None));
    let (log, mut runner) = fake_runner();

    play_videos(&mut app, &mut state, &mut runner);

    assert_eq!(starts(&log).len(), 1);
    assert!(app.video.is_none());
    assert_eq!(
        state.status.as_deref(),
        Some("ffprobe not found on PATH: clip.mp4 plays in an mpv window")
    );
}

#[test]
fn no_ffmpeg_or_mpv_goes_back() {
    let mut app = slideshow_video_app();
    let mut state = terminal_state();
    state.video.ffmpeg = None;
    state.video.mpv = None;
    let (log, mut runner) = fake_runner();

    play_videos(&mut app, &mut state, &mut runner);

    assert!(log.borrow().is_empty());
    assert_eq!(app.mode, ViewMode::Fullscreen);
    assert!(app.shows_video_placeholder());
    assert!(!app.slideshow_running(), "nothing can play the video");
    assert_eq!(
        state.status.as_deref(),
        Some(format!("ffmpeg not found on PATH: {MPV_MISSING_NOTE}").as_str())
    );
}

#[test]
fn mpv_start_failure_goes_back() {
    let mut app = video_app();
    let mut state = window_state();
    let (_log, mut runner) = fake_runner();
    runner.fails = true;

    play_videos(&mut app, &mut state, &mut runner);

    assert_eq!(app.mode, ViewMode::Gallery);
    assert!(!app.watches_video_window());
    assert_eq!(
        state.status.as_deref(),
        Some("mpv did not start (entity not found)")
    );
}

#[test]
fn window_request_overrides_terminal() {
    let decoder = fake_decoder(clip_info());
    let mut app = fullscreen_video_app();
    let mut state = state_with(VideoPlayer::Terminal, std::rc::Rc::clone(&decoder));
    let (log, mut runner) = fake_runner();
    app.request_video_window();

    play_videos(&mut app, &mut state, &mut runner);

    assert_eq!(starts(&log).len(), 1);
    assert!(decoder.starts().is_empty(), "no ffmpeg");
    assert_eq!(app.mode, ViewMode::Fullscreen);
    assert!(app.watches_video_window());
    assert_eq!(
        state.status.as_deref(),
        Some("clip.mp4 plays in an mpv window")
    );
}

#[test]
fn enter_on_placeholder_replays() {
    let mut app = fullscreen_video_app();
    let mut state = terminal_state();
    let (_log, mut runner) = fake_runner();
    play_videos(&mut app, &mut state, &mut runner);
    assert!(app.video.is_some());
    app.graphics_delete_all().unwrap();
    app.return_from_video();
    assert!(app.video.is_none());
    assert_eq!(app.mode, ViewMode::Fullscreen);

    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    assert!(!handle_fullscreen_key(&mut app, enter).unwrap());
    assert_eq!(app.mode, ViewMode::Video);
    play_videos(&mut app, &mut state, &mut runner);
    assert!(app.video.is_some());
}

/// A step that moved nothing names the end of the list that it hit.
#[test]
fn step_past_list_end_names_end() {
    let cases = [
        (
            ["clip.mp4", "last.mov"],
            1,
            mpv::NEXT_FILE,
            "last.mov is the last file",
        ),
        (
            ["first.mp4", "b.mov"],
            0,
            mpv::PREVIOUS_FILE,
            "first.mp4 is the first file",
        ),
    ];
    for (names, index, code, note) in cases {
        let mut app = gallery_app_on_video(&names, index);
        let mut state = window_state();
        let (_log, mut runner) = fake_runner();
        play_videos(&mut app, &mut state, &mut runner);
        runner.window.take().unwrap().send(Some(code)).unwrap();
        super::window::poll_mpv_window(&mut app, &mut state);
        assert_eq!(app.current, index, "the step moved nothing");
        assert_eq!(state.status.as_deref(), Some(note));
    }
}

mod live;
