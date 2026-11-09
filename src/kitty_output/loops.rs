//! The grid loops of the video and animated cells: the frames or the Kitty animation, the
//! placement and the swaps on the ticks.

use super::grid::{EMIT_PASS_BYTES, place_at, visible_image_ids};
use super::terminal_images::trim_to_budget;
use crate::app::App;
use crate::encoder;
use crate::encoder::kitty;
use crossterm::queue;
use std::io::{self, Write};
use std::time::Instant;

/// Place the loop frame of `img_idx` at `tick` when the terminal stores every frame. Returns
/// `false` when the cell shows its still.
pub(super) fn place_loop_frame_to(
    app: &mut App,
    out: &mut impl Write,
    vis_idx: usize,
    img_idx: usize,
    tick: u64,
) -> io::Result<bool> {
    let shown = app.grid_loops_shown();
    let Some(cell_loop) = app.grid_loops.loops.get_mut(&img_idx) else {
        return Ok(false);
    };
    if !shown
        || !cell_loop
            .ids
            .iter()
            .all(|&id| app.terminal_images.contains(id))
    {
        return Ok(false);
    }
    cell_loop.start_at(tick);
    let id = cell_loop.id_at(tick);
    cell_loop.placed = Some(id);
    for &frame in &cell_loop.ids {
        app.terminal_images.touch(frame);
    }
    let cells = cell_loop.cells();
    place_at(app, out, vis_idx, cells, id)?;
    Ok(true)
}

/// Write the new loops at `now`, then the due frames. Returns `true` when a cell started its
/// loop and the draw drops its marker.
pub(crate) fn play_grid_loops(app: &mut App, now: Instant) -> io::Result<bool> {
    if !app.loops_to_send() && !app.loop_frames_due(now) {
        return Ok(false);
    }
    let mut started = false;
    app.with_graphics_out(|app, out| {
        started = play_grid_loops_to(app, out, now)?;
        Ok(())
    })?;
    Ok(started)
}

/// [`play_grid_loops`] in one synchronized update.
pub(crate) fn play_grid_loops_to(
    app: &mut App,
    out: &mut impl Write,
    now: Instant,
) -> io::Result<bool> {
    use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};

    if !app.grid_loops_shown() {
        return Ok(false);
    }
    let tick = app.grid_loops.tick_at(now);
    queue!(out, BeginSynchronizedUpdate)?;
    let started = start_grid_loops_to(app, out, tick)?;
    swap_loop_frames_to(app, out, tick)?;
    queue!(out, EndSynchronizedUpdate)?;
    out.flush()?;
    Ok(started)
}

/// Send the waiting loops of the visible cells in grid order, as frames without a placement
/// (`a=t`) or as one Kitty animation. Then the loop takes the place of the still.
///
/// A pass sends up to `EMIT_PASS_BYTES`. A loop that does not fit the terminal budget goes, and
/// its cell keeps the still.
fn start_grid_loops_to(app: &mut App, out: &mut impl Write, tick: u64) -> io::Result<bool> {
    let mut kept = visible_image_ids(app);
    let mut kept_bytes: usize = kept
        .iter()
        .map(|&id| app.terminal_images.bytes_of(id))
        .sum();
    let mut written = 0;
    let mut started = false;
    let visible: Vec<(usize, usize)> = app.gallery.visible_items().collect();
    for (vis_idx, img_idx) in visible {
        let Some(cell_loop) = app.grid_loops.loops.get(&img_idx) else {
            continue;
        };
        if cell_loop.placed.is_some() {
            continue;
        }
        if !cell_loop
            .ids
            .iter()
            .all(|&id| app.terminal_images.contains(id))
        {
            if written >= EMIT_PASS_BYTES {
                continue;
            }
            let bytes = cell_loop.bytes();
            if kept_bytes + bytes > app.terminal_images.budget {
                app.grid_loops.drop_over_budget(img_idx);
                continue;
            }
            transmit_loop_to(app, out, img_idx)?;
            written += bytes;
            kept_bytes += bytes;
        }
        if let Some((_, still)) = app.thumb_cache.peek(img_idx) {
            kitty::delete_placements_to(out, still)?;
        }
        kept.extend(start_loop_to(app, out, vis_idx, img_idx, tick)?);
        started = true;
    }
    trim_to_budget(app, out, &kept)?;
    Ok(started)
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
        if app.terminal_images.contains(id) {
            continue;
        }
        app.graphics.transmit(
            out,
            (&frame.image).into(),
            &encoder::DisplayOptions {
                id: Some(id),
                cell_box: None,
                columns: None,
                rows: None,
                below_backgrounds: false,
                transmit_only: true,
                transient: true,
            },
        )?;
        app.terminal_images.insert(id, frame.image.as_raw().len());
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
        &encoder::DisplayOptions {
            id: Some(id),
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
    kitty::animate_to(out, id, cell_loop.gap_ms(0))?;
    app.terminal_images.insert(id, cell_loop.bytes());
    Ok(())
}

/// Place the loop frame of `img_idx` at `tick` and start the loop clock. Returns the frame ids.
fn start_loop_to(
    app: &mut App,
    out: &mut impl Write,
    vis_idx: usize,
    img_idx: usize,
    tick: u64,
) -> io::Result<Vec<u32>> {
    let Some(cell_loop) = app.grid_loops.loops.get_mut(&img_idx) else {
        return Ok(Vec::new());
    };
    cell_loop.start_at(tick);
    let id = cell_loop.id_at(tick);
    cell_loop.placed = Some(id);
    let (cells, ids) = (cell_loop.cells(), cell_loop.ids.clone());
    place_at(app, out, vis_idx, cells, id)?;
    Ok(ids)
}

/// Swap each placed loop whose next frame is due at `tick`: delete the old placement (`d=i`,
/// the pixels stay) and place the new frame by its id.
fn swap_loop_frames_to(app: &mut App, out: &mut impl Write, tick: u64) -> io::Result<()> {
    let mut swaps = Vec::new();
    for (vis_idx, img_idx) in app.gallery.visible_items() {
        let Some(cell_loop) = app.grid_loops.loops.get(&img_idx) else {
            continue;
        };
        let Some(old) = cell_loop.placed else {
            continue;
        };
        let new = cell_loop.id_at(tick);
        if new != old {
            swaps.push((vis_idx, img_idx, old, new, cell_loop.cells()));
        }
    }
    for (vis_idx, img_idx, old, new, cells) in swaps {
        kitty::delete_placements_to(out, old)?;
        place_at(app, out, vis_idx, cells, new)?;
        if let Some(cell_loop) = app.grid_loops.loops.get_mut(&img_idx) {
            cell_loop.placed = Some(new);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
