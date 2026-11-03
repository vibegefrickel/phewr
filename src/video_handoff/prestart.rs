//! The start of the terminal player while the gallery cursor rests on a video.
//!
//! The start of ffmpeg, the `VideoToolbox` setup and, for HDR, the cube parse take about 150 ms
//! to 300 ms before the first frame. After the cursor rests on a video cell for
//! `PRESTART_DELAY` and the grid has no work, the player of that cell starts. Enter then takes
//! the running player with its first frame ready ([`take_prestart`]). The clock of a run starts
//! at its first frame, so the clip plays in real time from Enter, and nothing drops.
//!
//! [`super::play_videos`] looks at the rest in each pass of the run loop. The run loop does not
//! wake up at the end of the delay: on an idle grid, it waits 250 ms after the status-row read
//! of the file, so the player starts about 270 ms into the rest. ffmpeg then needs about 180 ms
//! for its first frame, so the first frame shows 1 ms after Enter after a rest of about 450 ms.
//! After a rest of 300 ms, the gain is small.
//!
//! At most one such player exists. It belongs to the file under the cursor and to the frame
//! box. A cursor move, a view change, the help popup, a slideshow and an mpv window drop it. A
//! new frame box (a resize, the info panel) drops it and starts it again for the new box. A
//! player that waits `PRESTART_LIFETIME` for Enter ends, because it holds an ffmpeg process
//! with its `VideoToolbox` session (452 MB resident for a 4K HLG clip, at 0 % CPU) and up to 3
//! frame buffers (19 MB at 1920x1080). The drop kills ffmpeg, and a thread reaps it, so the
//! pass does not wait for the exit. ffmpeg blocks on the full pipe after its first frames, so a
//! long rest costs no decode work. A file whose probe or start fails gets no second try until
//! the cursor leaves.
//!
//! The pass that starts the player takes about 1 ms on the UI thread: the probe from the
//! cache, the cube lookup and the spawn of ffmpeg. When the HDR cube is not on disk yet, the
//! pass writes it and takes about 23 ms. The first-frame thumbnail of an HDR video writes the
//! same cube, so this happens only for the first HDR video of a new cache directory.

use super::terminal_player_program;
use crate::app::{App, ViewMode};
use crate::player::{Player, frame_limit};
use crate::run_loop::LoopState;
use crate::video::cube_dirs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// The rest of the cursor on a video before its player starts. A key repeat moves the cursor
/// every 30 ms to 50 ms, so a scroll through the grid starts no ffmpeg.
const PRESTART_DELAY: Duration = Duration::from_millis(200);
/// A started player that waits longer than this for Enter ends. An idle viewr on a video cell
/// then holds no ffmpeg, no `VideoToolbox` session and no frame buffers.
const PRESTART_LIFETIME: Duration = Duration::from_secs(10);

/// The player that starts ahead of Enter, and the rest of the cursor that it belongs to.
#[derive(Default)]
pub(crate) struct VideoPrestart {
    /// The file under the gallery cursor and the time when the cursor came to it.
    rest: Option<(PathBuf, Instant)>,
    /// The frame limit of the start for the rest, after the start ran, also a failed one.
    tried: Option<(u32, u32)>,
    /// The player of the rest and the time of its start.
    player: Option<(Player, Instant)>,
}

impl VideoPrestart {
    /// True while a started player waits for Enter.
    #[cfg(test)]
    pub(crate) fn running(&self) -> bool {
        self.player.is_some()
    }

    /// Drop the player when the frame box changed since its start (a resize, the info panel),
    /// so that the next pass starts it for the new box, and when it waited too long.
    fn expire(&mut self, limit: (u32, u32), now: Instant) {
        if self.tried.is_some_and(|tried| tried != limit) {
            self.player = None;
            self.tried = None;
        }
        if self.player.as_ref().is_some_and(|(_, started)| {
            now.saturating_duration_since(*started) >= PRESTART_LIFETIME
        }) {
            self.player = None;
        }
    }
}

/// The floor of the wait for a rest whose start is due but held: the grid has work, and the run
/// loop already looks every 10 ms for it.
const HELD_POLL: Duration = Duration::from_millis(10);

/// How long the run loop can wait before the player of the rest can start, so that the start
/// comes at the end of `PRESTART_DELAY` and not at the next idle look. `None` without a rest on
/// a video or an animation, and after the start of the rest ran.
pub(crate) fn prestart_timeout(app: &App, now: Instant) -> Option<Duration> {
    let prestart = &app.video_prestart;
    let (path, since) = prestart.rest.as_ref()?;
    if prestart.tried.is_some()
        || !(crate::video::is_video(path)
            || crate::player::animation::has_animation_extension(path))
    {
        return None;
    }
    let due = *since + PRESTART_DELAY;
    Some(due.saturating_duration_since(now).max(HELD_POLL))
}

/// The file whose player can start ahead of Enter: the entry under the gallery cursor, while
/// no overlay, no slideshow and no mpv window is on.
fn rest_path(app: &App) -> Option<&Path> {
    if app.mode != ViewMode::Gallery
        || app.help_visible
        || app.slideshow_running()
        || app.watches_video_window()
    {
        return None;
    }
    let index = app.gallery.selected_index()?;
    if app.is_folder(index) {
        return None;
    }
    app.images.get(index).map(PathBuf::as_path)
}

