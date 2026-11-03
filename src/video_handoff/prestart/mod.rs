//! The start of the terminal player while the view rests on a video or a Live Photo.
//!
//! After the gallery cursor rests on a video cell for `PRESTART_DELAY` and the grid has no
//! work, the cell's player starts. The fullscreen view of a Live Photo still starts the player
//! of its motion in the same way, as a run that plays once. Enter then takes it with its first
//! frame ready ([`take_prestart`]). The clock of a run starts at its first frame. The clip
//! plays in real time from Enter.
//!
//! At most one such player exists. It belongs to the file of the rest and to the frame box. A
//! cursor move, a step, a view change, an overlay, a slideshow and an mpv window drop it. A
//! player that waits `PRESTART_LIFETIME` for Enter ends. A file whose probe or start fails
//! gets no second try until the rest moves.
//!
//! The UI thread runs no ffprobe. Without an entry in the probe cache, a thread probes the file
//! and writes its cube, and the start waits for it. After a motion played, the rest on its still
//! starts nothing until a step to another file.

use super::terminal_player_program;
use crate::app::{App, ViewMode};
use crate::player::{Player, frame_limit};
use crate::run_loop::LoopState;
use crate::video::cube_dirs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, TryRecvError};
use std::time::{Duration, Instant};

/// A key repeat moves the cursor every 30 ms to 50 ms. A scroll through the grid starts no
/// ffmpeg.
const PRESTART_DELAY: Duration = Duration::from_millis(200);
/// An idle phewr on a video cell then holds no ffmpeg, no `VideoToolbox` session and no frame
/// buffers.
const PRESTART_LIFETIME: Duration = Duration::from_secs(10);

/// The file of the player that a rest starts, and the way it plays.
#[derive(Clone, Copy, PartialEq, Eq)]
struct RestTarget<'a> {
    path: &'a Path,
    /// A video cell of the grid loops. The motion of a Live Photo plays once.
    repeat: bool,
}

/// The file of the rest, the way its player plays and the time when the rest began.
struct Rest {
    path: PathBuf,
    repeat: bool,
    since: Instant,
}

impl Rest {
    fn target(&self) -> RestTarget<'_> {
        RestTarget {
            path: &self.path,
            repeat: self.repeat,
        }
    }
}

/// The player that starts ahead of Enter, and the rest that it belongs to.
#[derive(Default)]
pub(crate) struct VideoPrestart {
    rest: Option<Rest>,
    /// The frame limit of the last start, a failed one too.
    tried: Option<(u32, u32)>,
    player: Option<(Player, Instant)>,
    /// The probe of the rest file on a thread, while the probe cache holds no entry for it.
    preparing: Option<mpsc::Receiver<bool>>,
    /// The Live Photo still whose motion played. Its rest starts no second motion until a step
    /// to another file.
    played: Option<PathBuf>,
}

impl VideoPrestart {
    #[cfg(test)]
    pub(crate) fn running(&self) -> bool {
        self.player.is_some()
    }

    /// True while a rest on a file that can play ahead runs.
    #[cfg(test)]
    pub(crate) fn rests(&self) -> bool {
        self.rest.is_some()
    }

    /// The motion of the still `still` plays. A rest on the still then holds no second ffmpeg
    /// that waits 10 s.
    pub(crate) fn mark_played(&mut self, still: PathBuf) {
        self.played = Some(still);
    }

    /// A new run ends the prestarted ffmpeg before its own start.
    pub(crate) fn drop_player(&mut self) {
        self.player = None;
    }

    /// Drop the player after a new frame box or a long wait. The next pass starts it again for
    /// a new box.
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

/// The grid has work, and the run loop polls every 10 ms anyway.
const HELD_POLL: Duration = Duration::from_millis(10);

/// The wait until the player of the rest can start. `None` without a rest on a video, an
/// animation or a Live Photo motion, and after the start ran.
pub(crate) fn prestart_timeout(app: &App, now: Instant) -> Option<Duration> {
    let prestart = &app.video_prestart;
    let rest = prestart.rest.as_ref()?;
    if prestart.tried.is_some()
        || !(crate::video::is_video(&rest.path)
            || crate::player::animation::has_animation_extension(&rest.path))
    {
        return None;
    }
    let due = rest.since + PRESTART_DELAY;
    Some(due.saturating_duration_since(now).max(HELD_POLL))
}

/// The file under the gallery cursor, or the motion of the Live Photo still in the fullscreen
/// view, while no overlay, no slideshow and no mpv window is on.
fn rest_target(app: &App) -> Option<RestTarget<'_>> {
    if app.help_visible || app.slideshow_running() || app.watches_video_window() {
        return None;
    }
    match app.mode {
        ViewMode::Gallery => {
            let index = app.gallery.selected_index()?;
            if app.is_folder(index) {
                return None;
            }
            let path = app.images.get(index)?;
            Some(RestTarget { path, repeat: true })
        }
        ViewMode::Fullscreen if app.fullscreen_on_live() => {
            if app.video_prestart.played.as_deref()
                == app.images.get(app.current).map(PathBuf::as_path)
            {
                return None;
            }
            let path = app.live_motion(app.current)?;
            Some(RestTarget {
                path,
                repeat: false,
            })
        }
        _ => None,
    }
}

