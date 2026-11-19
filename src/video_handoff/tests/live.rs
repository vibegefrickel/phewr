//! The tests of a Live Photo pair: one cell, and the motion that plays like a video in every
//! view.

use super::super::play_videos;
use crate::app::ViewMode;
use crate::config::VideoPlayer;
use crate::run_loop::LoopState;
use crate::test_support::live::{LIVE_ID, grid_on_still, index_of, slideshow_on_still};
use crate::test_support::video::{
    FakeDecoder, clip_info, fake_decoder, fake_runner, last_start, state_with,
};
use crate::test_support::{char_key, draw_frame, mouse, press};
use crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use std::rc::Rc;

fn terminal(decoder: &Rc<FakeDecoder>) -> LoopState {
    state_with(VideoPlayer::Terminal, Rc::clone(decoder))
}

/// The fake ffmpeg sends one frame and ends the run.
fn play_to_end(app: &mut crate::app::App, decoder: &FakeDecoder) {
    decoder.feed(vec![0; 640 * 480 * 4]);
    assert!(app.video.as_mut().unwrap().poll());
    decoder.end();
    assert!(!app.video.as_mut().unwrap().poll());
}

#[test]
fn pair_shows_one_marked_cell() {
    let dir = tempfile::tempdir().unwrap();
    let app = grid_on_still(dir.path(), LIVE_ID);

    assert_eq!(
        app.filenames[app.folder_count..],
        ["a.png", "IMG_0001.JPG", "z.png"]
    );
    let still = index_of(&app, "IMG_0001.JPG");
    assert!(app.is_live(still));
    assert!(!app.is_live(index_of(&app, "a.png")));
    assert_eq!(
        app.live_motion(still).unwrap().file_name().unwrap(),
        "IMG_0001.MOV"
    );
}

#[test]
fn grid_enter_loops_motion() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = grid_on_still(dir.path(), LIVE_ID);
    let still = index_of(&app, "IMG_0001.JPG");
    let decoder = fake_decoder(clip_info());
    let mut state = terminal(&decoder);
    let (log, mut runner) = fake_runner();

    press(&mut app, KeyCode::Enter);
    assert_eq!(app.mode, ViewMode::Video);
    assert_eq!(app.current, still, "the cursor stays on the still");
    assert!(app.plays_motion());
    play_videos(&mut app, &mut state, &mut runner);

    assert!(log.borrow().is_empty(), "no mpv");
    let args = decoder.last_args();
    assert!(args.contains("IMG_0001.MOV"), "{args}");
    assert_eq!(state.status, None, "the identifiers match");
    play_to_end(&mut app, &decoder);
    assert_eq!(decoder.starts().len(), 2, "the motion loops");
    assert!(!app.video.as_ref().unwrap().finished());

    play_videos(&mut app, &mut state, &mut runner);
    assert_eq!(app.mode, ViewMode::Video, "no way back to the still");
    assert_eq!(decoder.starts().len(), 2);
}

#[test]
fn fullscreen_step_opens_motion() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = grid_on_still(dir.path(), LIVE_ID);
    let still = index_of(&app, "IMG_0001.JPG");
    app.enter_fullscreen_at(index_of(&app, "a.png"));
    assert_eq!(app.mode, ViewMode::Fullscreen);

    char_key(&mut app, 'l');
    assert_eq!(app.mode, ViewMode::Video, "the step opens the player");
    assert_eq!(app.current, still);
    assert!(app.plays_motion());

    char_key(&mut app, 'l');
    assert_eq!(app.current, still + 1);
    assert_eq!(app.mode, ViewMode::Fullscreen, "z.png is a photo");
    assert!(!app.plays_motion());

    char_key(&mut app, 'h');
    assert_eq!(app.mode, ViewMode::Video, "a step back plays the motion");
    assert!(app.plays_motion());
}