/// Follow the rest of the cursor at `now`. A new rest drops the player of the old one. After
/// `PRESTART_DELAY` on a video, and while the grid has no work, the player starts.
pub(crate) fn tick_prestart(app: &mut App, state: &LoopState, now: Instant) {
    let path = rest_path(app);
    if app
        .video_prestart
        .rest
        .as_ref()
        .map(|(rest, _)| rest.as_path())
        != path
    {
        let rest = path.map(|path| (path.to_path_buf(), now));
        app.video_prestart = VideoPrestart {
            rest,
            ..VideoPrestart::default()
        };
        return;
    }
    let limit = frame_limit(app.fullscreen_box, app.cell_px);
    app.video_prestart.expire(limit, now);
    let prestart = &app.video_prestart;
    let Some((path, since)) = prestart.rest.clone() else {
        return;
    };
    if prestart.tried.is_some()
        || now.saturating_duration_since(since) < PRESTART_DELAY
        || app.has_pending_work()
    {
        return;
    }
    app.video_prestart.tried = Some(limit);
    if let Some(player) = start_player(app, state, &path, limit) {
        app.video_prestart.player = Some((player, now));
    }
}

/// The player of `path` at `limit` for the file under the cursor, when it opens in the
/// terminal player. A failed probe (no video stream) or start leaves the cell to Enter, which
/// shows the error.
fn start_player(app: &App, state: &LoopState, path: &Path, limit: (u32, u32)) -> Option<Player> {
    let index = app.gallery.selected_index()?;
    if !app.opens_in_player(index) {
        return None;
    }
    let program = terminal_player_program(app, state).ok()?;
    Player::open(
        Rc::clone(&state.video.decoder),
        program,
        path,
        limit,
        &cube_dirs(&app.disk_cache_dir).paths(),
        true,
    )
    .ok()
}

/// The player that started ahead of Enter for `path` at `limit`, with its clock from `now`.
/// `None` when the rest was on another file, or the window changed its size since the start.
pub(crate) fn take_prestart(
    app: &mut App,
    path: &Path,
    limit: (u32, u32),
    now: Instant,
) -> Option<Player> {
    let prestart = &mut app.video_prestart;
    if prestart.rest.as_ref().is_none_or(|(rest, _)| rest != path) {
        return None;
    }
    let (mut player, _) = prestart.player.take()?;
    if prestart.tried != Some(limit) {
        return None;
    }
    player.begin_at(now);
    Some(player)
}

#[cfg(test)]
mod tests {
    use super::{
        PRESTART_DELAY, PRESTART_LIFETIME, prestart_timeout, take_prestart, tick_prestart,
    };
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

    /// The frame of the test clip: 640x480 RGB.
    const FRAME: usize = 640 * 480 * 3;

    /// A gallery of a photo and a video with the cursor on the video, after the scan.
    fn gallery_on_video() -> App {
        let mut app = with_temp_file_medium(app_with(&["/videos/a.jpg", "/videos/clip.mp4"]));
        app.scan_complete = true;
        app.gallery.cursor = 1;
        app.fullscreen_box = Rect::new(0, 0, 100, 40);
        app
    }

    /// The rest begins at `start` and the player starts at the end of the delay.
    fn rest_until_started(app: &mut App, decoder: &Rc<FakeDecoder>, start: Instant) -> Instant {
        let state = state_with(VideoPlayer::Terminal, Rc::clone(decoder));
        tick_prestart(app, &state, start);
        let due = start + PRESTART_DELAY;
        tick_prestart(app, &state, due);
        assert!(app.video_prestart.running());
        due
    }

    /// The run loop wakes at the end of the delay of a rest on a video, and not for a photo or
    /// after the start.
    #[test]
    fn the_run_loop_wakes_at_the_end_of_the_rest_delay() {
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
    fn a_rest_on_a_video_starts_its_player_after_the_delay() {
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
    fn a_rest_on_a_photo_or_without_the_terminal_player_starts_nothing() {
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
    fn a_cursor_move_drops_the_player_and_its_ffmpeg() {
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
    fn the_help_popup_a_slideshow_and_an_mpv_window_drop_the_player() {
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
    fn no_player_starts_while_the_grid_has_work() {
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
    fn no_player_starts_while_a_loop_job_of_the_grid_runs() {
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
    fn a_new_frame_box_during_the_rest_starts_the_player_again_for_it() {
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
    fn a_player_that_waits_too_long_for_enter_ends() {
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
    fn a_file_whose_probe_finds_no_video_starts_nothing_and_is_not_probed_again() {
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
    fn enter_takes_the_player_and_its_clock_starts_at_enter() {
        let decoder = fake_decoder(clip_info());
        let mut app = gallery_on_video();
        let rest = Instant::now();
        rest_until_started(&mut app, &decoder, rest);
        // ffmpeg decodes two frames during the rest, which lasts far longer than the deadline
        // of a first frame.
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
    fn a_player_without_a_frame_gets_its_first_frame_deadline_from_enter() {
        let decoder = fake_decoder(clip_info());
        let mut app = gallery_on_video();
        let rest = Instant::now();
        rest_until_started(&mut app, &decoder, rest);
        let path = app.images[1].clone();
        let limit = crate::player::frame_limit(app.fullscreen_box, app.cell_px);
        let mut player =
            take_prestart(&mut app, &path, limit, rest + Duration::from_secs(9)).unwrap();
        // 9 s after the start of ffmpeg, past the first-frame deadline, but 1 s after Enter: the
        // run still waits.
        assert!(!player.poll_at(rest + Duration::from_secs(10)));
        assert!(player.playing());
        assert_eq!(decoder.starts().len(), 1, "no software fallback yet");
    }

    #[test]
    fn enter_after_a_resize_or_on_another_file_starts_a_new_player() {
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
}
