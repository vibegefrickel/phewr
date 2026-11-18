//! The tests of a Live Photo pair: one cell, the motion on Enter, and the way back to the still.

use super::super::play_videos;
use crate::app::ViewMode;
use crate::config::VideoPlayer;
use crate::run_loop::LoopState;
use crate::test_support::live::{LIVE_ID, fullscreen_on_still, index_of};
use crate::test_support::video::{
    FakeDecoder, clip_info, fake_decoder, fake_runner, last_start, state_with,
};
use crate::test_support::{char_key, click, draw_frame, mouse, press};
use crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use std::rc::Rc;

fn terminal(decoder: &Rc<FakeDecoder>) -> LoopState {
    state_with(VideoPlayer::Terminal, Rc::clone(decoder))
}

#[test]
fn pair_shows_one_marked_cell() {
    let dir = tempfile::tempdir().unwrap();
    let app = fullscreen_on_still(dir.path(), LIVE_ID);

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
fn enter_plays_motion_once_and_returns() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = fullscreen_on_still(dir.path(), LIVE_ID);
    let still = app.current;
    let decoder = fake_decoder(clip_info());
    let mut state = terminal(&decoder);
    let (log, mut runner) = fake_runner();

    press(&mut app, KeyCode::Enter);
    assert_eq!(app.mode, ViewMode::Video);
    assert_eq!(app.current, still, "the cursor stays on the still");
    play_videos(&mut app, &mut state, &mut runner);

    assert!(log.borrow().is_empty(), "no mpv");
    let args = decoder.last_args();
    assert!(args.contains("IMG_0001.MOV"), "{args}");
    assert_eq!(state.status, None, "the identifiers match");
    decoder.feed(vec![0; 640 * 480 * 4]);
    assert!(app.video.as_mut().unwrap().poll());
    decoder.end();
    assert!(!app.video.as_mut().unwrap().poll());

    play_videos(&mut app, &mut state, &mut runner);
    assert_eq!(decoder.starts().len(), 1, "the motion plays one time");
    assert_eq!(app.mode, ViewMode::Fullscreen);
    assert_eq!(app.current, still);
    assert!(app.video.is_none());
    assert!(!app.plays_motion());
    assert!(app.needs_render, "the still comes back");
}

#[test]
fn q_and_right_click_return_to_still() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = fullscreen_on_still(dir.path(), LIVE_ID);
    let still = app.current;
    let decoder = fake_decoder(clip_info());
    let mut state = terminal(&decoder);
    let (_log, mut runner) = fake_runner();

    press(&mut app, KeyCode::Enter);
    play_videos(&mut app, &mut state, &mut runner);
    assert!(!char_key(&mut app, 'q'), "q leaves the motion, not phewr");
    assert_eq!(app.mode, ViewMode::Fullscreen);
    assert_eq!(app.current, still);
    assert!(!decoder.streaming(), "ffmpeg is gone");

    press(&mut app, KeyCode::Enter);
    play_videos(&mut app, &mut state, &mut runner);
    draw_frame(&mut app, 120, 24);
    mouse(&mut app, MouseEventKind::Down(MouseButton::Right), (60, 10));
    assert_eq!(app.mode, ViewMode::Fullscreen, "a right click goes back");
    assert_eq!(app.current, still);
}

#[test]
fn middle_click_plays_motion() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = fullscreen_on_still(dir.path(), LIVE_ID);
    draw_frame(&mut app, 120, 24);

    click(&mut app, (60, 10));

    assert_eq!(app.mode, ViewMode::Video);
    assert!(app.plays_motion());
}

#[test]
fn step_from_motion_leaves_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = fullscreen_on_still(dir.path(), LIVE_ID);
    let still = app.current;
    press(&mut app, KeyCode::Enter);

    char_key(&mut app, 'l');

    assert_eq!(app.current, still + 1);
    assert_eq!(app.mode, ViewMode::Fullscreen, "z.png is a photo");
    assert!(!app.plays_motion());
}

