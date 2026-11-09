//! The SVG icons of the grid cells without a thumbnail. Each icon is one Kitty image per pixel
//! size of the icon box, and every cell places it with its own placement id. The draw records
//! the icons that the cells need, and `sync_grid_icons` writes only the placements that changed.

use crate::app::App;
use crate::encoder::{self, kitty};
use crate::icons::{self, Icon};
use crossterm::{cursor, queue};
use image::RgbaImage;
use ratatui::layout::Rect;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::io::{self, Write};

/// The image ids of each icon. The first grid rows get the spare rows, so one grid can need two
/// sizes of an icon, and a zoom brings two more.
const SLOTS: u32 = 4;
/// The image ids of the icons, below the loop ids. The thumbnails count up from 1 and never
/// reach them.
const ICON_ID_BASE: u32 = (1 << 31) - 4 * SLOTS;
/// The rendered pictures in memory. A zoom or a resize renders a new size, and this many cover
/// every grid size of a window.
const MAX_RENDERED: usize = 64;

/// The icon of one visible cell, in the area of the last draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IconCell {
    pub(crate) cell: usize,
    pub(crate) icon: Icon,
    pub(crate) area: Rect,
}

#[derive(Default)]
pub(crate) struct GridIcons {
    /// The terminal foreground. Without it, the icons are octant text in the ratatui frame.
    color: Option<[u8; 3]>,
    /// The icons that the last draw asked for.
    pub(crate) wanted: Vec<IconCell>,
    /// The placements in the terminal with their image ids, by visible cell.
    placed: HashMap<usize, (IconCell, u32)>,
    /// The pixel size of each icon image that the terminal stores, by image id.
    stored: HashMap<u32, (u32, u32)>,
    rendered: HashMap<(Icon, (u32, u32)), RgbaImage>,
}

impl GridIcons {
    pub(crate) fn set_color(&mut self, color: Option<[u8; 3]>) {
        self.color = color;
        self.rendered.clear();
    }

    /// True when the icons are Kitty images, not octant text.
    pub(crate) const fn as_images(&self) -> bool {
        self.color.is_some()
    }

    /// After `d=a`: the terminal keeps the images, but the placements are gone.
    pub(crate) fn forget_placements(&mut self) {
        self.placed.clear();
    }

    /// After `d=A`: the terminal stores no icon.
    pub(crate) fn forget_images(&mut self) {
        self.placed.clear();
        self.stored.clear();
    }

    fn in_sync(&self) -> bool {
        self.placed.len() == self.wanted.len()
            && self.wanted.iter().all(|wanted| {
                self.placed.get(&wanted.cell).map(|(placed, _)| placed) == Some(wanted)
            })
    }

    /// The image of `icon` that the terminal stores at `size`.
    fn stored_id(&self, icon: Icon, size: (u32, u32)) -> Option<u32> {
        image_ids(icon).find(|id| self.stored.get(id) == Some(&size))
    }

    /// The image id for a new size of `icon`: a free one, else one of a size that no wanted cell
    /// needs.
    fn free_id(&self, icon: Icon, wanted: &[IconCell], cell_px: (u32, u32)) -> u32 {
        let needed = |id: &u32| {
            self.stored.get(id).is_some_and(|size| {
                wanted
                    .iter()
                    .any(|cell| cell.icon == icon && icon_px(cell.area, cell_px) == *size)
            })
        };
        image_ids(icon)
            .find(|id| !self.stored.contains_key(id))
            .or_else(|| image_ids(icon).find(|id| !needed(id)))
            .unwrap_or_else(|| first_id(icon))
    }

    /// The picture of `icon` at `size`, rendered once and kept.
    fn picture(&mut self, icon: Icon, size: (u32, u32)) -> Option<&RgbaImage> {
        let color = self.color?;
        if self.rendered.len() >= MAX_RENDERED {
            self.rendered.retain(|key, _| *key == (icon, size));
        }
        match self.rendered.entry((icon, size)) {
            Entry::Occupied(entry) => Some(entry.into_mut()),
            Entry::Vacant(entry) => Some(entry.insert(icons::render(icon, size, color)?)),
        }
    }
}