#[test]
fn q_and_right_click_go_to_grid() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = grid_on_still(dir.path(), LIVE_ID);
    let still = index_of(&app, "IMG_0001.JPG");
    let decoder = fake_decoder(clip_info());
    let mut state = terminal(&decoder);
    let (_log, mut runner) = fake_runner();

    app.enter_fullscreen_at(index_of(&app, "a.png"));
    char_key(&mut app, 'l');
    play_videos(&mut app, &mut state, &mut runner);
    assert!(!char_key(&mut app, 'q'), "q leaves the motion, not phewr");
    assert_eq!(app.mode, ViewMode::Gallery);
    assert_eq!(app.gallery.selected_index(), Some(still));
    assert!(!app.plays_motion());
    assert!(!decoder.streaming(), "ffmpeg is gone");

    press(&mut app, KeyCode::Enter);
    play_videos(&mut app, &mut state, &mut runner);
    draw_frame(&mut app, 120, 24);
    mouse(&mut app, MouseEventKind::Down(MouseButton::Right), (60, 10));
    assert_eq!(app.mode, ViewMode::Gallery, "a right click goes back");
    assert_eq!(app.gallery.selected_index(), Some(still));
}

#[test]
fn mismatch_plays_with_note() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = grid_on_still(dir.path(), "0F1E2D3C-4B5A-4968-8776-A5B4C3D2E1F0");
    let decoder = fake_decoder(clip_info());
    let mut state = terminal(&decoder);
    let (_log, mut runner) = fake_runner();

    press(&mut app, KeyCode::Enter);
    play_videos(&mut app, &mut state, &mut runner);

    assert!(app.video.is_some(), "the motion plays anyway");
    assert_eq!(
        state.status.as_deref(),
        Some("IMG_0001.JPG and IMG_0001.MOV carry different Live Photo identifiers")
    );
}

#[test]
fn p_in_player_opens_motion_and_returns_to_player() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = grid_on_still(dir.path(), LIVE_ID);
    let still = index_of(&app, "IMG_0001.JPG");
    let decoder = fake_decoder(clip_info());
    let mut state = terminal(&decoder);
    let (log, mut runner) = fake_runner();
    press(&mut app, KeyCode::Enter);
    play_videos(&mut app, &mut state, &mut runner);
    app.graphics_out.take_bytes();

    char_key(&mut app, 'p');
    assert!(
        app.graphics_out.take_bytes().is_empty(),
        "the last frame stays on the screen"
    );
    play_videos(&mut app, &mut state, &mut runner);
    assert!(
        last_start(&log).contains("IMG_0001.MOV"),
        "{:?}",
        log.borrow()
    );
    assert_eq!(app.mode, ViewMode::Video, "the player view waits");
    assert!(app.video.is_none(), "the decoder stops");
    assert_eq!(decoder.starts().len(), 1);

    runner
        .window
        .take()
        .unwrap()
        .send(Some(crate::mpv::USER_QUIT))
        .unwrap();
    super::super::poll_mpv_window(&mut app, &mut state);
    play_videos(&mut app, &mut state, &mut runner);
    assert_eq!(app.mode, ViewMode::Video);
    assert_eq!(app.current, still);
    assert!(app.plays_motion(), "the motion plays again");
    assert_eq!(decoder.starts().len(), 2);
    assert!(decoder.last_args().contains("IMG_0001.MOV"));
}

#[test]
fn p_in_gallery_opens_motion() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = grid_on_still(dir.path(), LIVE_ID);
    let still = index_of(&app, "IMG_0001.JPG");
    let decoder = fake_decoder(clip_info());
    let mut state = terminal(&decoder);
    let (log, mut runner) = fake_runner();

    char_key(&mut app, 'p');
    play_videos(&mut app, &mut state, &mut runner);

    assert!(last_start(&log).contains("IMG_0001.MOV"));
    assert!(decoder.starts().is_empty(), "no terminal player");
    assert_eq!(app.mode, ViewMode::Gallery);
    assert_eq!(app.gallery.selected_index(), Some(still));
}