/// Follow the rest. After `PRESTART_DELAY` on a video or a Live Photo still, and while the app
/// has no work, the player starts.
pub(crate) fn tick_prestart(app: &mut App, state: &LoopState, now: Instant) {
    forget_played(app);
    let target = rest_target(app);
    if app.video_prestart.rest.as_ref().map(Rest::target) != target {
        let rest = target.map(|target| Rest {
            path: target.path.to_path_buf(),
            repeat: target.repeat,
            since: now,
        });
        app.video_prestart = VideoPrestart {
            rest,
            played: app.video_prestart.played.take(),
            ..VideoPrestart::default()
        };
        return;
    }
    let limit = frame_limit(app.fullscreen_box, app.cell_px);
    app.video_prestart.expire(limit, now);
    let prestart = &app.video_prestart;
    let Some(rest) = prestart.rest.as_ref() else {
        return;
    };
    if prestart.tried.is_some()
        || now.saturating_duration_since(rest.since) < PRESTART_DELAY
        || app.has_pending_work()
    {
        return;
    }
    let (path, repeat) = (rest.path.clone(), rest.repeat);
    if !plays_in_terminal(app, state, repeat) {
        app.video_prestart.tried = Some(limit);
        return;
    }
    let Some(ready) = probe_ready(app, state, &path) else {
        return;
    };
    app.video_prestart.tried = Some(limit);
    if ready && let Some(player) = start_player(app, state, &path, repeat, limit) {
        app.video_prestart.player = Some((player, now));
    }
}

/// A step to another file, and the grid, end the mark of a played motion.
fn forget_played(app: &mut App) {
    let current = (app.mode != ViewMode::Gallery)
        .then(|| app.images.get(app.current))
        .flatten();
    if app.video_prestart.played.as_ref() != current {
        app.video_prestart.played = None;
    }
}

/// A motion is a `.mov` of the pair. A grid cell plays only as a video or an animation, and
/// only the terminal player starts ahead.
fn plays_in_terminal(app: &App, state: &LoopState, repeat: bool) -> bool {
    let plays = !repeat
        || app
            .gallery
            .selected_index()
            .is_some_and(|index| app.opens_in_player(index));
    plays && terminal_player_program(app, state).is_ok()
}

/// `Some(true)` when the probe cache holds `path`, `None` while a thread probes it. The UI
/// thread runs no ffprobe: a cold probe takes 25 ms to 40 ms, up to the 2 s timeout.
fn probe_ready(app: &mut App, state: &LoopState, path: &Path) -> Option<bool> {
    // The answer of the thread wins: a failed probe gets no second try during the rest.
    if let Some(receiver) = app.video_prestart.preparing.as_ref() {
        return match receiver.try_recv() {
            Ok(ready) => Some(ready),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(false),
        };
    }
    let decoder = &state.video.decoder;
    if decoder.cached_probe(path).is_some() {
        return Some(true);
    }
    let dirs = Vec::from(
        cube_dirs(&app.disk_cache_dir)
            .paths()
            .map(Path::to_path_buf),
    );
    app.video_prestart.preparing = Some(decoder.prepare_on_thread(path, dirs));
    None
}

/// A failed start leaves the file to Enter. Enter shows the error.
fn start_player(
    app: &App,
    state: &LoopState,
    path: &Path,
    repeat: bool,
    limit: (u32, u32),
) -> Option<Player> {
    let program = terminal_player_program(app, state).ok()?;
    Player::open(
        Rc::clone(&state.video.decoder),
        program,
        path,
        limit,
        &cube_dirs(&app.disk_cache_dir).paths(),
        repeat,
    )
    .ok()
}

/// The prestarted player for `path` at `limit` that plays as `repeat` asks, with its clock from
/// `now`. `None` for another file, another way to play or a new window size.
pub(crate) fn take_prestart(
    app: &mut App,
    path: &Path,
    repeat: bool,
    limit: (u32, u32),
    now: Instant,
) -> Option<Player> {
    let prestart = &mut app.video_prestart;
    if prestart.rest.as_ref().map(Rest::target) != Some(RestTarget { path, repeat }) {
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
mod tests;
