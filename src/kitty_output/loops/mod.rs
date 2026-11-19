//! The grid loops of the video and animated cells: the transfer of the frames or of the Kitty
//! animation, and the loop frame that a cell shows at a tick. The frame engine swaps the
//! placements.

use super::grid::{GridPass, grid_placement, visible_image_ids, write_grid_to};
use crate::app::App;
use std::io::{self, Write};
use std::time::Instant;
use tuilibr::pictures::kitty;
use tuilibr::pictures::kitty::EMIT_PASS_BYTES;
use tuilibr::pictures::kitty::protocol;
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
    for &id in &cell_loop.ids {
        frame.keep(id);
    }
    // A running loop needs only its frame of this tick. A lost frame breaks the loop: the cell
    // shows its still, and the loop goes out again.
    let started = cell_loop.placed.is_none();
    let stored = if started {
        cell_loop.ids.iter().all(|&id| app.screen.stores(id))
    } else {
        app.screen.stores(cell_loop.id_at(tick))
    };
    if !shown || !stored {
        cell_loop.placed = None;
        return None;
    }
    cell_loop.start_at(tick);
    let id = cell_loop.id_at(tick);
    cell_loop.placed = Some(id);
    frame.place(id, grid_placement(&app.gallery, vis_idx, cell_loop.cells()));
    Some(started)
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
    if cell_loop.animation {
        return transmit_animation_to(app, out, img_idx);
    }
    for (frame, &id) in cell_loop.frames.iter().zip(&cell_loop.ids) {
        if app.screen.stores(id) {
            continue;
        }
        app.graphics.transmit(
            out,
            (&frame.image).into(),
            &kitty::DisplayOptions {
                id: Some(id),
                placement: None,
                cell_box: None,
                columns: None,
                rows: None,
                below_backgrounds: false,
                transmit_only: true,
                transient: true,
            },
        )?;
        app.screen.record(id, frame.image.as_raw().len());
    }
    Ok(())
}

/// Send the loop of `img_idx` as one Kitty animation under one id: the root image (`a=t`), each
/// further frame with its gap (`a=f`), then the endless run (`a=a`).
fn transmit_animation_to(app: &mut App, out: &mut impl Write, img_idx: usize) -> io::Result<()> {
    let Some(cell_loop) = app.grid_loops.loops.get(&img_idx) else {
        return Ok(());
    };
    let (Some(&id), Some(root)) = (cell_loop.ids.first(), cell_loop.frames.first()) else {
        return Ok(());
    };
    app.graphics.transmit(
        out,
        (&root.image).into(),
        &kitty::DisplayOptions {
            id: Some(id),
            placement: None,
            cell_box: None,
            columns: None,
            rows: None,
            below_backgrounds: false,
            transmit_only: true,
            transient: true,
        },
    )?;
    for (frame, picture) in cell_loop.frames.iter().enumerate().skip(1) {
        app.graphics
            .transmit_frame(out, (&picture.image).into(), id, cell_loop.gap_ms(frame))?;
    }
    protocol::animate_to(out, id, cell_loop.gap_ms(0))?;
    app.screen.record(id, cell_loop.bytes());
    Ok(())
}

#[cfg(test)]
mod tests;
