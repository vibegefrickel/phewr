//! The SVG icons of the grid cells without a thumbnail. Each icon is one Kitty image per pixel
//! size of the icon box, and the frame engine gives every cell its own placement. The draw
//! records the icons that the cells need, and `sync_grid_icons` writes a grid pass when they
//! changed.

use super::grid::write_grid_to;
use crate::app::{App, ViewMode};
use crate::icons::{self, Icon};
use image::RgbaImage;
use ratatui::layout::Rect;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::io::{self, Write};
use std::time::Instant;
use tuilibr::pictures::kitty::Pixels;
use tuilibr::pictures::kitty::screen::Update;

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
    /// The icons of the last grid pass.
    flushed: Vec<IconCell>,
    /// The icon and the pixel size of the picture under each image id.
    stored: HashMap<u32, (Icon, (u32, u32))>,
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

    /// After `d=A`: the terminal stores no icon.
    pub(crate) fn forget_images(&mut self) {
        self.flushed.clear();
        self.stored.clear();
    }

    fn in_sync(&self) -> bool {
        self.wanted == self.flushed
    }

    /// The image id and the area of each wanted icon that renders, in the order of the cells.
    /// An id that takes a new size goes into `replaced`, so the terminal gets its new pixels.
    pub(crate) fn resolve(
        &mut self,
        cell_px: (u32, u32),
        replaced: &mut Vec<u32>,
    ) -> Vec<(usize, u32, Rect)> {
        let wanted = self.wanted.clone();
        let mut places = Vec::with_capacity(wanted.len());
        for cell in &wanted {
            let size = icon_px(cell.area, cell_px);
            let id = self.stored_id(cell.icon, size).unwrap_or_else(|| {
                let id = self.free_id(cell.icon, &wanted, cell_px);
                if self.stored.insert(id, (cell.icon, size)).is_some() {
                    replaced.push(id);
                }
                id
            });
            if self.picture(cell.icon, size).is_some() {
                places.push((cell.cell, id, cell.area));
            }
        }
        places.sort_by_key(|&(cell, ..)| cell);
        self.flushed = wanted;
        places
    }

    /// The pixels of the icon image `id`, when it rendered.
    pub(crate) fn pixels(&self, id: u32) -> Option<Pixels<'_>> {
        let key = self.stored.get(&id)?;
        self.rendered.get(key).map(Pixels::from)
    }

    /// The image of `icon` at `size`.
    fn stored_id(&self, icon: Icon, size: (u32, u32)) -> Option<u32> {
        image_ids(icon).find(|id| self.stored.get(id) == Some(&(icon, size)))
    }

    /// The image id for a new size of `icon`: a free one, else one of a size that no wanted cell
    /// needs.
    fn free_id(&self, icon: Icon, wanted: &[IconCell], cell_px: (u32, u32)) -> u32 {
        let needed = |id: &u32| {
            self.stored.get(id).is_some_and(|&(_, size)| {
                wanted
                    .iter()
                    .any(|cell| cell.icon == icon && icon_px(cell.area, cell_px) == size)
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

const fn image_ids(icon: Icon) -> std::ops::Range<u32> {
    first_id(icon)..first_id(icon) + SLOTS
}

fn icon_px(area: Rect, cell_px: (u32, u32)) -> (u32, u32) {
    (
        u32::from(area.width) * cell_px.0,
        u32::from(area.height) * cell_px.1,
    )
}

/// Write a grid pass when the icons of the last draw changed or the terminal lost a picture,
/// and no other pass is due. The visible cells whose picture waits come back in `pending`.
pub(crate) fn sync_grid_icons(app: &mut App, pending: &mut Vec<usize>) -> io::Result<()> {
    // A waiting picture brings the next pass at once, and that pass writes the icons too: one
    // run-loop pass writes one grid pass.
    let due = !app.grid_icons.in_sync() || app.screen.has_lost();
    if app.mode != ViewMode::Gallery || !pending.is_empty() || !due {
        return Ok(());
    }
    app.with_graphics_out(|app, out| {
        *pending = sync_grid_icons_to(app, out)?;
        Ok(())
    })
}

/// One grid pass for the icons, in one synchronized update. Returns the visible cells whose
/// picture waits.
fn sync_grid_icons_to(app: &mut App, out: &mut impl Write) -> io::Result<Vec<usize>> {
    let mut update = Update::new(out);
    let pass = write_grid_to(app, &mut update, Instant::now(), Vec::new(), false)?;
    update.finish()?;
    Ok(pass.waiting)
}

#[cfg(test)]
mod tests;
