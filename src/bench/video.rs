//! The terminal player, animated images and video probes.

use super::{
    CELL_PX, LIMIT, count_transmissions, fine_millis, fullscreen_rect, is_video, median, millis,
    remove_temp_files, report, run_pass, test_dir,
};
use crate::app::App;
use crate::image_list::SharedImageList;
use crate::run_loop::next_timeout;
use crate::run_loop::poll_background;
use crate::scanner;
use ratatui::layout::Rect;
use std::path::Path;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use std::time::Instant;
use tuilibr::pictures::decode::color;
use tuilibr::pictures::kitty::KittyBackend;
use tuilibr::pictures::kitty::Transfer;
use tuilibr::pictures::motion::decoder::{Decoder, SystemDecoder};

/// The real ffmpeg and a decoder with the real ffprobe, or `None` with a note.
fn video_programs(label: &str) -> Option<(PathBuf, SystemDecoder)> {
    let tools = crate::video::Tools::find(std::env::var_os("PATH").as_deref());
    let (Some(ffmpeg), Some(ffprobe)) = (tools.ffmpeg, tools.ffprobe) else {
        report(&format!("{label}: no ffmpeg or ffprobe on PATH"));
        return None;
    };
    Some((ffmpeg, SystemDecoder::new(Some(ffprobe))))
}

/// The decode side of the terminal player: ffmpeg frames into a sink at two frame boxes.
#[test]
#[ignore = "reads the folder PHEWR_TEST_DIR"]
fn player_decode_rate() {
    use std::sync::mpsc::TryRecvError;
    use tuilibr::pictures::motion::decoder::{Pipeline, args, cube_for, pixel_format};
    use tuilibr::pictures::motion::frame_box::frame_box;
    use tuilibr::pictures::motion::probe::VideoInfo;

    const FRAMES: usize = 300;

    let Some(dir) = test_dir() else {
        return;
    };
    let Some((ffmpeg, decoder)) = video_programs("terminal player") else {
        return;
    };
    let mut videos: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| is_video(path))
        .collect();
    videos.sort();
    let Some(video) = videos.first() else {
        report("terminal player: no video in the folder");
        return;
    };
    let info: VideoInfo = decoder.probe(video).unwrap();
    let cache = tempfile::tempdir().unwrap();
    let cube = cube_for(&info, color::target(), &[cache.path()]);
    report(&format!(
        "terminal player: {} {}x{} {:.2} fps, hdr {:?}, rotation {}",
        video.file_name().unwrap().to_string_lossy(),
        info.width,
        info.height,
        info.fps,
        info.hdr,
        info.rotation
    ));
    for limit in [(1280, 720), (1920, 1080)] {
        let frame = frame_box((info.width, info.height), limit);
        let args = args(video, &info, frame, cube.as_deref(), 0.0, Pipeline::Gpu);
        let bytes = frame.0 as usize * frame.1 as usize * pixel_format(&info).1;
        let start = Instant::now();
        let stream = decoder.start(&ffmpeg, &args, bytes).unwrap();
        let mut first = None;
        let mut frames = 0;
        while frames < FRAMES {
            match stream.try_next() {
                Ok(_) => {
                    frames += 1;
                    first.get_or_insert_with(Instant::now);
                }
                Err(TryRecvError::Empty) => {
                    assert!(start.elapsed() < LIMIT, "ffmpeg did not deliver");
                    thread::sleep(Duration::from_micros(200));
                }
                Err(TryRecvError::Disconnected) => break,
            }
        }
        let first = first.unwrap_or_else(Instant::now);
        let decode = first.elapsed();
        let fps = if decode.is_zero() {
            0.0
        } else {
            (frames.saturating_sub(1)) as f64 / decode.as_secs_f64()
        };
        report(&format!(
            "terminal player decode {}x{}: first frame after {}, {frames} frames in {} = {fps:.0} fps",
            frame.0,
            frame.1,
            millis(first - start),
            millis(decode)
        ));
    }
}

