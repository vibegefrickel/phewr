//! The start of a video: the terminal player, or the mpv window, and the way back.

mod prestart;
mod window;

use crate::app::{App, ViewMode};
use crate::config::{Config, VideoPlayer};
use crate::mpv;
use crate::pictures::kitty::Transfer;
use crate::player::{Decoder, Player, SystemDecoder, frame_limit};
use crate::run_loop::LoopState;
use crate::video::cube_dirs;
use crate::yazi::find_on_path;
pub(crate) use prestart::{VideoPrestart, prestart_timeout};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;
pub(crate) use window::poll_mpv_window;

const MPV_MISSING_NOTE: &str = "mpv not found on PATH";

/// The video player of the session, after the PATH lookups for mpv and ffmpeg.
pub(crate) struct VideoState {
    pub(crate) player: VideoPlayer,
    pub(crate) mpv: Option<PathBuf>,
    pub(crate) ffmpeg: Option<PathBuf>,
    /// Holds the ffprobe program. The tests put a fake here.
    pub(crate) decoder: Rc<dyn Decoder>,
}

impl VideoState {
    /// Look up mpv once at the start. A missing mpv then costs no video start.
    pub(crate) fn new(config: &Config) -> Self {
        let tools = crate::video::session_tools();
        Self {
            player: config.video_player,
            mpv: find_on_path("mpv", std::env::var_os("PATH").as_deref()),
            ffmpeg: tools.ffmpeg.clone(),
            decoder: Rc::new(SystemDecoder::new(tools.ffprobe.clone())),
        }
    }
}

/// Start the player for the current video, when the view waits for one.
///
/// The terminal player needs the temp file medium, ffmpeg and ffprobe. Without them, the
/// video goes to the mpv window with a note. A rest of the gallery cursor on a video, and a
/// rest of the fullscreen view on a Live Photo still, start the player ahead of Enter, see
/// [`prestart`].
pub(crate) fn play_videos(app: &mut App, state: &mut LoopState, runner: &mut impl mpv::Runner) {
    finish_motion(app, state);
    start_video_view(app, state, runner);
    prestart::tick_prestart(app, state, Instant::now());
}

fn start_video_view(app: &mut App, state: &mut LoopState, runner: &mut impl mpv::Runner) {
    // The player view waits for the mpv window of its video with the last frame.
    if app.mode != ViewMode::Video || app.video.is_some() || app.watches_video_window() {
        return;
    }
    if app.plays_motion() {
        app.video_prestart
            .mark_played(app.current_path().to_path_buf());
    }
    let mismatch = identifier_note(app);
    if app.take_video_window_request() {
        let keep_player =
            app.take_window_keeps_player() && terminal_player_program(app, state).is_ok();
        open_window(app, state, runner, keep_player, None);
    } else {
        // The note of a failed motion goes to `state`, so the program path is a copy.
        match terminal_player_program(app, state).map(Path::to_path_buf) {
            Ok(program) => open_terminal_player(app, state, &program),
            Err(Blocker::Configured) => open_window(app, state, runner, false, None),
            Err(Blocker::Missing(reason)) => open_window(app, state, runner, false, Some(reason)),
        }
    }
    if let Some(note) = mismatch {
        state.status = Some(match state.status.take() {
            Some(other) => format!("{note}; {other}"),
            None => note,
        });
    }
}

/// The note for a Live Photo pair whose content identifiers differ. The motion plays anyway:
/// the base name paired it. The read of both files costs less than 1 ms.
fn identifier_note(app: &App) -> Option<String> {
    if !app.plays_motion() {
        return None;
    }
    let (still, motion) = (app.current_path(), app.video_path());
    crate::scanner::identifiers_differ(still, motion).then(|| {
        format!(
            "{} and {} carry different Live Photo identifiers",
            mpv::file_name(still),
            mpv::file_name(motion)
        )
    })
}

/// Go back to the still after the motion of a Live Photo. A failed decode leaves its note.
fn finish_motion(app: &mut App, state: &mut LoopState) {
    if app.mode != ViewMode::Video || !app.plays_motion() {
        return;
    }
    let Some(player) = app.video.as_ref() else {
        return;
    };
    if !player.finished() {
        return;
    }
    if let Some(error) = player.error.clone() {
        state.status = Some(error);
    }
    // A running slideshow moves on from the finished motion, see `slideshow_video_holds`.
    if !app.slideshow_advancing() {
        app.return_from_video();
    }
}

