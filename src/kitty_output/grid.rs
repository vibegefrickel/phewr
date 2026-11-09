//! The grid thumbnails: the new ones of each pass, the placements of a redraw and the deletes.

use super::loops::place_loop_frame_to;
use crate::app::App;
use crate::pictures::kitty;
use crate::pictures::kitty::EMIT_PASS_BYTES;
use crate::pictures::kitty::placement::centered;
use crate::pictures::kitty::protocol;
use crate::pictures::kitty::terminal_images::{delete_image_to, trim_to_budget};
use crossterm::{cursor, queue};
use ratatui::prelude::*;
use std::collections::HashSet;
use std::io::{self, Write};
use std::time::Instant;

/// Write the new thumbnails of `pending` that fit one pass, and keep the visible rest.
pub(crate) fn emit_new_thumbnails(app: &mut App, pending: &mut Vec<usize>) -> io::Result<()> {
    app.with_graphics_out(|app, out| {
        *pending = emit_new_thumbnails_to(app, pending, out)?;
        Ok(())
    })
}

/// Write the new thumbnails in grid order, in one synchronized update. After `EMIT_PASS_BYTES`,
/// the visible rest comes back for the next pass.
fn emit_new_thumbnails_to(
    app: &mut App,
    new_indices: &[usize],
    out: &mut impl Write,
) -> io::Result<Vec<usize>> {
    use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};

    queue!(out, BeginSynchronizedUpdate)?;
    delete_evicted_images_to(app, out)?;
    let new_indices: HashSet<usize> = new_indices.iter().copied().collect();
    let shown = visible_image_ids(app);
    let mut written = 0;
    let mut left = Vec::new();
    let mut sent = HashSet::new();
    for (vis_idx, position) in app.gallery.visible_range().enumerate() {
        let img_idx = app.gallery.filtered_indices[position];
        if !new_indices.contains(&img_idx) {
            continue;
        }
        if written >= EMIT_PASS_BYTES {
            left.push(img_idx);
            continue;
        }
        written += transmit_cell(app, out, vis_idx, img_idx)?;
        sent.insert(img_idx);
    }
    // The cells left for the next pass keep their previews until their sharp image goes out.
    delete_stale_images_to(app, out, |cell| sent.contains(&cell))?;
    trim_to_budget(&mut app.terminal_images, out, &shown)?;
    queue!(out, EndSynchronizedUpdate)?;
    out.flush()?;
    Ok(left)
}

/// Transmit the thumbnail of `img_idx` into the visible cell `vis_idx`. Returns its bytes.
fn transmit_cell(
    app: &mut App,
    out: &mut impl Write,
    vis_idx: usize,
    img_idx: usize,
) -> io::Result<usize> {
    let Some((picture, id)) = app.thumb_cache.peek(img_idx) else {
        return Ok(0);
    };
    let bytes = picture.image.as_raw().len();
    let area = grid_placement(app, vis_idx, picture.cells);
    queue!(out, cursor::MoveTo(area.x, area.y))?;
    app.graphics.transmit(
        out,
        (&picture.image).into(),
        &kitty::DisplayOptions {
            id: Some(id),
            cell_box: Some((area.width, area.height)),
            transmit_only: false,
            columns: None,
            rows: None,
            below_backgrounds: false,
            transient: false,
        },
    )?;
    app.terminal_images.insert(id, bytes);
    Ok(bytes)
}

/// Delete the previews that a sharp thumbnail replaced, for the cells that `select` picks. The
/// sharp image has a new id.
fn delete_stale_images_to(
    app: &mut App,
    out: &mut impl Write,
    select: impl Fn(usize) -> bool,
) -> io::Result<()> {
    // Remove in place and keep the order. A pass allocates no `Vec` per cell.
    while let Some(position) = app
        .stale_image_ids
        .iter()
        .position(|&(cell, _)| select(cell))
    {
        let (_, id) = app.stale_image_ids.remove(position);
        delete_image_to(&mut app.terminal_images, out, id)?;
    }
    Ok(())
}

/// Delete the images of the thumbnails that the cache dropped.
fn delete_evicted_images_to(app: &mut App, out: &mut impl Write) -> io::Result<()> {
    for id in app.thumb_cache.take_evicted_ids() {
        delete_image_to(&mut app.terminal_images, out, id)?;
    }
    Ok(())
}