#[test]
fn mismatch_plays_with_note() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = fullscreen_on_still(dir.path(), "0F1E2D3C-4B5A-4968-8776-A5B4C3D2E1F0");
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
fn p_opens_motion_in_window() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = fullscreen_on_still(dir.path(), LIVE_ID);
    let still = app.current;
    let decoder = fake_decoder(clip_info());
    let mut state = terminal(&decoder);
    let (log, mut runner) = fake_runner();

    char_key(&mut app, 'p');
    play_videos(&mut app, &mut state, &mut runner);

    assert!(
        last_start(&log).contains("IMG_0001.MOV"),
        "{:?}",
        log.borrow()
    );
    assert!(decoder.starts().is_empty(), "no terminal player");
    assert_eq!(app.mode, ViewMode::Fullscreen, "the still stays");
    assert_eq!(app.current, still);
    assert!(!app.plays_motion());
}

#[test]
fn p_in_gallery_opens_motion() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = fullscreen_on_still(dir.path(), LIVE_ID);
    let still = app.current;
    app.enter_gallery();
    app.gallery.cursor = still;
    let decoder = fake_decoder(clip_info());
    let mut state = terminal(&decoder);
    let (log, mut runner) = fake_runner();

    char_key(&mut app, 'p');
    play_videos(&mut app, &mut state, &mut runner);

    assert!(last_start(&log).contains("IMG_0001.MOV"));
    assert_eq!(app.mode, ViewMode::Gallery);
    assert_eq!(app.gallery.selected_index(), Some(still));
}

#[test]
fn failed_motion_returns_with_note() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = fullscreen_on_still(dir.path(), LIVE_ID);
    let decoder = fake_decoder(clip_info());
    decoder.fail_probe.set(true);
    let mut state = terminal(&decoder);
    let (_log, mut runner) = fake_runner();

    press(&mut app, KeyCode::Enter);
    play_videos(&mut app, &mut state, &mut runner);

    assert_eq!(app.mode, ViewMode::Fullscreen);
    assert!(!app.plays_motion());
    assert_eq!(app.error, None, "the still shows");
    assert_eq!(state.status.as_deref(), Some("no video stream"));
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
            "IMG_0711.HEIC",
            "IMG_1043.HEIC",
            "IMG_1050.HEIC",
            "IMG_1060.HEIC",
            "IMG_1069.HEIC",
            "IMG_1076.HEIC",
            "IMG_1102.HEIC",
            "IMG_1126.HEIC",
            "IMG_1134.HEIC",
            "IMG_2927.HEIC",
            "IMG_2928.HEIC",
            "IMG_2929.HEIC",
            "IMG_4410.MOV",
            "IMG_5001.JPG",
            "IMG_E2928.HEIC",
        ],
        "the motions of the 12 pairs have no cell"
    );
    for (name, motion) in [
        ("IMG_1043.HEIC", Some("IMG_1043.MOV")),
        ("IMG_1050.HEIC", Some("IMG_1050.MOV")),
        ("IMG_1060.HEIC", Some("IMG_1060.MOV")),
        ("IMG_1069.HEIC", Some("IMG_1069.MOV")),
        ("IMG_1076.HEIC", Some("IMG_1076.MOV")),
        ("IMG_1102.HEIC", Some("IMG_1102.MOV")),
        ("IMG_1126.HEIC", Some("IMG_1126.MOV")),
        ("IMG_1134.HEIC", Some("IMG_1134.MOV")),
        ("IMG_2927.HEIC", Some("IMG_2927.MOV")),
        ("IMG_2928.HEIC", Some("IMG_2928.MOV")),
        ("IMG_2929.HEIC", Some("IMG_2929.MOV")),
        ("IMG_5001.JPG", Some("IMG_5001.MOV")),
        ("IMG_E2928.HEIC", None),
        ("IMG_0711.HEIC", None),
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
    assert_eq!(off.file_count(), 27, "without pairs, every movie is a cell");
}