/// The whole terminal player without a terminal, for 5 s of the first video: frames shown and dropped.
#[test]
#[ignore = "reads the folder PHEWR_TEST_DIR"]
fn player_realtime() {
    use crate::player::Player;
    use std::rc::Rc;
    use tuilibr::pictures::kitty::{KittyBackend, Transfer};

    const PLAY: Duration = Duration::from_secs(5);

    let Some(dir) = test_dir() else {
        return;
    };
    let Some((ffmpeg, decoder)) = video_programs("terminal player") else {
        return;
    };
    let list = SharedImageList::new();
    scanner::spawn(vec![dir], list.clone());
    let mut app = App::new(CELL_PX, list);
    app.graphics = KittyBackend::new(Transfer::TempFile);
    let deadline = Instant::now() + LIMIT;
    while !app.scan_complete {
        assert!(Instant::now() < deadline, "the scan did not finish");
        app.refresh_from_scanner();
        thread::sleep(Duration::from_millis(5));
    }
    let Some(index) = app.images.iter().position(|path| is_video(path)) else {
        report("terminal player: no video in the folder");
        return;
    };
    let cache = tempfile::tempdir().unwrap();
    app.enter_video(index);
    app.image_rect = fullscreen_rect();
    let limit = tuilibr::pictures::motion::frame_box::frame_limit(fullscreen_rect(), CELL_PX);
    let player = Player::open(
        Rc::new(decoder),
        &ffmpeg,
        &app.images[index],
        limit,
        &[cache.path()],
        true,
    )
    .unwrap();
    app.video = Some(player);
    let start = Instant::now();
    let mut pending = Vec::new();
    let mut frames = 0;
    let mut bytes = 0;
    let mut first = None;
    let mut file_bytes = 0;
    while start.elapsed() < PLAY {
        run_pass(&mut app, &mut pending);
        let out = app.graphics_out.take_bytes();
        file_bytes += remove_temp_files(&out);
        if !out.is_empty() {
            frames += count_transmissions(&out);
            bytes += out.len();
            first.get_or_insert_with(Instant::now);
        }
        thread::sleep(next_timeout(&app, &pending));
    }
    let player = app.video.as_ref().unwrap();
    let shown = first.map_or(Duration::ZERO, |first| first.elapsed());
    report(&format!(
        "terminal player realtime {}x{} in {}x{} cells: {frames} frames in {}, {} dropped, \
         {:.1} fps on the screen, position {:.1} s, {bytes} bytes of sequences",
        limit.0,
        limit.1,
        fullscreen_rect().width,
        fullscreen_rect().height,
        millis(shown),
        player.dropped,
        if shown.is_zero() {
            0.0
        } else {
            (frames.saturating_sub(1)) as f64 / shown.as_secs_f64()
        },
        player.position(),
    ));
    report_video_frame_write(&app, frames, file_bytes);
}

/// The bytes per frame of the realtime run and the time of one frame write.
fn report_video_frame_write(app: &App, frames: usize, file_bytes: u64) {
    const RUNS: usize = 20;

    let mut times = Vec::new();
    for _ in 0..RUNS {
        let mut out = Vec::new();
        let start = Instant::now();
        crate::kitty_output::render_video_frame_to(app, &mut out, false).unwrap();
        times.push(start.elapsed());
        remove_temp_files(&out);
    }
    report(&format!(
        "terminal player frame: {} bytes in temp files per frame, median {} per frame write",
        file_bytes / u64::try_from(frames.max(1)).unwrap_or(1),
        fine_millis(median(&mut times))
    ));
}

