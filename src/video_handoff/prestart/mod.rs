//! The start of the terminal player while the gallery cursor rests on a video.
//!
//! After the cursor rests on a video cell for `PRESTART_DELAY` and the grid has no work, the
//! cell's player starts. Enter then takes it with its first frame ready ([`take_prestart`]).
//! The clock of a run starts at its first frame. The clip plays in real time from Enter.
//!
//! At most one such player exists. It belongs to the file under the cursor and to the frame
//! box. A cursor move, a view change, an overlay, a slideshow and an mpv window drop it. A
//! player that waits `PRESTART_LIFETIME` for Enter ends. A file whose probe or start fails
//! gets no second try until the cursor leaves.

use super::terminal_player_program;
use crate::app::{App, ViewMode};
use crate::player::{Player, frame_limit};
use crate::run_loop::LoopState;
use crate::video::cube_dirs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// A key repeat moves the cursor every 30 ms to 50 ms. A scroll through the grid starts no
/// ffmpeg.
const PRESTART_DELAY: Duration = Duration::from_millis(200);
/// An idle viewr on a video cell then holds no ffmpeg, no `VideoToolbox` session and no frame
/// buffers.
const PRESTART_LIFETIME: Duration = Duration::from_secs(10);

/// The player that starts ahead of Enter, and the cursor rest that it belongs to.
#[derive(Default)]
pub(crate) struct VideoPrestart {
    /// The file under the cursor and the time when the cursor came to it.
    rest: Option<(PathBuf, Instant)>,
    /// The frame limit of the last start, a failed one too.
    tried: Option<(u32, u32)>,
    player: Option<(Player, Instant)>,
}

impl VideoPrestart {
    #[cfg(test)]
    pub(crate) fn running(&self) -> bool {
        self.player.is_some()
    }

    /// A run that plays once ends the prestarted ffmpeg before its own start.
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

/// The wait until the player of the rest can start. `None` without a rest on a video or an
/// animation, and after the start ran.
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

/// The file under the gallery cursor, while no overlay, no slideshow and no mpv window is on.
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

/// Follow the cursor rest. After `PRESTART_DELAY` on a video, and while the grid has no work,
/// the player starts.
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

/// A failed probe or start leaves the cell to Enter. Enter shows the error.
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

/// The prestarted player for `path` at `limit`, with its clock from `now`. `None` for another
/// file or a new window size.
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
mod tests;