/// Enter on each real Live still: the mismatch pair alone gets the note.
#[test]
#[ignore = "needs PHEWR_TEST_LIVE_DIR"]
fn real_mismatch_note() {
    use crate::config::Config;
    use crate::test_support::live::{real_live_dir, scanned_app};
    let Some(dir) = real_live_dir() else {
        return;
    };
    for (name, note) in [
        ("IMG_2927.HEIC", None),
        ("IMG_2928.HEIC", None),
        ("IMG_5001.JPG", None),
        (
            "IMG_2929.HEIC",
            Some("IMG_2929.HEIC and IMG_2929.MOV carry different Live Photo identifiers"),
        ),
    ] {
        let mut app = scanned_app(&dir, &Config::default());
        app.enter_fullscreen_at(index_of(&app, name));
        let decoder = fake_decoder(clip_info());
        let mut state = terminal(&decoder);
        let (_log, mut runner) = fake_runner();
        press(&mut app, KeyCode::Enter);
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

/// The real motion of `IMG_2927.HEIC` plays one time through the real ffmpeg at the wall clock,
/// and the still comes back.
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
    let still = index_of(&app, "IMG_2927.HEIC");
    app.enter_fullscreen_at(still);
    let decoder: Rc<dyn Decoder> = Rc::new(SystemDecoder::new(Some(ffprobe)));
    let mut state = state_with_decoder(VideoPlayer::Terminal, decoder, ffmpeg);
    let (log, mut runner) = fake_runner();

    press(&mut app, KeyCode::Enter);
    let start = Instant::now();
    play_videos(&mut app, &mut state, &mut runner);
    assert!(app.video.is_some(), "{:?}", state.status);
    let mut shown = 0_u32;
    while app.mode == ViewMode::Video {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "the motion hangs"
        );
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
        "IMG_2927.MOV: {shown} frames shown in {seconds:.2} s, status {:?}",
        state.status
    );
    assert!(log.borrow().is_empty(), "no mpv");
    assert_eq!(state.status, None, "no error and no mismatch");
    assert_eq!(app.mode, ViewMode::Fullscreen);
    assert_eq!(app.current, still, "back on the still");
    assert!(shown > 40, "{shown} frames");
    assert!(
        (2.5..4.5).contains(&seconds),
        "{seconds} s, never slow motion"
    );
}

/// The real motion of a fullscreen still, through the real ffmpeg and ffprobe. The first rest
/// probes on a thread, and the slowest pass of the UI thread stays short. Then the time from
/// Enter to the first frame, without a rest and after a rest of 500 ms.
#[test]
#[ignore = "needs PHEWR_TEST_LIVE_DIR, ffmpeg and ffprobe, runs about 15 s"]
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
        "IMG_0711.HEIC",
        "IMG_2927.HEIC",
        "IMG_2927.MOV",
        "IMG_2928.HEIC",
        "IMG_2928.MOV",
        "IMG_2929.HEIC",
        "IMG_2929.MOV",
        "IMG_5001.JPG",
        "IMG_5001.MOV",
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
    let other = index_of(&app, "IMG_0711.HEIC");
    for name in [
        "IMG_2927.HEIC",
        "IMG_2928.HEIC",
        "IMG_2929.HEIC",
        "IMG_5001.JPG",
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
                // A step away ends the mark of the played motion.
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
    /// The fullscreen view of `index`, after its decode and the prefetch.
    fn show(&mut self, app: &mut crate::app::App, index: usize) {
        app.enter_fullscreen_at(index);
        crate::test_support::wait_until(|| {
            app.load_if_needed();
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

    /// Enter, the first frame, and `q` back to the still.
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
        assert_eq!(app.mode, ViewMode::Fullscreen);
        time
    }
}
