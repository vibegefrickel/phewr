//! The grid pictures: one frame of the visible cells per pass (the thumbnails, the previews, the
//! loop frames and the icons), written by the frame engine of `tuilibr::pictures::kitty::screen`.

use super::loops::{place_loop_frame, send_grid_loops_to};
use crate::app::App;
use crate::gallery::GalleryState;
use ratatui::prelude::*;
use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::time::Instant;
use tuilibr::pictures::jobs::queue::{Priority, ThumbJob};
use tuilibr::pictures::kitty::Pixels;
use tuilibr::pictures::kitty::placement::centered;
use tuilibr::pictures::kitty::screen::{Placements, Update};

/// The result of one grid pass.
pub(crate) struct GridPass {
    /// The visible cells whose picture waits for a later pass, in grid order.
    pub(crate) waiting: Vec<usize>,
    /// True when a cell started its loop, and the draw drops its marker.
    pub(crate) started: bool,
}

/// Write one grid pass. The visible cells whose picture waits for the byte cap come back.
pub(crate) fn emit_new_thumbnails(app: &mut App, pending: &mut Vec<usize>) -> io::Result<()> {
    app.with_graphics_out(|app, out| {
        *pending = emit_new_thumbnails_to(app, out)?;
        Ok(())
    })
}

/// One grid pass without new loops, in one synchronized update. After `EMIT_PASS_BYTES`, the
/// visible rest comes back for the next pass.
pub(super) fn emit_new_thumbnails_to(
    app: &mut App,
    out: &mut impl Write,
) -> io::Result<Vec<usize>> {
    let mut update = Update::new(out);
    let pass = write_grid_to(app, &mut update, Instant::now(), Vec::new(), false)?;
    update.finish()?;
    Ok(pass.waiting)
}

/// One grid pass into `update`: the loops that start (with `send_loops`), then the frame of the
/// visible cells. `deleted` names the pictures that the terminal must drop.
pub(super) fn write_grid_to<W: Write + ?Sized>(
    app: &mut App,
    update: &mut Update<'_, W>,
    now: Instant,
    mut deleted: Vec<u32>,
    send_loops: bool,
) -> io::Result<GridPass> {
    if send_loops {
        send_grid_loops_to(app, update)?;
    }
    deleted.extend(app.thumb_cache.take_evicted_ids());
    deleted.append(&mut app.grid_loops.stale_ids);
    let tick = app.grid_loops.tick_at(now);
    let (mut frame, stills, icon_cells, started) = grid_frame(app, tick, &mut deleted);
    app.screen.write_frame(
        update,
        app.graphics,
        &mut frame,
        &deleted,
        |id| match stills.get(&id) {
            Some(&index) => app
                .thumb_cache
                .peek(&index)
                .map(|(picture, _)| Pixels::from(&picture.image)),
            None => app.grid_icons.pixels(id),
        },
    )?;
    // A preview goes when its sharp picture shows, and the engine deletes it.
    app.stale_image_ids.retain(|&(_, id)| app.screen.stores(id));
    // An icon that waits counts as its cell, so the next pass comes without a delay.
    let mut waiting: Vec<usize> = app
        .screen
        .waiting()
        .iter()
        .filter_map(|id| stills.get(id).or_else(|| icon_cells.get(id)).copied())
        .collect();
    waiting.dedup();
    Ok(GridPass { waiting, started })
}

/// The cell of each thumbnail id or icon id of a frame.
type Cells = HashMap<u32, usize>;