/// The time from Enter on the first video to its first frame write, after cursor rests of 0 ms to 1 s.
#[test]
#[ignore = "reads the folder PHEWR_TEST_DIR"]
fn player_first_frame_after_enter() {
    use crate::config::VideoPlayer;
    use crate::run_loop::LoopState;
    use std::rc::Rc;

    let Some(dir) = test_dir() else {
        return;
    };
    let Some((ffmpeg, decoder)) = video_programs("terminal player enter") else {
        return;
    };
    let decoder: Rc<dyn Decoder> = Rc::new(decoder);
    let mut state = LoopState::new(&crate::config::Config::default());
    state.video.player = VideoPlayer::Terminal;
    state.video.ffmpeg = Some(ffmpeg);
    state.video.decoder = Rc::clone(&decoder);
    let cases = [
        (0, Passes::RunLoop),
        (300, Passes::RunLoop),
        (500, Passes::RunLoop),
        (1000, Passes::RunLoop),
        (300, Passes::Every10Ms),
    ];
    for (rest, passes) in cases {
        if report_enter(&dir, &mut state, Duration::from_millis(rest), passes).is_none() {
            report("terminal player enter: no video in the folder");
            return;
        }
    }
    report_start_and_drop(&dir, &mut state);
}

const ENTER_ROUNDS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Passes {
    /// As an idle grid: the background polls and the wait of `next_timeout`.
    RunLoop,
    /// A pass every 10 ms.
    Every10Ms,
}

fn report_enter(
    dir: &Path,
    state: &mut crate::run_loop::LoopState,
    rest: Duration,
    passes: Passes,
) -> Option<()> {
    let mut firsts = Vec::new();
    let mut starts = Vec::new();
    let mut plays = Vec::new();
    for _ in 0..ENTER_ROUNDS {
        let run = enter_and_play(dir, state, rest, passes)?;
        firsts.push(run.first);
        starts.extend(run.started);
        plays.push(format!("{:.1} fps {} dropped", run.fps, run.dropped));
    }
    let started = if starts.is_empty() {
        "no player start".to_owned()
    } else {
        format!("player start {} into the rest", millis(median(&mut starts)))
    };
    let passes = match passes {
        Passes::RunLoop => "run loop waits",
        Passes::Every10Ms => "a pass every 10 ms",
    };
    report(&format!(
        "terminal player enter after a rest of {} ({passes}): {started}, first frame after {} \
         (median of {ENTER_ROUNDS}), then {}",
        millis(rest),
        millis(median(&mut firsts)),
        plays.join(", ")
    ));
    Some(())
}

/// Report the pass that starts the player (HDR cube on disk, and in a new cache) and the pass that drops it.
fn report_start_and_drop(dir: &Path, state: &mut crate::run_loop::LoopState) {
    let mut warm = Vec::new();
    let mut cold = Vec::new();
    let mut drops = Vec::new();
    for _ in 0..ENTER_ROUNDS {
        let cache = tempfile::tempdir().unwrap();
        for (times, cache) in [(&mut warm, None), (&mut cold, Some(cache.path()))] {
            let rested = rest_on_the_first_video(dir, state, REST, Passes::Every10Ms, cache);
            times.extend(rested.and_then(|rested| rested.start_pass));
        }
        drops.extend(drop_after_a_rest(dir, state));
    }
    report(&format!(
        "terminal player enter: the pass that starts the player takes {} with the cube on \
         disk, {} with the write of the cube; a cursor move drops the player in {} (median of \
         {ENTER_ROUNDS})",
        fine_millis(median(&mut warm)),
        fine_millis(median(&mut cold)),
        fine_millis(median(&mut drops))
    ));
}

const REST: Duration = Duration::from_secs(1);

struct EnterRun {
    /// From the begin of the rest to the start of the player, if it started.
    started: Option<Duration>,
    /// From Enter to the first frame write.
    first: Duration,
    /// The frames on the screen per second after the first one.
    fps: f64,
    dropped: u32,
}