#[test]
fn live_without_any_player_shows_still() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = grid_on_still(dir.path(), LIVE_ID);
    let still = index_of(&app, "IMG_0001.JPG");
    let decoder = fake_decoder(clip_info());
    let mut state = terminal(&decoder);
    state.video.ffmpeg = None;
    state.video.mpv = None;
    let (log, mut runner) = fake_runner();

    press(&mut app, KeyCode::Enter);
    play_videos(&mut app, &mut state, &mut runner);

    assert!(log.borrow().is_empty());
    assert_eq!(app.mode, ViewMode::Fullscreen, "the still shows");
    assert_eq!(app.current, still);
    assert!(!app.plays_motion());
    assert!(state.status.as_deref().unwrap().contains("mpv not found"));
}

#[test]
fn failed_motion_shows_still_with_note() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = grid_on_still(dir.path(), LIVE_ID);
    let still = index_of(&app, "IMG_0001.JPG");
    let decoder = fake_decoder(clip_info());
    decoder.fail_probe.set(true);
    let mut state = terminal(&decoder);
    let (_log, mut runner) = fake_runner();

    press(&mut app, KeyCode::Enter);
    play_videos(&mut app, &mut state, &mut runner);

    assert_eq!(app.mode, ViewMode::Fullscreen);
    assert_eq!(app.current, still);
    assert!(!app.plays_motion());
    assert_eq!(app.error, None, "the still shows");
    assert_eq!(state.status.as_deref(), Some("no video stream"));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.mode, ViewMode::Fullscreen, "Enter does not retry");
}

/// The cells and the Live pairs of the real folder, as its `README.md` lists them.
#[test]
#[ignore = "needs PHEWR_TEST_LIVE_DIR"]
fn real_folder_cases() {
    use crate::config::Config;
    use crate::test_support::live::{real_live_dir, scanned_app};
    let Some(dir) = real_live_dir() else {
        return;
    };
    let app = scanned_app(&dir, &Config::default());
    assert_eq!(
        app.filenames[app.folder_count..],
        [
            "IMG_1043.HEIC",
            "IMG_1050.HEIC",
            "IMG_1052.HEIC",
            "IMG_1060.HEIC",
            "IMG_1069.HEIC",
            "IMG_1076.HEIC",
            "IMG_1102.JPG",
            "IMG_1126.HEIC",
            "IMG_1134.HEIC",
            "IMG_4410.MOV",
            "IMG_E1050.HEIC",
        ],
        "the motions of the 8 pairs have no cell"
    );
    for (name, motion) in [
        ("IMG_1043.HEIC", Some("IMG_1043.MOV")),
        ("IMG_1050.HEIC", Some("IMG_1050.MOV")),
        ("IMG_1060.HEIC", Some("IMG_1060.MOV")),
        ("IMG_1069.HEIC", Some("IMG_1069.MOV")),
        ("IMG_1076.HEIC", Some("IMG_1076.MOV")),
        ("IMG_1102.JPG", Some("IMG_1102.MOV")),
        ("IMG_1126.HEIC", Some("IMG_1126.MOV")),
        ("IMG_1134.HEIC", Some("IMG_1134.MOV")),
        ("IMG_E1050.HEIC", None),
        ("IMG_1052.HEIC", None),
        ("IMG_4410.MOV", None),
    ] {
        let index = index_of(&app, name);
        let found = app
            .live_motion(index)
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned());
        assert_eq!(found.as_deref(), motion, "{name}");
        assert_eq!(app.is_live(index), motion.is_some(), "{name}");
    }
    assert!(
        app.is_video(index_of(&app, "IMG_4410.MOV")),
        "the lone movie is a video cell"
    );

    let off = scanned_app(
        &dir,
        &Config {
            live_photos: false,
            ..Config::default()
        },
    );
    assert_eq!(off.file_count(), 19, "without pairs, every movie is a cell");
}