const fn first_id(icon: Icon) -> u32 {
    ICON_ID_BASE + icon as u32 * SLOTS
}

fn image_ids(icon: Icon) -> std::ops::Range<u32> {
    first_id(icon)..first_id(icon) + SLOTS
}

fn icon_px(area: Rect, cell_px: (u32, u32)) -> (u32, u32) {
    (
        u32::from(area.width) * cell_px.0,
        u32::from(area.height) * cell_px.1,
    )
}

/// A placement id is not zero.
fn placement_id(cell: usize) -> u32 {
    u32::try_from(cell).map_or(u32::MAX, |cell| cell.saturating_add(1))
}

/// Bring the icon placements in line with the last draw. Without a change, nothing goes out.
pub(crate) fn sync_grid_icons(app: &mut App) -> io::Result<()> {
    if app.grid_icons.in_sync() {
        return Ok(());
    }
    app.with_graphics_out(sync_grid_icons_to)
}

fn sync_grid_icons_to(app: &mut App, out: &mut impl Write) -> io::Result<()> {
    use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};

    queue!(out, BeginSynchronizedUpdate)?;
    let wanted = std::mem::take(&mut app.grid_icons.wanted);
    // A changed cell can need another image, so its old placement goes first.
    let changed: Vec<(IconCell, u32)> = app
        .grid_icons
        .placed
        .values()
        .filter(|(placed, _)| !wanted.contains(placed))
        .copied()
        .collect();
    for (placed, id) in changed {
        unplace(app, out, placed.cell, id)?;
    }
    for cell in &wanted {
        if !app.grid_icons.placed.contains_key(&cell.cell) {
            place(app, out, *cell, &wanted)?;
        }
    }
    app.grid_icons.wanted = wanted;
    queue!(out, EndSynchronizedUpdate)?;
    out.flush()
}

fn unplace(app: &mut App, out: &mut impl Write, cell: usize, id: u32) -> io::Result<()> {
    kitty::delete_placement_to(out, id, placement_id(cell))?;
    app.grid_icons.placed.remove(&cell);
    Ok(())
}

/// Place the icon of `cell`, and send its image first when the terminal has no image of that size.
fn place(
    app: &mut App,
    out: &mut impl Write,
    cell: IconCell,
    wanted: &[IconCell],
) -> io::Result<()> {
    let size = icon_px(cell.area, app.cell_px);
    let id = if let Some(id) = app.grid_icons.stored_id(cell.icon, size) {
        id
    } else {
        let id = app.grid_icons.free_id(cell.icon, wanted, app.cell_px);
        if !store(app, out, cell.icon, size, id)? {
            return Ok(());
        }
        id
    };
    queue!(out, cursor::MoveTo(cell.area.x, cell.area.y))?;
    kitty::place_as_to(
        out,
        id,
        placement_id(cell.cell),
        (cell.area.width, cell.area.height),
    )?;
    app.grid_icons.placed.insert(cell.cell, (cell, id));
    Ok(())
}

/// Send the picture of `icon` at `size` as the image `id`. The new image replaces the old one,
/// so the placements of the old one go first. `false` when the icon does not render.
fn store(
    app: &mut App,
    out: &mut impl Write,
    icon: Icon,
    size: (u32, u32),
    id: u32,
) -> io::Result<bool> {
    let old: Vec<usize> = app
        .grid_icons
        .placed
        .iter()
        .filter(|(_, (_, placed_id))| *placed_id == id)
        .map(|(cell, _)| *cell)
        .collect();
    for cell in old {
        unplace(app, out, cell, id)?;
    }
    let Some(picture) = app.grid_icons.picture(icon, size) else {
        return Ok(false);
    };
    app.graphics.transmit(
        out,
        picture.into(),
        &encoder::DisplayOptions {
            id: Some(id),
            cell_box: None,
            transmit_only: true,
            columns: None,
            rows: None,
            below_backgrounds: false,
            transient: false,
        },
    )?;
    app.grid_icons.stored.insert(id, size);
    Ok(true)
}

#[cfg(test)]
mod tests;
