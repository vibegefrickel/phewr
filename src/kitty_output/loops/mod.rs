//! The grid loops of the video and animated cells: the grid order and the budget of the
//! transfers. The per-frame write is in `tuilibr::pictures::kitty::loops`. The frame engine swaps
//! the placements.

use super::grid::{GridPass, grid_placement, visible_image_ids, write_grid_to};
use crate::app::App;
use std::io::{self, Write};
use std::time::Instant;
use tuilibr::pictures::kitty::EMIT_PASS_BYTES;
use tuilibr::pictures::kitty::loops;
use tuilibr::pictures::kitty::screen::{Placements, Update};

/// Put the loop frame of `img_idx` at `tick` into `frame` when the terminal stores every frame
/// of the loop, and keep the other frames. Returns `None` when the cell shows its still, else
/// whether the loop started now.
pub(super) fn place_loop_frame(
    app: &mut App,
    frame: &mut Placements,
    vis_idx: usize,
    img_idx: usize,
    tick: u64,
) -> Option<bool> {
    let shown = app.grid_loops_shown();
    let cell_loop = app.grid_loops.loops.get_mut(&img_idx)?;
    let gallery = &app.gallery;
    loops::place_loop_frame(cell_loop, &app.screen, frame, shown, tick, |cells| {
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
/// (`a=t`) or as one Kitty animation. The frame of the pass then puts the loop in place of the
/// still.
///
/// A pass sends up to `EMIT_PASS_BYTES`. A loop that does not fit the terminal budget goes, and
/// its cell keeps the still.
pub(super) fn send_grid_loops_to<W: Write + ?Sized>(
    app: &mut App,
    update: &mut Update<'_, W>,
) -> io::Result<()> {
    // The bytes of the visible pictures, counted only when a loop goes out.
    let mut kept_bytes: Option<usize> = None;
    let visible: Vec<usize> = app
        .gallery
        .visible_items()
        .map(|(_, index)| index)
        .collect();
    for img_idx in visible {
        let Some(cell_loop) = app.grid_loops.loops.get(&img_idx) else {
            continue;
        };
        if cell_loop.placed.is_some() || cell_loop.ids.iter().all(|&id| app.screen.stores(id)) {
            continue;
        }
        if update.pixel_bytes() >= EMIT_PASS_BYTES {
            continue;
        }
        let kept = *kept_bytes.get_or_insert_with(|| {
            visible_image_ids(app)
                .iter()
                .map(|&id| app.screen.bytes_of(id))
                .sum()
        });
        let bytes = cell_loop.bytes();
        if kept + bytes > app.screen.budget() {
            app.grid_loops.drop_over_budget(img_idx);
            continue;
        }
        transmit_loop_to(app, update, img_idx)?;
        update.add_pixel_bytes(bytes);
        kept_bytes = Some(kept + bytes);
    }
    Ok(())
}

/// Send each loop frame of `img_idx` that the terminal does not store, without a placement.
fn transmit_loop_to(app: &mut App, out: &mut impl Write, img_idx: usize) -> io::Result<()> {
    let Some(cell_loop) = app.grid_loops.loops.get(&img_idx) else {
        return Ok(());
    };
    loops::transmit_loop_to(app.graphics, &mut app.screen, out, cell_loop)
}

#[cfg(test)]
mod tests;