/// The player of each real Live still: the mismatch pair alone gets the note.
#[test]
#[ignore = "needs PHEWR_TEST_LIVE_DIR"]
fn real_mismatch_note() {
    use crate::config::Config;
    use crate::test_support::live::{real_live_dir, scanned_app};
    let Some(dir) = real_live_dir() else {
        return;
    };
    for (name, note) in [
        ("IMG_1043.HEIC", None),
        ("IMG_1050.HEIC", None),
        ("IMG_1102.JPG", None),
        (
            "IMG_1069.HEIC",
            Some("IMG_1069.HEIC and IMG_1069.MOV carry different Live Photo identifiers"),
        ),
    ] {
        let mut app = scanned_app(&dir, &Config::default());
        app.enter_fullscreen_at(index_of(&app, name));
        let decoder = fake_decoder(clip_info());
        let mut state = terminal(&decoder);
        let (_log, mut runner) = fake_runner();
        play_videos(&mut app, &mut state, &mut runner);
        assert!(app.video.is_some(), "{name}: the motion plays");
        assert!(
            decoder
                .last_args()
                .contains(&name.replace(".HEIC", ".MOV").replace(".JPG", ".MOV"))
        );
        assert_eq!(state.status.as_deref(), note, "{name}");
    }
}

/// The real motion of `IMG_1050.HEIC` plays through the real ffmpeg at the wall clock for one
/// length of the clip, and then it loops.
#[test]
#[ignore = "needs PHEWR_TEST_LIVE_DIR, ffmpeg and ffprobe, plays 2.8 s"]
fn real_motion_plays_once() {
    use crate::config::Config;
    use crate::player::{Decoder, SystemDecoder};
    use crate::test_support::live::{real_live_dir, scanned_app};
    use crate::test_support::video::{real_ffmpeg, real_ffprobe, state_with_decoder};
    use std::time::{Duration, Instant};
    let (Some(dir), Some(ffmpeg), Some(ffprobe)) = (real_live_dir(), real_ffmpeg(), real_ffprobe())
    else {
        return;
    };
    let cache = tempfile::tempdir().unwrap();
    let mut app = scanned_app(&dir, &Config::default());
    app.disk_cache_dir = cache.path().to_path_buf();
    app.fullscreen_box = ratatui::layout::Rect::new(0, 0, 100, 40);
    let still = index_of(&app, "IMG_1050.HEIC");
    app.enter_fullscreen_at(still);
    let decoder: Rc<dyn Decoder> = Rc::new(SystemDecoder::new(Some(ffprobe)));
    let mut state = state_with_decoder(VideoPlayer::Terminal, decoder, ffmpeg);
    let (log, mut runner) = fake_runner();

    let start = Instant::now();
    play_videos(&mut app, &mut state, &mut runner);
    assert!(app.video.is_some(), "{:?}", state.status);
    // The motion of IMG_1050 lasts about 2.8 s.
    let length = Duration::from_millis(2800);
    let mut shown = 0_u32;
    while start.elapsed() < length {
        assert_eq!(app.mode, ViewMode::Video, "{:?}", state.status);
        let player = app.video.as_mut().unwrap();
        if player.poll() {
            shown += 1;
        } else {
            std::thread::sleep(player.time_until_next_frame().min(Duration::from_millis(2)));
        }
        play_videos(&mut app, &mut state, &mut runner);
    }
    let seconds = start.elapsed().as_secs_f64();
    println!(
        "IMG_1050.MOV: {shown} frames shown in {seconds:.2} s, status {:?}",
        state.status
    );
    assert!(log.borrow().is_empty(), "no mpv");
    assert_eq!(state.status, None, "no error and no mismatch");
    assert_eq!(app.mode, ViewMode::Video);
    assert_eq!(app.current, still, "the cursor stays on the still");
    let player = app.video.as_ref().unwrap();
    assert!(player.error.is_none(), "{:?}", player.error);
    assert!(!player.finished(), "the motion loops");
    assert!(shown > 40, "{shown} frames, never slow motion");
}