/// The frame of the visible cells in grid order: the loop frame or the thumbnail of each cell,
/// and the icons. Returns the frame, the cell of each thumbnail id, the first cell of each icon
/// id and whether a loop started.
fn grid_frame(
    app: &mut App,
    tick: u64,
    deleted: &mut Vec<u32>,
) -> (Placements, Cells, Cells, bool) {
    let mut frame = Placements::default();
    let mut stills = HashMap::new();
    let mut icon_cells = HashMap::new();
    let mut started = false;
    let mut icons = app
        .grid_icons
        .resolve(app.cell_px, deleted)
        .into_iter()
        .peekable();
    let visible: Vec<(usize, usize)> = app.gallery.visible_items().collect();
    for (vis_idx, img_idx) in visible {
        while let Some((_, id, area)) = icons.next_if(|&(cell, ..)| cell <= vis_idx) {
            frame.place(id, area);
            icon_cells.entry(id).or_insert(img_idx);
        }
        let still = app
            .thumb_cache
            .peek(&img_idx)
            .map(|(picture, id)| (picture.cells, id));
        if let Some(now_started) = place_loop_frame(app, &mut frame, vis_idx, img_idx, tick) {
            started |= now_started;
            // The still stays for the next overlay.
            if let Some((_, id)) = still {
                frame.keep(id);
            }
        } else if let Some((cells, id)) = still {
            let area = grid_placement(&app.gallery, vis_idx, cells);
            match app
                .stale_image_ids
                .iter()
                .find(|&&(cell, _)| cell == img_idx)
            {
                Some(&(_, earlier)) => frame.place_over(id, earlier, area),
                None => frame.place(id, area),
            }
            stills.insert(id, img_idx);
        }
    }
    for (_, id, area) in icons {
        frame.place(id, area);
    }
    (frame, stills, icon_cells, started)
}

/// The cell box of a picture of `cells` in the thumbnail area of `vis_idx`, centered.
///
/// Every grid picture is padded to whole cells: a sharp thumbnail at its pixel size, a preview
/// at its own size in the shape of those cells. `c=` and `r=` fit both to the same cells. No
/// picture needs a pixel offset. A relaying multiplexer gets offsets wrong.
pub(super) fn grid_placement(gallery: &GalleryState, vis_idx: usize, cells: (u16, u16)) -> Rect {
    let cols = cells.0.clamp(1, gallery.thumb_cols.max(1));
    let rows = cells.1.clamp(1, gallery.thumb_rows.max(1));
    centered(gallery.thumb_rect(vis_idx), cols, rows)
}

/// The ids that the visible cells need: the stills and their loop frames.
pub(super) fn visible_image_ids(app: &App) -> HashSet<u32> {
    let mut ids = HashSet::new();
    for (_, img_idx) in app.gallery.visible_items() {
        if let Some((_, id)) = app.thumb_cache.peek(&img_idx) {
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
            .touch(&app.gallery.filtered_indices[position]);
    }
    app.drop_hidden_loops();
    if !app.cell_shape_pending() {
        // The cell at the cursor decodes first, then the other cells from the top left. A free
        // slot starts a job at once, so the cursor cell also spawns first.
        let cursor = app.gallery.cursor;
        let visible = app.gallery.visible_range();
        let others = visible.clone().filter(|&position| position != cursor);
        let order: Vec<usize> = visible
            .contains(&cursor)
            .then_some(cursor)
            .into_iter()
            .chain(others)
            .collect();
        for position in order {
            app.spawn_thumb_job(ThumbJob {
                key: app.gallery.filtered_indices[position],
                rect: app.gallery.thumb_box(),
                priority: Priority::on_screen(position, Some(cursor)),
            });
        }
    }

    app.with_graphics_out(place_gallery_images_to)
}

/// Write the visible thumbnails in one synchronized update, without a byte cap: a redraw shows
/// every visible picture in one step, as the return from another view expects. After a new list
/// or cell size, a `d=A` deletes every old picture first.
pub(super) fn place_gallery_images_to(app: &mut App, out: &mut impl Write) -> io::Result<()> {
    let mut update = Update::uncapped(out);
    let mut deleted = Vec::new();
    if app.graphics_storage_dirty {
        app.graphics_delete_all_to(&mut update)?;
        app.graphics_storage_dirty = false;
        app.stale_image_ids.clear();
        app.grid_loops.stale_ids.clear();
        app.thumb_cache.take_evicted_ids();
    } else {
        // A preview of a cell off the screen goes. A visible one keeps its place until its
        // sharp picture shows.
        let visible: HashSet<usize> = app
            .gallery
            .visible_items()
            .map(|(_, index)| index)
            .collect();
        app.stale_image_ids.retain(|&(cell, id)| {
            let keep = visible.contains(&cell);
            if !keep {
                deleted.push(id);
            }
            keep
        });
    }
    write_grid_to(app, &mut update, Instant::now(), deleted, false)?;
    update.finish()
}

#[cfg(test)]
mod tests;
