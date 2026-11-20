//! The start of the terminal player while the gallery cursor rests on a file that plays.
//!
//! After the gallery cursor rests on a video, an animation or a Live Photo cell for
//! `PRESTART_DELAY` and the grid has no work, the cell's player starts as a loop. A Live Photo
//! cell starts the player of its motion. Enter then takes it with its first frame ready
//! ([`take_prestart`]). The clock of a run starts at its first frame. The clip plays in real
//! time from Enter.
//!
//! At most one such player exists. It belongs to the file of the rest and to the frame box. A
//! cursor move, a step, a view change, an overlay, a slideshow and an mpv window drop it. A
//! player that waits `PRESTART_LIFETIME` for Enter ends. A file whose probe or start fails
//! gets no second try until the rest moves.
//!
//! The UI thread runs no ffprobe. Without an entry in the probe cache, a thread probes the file
//! and writes its cube, and the start waits for it.

use super::terminal_player_program;
use crate::app::{App, ViewMode};
use crate::player::Player;
use crate::run_loop::LoopState;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, TryRecvError};
use std::time::{Duration, Instant};
use tuilibr::pictures::motion::first_frame::cube_dirs;
use tuilibr::pictures::motion::frame_box::frame_limit;

/// A key repeat moves the cursor every 30 ms to 50 ms. A scroll through the grid starts no
/// ffmpeg.
const PRESTART_DELAY: Duration = Duration::from_millis(200);
/// An idle phewr on a video cell then holds no ffmpeg, no `VideoToolbox` session and no frame
/// buffers.
const PRESTART_LIFETIME: Duration = Duration::from_secs(10);

/// The file of the player that a rest starts, and the time when the rest began. The player
/// loops, as the player of Enter outside a slideshow.
struct Rest {
    path: PathBuf,
    since: Instant,
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
}

impl VideoPrestart {
    #[cfg(test)]
    pub(crate) const fn running(&self) -> bool {
        self.player.is_some()
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
        || !(tuilibr::pictures::motion::first_frame::is_video(&rest.path)
            || tuilibr::pictures::motion::animation::has_animation_extension(&rest.path))
    {
        return None;
    }
    let due = rest.since + PRESTART_DELAY;
    Some(due.saturating_duration_since(now).max(HELD_POLL))
}

/// The file under the gallery cursor, while no overlay, no slideshow and no mpv window is on.
/// A Live Photo cell gives its motion: the player of Enter plays the motion.
fn rest_target(app: &App) -> Option<&Path> {
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
    app.live_motion(index)
        .or_else(|| app.images.get(index).map(PathBuf::as_path))
}

/// Follow the rest. After `PRESTART_DELAY` on a file that plays, and while the app has no work,
/// the player starts.
pub(crate) fn tick_prestart(app: &mut App, state: &LoopState, now: Instant) {
    let target = rest_target(app);
    if app
        .video_prestart
        .rest
        .as_ref()
        .map(|rest| rest.path.as_path())
        != target
    {
        let rest = target.map(|path| Rest {
            path: path.to_path_buf(),
            since: now,
        });
        app.video_prestart = VideoPrestart {
            rest,
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
    let path = rest.path.clone();
    if !plays_in_terminal(app, state) {
        app.video_prestart.tried = Some(limit);
        return;
    }
    let Some(ready) = probe_ready(app, state, &path) else {
        return;
    };
    app.video_prestart.tried = Some(limit);
    if ready && let Some(player) = start_player(app, state, &path, limit) {
        app.video_prestart.player = Some((player, now));
    }
}

/// A grid cell plays only as a video, an animation or a Live Photo, and only the terminal
/// player starts ahead.
fn plays_in_terminal(app: &App, state: &LoopState) -> bool {
    app.gallery
        .selected_index()
        .is_some_and(|index| app.opens_in_player(index))
        && terminal_player_program(app, state).is_ok()
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
fn start_player(app: &App, state: &LoopState, path: &Path, limit: (u32, u32)) -> Option<Player> {
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

/// The prestarted loop for `path` at `limit`, with its clock from `now`. `None` for another
/// file or a new window size.
pub(crate) fn take_prestart(
    app: &mut App,
    path: &Path,
    limit: (u32, u32),
    now: Instant,
) -> Option<Player> {
    let prestart = &mut app.video_prestart;
    if prestart.rest.as_ref().map(|rest| rest.path.as_path()) != Some(path) {
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
