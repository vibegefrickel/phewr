//! The grid loops of the video, animated and Live cells in the grid pass. The loop player of
//! `tuilibr::pictures::kitty::loops::player` keeps the grid order, the budget and the frames of
//! each tick. The frame engine swaps the placements.

use super::grid::{GridPass, grid_placement, visible_still_bytes, write_grid_to};
use crate::app::App;
use std::io::{self, Write};
use std::time::Instant;
use tuilibr::pictures::kitty::screen::{Placements, Update};

/// Put the loop frame of `img_idx` at the tick of the frame into `frame` when the terminal
/// stores every frame of the loop, and keep the other frames. Returns `None` when the cell shows
/// its still, else whether the loop started now.
pub(super) fn place_loop_frame(
    app: &mut App,
    frame: &mut Placements,
    vis_idx: usize,
    img_idx: usize,
) -> Option<bool> {
    let gallery = &app.gallery;
    app.grid_loops
        .player
        .place(&img_idx, &app.screen, frame, |cells| {
            grid_placement(gallery, vis_idx, cells)
        })
}

/// Write the new loops at `now`, then the due frames. Returns `true` when a cell started its
/// loop and the draw drops its marker. The visible cells whose still waits come back in
/// `pending`.
pub(crate) fn play_grid_loops(
    app: &mut App,
    now: Instant,
    pending: &mut Vec<usize>,
) -> io::Result<bool> {
    if !app.loops_to_send() && !app.loop_frames_due(now) {
        return Ok(false);
    }
    let mut started = false;
    app.with_graphics_out(|app, out| {
        let pass = play_grid_loops_to(app, out, now)?;
        started = pass.started;
        *pending = pass.waiting;
        Ok(())
    })?;
    Ok(started)
}

/// [`play_grid_loops`] in one synchronized update.
pub(crate) fn play_grid_loops_to(
    app: &mut App,
    out: &mut impl Write,
    now: Instant,
) -> io::Result<GridPass> {
    if !app.grid_loops_shown() {
        return Ok(GridPass {
            waiting: Vec::new(),
            started: false,
        });
    }
    let mut update = Update::new(out);
    let pass = write_grid_to(app, &mut update, now, Vec::new(), true)?;
    update.finish()?;
    Ok(pass)
}

/// Send the waiting loops of the visible cells in grid order, as frames without a placement
/// (`a=t`) or as one Kitty animation, and keep the ids of every loop in `frame`. The frame of the
/// pass then puts the loop in place of the still.
///
/// A pass sends up to `EMIT_PASS_BYTES`. A loop that does not fit the terminal budget goes, and
/// its cell keeps the still.
pub(super) fn send_grid_loops_to<W: Write + ?Sized>(
    app: &mut App,
    update: &mut Update<'_, W>,
    frame: &mut Placements,
) -> io::Result<()> {
    // The bytes of the visible stills, counted only when a loop can go out.
    let kept_bytes = if app.grid_loops.player.needs_send() {
        visible_still_bytes(app)
    } else {
        0
    };
    app.grid_loops
        .player
        .send_frames(update, &mut app.screen, app.graphics, frame, kept_bytes)
}

#[cfg(test)]
mod tests;