/// The cell box of a picture of `cells` in the thumbnail area of `vis_idx`, centered.
///
/// Every grid picture is padded to whole cells: a sharp thumbnail at its pixel size, a preview
/// at its own size in the shape of those cells. `c=` and `r=` fit both to the same cells. No
/// picture needs a pixel offset. A relaying multiplexer gets offsets wrong.
fn grid_placement(app: &App, vis_idx: usize, cells: (u16, u16)) -> Rect {
    let cols = cells.0.clamp(1, app.gallery.thumb_cols.max(1));
    let rows = cells.1.clamp(1, app.gallery.thumb_rows.max(1));
    centered(app.gallery.thumb_rect(vis_idx), cols, rows)
}

/// Place the stored image `id` centered in the visible cell `vis_idx`, without a transmit.
pub(super) fn place_at(
    app: &App,
    out: &mut impl Write,
    vis_idx: usize,
    cells: (u16, u16),
    id: u32,
) -> io::Result<()> {
    let area = grid_placement(app, vis_idx, cells);
    queue!(out, cursor::MoveTo(area.x, area.y))?;
    protocol::place_by_id_to(out, id, Some((area.width, area.height)))
}

/// The ids that the visible cells need: the stills and their loop frames.
pub(super) fn visible_image_ids(app: &App) -> HashSet<u32> {
    let mut ids = HashSet::new();
    for (_, img_idx) in app.gallery.visible_items() {
        if let Some((_, id)) = app.thumb_cache.peek(img_idx) {
            ids.insert(id);
        }
        if let Some(cell_loop) = app.grid_loops.loops.get(&img_idx) {
            ids.extend(cell_loop.ids.iter().copied());
        }
    }
    ids
}

pub(crate) fn render_gallery_images(app: &mut App) -> io::Result<()> {
    // The visible cells queue again below. The first screen waits for the cell shape, and no cell
    // decodes for two sizes.
    app.clear_thumb_queue();
    app.request_cell_shape();
    app.thumb_cache.fit_capacity(app.gallery.visible_cells());
    app.thumb_cache
        .set_visible(app.gallery.visible_items().map(|(_, index)| index));
    for position in app.gallery.visible_range() {
        app.thumb_cache
            .touch(app.gallery.filtered_indices[position]);
    }
    app.drop_hidden_loops();
    if !app.cell_shape_pending() {
        for position in app.gallery.visible_range() {
            let img_idx = app.gallery.filtered_indices[position];
            app.spawn_thumb_decode(img_idx, app.gallery.thumb_box());
        }
    }

    app.with_graphics_out(place_gallery_images_to)
}

/// Write the visible thumbnails in one synchronized update.
pub(super) fn place_gallery_images_to(app: &mut App, out: &mut impl Write) -> io::Result<()> {
    use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};

    queue!(out, BeginSynchronizedUpdate)?;
    if app.graphics_storage_dirty {
        app.graphics_delete_all_to(out)?;
        app.graphics_storage_dirty = false;
        app.stale_image_ids.clear();
        app.grid_loops.stale_ids.clear();
        app.thumb_cache.take_evicted_ids();
    } else {
        // The terminal keeps the pixels. A stored thumbnail goes out again as an `a=p`.
        protocol::clear_placements_to(out)?;
        app.grid_loops.forget_placements();
        app.grid_icons.forget_placements();
        delete_stale_images_to(app, out, |_| true)?;
        delete_evicted_images_to(app, out)?;
        for id in std::mem::take(&mut app.grid_loops.stale_ids) {
            delete_image_to(&mut app.terminal_images, out, id)?;
        }
    }

    let shown = visible_image_ids(app);
    let tick = app.grid_loops.tick_at(Instant::now());
    for (vis_idx, position) in app.gallery.visible_range().enumerate() {
        let img_idx = app.gallery.filtered_indices[position];
        let Some((cells, id)) = app
            .thumb_cache
            .peek(img_idx)
            .map(|(picture, id)| (picture.cells, id))
        else {
            continue;
        };
        if place_loop_frame_to(app, out, vis_idx, img_idx, tick)? {
            // The still stays for the next overlay, and the trim keeps the newest images.
            app.terminal_images.touch(id);
            continue;
        }
        if app.terminal_images.contains(id) {
            place_at(app, out, vis_idx, cells, id)?;
            app.terminal_images.touch(id);
        } else {
            transmit_cell(app, out, vis_idx, img_idx)?;
        }
    }
    trim_to_budget(&mut app.terminal_images, out, &shown)?;
    queue!(out, EndSynchronizedUpdate)?;
    out.flush()
}

#[cfg(test)]
mod tests;