enum Blocker {
    /// The configuration asks for the window. No note.
    Configured,
    /// The note names the missing part.
    Missing(&'static str),
}

fn terminal_player_program<'a>(app: &App, state: &'a LoopState) -> Result<&'a Path, Blocker> {
    if state.video.player == VideoPlayer::Window {
        return Err(Blocker::Configured);
    }
    if app.graphics.transfer() != Transfer::TempFile {
        return Err(Blocker::Missing("the terminal has no temp file medium"));
    }
    let Some(program) = state.video.ffmpeg.as_deref() else {
        return Err(Blocker::Missing("ffmpeg not found on PATH"));
    };
    if !state.video.decoder.has_ffprobe() {
        return Err(Blocker::Missing("ffprobe not found on PATH"));
    }
    Ok(program)
}

/// Open the current video in the terminal player, or take its prestarted player. A failed
/// open shows its error in the fullscreen view, where Enter does not retry it. The motion of a
/// Live Photo plays one time, and a failed open goes back to the still with a note.
fn open_terminal_player(app: &mut App, state: &mut LoopState, program: &Path) {
    let path = app.video_path().to_path_buf();
    let repeat = !app.slideshow_advancing() && !app.plays_motion();
    let limit = frame_limit(app.fullscreen_box, app.cell_px);
    // A grid cell prestarts a loop, a Live Photo still a motion that plays once. A slideshow
    // run plays once and takes no prestart.
    let prestarted = if app.slideshow_advancing() {
        None
    } else {
        prestart::take_prestart(app, &path, repeat, limit, Instant::now())
    };
    // A new run ends the prestarted ffmpeg first: two waiting ffmpeg runs of a 4K HLG clip hold
    // about 900 MB.
    app.video_prestart.drop_player();
    let started = match prestarted {
        Some(player) => Ok(player),
        None => Player::open(
            Rc::clone(&state.video.decoder),
            program,
            &path,
            limit,
            &cube_dirs(&app.disk_cache_dir).paths(),
            repeat,
        ),
    };
    match started {
        Ok(mut player) => {
            // A slideshow holds an animation for its interval. A short loop then does not flash.
            if let Some(interval) = app.slideshow_interval_running()
                && player.is_animation()
            {
                player.play_at_least(interval);
            }
            app.video = Some(player);
        }
        Err(error) if app.plays_motion() => {
            state.status = Some(error.to_string());
            app.return_from_video();
        }
        Err(error) => {
            app.error = Some(error.to_string());
            app.mode = ViewMode::Fullscreen;
        }
    }
}

/// Open the current video in the mpv window, and show the view behind it
/// ([`App::leave_for_window`]).
///
/// `reason` is the note of a fallback from the terminal player. A running slideshow plays
/// the video once and waits for the window. Each other playback loops until the user quits.
/// Without the window, the player view plays the video again, and the grid stops the slideshow.
fn open_window(
    app: &mut App,
    state: &mut LoopState,
    runner: &mut impl mpv::Runner,
    keep_player: bool,
    reason: Option<&str>,
) {
    let path = app.video_path().to_path_buf();
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
        leave_without_window(app, keep_player);
        state.status = Some(format!("{prefix}{MPV_MISSING_NOTE}"));
        return;
    };
    // Only a slideshow needs the end reason: the socket tells the quit of the app from the end.
    let socket = once
        .then(|| mpv::socket_for_window(&app.disk_cache_dir))
        .flatten();
    match runner.spawn(
        program,
        &mpv::window_args(&path, playback, socket.as_deref()),
        socket.as_deref(),
    ) {
        Ok(exit) => {
            leave_for_window(app, keep_player);
            if !once && app.mode != ViewMode::Video {
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
            leave_without_window(app, keep_player);
            state.status = Some(format!("{prefix}mpv did not start ({error})"));
        }
    }
}

/// Show the view behind the mpv window, with the gallery cursor on the video.
fn leave_for_window(app: &mut App, keep_player: bool) {
    app.leave_for_window(keep_player);
    focus_gallery_on_current(app);
}

/// The window did not open: the player view plays on, and the grid stops the slideshow. A Live
/// Photo motion that cannot play leaves the slideshow on its still.
fn leave_without_window(app: &mut App, keep_player: bool) {
    let motion = app.plays_motion();
    leave_for_window(app, keep_player);
    if app.mode != ViewMode::Video && !motion {
        app.stop_slideshow();
    }
}

/// A failed playback, or a step past the list end.
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
mod tests;