/// Rest the gallery cursor on the first video for `rest`, press Enter and play on for 3 s.
fn enter_and_play(
    dir: &Path,
    state: &mut crate::run_loop::LoopState,
    rest: Duration,
    passes: Passes,
) -> Option<EnterRun> {
    const PLAY: Duration = Duration::from_secs(3);

    let rested = rest_on_the_first_video(dir, state, rest, passes, None)?;
    let mut app = rested.app;
    let mut pending = Vec::new();
    let start = Instant::now();
    app.enter_fullscreen_selected();
    app.image_rect = fullscreen_rect();
    let mut first = None;
    let mut frames = 0;
    while first.is_none_or(|first: Instant| first.elapsed() < PLAY) {
        assert!(start.elapsed() < LIMIT + PLAY, "no frame");
        crate::video_handoff::play_videos(&mut app, state, &mut crate::mpv::SystemRunner);
        run_pass(&mut app, &mut pending);
        let out = app.graphics_out.take_bytes();
        remove_temp_files(&out);
        let written = count_transmissions(&out);
        if written > 0 {
            first.get_or_insert_with(Instant::now);
            frames += written;
        }
        // Until the first frame, a look every millisecond, for the measure.
        let wait = next_timeout(&app, &pending);
        thread::sleep(if first.is_some() {
            wait
        } else {
            wait.min(Duration::from_millis(1))
        });
    }
    let first = first?;
    let shown = first.elapsed();
    Some(EnterRun {
        started: rested.started,
        first: first - start,
        fps: (frames.saturating_sub(1)) as f64 / shown.as_secs_f64(),
        dropped: app.video.as_ref().map_or(0, |player| player.dropped),
    })
}

struct Rested {
    app: App,
    /// From the begin of the rest to the pass that started the player, if one did.
    started: Option<Duration>,
    /// The time of the pass that started the player.
    start_pass: Option<Duration>,
}

/// Rest the cursor on the first video of an idle grid for `rest`. `cache` replaces the cache
/// directory, so the start writes its cube. The grid draws nothing: no thumbnail or loop job runs.
fn rest_on_the_first_video(
    dir: &Path,
    state: &mut crate::run_loop::LoopState,
    rest: Duration,
    passes: Passes,
    cache: Option<&Path>,
) -> Option<Rested> {
    let list = SharedImageList::new();
    scanner::spawn(vec![dir.to_path_buf()], list.clone());
    let mut app = App::new(CELL_PX, list);
    app.graphics = KittyBackend::new(Transfer::TempFile);
    if let Some(cache) = cache {
        app.disk_cache_dir = cache.to_path_buf();
    }
    let deadline = Instant::now() + LIMIT;
    let mut pending = Vec::new();
    while !app.scan_complete || app.has_pending_work() {
        assert!(Instant::now() < deadline, "the scan did not finish");
        poll_background(&mut app, &mut pending);
        thread::sleep(Duration::from_millis(5));
    }
    let index = app.images.iter().position(|path| is_video(path))?;
    state.video.decoder.probe(&app.images[index]).unwrap();
    app.gallery.cursor = app
        .gallery
        .filtered_indices
        .iter()
        .position(|&entry| entry == index)?;
    app.fullscreen_box = fullscreen_rect();
    let mut rested = None;
    let rest_start = Instant::now();
    loop {
        if passes == Passes::RunLoop {
            poll_background(&mut app, &mut pending);
        }
        let pass = Instant::now();
        crate::video_handoff::play_videos(&mut app, state, &mut crate::mpv::SystemRunner);
        if rested.is_none() && app.video_prestart.running() {
            rested = Some((pass - rest_start, pass.elapsed()));
        }
        let Some(left) = rest.checked_sub(rest_start.elapsed()) else {
            break;
        };
        let wait = match passes {
            Passes::RunLoop => next_timeout(&app, &pending),
            Passes::Every10Ms => Duration::from_millis(10),
        };
        // The key press of Enter wakes the run loop at the end of the rest.
        thread::sleep(wait.min(left));
    }
    Some(Rested {
        app,
        started: rested.map(|(started, _)| started),
        start_pass: rested.map(|(_, pass)| pass),
    })
}

