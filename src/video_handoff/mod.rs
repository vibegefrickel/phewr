//! The start of a video: the terminal player, or the mpv window, and the way back.

mod window;

use crate::app::{App, ViewMode};
use crate::config::{Config, VideoPlayer};
use crate::encoder::Transfer;
use crate::mpv;
use crate::player::{Decoder, Player, SystemDecoder, frame_limit};
use crate::run_loop::LoopState;
use crate::yazi::find_on_path;
use std::path::{Path, PathBuf};
use std::rc::Rc;
pub(crate) use window::poll_mpv_window;

/// The status note when a video needs mpv and PATH has none.
pub(crate) const MPV_MISSING_NOTE: &str = "mpv not found on PATH";

/// The video player of the session, after the PATH lookups for mpv and ffmpeg.
pub(crate) struct VideoState {
    pub(crate) player: VideoPlayer,
    pub(crate) mpv: Option<PathBuf>,
    pub(crate) ffmpeg: Option<PathBuf>,
    /// Starts ffmpeg for the terminal player. The tests put a fake here.
    pub(crate) decoder: Rc<dyn Decoder>,
}

impl VideoState {
    /// Look up mpv and ffmpeg once at the start, so that a missing program costs no video start.
    pub(crate) fn new(config: &Config) -> Self {
        let path = std::env::var_os("PATH");
        Self {
            player: config.video_player,
            mpv: find_on_path("mpv", path.as_deref()),
            ffmpeg: find_on_path("ffmpeg", path.as_deref()),
            decoder: Rc::new(SystemDecoder),
        }
    }
}

/// Start the player for the current video, when the view waits for one.
///
/// The video view without a player asks for a start. `p` asks for the mpv window. The terminal
/// player needs the temp file medium and ffmpeg. Without them, the video goes to the mpv
/// window with a note. Without mpv, the note stays and the view goes back.
pub(crate) fn play_videos(app: &mut App, state: &mut LoopState, runner: &mut impl mpv::Runner) {
    if app.mode != ViewMode::Video || app.video.is_some() {
        return;
    }
    if app.take_video_window_request() {
        open_window(app, state, runner, None);
        return;
    }
    match terminal_player_blocker(app, state) {
        None => open_terminal_player(app, state),
        Some(Blocker::Configured) => open_window(app, state, runner, None),
        Some(Blocker::Missing(reason)) => open_window(app, state, runner, Some(reason)),
    }
}