/// The real motion of a Live cell in the grid, through the real ffmpeg and ffprobe. The first
/// rest probes on a thread, and the slowest pass of the UI thread stays short. Then the time
/// from Enter to the first frame, without a rest and after a rest of 500 ms.
#[test]
#[ignore = "needs PHEWR_TEST_LIVE_DIR, ffmpeg and ffprobe, runs about 30 s"]
fn real_rest_starts_motion_ahead() {
    use crate::config::Config;
    use crate::player::{Decoder, SystemDecoder};
    use crate::test_support::live::{real_live_dir, scanned_app};
    use crate::test_support::video::{real_ffmpeg, real_ffprobe, state_with_decoder};
    use std::time::Duration;
    let (Some(dir), Some(ffmpeg), Some(ffprobe)) = (real_live_dir(), real_ffmpeg(), real_ffprobe())
    else {
        return;
    };
    // New paths: the probe cache of the process knows none of them.
    let copy = tempfile::tempdir().unwrap();
    for name in [
        "IMG_1052.HEIC",
        "IMG_1043.HEIC",
        "IMG_1043.MOV",
        "IMG_1050.HEIC",
        "IMG_1050.MOV",
        "IMG_1060.HEIC",
        "IMG_1060.MOV",
        "IMG_1069.HEIC",
        "IMG_1069.MOV",
        "IMG_1076.HEIC",
        "IMG_1076.MOV",
        "IMG_1126.HEIC",
        "IMG_1126.MOV",
        "IMG_1134.HEIC",
        "IMG_1134.MOV",
        "IMG_1102.JPG",
        "IMG_1102.MOV",
    ] {
        std::fs::copy(dir.join(name), copy.path().join(name)).unwrap();
    }
    let cache = tempfile::tempdir().unwrap();
    let mut app = scanned_app(copy.path(), &Config::default());
    app.disk_cache_dir = cache.path().to_path_buf();
    app.fullscreen_box = ratatui::layout::Rect::new(0, 0, 100, 40);
    let decoder: Rc<dyn Decoder> = Rc::new(SystemDecoder::new(Some(ffprobe)));
    let mut real = Real {
        state: state_with_decoder(VideoPlayer::Terminal, Rc::clone(&decoder), ffmpeg),
        runner: fake_runner(),
        pending: Vec::new(),
    };
    let other = index_of(&app, "IMG_1052.HEIC");
    for name in [
        "IMG_1043.HEIC",
        "IMG_1050.HEIC",
        "IMG_1060.HEIC",
        "IMG_1069.HEIC",
        "IMG_1076.HEIC",
        "IMG_1102.JPG",
        "IMG_1126.HEIC",
        "IMG_1134.HEIC",
    ] {
        let still = index_of(&app, name);
        let motion = app.live_motion(still).unwrap().to_path_buf();
        assert!(
            decoder.cached_probe(&motion).is_none(),
            "{name}: a cold probe"
        );
        let (started, slowest) = real.cold_rest(&mut app, still);
        assert!(
            decoder.cached_probe(&motion).is_some(),
            "{name}: the thread probed"
        );
        let mut report = vec![format!(
            "cold rest: start at {} ms, slowest pass {:.1} ms, Enter {:.1} ms",
            started.as_millis(),
            slowest.as_secs_f64() * 1000.0,
            real.enter_to_first_frame(&mut app).as_secs_f64() * 1000.0
        )];
        for rest in [Duration::ZERO, Duration::from_millis(500)] {
            for _ in 0..3 {
                // A step away ends the rest.
                real.show(&mut app, other);
                real.show(&mut app, still);
                real.rest(&mut app, rest);
                assert_eq!(app.video_prestart.running(), !rest.is_zero(), "{name}");
                let time = real.enter_to_first_frame(&mut app);
                report.push(format!(
                    "{} ms rest: {:.1} ms",
                    rest.as_millis(),
                    time.as_secs_f64() * 1000.0
                ));
            }
        }
        println!("{name}: {}", report.join(", "));
    }
    assert!(real.runner.0.borrow().is_empty(), "no mpv");
}

/// The loop state, the mpv runner and the emits of the real run.
struct Real {
    state: LoopState,
    runner: (
        crate::test_support::video::Log,
        crate::test_support::video::FakeRunner,
    ),
    pending: Vec<usize>,
}

