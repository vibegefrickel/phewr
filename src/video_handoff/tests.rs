//! Tests of the start of a video in the terminal player or the mpv window.

use super::{MPV_MISSING_NOTE, play_videos};
use crate::app::ViewMode;
use crate::config::VideoPlayer;
use crate::keys::fullscreen::handle_fullscreen_key;
use crate::mpv;
use crate::pictures::kitty::{KittyBackend, Transfer};
use crate::player::SystemDecoder;
use crate::test_support::video::{
    FakeRunner, clip_info, fake_decoder, fake_runner, fullscreen_app_on_video,
    fullscreen_video_app, gallery_app_on_video, last_start, slideshow_video_app, starts,
    state_with, terminal_state, video_app, window_state,
};
use crate::test_support::{char_key, press};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const NAMES: &[&str] = &["a.jpg", "clip.mp4", "b.jpg"];

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
fn resume_keeps_slideshow_run() {
    let decoder = fake_decoder(clip_info());
    let mut app = slideshow_video_app();
    let mut state = state_with(VideoPlayer::Terminal, std::rc::Rc::clone(&decoder));
    let (_log, mut runner) = fake_runner();
    play_videos(&mut app, &mut state, &mut runner);
    decoder.feed(vec![0; 640 * 480 * 4]);
    assert!(app.video.as_mut().unwrap().poll());

    char_key(&mut app, ' ');
    char_key(&mut app, ' ');
    play_videos(&mut app, &mut state, &mut runner);
    assert!(app.video.is_some(), "the run plays on");
    assert_eq!(decoder.starts().len(), 1, "no restart at the start");

    decoder.end();
    assert!(!app.video.as_mut().unwrap().poll());
    app.slideshow_tick();
    assert_eq!(app.current, 2, "the end of the run moves on");
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
    assert!(log.borrow().is_empty(), "no mpv start");
}

#[test]
fn window_without_player_returns_to_grid() {
    let mut app = fullscreen_video_app();
    let mut state = window_state();
    let (log, mut runner) = fake_runner();

    play_videos(&mut app, &mut state, &mut runner);

    let start = last_start(&log);
    assert!(start.starts_with("spawn /bin/mpv --no-terminal"), "{start}");
    assert!(start.contains(" --loop-file=inf "), "{start}");
    assert!(start.ends_with(" -- /videos/clip.mp4"), "{start}");
    assert_eq!(
        app.mode,
        ViewMode::Gallery,
        "the fullscreen view shows no video"
    );
    assert_eq!(app.gallery.cursor, 1);
    assert!(app.watches_video_window());
    assert_eq!(
        state.status.as_deref(),
        Some("clip.mp4 plays in an mpv window")
    );

    runner
        .window
        .take()
        .unwrap()
        .send(Some(mpv::USER_QUIT))
        .unwrap();
    super::poll_mpv_window(&mut app, &mut state);
    assert_eq!(app.mode, ViewMode::Gallery);
}

#[test]
fn gallery_window_returns_to_player() {
    let decoder = fake_decoder(clip_info());
    let mut state = state_with(VideoPlayer::Terminal, std::rc::Rc::clone(&decoder));
    let (log, mut runner) = fake_runner();
    let mut app = gallery_app_on_video(&["a.jpg", "clip.mp4", "b.jpg"], 1);
    play_videos(&mut app, &mut state, &mut runner);
    assert!(app.video.is_some(), "the grid opened the terminal player");
    app.graphics_out.take_bytes();

    char_key(&mut app, 'p');
    assert!(
        app.graphics_out.take_bytes().is_empty(),
        "the last frame stays on the screen"
    );
    play_videos(&mut app, &mut state, &mut runner);
    assert_eq!(starts(&log).len(), 1);
    assert_eq!(app.mode, ViewMode::Video, "the player view waits");
    assert!(app.video.is_none(), "the decoder stops");
    assert!(app.watches_video_window());
    assert_eq!(decoder.starts().len(), 1, "no second ffmpeg");

    runner
        .window
        .take()
        .unwrap()
        .send(Some(mpv::USER_QUIT))
        .unwrap();
    super::poll_mpv_window(&mut app, &mut state);
    play_videos(&mut app, &mut state, &mut runner);
    assert_eq!(app.mode, ViewMode::Video);
    assert!(app.video.is_some(), "the terminal player plays again");
    assert_eq!(decoder.starts().len(), 2);

    char_key(&mut app, 'q');
    assert_eq!(app.mode, ViewMode::Gallery, "q goes to the grid");
    assert_eq!(app.gallery.cursor, 1);
}