/// Why the terminal player cannot play.
enum Blocker {
    /// The configuration asks for the window. No note.
    Configured,
    /// A part is missing. The note tells which.
    Missing(&'static str),
}

/// Why the terminal player cannot play, or `None` when it can.
fn terminal_player_blocker(app: &App, state: &LoopState) -> Option<Blocker> {
    if state.video.player == VideoPlayer::Window {
        return Some(Blocker::Configured);
    }
    if app.graphics.transfer() != Transfer::TempFile {
        return Some(Blocker::Missing("the terminal has no temp file medium"));
    }
    if state.video.ffmpeg.is_none() {
        return Some(Blocker::Missing("ffmpeg not found on PATH"));
    }
    None
}

/// Open the current video in the terminal player. A failed open shows its error in the
/// fullscreen view, where Enter does not retry it.
fn open_terminal_player(app: &mut App, state: &LoopState) {
    let Some(program) = state.video.ffmpeg.as_deref() else {
        return;
    };
    let path = app.current_path().to_path_buf();
    let repeat = !app.slideshow_advancing();
    let temp = std::env::temp_dir().join("viewr");
    let limit = frame_limit(app.fullscreen_box, app.cell_px);
    match Player::open(
        Rc::clone(&state.video.decoder),
        program,
        &path,
        limit,
        &[&app.disk_cache_dir, &temp],
        repeat,
    ) {
        Ok(player) => app.video = Some(player),
        Err(error) => {
            app.error = Some(error.to_string());
            app.mode = ViewMode::Fullscreen;
        }
    }
}

/// Open the current video in the mpv window, and go back to the view that opened it.
///
/// `reason` is the note of a fallback from the terminal player. A running slideshow that is
/// not paused plays the video one time and waits for the window, see
/// [`App::poll_video_window`]. Each other playback loops until the user quits mpv.
fn open_window(
    app: &mut App,
    state: &mut LoopState,
    runner: &mut impl mpv::Runner,
    reason: Option<&str>,
) {
    let path = app.current_path().to_path_buf();
    let name = mpv::file_name(&path);
    let once = app.slideshow_advancing();
    let playback = if once {
        mpv::Playback::Once
    } else {
        mpv::Playback::Loop
    };
    let prefix = reason
        .map(|reason| format!("{reason}: "))
        .unwrap_or_default();
    let Some(program) = state.video.mpv.as_deref() else {
        back_from_video(app);
        app.stop_slideshow();
        state.status = Some(format!("{prefix}{MPV_MISSING_NOTE}"));
        return;
    };
    match runner.spawn(program, &mpv::window_args(&path, playback)) {
        Ok(exit) => {
            back_from_video(app);
            if !once {
                app.stop_slideshow();
            }
            app.watch_video_window(exit, once);
            state.status = Some(if once {
                format!("{prefix}{name} plays in an mpv window, the slideshow waits for its end")
            } else {
                format!("{prefix}{name} plays in an mpv window")
            });
        }
        Err(error) => {
            back_from_video(app);
            app.stop_slideshow();
            state.status = Some(format!("{prefix}mpv did not start ({error})"));
        }
    }
}

/// Return to the view that opened the video, with the gallery cursor on it.
pub(crate) fn back_from_video(app: &mut App) {
    app.return_from_video();
    focus_gallery_on_current(app);
}

/// The status note after an mpv window played `path`: a failed playback, or a step past the
/// list end.
fn mpv_note(app: &App, code: Option<i32>, path: &Path) -> Option<String> {
    let Some(step) = mpv::step(code) else {
        return mpv::exit_note(code, path);
    };
    // A step that moved nothing hit an end of the list.
    (app.current_path() == path).then(|| {
        let end = match step {
            mpv::Step::Next => "last",
            mpv::Step::Previous => "first",
        };
        format!("{} is the {end} file", mpv::file_name(path))
    })
}

/// Put the gallery cursor on the current file, so that the gallery shows where the user was.
pub(crate) fn focus_gallery_on_current(app: &mut App) {
    if let Some(position) = app
        .gallery
        .filtered_indices
        .iter()
        .position(|&index| index == app.current)
    {
        app.gallery.cursor = position;
        app.gallery.ensure_cursor_visible();
    }
}

#[cfg(test)]
mod tests {
    use super::{MPV_MISSING_NOTE, play_videos};
    use crate::app::ViewMode;
    use crate::config::VideoPlayer;
    use crate::encoder::{KittyBackend, Transfer};
    use crate::keys::fullscreen::handle_fullscreen_key;
    use crate::test_support::video::{
        clip_info, fake_decoder, fake_runner, fullscreen_video_app, gallery_app_on_video,
        last_start, slideshow_video_app, starts, state_with, terminal_state, video_app,
        window_state,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn the_terminal_player_starts_ffmpeg_for_the_video_view() {
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
        // 640x480 into 100x40 cells of 8x16 pixels: 800x640 pixels, so the frame is 640x480.
        assert!(args.contains("scale_vt=w=640:h=480,"), "{args}");
        assert_eq!(state.status, None);

        play_videos(&mut app, &mut state, &mut runner);
        assert_eq!(decoder.starts().len(), 1, "a running player starts nothing");
    }

    #[test]
    fn a_slideshow_plays_the_video_once_and_moves_on_at_its_end() {
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
    fn outside_a_slideshow_the_clip_starts_again_at_its_end() {
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
    fn a_paused_slideshow_loops_the_video() {
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
    fn a_video_that_does_not_open_shows_its_error_and_no_play_hint() {
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
    fn the_window_player_spawns_mpv_and_returns_to_the_fullscreen_placeholder() {
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
    fn a_video_from_the_gallery_returns_to_the_gallery_after_the_window_start() {
        let mut app = gallery_app_on_video(&["a.jpg", "clip.mp4", "b.jpg"], 1);
        app.gallery.cursor = 0;
        let mut state = window_state();
        let (_log, mut runner) = fake_runner();

        play_videos(&mut app, &mut state, &mut runner);

        assert_eq!(app.mode, ViewMode::Gallery, "the gallery opened this video");
        assert_eq!(app.gallery.cursor, 1);
    }

    #[test]
    fn without_the_temp_file_medium_the_video_goes_to_the_window_with_a_note() {
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
    fn without_ffmpeg_the_video_goes_to_the_window_with_a_note() {
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
    fn without_ffmpeg_and_mpv_the_view_goes_back_with_a_note() {
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
    fn an_mpv_that_does_not_start_shows_the_error_and_goes_back() {
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
    fn the_window_request_opens_mpv_also_with_the_terminal_player() {
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
    fn enter_on_the_fullscreen_placeholder_plays_the_video_again() {
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
}