impl Real {
    /// The gallery cursor on `index`, after the background work.
    fn show(&mut self, app: &mut crate::app::App, index: usize) {
        let name = app.filenames[index].clone();
        crate::test_support::live::select(app, &name);
        crate::test_support::wait_until(|| {
            crate::run_loop::poll_background(app, &mut self.pending);
            !app.has_pending_work()
        });
        play_videos(app, &mut self.state, &mut self.runner.1);
    }

    /// The passes of the run loop for `rest`.
    fn rest(&mut self, app: &mut crate::app::App, rest: std::time::Duration) {
        let start = std::time::Instant::now();
        while start.elapsed() < rest {
            play_videos(app, &mut self.state, &mut self.runner.1);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// The rest on `still` until its player runs: the time of the start and the slowest pass.
    fn cold_rest(
        &mut self,
        app: &mut crate::app::App,
        still: usize,
    ) -> (std::time::Duration, std::time::Duration) {
        use std::time::{Duration, Instant};
        self.show(app, still);
        assert!(!app.video_prestart.running());
        let start = Instant::now();
        let mut slowest = Duration::ZERO;
        while !app.video_prestart.running() {
            assert!(start.elapsed() < Duration::from_secs(3), "no start");
            let pass = Instant::now();
            play_videos(app, &mut self.state, &mut self.runner.1);
            slowest = slowest.max(pass.elapsed());
            std::thread::sleep(Duration::from_millis(10));
        }
        (start.elapsed(), slowest)
    }

    /// Enter, the first frame, and `q` back to the grid.
    fn enter_to_first_frame(&mut self, app: &mut crate::app::App) -> std::time::Duration {
        use std::time::{Duration, Instant};
        press(app, KeyCode::Enter);
        let enter = Instant::now();
        play_videos(app, &mut self.state, &mut self.runner.1);
        let player = app.video.as_mut().unwrap();
        while !player.poll() {
            assert!(enter.elapsed() < Duration::from_secs(5), "no frame");
            std::thread::sleep(Duration::from_micros(200));
        }
        let time = enter.elapsed();
        assert!(!char_key(app, 'q'));
        assert_eq!(app.mode, ViewMode::Gallery);
        time
    }
}

/// A running slideshow on the still of the pair, with the decoded still on the screen.
fn running_slideshow(dir: &std::path::Path) -> crate::app::App {
    let mut app = slideshow_on_still(dir, LIVE_ID);
    app.loaded = Some(image::RgbaImage::new(1, 1).into());
    app
}

#[test]
fn running_slideshow_plays_motion_then_moves_on() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = running_slideshow(dir.path());
    let still = app.current;
    let decoder = fake_decoder(clip_info());
    let mut state = terminal(&decoder);
    let (log, mut runner) = fake_runner();

    app.slideshow_tick();
    assert_eq!(app.mode, ViewMode::Video, "the motion starts at once");
    assert!(app.plays_motion());
    assert_eq!(app.current, still);
    play_videos(&mut app, &mut state, &mut runner);
    assert!(decoder.last_args().contains("IMG_0001.MOV"));
    assert!(log.borrow().is_empty(), "no mpv");

    app.slideshow_tick();
    assert_eq!(app.current, still, "the motion holds the slide");
    decoder.feed(vec![0; 640 * 480 * 4]);
    assert!(app.video.as_mut().unwrap().poll());
    decoder.end();
    assert!(!app.video.as_mut().unwrap().poll());

    play_videos(&mut app, &mut state, &mut runner);
    assert_eq!(
        app.mode,
        ViewMode::Video,
        "the slideshow moves on, not the still"
    );
    app.slideshow_tick();
    assert_eq!(decoder.starts().len(), 1, "the motion plays one time");
    assert_eq!(app.current, still + 1);
    assert_eq!(app.mode, ViewMode::Fullscreen);
    assert!(!app.plays_motion());
    assert!(app.slideshow_running());
    let interval = app.slideshow.as_ref().unwrap().interval;
    assert!(app.slideshow_timeout().unwrap() > interval / 2);
}

#[test]
fn paused_slideshow_loops_motion() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = running_slideshow(dir.path());
    let still = app.current;
    let decoder = fake_decoder(clip_info());
    let mut state = terminal(&decoder);
    let (_log, mut runner) = fake_runner();
    app.slideshow_tick();
    play_videos(&mut app, &mut state, &mut runner);
    assert!(app.plays_motion());