#[test]
fn gallery_p_keeps_gallery() {
    for mut state in [window_state(), terminal_state()] {
        let mut app = gallery_app_on_video(&["a.jpg", "clip.mp4", "b.jpg"], 1);
        app.gallery.cursor = 0;
        app.request_video_window();
        let (_log, mut runner) = fake_runner();

        play_videos(&mut app, &mut state, &mut runner);
        assert_eq!(app.mode, ViewMode::Gallery, "p in the grid keeps the grid");
        assert_eq!(app.gallery.cursor, 1);

        runner
            .window
            .take()
            .unwrap()
            .send(Some(mpv::USER_QUIT))
            .unwrap();
        super::poll_mpv_window(&mut app, &mut state);
        play_videos(&mut app, &mut state, &mut runner);
        assert_eq!(app.mode, ViewMode::Gallery, "the grid stays");
    }
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
    assert_eq!(app.mode, ViewMode::Gallery);
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

/// A player from the fullscreen view on the second of `names` that opened its video in the
/// mpv window with `p`.
fn fullscreen_player_in_window(
    names: &[&str],
    state: &mut crate::run_loop::LoopState,
    runner: &mut FakeRunner,
) -> crate::app::App {
    let mut app = fullscreen_app_on_video(names, false);
    play_videos(&mut app, state, runner);
    assert!(app.video.is_some());
    app.graphics_out.take_bytes();
    char_key(&mut app, 'p');
    assert!(
        app.graphics_out.take_bytes().is_empty(),
        "the last frame stays on the screen"
    );
    play_videos(&mut app, state, runner);
    app
}

#[test]
fn fullscreen_window_returns_to_player() {
    let decoder = fake_decoder(clip_info());
    let mut state = state_with(VideoPlayer::Terminal, std::rc::Rc::clone(&decoder));
    let (log, mut runner) = fake_runner();
    let mut app = fullscreen_player_in_window(NAMES, &mut state, &mut runner);

    assert_eq!(starts(&log).len(), 1);
    assert!(last_start(&log).contains(" --loop-file=inf "));
    assert_eq!(app.mode, ViewMode::Video, "the player view waits");
    assert!(app.video.is_none(), "the decoder stops");
    assert!(app.watches_video_window());
    assert_eq!(
        state.status.as_deref(),
        Some("clip.mp4 plays in an mpv window")
    );
    play_videos(&mut app, &mut state, &mut runner);
    assert_eq!(decoder.starts().len(), 1, "no second ffmpeg");
    char_key(&mut app, 'p');
    assert!(!app.take_video_window_request(), "one window at a time");

    runner
        .window
        .take()
        .unwrap()
        .send(Some(mpv::USER_QUIT))
        .unwrap();
    super::poll_mpv_window(&mut app, &mut state);
    play_videos(&mut app, &mut state, &mut runner);
    assert_eq!(app.mode, ViewMode::Video);
    assert_eq!(app.current, 1);
    assert!(app.video.is_some(), "the terminal player plays again");
    assert_eq!(decoder.starts().len(), 2);
}

#[test]
fn gallery_window_step_keeps_player() {
    let decoder = fake_decoder(clip_info());
    let mut state = state_with(VideoPlayer::Terminal, std::rc::Rc::clone(&decoder));
    let (log, mut runner) = fake_runner();
    let mut app = gallery_app_on_video(&["a.jpg", "clip.mp4", "next.mov"], 1);
    play_videos(&mut app, &mut state, &mut runner);
    char_key(&mut app, 'p');
    play_videos(&mut app, &mut state, &mut runner);
    assert_eq!(starts(&log).len(), 1);

    runner
        .window
        .take()
        .unwrap()
        .send(Some(mpv::NEXT_FILE))
        .unwrap();
    super::poll_mpv_window(&mut app, &mut state);
    play_videos(&mut app, &mut state, &mut runner);
    assert_eq!(starts(&log).len(), 2, "a new window opens");
    assert!(last_start(&log).ends_with(" -- /videos/next.mov"));
    assert_eq!(app.mode, ViewMode::Video, "the player view stays");
    assert_eq!(app.current, 2);
    assert!(app.video.is_none(), "mpv plays the next file");
    assert_eq!(decoder.starts().len(), 1, "no second ffmpeg");
}

#[test]
fn window_step_opens_neighbor_window() {
    let names = ["a.jpg", "clip.mp4", "next.mov"];
    let mut state = state_with(VideoPlayer::Terminal, fake_decoder(clip_info()));
    let (log, mut runner) = fake_runner();
    let mut app = fullscreen_player_in_window(&names, &mut state, &mut runner);
    runner
        .window
        .take()
        .unwrap()
        .send(Some(mpv::NEXT_FILE))
        .unwrap();
    super::poll_mpv_window(&mut app, &mut state);
    play_videos(&mut app, &mut state, &mut runner);
    assert!(last_start(&log).ends_with(" -- /videos/next.mov"));
    assert_eq!(app.mode, ViewMode::Video);
    assert_eq!(app.current, 2);
    assert!(app.video.is_none(), "mpv plays the next file");
    assert!(app.watches_video_window());

    let mut app = fullscreen_player_in_window(&names, &mut state, &mut runner);
    runner
        .window
        .take()
        .unwrap()
        .send(Some(mpv::PREVIOUS_FILE))
        .unwrap();
    super::poll_mpv_window(&mut app, &mut state);
    assert_eq!(app.mode, ViewMode::Fullscreen, "a.jpg is a photo");
    assert_eq!(app.current, 0);
}

#[test]
fn missing_mpv_keeps_player() {
    let decoder = fake_decoder(clip_info());
    let mut state = state_with(VideoPlayer::Terminal, std::rc::Rc::clone(&decoder));
    state.video.mpv = None;
    let (log, mut runner) = fake_runner();
    let mut app = fullscreen_player_in_window(NAMES, &mut state, &mut runner);

    assert!(log.borrow().is_empty());
    assert_eq!(state.status.as_deref(), Some(MPV_MISSING_NOTE));
    assert_eq!(app.mode, ViewMode::Video);
    play_videos(&mut app, &mut state, &mut runner);
    assert!(app.video.is_some(), "the terminal player plays on");
}

#[test]
fn slideshow_end_loops_last_clip() {
    let decoder = fake_decoder(clip_info());
    let mut app = fullscreen_app_on_video(&["a.jpg", "clip.mp4"], true);
    app.slideshow_loop = false;
    let mut state = state_with(VideoPlayer::Terminal, std::rc::Rc::clone(&decoder));
    let (_log, mut runner) = fake_runner();
    play_videos(&mut app, &mut state, &mut runner);
    decoder.feed(vec![0; 640 * 480 * 4]);
    assert!(app.video.as_mut().unwrap().poll());
    decoder.end();
    assert!(!app.video.as_mut().unwrap().poll());

    app.slideshow_tick();
    assert!(!app.slideshow_running(), "the list ends");
    assert_eq!(app.mode, ViewMode::Video);
    play_videos(&mut app, &mut state, &mut runner);
    assert!(app.video.is_some(), "the clip plays again");
    assert_eq!(decoder.starts().len(), 2);
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

/// An app in the fullscreen view on the photo before an animation.
fn app_before_animation() -> crate::app::App {
    use crate::test_support::{app_with, fixture, video::with_temp_file_medium};
    let still = fixture("still.gif");
    let anim = fixture("anim.webp");
    let mut app =
        with_temp_file_medium(app_with(&[still.to_str().unwrap(), anim.to_str().unwrap()]));
    app.enter_fullscreen_at(0);
    assert_eq!(app.mode, ViewMode::Fullscreen);
    app
}

#[test]
fn fullscreen_step_loops_animation() {
    use crate::test_support::video::animation_info;
    let mut app = app_before_animation();
    let decoder = fake_decoder(animation_info());
    let mut state = state_with(VideoPlayer::Terminal, std::rc::Rc::clone(&decoder));
    let (log, mut runner) = fake_runner();

    char_key(&mut app, 'l');
    assert_eq!(app.mode, ViewMode::Video, "the step opens the player");
    play_videos(&mut app, &mut state, &mut runner);
    decoder.feed(vec![0; 64 * 48 * 4]);
    assert!(app.video.as_mut().unwrap().poll());
    decoder.end();
    assert!(!app.video.as_mut().unwrap().poll());
    assert_eq!(decoder.starts().len(), 2, "the animation loops");
    assert!(log.borrow().is_empty(), "no mpv");

    assert!(!char_key(&mut app, 'q'));
    assert_eq!(app.mode, ViewMode::Gallery, "q goes to the grid");
    assert_eq!(app.gallery.cursor, 1);
}

#[test]
fn grid_enter_loops_animation() {
    use crate::test_support::video::animation_info;
    let mut app = app_before_animation();
    app.enter_gallery();
    app.gallery.cursor = 1;
    let decoder = fake_decoder(animation_info());
    let mut state = state_with(VideoPlayer::Terminal, std::rc::Rc::clone(&decoder));
    let (_log, mut runner) = fake_runner();

    press(&mut app, KeyCode::Enter);
    play_videos(&mut app, &mut state, &mut runner);
    assert_eq!(app.mode, ViewMode::Video);
    decoder.feed(vec![0; 64 * 48 * 4]);
    assert!(app.video.as_mut().unwrap().poll());
    decoder.end();
    assert!(!app.video.as_mut().unwrap().poll());
    assert_eq!(decoder.starts().len(), 2, "the animation loops");
}

#[test]
fn failed_animation_open_shows_still_with_note() {
    use crate::test_support::video::animation_info;
    let mut app = app_before_animation();
    let decoder = fake_decoder(animation_info());
    decoder.fail_probe.set(true);
    let mut state = state_with(VideoPlayer::Terminal, std::rc::Rc::clone(&decoder));
    let (log, mut runner) = fake_runner();

    char_key(&mut app, 'l');
    play_videos(&mut app, &mut state, &mut runner);
    assert_eq!(app.mode, ViewMode::Fullscreen, "the still shows");
    assert_eq!(app.current, 1);
    assert_eq!(state.status.as_deref(), Some("no video stream"));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.mode, ViewMode::Fullscreen, "Enter does not retry");

    char_key(&mut app, 'p');
    play_videos(&mut app, &mut state, &mut runner);
    assert_eq!(starts(&log).len(), 1, "p opens the mpv window");
    assert_eq!(app.mode, ViewMode::Video, "the player view waits");
}

#[test]
fn animation_without_any_player_shows_still() {
    let mut app = app_before_animation();
    let mut state = terminal_state();
    state.video.ffmpeg = None;
    state.video.mpv = None;
    let (log, mut runner) = fake_runner();

    char_key(&mut app, 'l');
    play_videos(&mut app, &mut state, &mut runner);
    assert!(log.borrow().is_empty());
    assert_eq!(app.mode, ViewMode::Fullscreen, "an animation has a still");
    assert_eq!(app.current, 1);
    assert_eq!(
        state.status.as_deref(),
        Some(format!("ffmpeg not found on PATH: {MPV_MISSING_NOTE}").as_str())
    );
}

#[test]
fn animation_without_player_shows_grid() {
    let mut app = app_before_animation();
    let mut state = window_state();
    let (log, mut runner) = fake_runner();

    char_key(&mut app, 'l');
    play_videos(&mut app, &mut state, &mut runner);
    assert_eq!(starts(&log).len(), 1);
    assert_eq!(
        app.mode,
        ViewMode::Gallery,
        "an animation plays like a video"
    );
    assert_eq!(app.gallery.cursor, 1);
}