/// The pass after a cursor move away from a video: the drop of the player and of its ffmpeg.
fn drop_after_a_rest(dir: &Path, state: &mut crate::run_loop::LoopState) -> Option<Duration> {
    let mut app = rest_on_the_first_video(dir, state, REST, Passes::Every10Ms, None)?.app;
    assert!(app.video_prestart.running(), "the player started");
    let entries = app.gallery.filtered_indices.len();
    if entries < 2 {
        return None;
    }
    app.gallery.cursor = (app.gallery.cursor + 1) % entries;
    let start = Instant::now();
    crate::video_handoff::play_videos(&mut app, state, &mut crate::mpv::SystemRunner);
    let elapsed = start.elapsed();
    assert!(!app.video_prestart.running(), "the move drops the player");
    Some(elapsed)
}

/// The header check and the probe of `Player::open` of each animated file, on the UI thread.
#[test]
#[ignore = "reads the folder PHEWR_TEST_DIR"]
fn animation_open() {
    let Some(dir) = test_dir() else {
        report("animation open: skipped, PHEWR_TEST_DIR is not set");
        return;
    };
    let Some((_, decoder)) = video_programs("animation open") else {
        return;
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| tuilibr::pictures::motion::animation::is_animated(path))
        .collect();
    files.sort();
    if files.is_empty() {
        report("animation open: no animated file in the folder");
    }
    for path in files {
        let size = std::fs::metadata(&path).map_or(0, |meta| meta.len());
        let start = Instant::now();
        assert!(tuilibr::pictures::motion::animation::is_animated(&path));
        let check = start.elapsed();
        let start = Instant::now();
        let info = Decoder::probe(&decoder, &path).unwrap();
        let probe = start.elapsed();
        report(&format!(
            "animation open {}: {} MB, header check {:.2} ms, probe {:.1} ms, {:.1} fps, {:.2} s",
            path.file_name().unwrap_or_default().to_string_lossy(),
            size / 1_000_000,
            check.as_secs_f64() * 1000.0,
            probe.as_secs_f64() * 1000.0,
            info.fps,
            info.duration,
        ));
    }
}

/// The probe and the first-frame thumbnail of each video in the folder, process start included.
#[test]
#[ignore = "reads the folder PHEWR_TEST_DIR"]
fn probe_and_first_frame() {
    let Some(dir) = test_dir() else {
        report("video probe: skipped, PHEWR_TEST_DIR is not set");
        return;
    };
    let Some((ffmpeg, decoder)) = video_programs("video probe") else {
        return;
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| is_video(path))
        .collect();
    files.sort();
    let target =
        tuilibr::pictures::resize::padding::thumb_target_px(Rect::new(0, 0, 40, 9), CELL_PX);
    let cache = tempfile::tempdir().unwrap();
    let cube_dirs = [cache.path()];
    let wait = Duration::from_secs(10);
    let ffprobe = crate::yazi::find_on_path("ffprobe", std::env::var_os("PATH").as_deref());
    let Some(ffprobe) = ffprobe else {
        return;
    };
    for path in files {
        // The probe without the cache of the session, as at the first open of a video.
        let start = Instant::now();
        let info = tuilibr::pictures::motion::probe::probe(&ffprobe, &path).unwrap();
        let probe = start.elapsed();
        // The thumbnail job: the probe misses the cache and fills it.
        let start = Instant::now();
        let frame = tuilibr::pictures::motion::first_frame::decode_first_frame(
            &decoder, &ffmpeg, &path, target, &cube_dirs, wait,
        )
        .unwrap();
        let first = start.elapsed();
        // The probe of `Player::open` after the thumbnail: a cache hit.
        let start = Instant::now();
        Decoder::probe(&decoder, &path).unwrap();
        let cached = start.elapsed();
        report(&format!(
            "video {}: probe {:.1} ms, first frame {:.1} ms (probe included), probe after \
             the thumbnail {:.2} ms, {}x{} hdr {:?} -> {:?}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            probe.as_secs_f64() * 1000.0,
            first.as_secs_f64() * 1000.0,
            cached.as_secs_f64() * 1000.0,
            info.width,
            info.height,
            info.hdr,
            frame.dimensions(),
        ));
    }
}