    char_key(&mut app, ' ');
    assert!(app.slideshow.as_ref().unwrap().paused);
    play_to_end(&mut app, &decoder);
    assert!(
        app.video.as_ref().unwrap().finished(),
        "the run played once"
    );
    play_videos(&mut app, &mut state, &mut runner);
    assert_eq!(decoder.starts().len(), 2, "the paused slideshow loops it");
    play_to_end(&mut app, &decoder);
    assert_eq!(decoder.starts().len(), 3);
    app.slideshow_tick();
    assert_eq!(app.current, still, "a paused slideshow waits");
    assert_eq!(app.mode, ViewMode::Video);

    char_key(&mut app, ' ');
    assert!(
        app.video.is_none(),
        "the resume plays the motion from its start"
    );
    play_videos(&mut app, &mut state, &mut runner);
    play_to_end(&mut app, &decoder);
    app.slideshow_tick();
    assert_eq!(app.current, still + 1, "then the slideshow moves on");
}

#[test]
fn pause_on_still_loops_motion() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = slideshow_on_still(dir.path(), LIVE_ID);
    let still = app.current;
    assert_eq!(
        app.mode,
        ViewMode::Fullscreen,
        "the still waits for its decode"
    );

    char_key(&mut app, ' ');

    assert_eq!(app.mode, ViewMode::Video, "the pause plays the motion");
    assert_eq!(app.current, still);
    assert!(app.plays_motion());
}

#[test]
fn s_in_grid_on_live_still_starts_motion() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = grid_on_still(dir.path(), LIVE_ID);
    let still = index_of(&app, "IMG_0001.JPG");

    char_key(&mut app, 's');
    assert!(app.slideshow_running());
    assert_eq!(app.mode, ViewMode::Fullscreen);
    app.slideshow_tick();
    assert_eq!(
        app.mode,
        ViewMode::Fullscreen,
        "the motion waits for the still"
    );

    app.loaded = Some(image::RgbaImage::new(1, 1).into());
    app.slideshow_tick();
    assert_eq!(app.mode, ViewMode::Video);
    assert!(app.plays_motion());
    assert_eq!(app.current, still);
}

#[test]
fn quit_in_window_during_motion_pauses_in_player() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = running_slideshow(dir.path());
    let still = app.current;
    let decoder = fake_decoder(clip_info());
    let mut state = terminal(&decoder);
    let (log, mut runner) = fake_runner();
    app.slideshow_tick();
    play_videos(&mut app, &mut state, &mut runner);
    assert!(app.video.is_some());

    char_key(&mut app, 'p');
    play_videos(&mut app, &mut state, &mut runner);
    assert_eq!(log.borrow().len(), 1, "the window opens");
    assert_eq!(
        app.mode,
        ViewMode::Video,
        "the player view waits for the window"
    );
    app.slideshow_tick();
    assert_eq!(app.current, still, "the window holds the slideshow");

    runner
        .window
        .take()
        .unwrap()
        .send(Some(crate::mpv::USER_QUIT))
        .unwrap();
    super::super::poll_mpv_window(&mut app, &mut state);

    assert_eq!(app.current, still);
    assert_eq!(app.mode, ViewMode::Video);
    assert!(app.slideshow.as_ref().unwrap().paused);
    play_videos(&mut app, &mut state, &mut runner);
    assert!(app.plays_motion(), "the paused slideshow loops the motion");
    play_to_end(&mut app, &decoder);
    assert_eq!(decoder.starts().len(), 3, "the motion starts again");
}

/// `p` on the still of a running slideshow, before its motion: the player view waits for the
/// window. The end of the window moves on, and a quit pauses the slideshow on the looping motion.
#[test]
fn p_on_slideshow_still_holds_the_slideshow() {
    for (code, moves_on) in [(Some(0), true), (Some(crate::mpv::USER_QUIT), false)] {
        let dir = tempfile::tempdir().unwrap();
        let mut app = running_slideshow(dir.path());
        let still = app.current;
        let decoder = fake_decoder(clip_info());
        let mut state = terminal(&decoder);
        let (log, mut runner) = fake_runner();

        char_key(&mut app, 'p');
        play_videos(&mut app, &mut state, &mut runner);
        assert!(last_start(&log).contains("IMG_0001.MOV"));
        assert_eq!(app.mode, ViewMode::Video, "the player view waits");
        assert!(decoder.starts().is_empty(), "no terminal player");
        app.slideshow_tick();
        assert_eq!(app.current, still, "the window holds the slideshow");

        runner.window.take().unwrap().send(code).unwrap();
        super::super::poll_mpv_window(&mut app, &mut state);
        if moves_on {
            assert_eq!(app.current, still + 1);
            assert!(app.slideshow_advancing());
        } else {
            assert_eq!(app.current, still);
            assert!(app.slideshow.as_ref().unwrap().paused);
            play_videos(&mut app, &mut state, &mut runner);
            assert_eq!(app.mode, ViewMode::Video);
            assert!(app.plays_motion(), "the paused slideshow loops the motion");
        }
    }
}

#[test]
fn end_of_window_during_motion_moves_on() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = running_slideshow(dir.path());
    let still = app.current;
    let decoder = fake_decoder(clip_info());
    let mut state = terminal(&decoder);
    let (_log, mut runner) = fake_runner();
    app.slideshow_tick();
    play_videos(&mut app, &mut state, &mut runner);
    char_key(&mut app, 'p');
    play_videos(&mut app, &mut state, &mut runner);

    runner.window.take().unwrap().send(Some(0)).unwrap();
    super::super::poll_mpv_window(&mut app, &mut state);

    assert_eq!(app.current, still + 1);
    assert!(!app.slideshow.as_ref().unwrap().paused);
}

#[test]
fn slideshow_without_players_keeps_running_on_live_still() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = running_slideshow(dir.path());
    let still = app.current;
    let decoder = fake_decoder(clip_info());
    let mut state = terminal(&decoder);
    state.video.mpv = None;
    state.video.ffmpeg = None;
    let (_log, mut runner) = fake_runner();

    app.slideshow_tick();
    play_videos(&mut app, &mut state, &mut runner);

    assert!(
        app.slideshow_running(),
        "the missing tools do not stop the show"
    );
    assert_eq!(app.mode, ViewMode::Fullscreen);
    assert_eq!(app.current, still);
    app.slideshow_tick();
    assert!(!app.plays_motion(), "the motion starts one time only");
    app.slideshow.as_mut().unwrap().next_at = crate::test_support::overdue();
    app.loaded = Some(image::RgbaImage::new(1, 1).into());
    app.slideshow_tick();
    assert_eq!(
        app.current,
        still + 1,
        "the slide moves on after the interval"
    );
}

#[test]
fn failed_still_decode_does_not_block_the_slideshow() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = slideshow_on_still(dir.path(), LIVE_ID);
    app.loaded = None;
    app.error = Some("broken".into());

    app.slideshow_tick();

    assert!(app.plays_motion(), "the motion starts without the still");
}

#[test]
fn manual_step_back_onto_still_replays_motion_in_slideshow() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = running_slideshow(dir.path());
    let still = app.current;
    app.slideshow_tick();
    assert!(app.plays_motion());

    char_key(&mut app, 'l');
    char_key(&mut app, 'h');
    app.loaded = Some(image::RgbaImage::new(1, 1).into());
    app.slideshow_tick();

    assert_eq!(app.current, still);
    assert!(app.plays_motion());
}
