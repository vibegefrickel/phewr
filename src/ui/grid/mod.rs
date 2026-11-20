//! The grid cells: the borders in the gaps, the folder names and the icons of the
//! cells without a thumbnail. Kitty draws the thumbnails above them.

use crate::app::App;
use crate::icons::{self, Icon};
use crate::kitty_output::IconCell;
use crate::styles::STYLES;
use ratatui::{
    prelude::*,
    widgets::{Block, BorderType, Borders, Paragraph},
};
use std::borrow::Cow;

/// The rows of a folder name below the folder icon.
const FOLDER_NAME_ROWS: u16 = 1;

/// The grid area and the icons that the cells ask Kitty for.
struct GridDraw<'a> {
    rect: Rect,
    icons: &'a mut Vec<IconCell>,
}

/// The grid cells. The selected border comes last and wins on a gap
/// that two cells share. `icons` gets the icons that Kitty draws.
pub(super) fn draw_thumbnail_cells(
    frame: &mut Frame<'_>,
    app: &App,
    grid_rect: Rect,
    icons: &mut Vec<IconCell>,
) {
    let mut grid = GridDraw {
        rect: grid_rect,
        icons,
    };
    let first_visible = app.gallery.scroll_offset * app.gallery.grid_cols;
    let mut selected_border = None;

    for (vis_idx, img_idx) in app.gallery.visible_items() {
        let is_selected = vis_idx + first_visible == app.gallery.cursor;
        let is_marked = app.is_marked(img_idx);
        let border = app.gallery.border_rect(vis_idx);
        if is_selected {
            selected_border = Some(border);
        } else if is_marked {
            draw_cell_border(frame, BorderType::Plain, STYLES.marked, border, grid_rect);
        }
        let style = if is_selected {
            STYLES.cursor_cell
        } else if is_marked {
            STYLES.marked
        } else {
            STYLES.text
        };
        if app.is_folder(img_idx) {
            draw_folder_cell(frame, app, vis_idx, img_idx, style, &mut grid);
        } else if !app.thumb_cache.contains(&img_idx) {
            let icon = if app.is_video(img_idx) {
                Icon::Video
            } else {
                Icon::Loading
            };
            draw_cell_icon(frame, app, vis_idx, icon, &mut grid);
        }
    }

    if let Some(border) = selected_border {
        draw_cell_border(
            frame,
            BorderType::Thick,
            STYLES.cursor_cell,
            border,
            grid_rect,
        );
    }
}

/// A large dim icon in the middle of the thumbnail area of `vis_idx`, or the one-cell symbol
/// when the area is small or the grid edge cuts the icon. The large icon is a Kitty image when
/// the terminal foreground is known, else octant text. Kitty draws the thumbnail above the text,
/// and the text icon needs no redraw. Returns the row below the icon.
fn draw_cell_icon(
    frame: &mut Frame<'_>,
    app: &App,
    vis_idx: usize,
    icon: Icon,
    grid: &mut GridDraw<'_>,
) -> Option<u16> {
    let thumb = app.gallery.thumb_rect(vis_idx);
    let shape =
        icons::icon_rect(thumb, app.cell_px).filter(|area| grid.rect.intersection(*area) == *area);
    if let Some(area) = shape {
        // The help popup hides the icons.
        if !app.help_visible {
            if app.grid_icons.as_images() {
                grid.icons.push(IconCell {
                    cell: vis_idx,
                    icon,
                    area,
                });
            } else {
                icons::draw_icon(frame.buffer_mut(), area, icon, STYLES.dim);
            }
        }
        return Some(area.bottom());
    }
    let center = Position::new(thumb.x + thumb.width / 2, thumb.y + thumb.height / 2);
    if !grid.rect.contains(center) {
        return None;
    }
    if !app.help_visible {
        frame
            .buffer_mut()
            .set_string(center.x, center.y, icon.symbol(), STYLES.dim);
    }
    Some(center.y + 1)
}

fn is_parent(app: &App, img_idx: usize) -> bool {
    app.is_parent_entry(img_idx)
}

/// A folder icon with the name below it, or two dots for `..`. The name goes below the icon, inside
/// the thumbnail area.
fn draw_folder_cell(
    frame: &mut Frame<'_>,
    app: &App,
    vis_idx: usize,
    img_idx: usize,
    style: Style,
    grid: &mut GridDraw<'_>,
) {
    let thumb = app.gallery.thumb_rect(vis_idx);
    let icon = if is_parent(app, img_idx) {
        Icon::Parent
    } else {
        Icon::Folder
    };
    let Some(below) = draw_cell_icon(frame, app, vis_idx, icon, grid) else {
        return;
    };
    if icon == Icon::Parent {
        return;
    }
    let name_rect = Rect {
        y: below,
        height: FOLDER_NAME_ROWS,
        ..thumb
    }
    .intersection(grid.rect);
    draw_centered_text(
        frame,
        folder_name_text(app, img_idx, name_rect.width),
        style,
        name_rect,
    );
}

/// A border without a fill. A side outside `grid_rect` stays open at the clip edge.
fn draw_cell_border(
    frame: &mut Frame<'_>,
    border_type: BorderType,
    style: Style,
    border: Rect,
    grid_rect: Rect,
) {
    let visible = border.intersection(grid_rect);
    let mut sides = Borders::ALL;
    if visible.x > border.x {
        sides.remove(Borders::LEFT);
    }
    if visible.y > border.y {
        sides.remove(Borders::TOP);
    }
    if visible.right() < border.right() {
        sides.remove(Borders::RIGHT);
    }
    if visible.bottom() < border.bottom() {
        sides.remove(Borders::BOTTOM);
    }
    frame.render_widget(
        Block::default()
            .borders(sides)
            .border_type(border_type)
            .border_style(style),
        visible,
    );
}

fn draw_centered_text(frame: &mut Frame<'_>, text: Cow<'_, str>, style: Style, area: Rect) {
    frame.render_widget(
        Paragraph::new(text)
            .style(style)
            .alignment(Alignment::Center),
        area,
    );
}

/// The folder name of `img_idx`, cut to `width` with a margin.
fn folder_name_text(app: &App, img_idx: usize, width: u16) -> Cow<'_, str> {
    let max_label = usize::from(width).saturating_sub(2);
    Cow::Owned(tuilibr::frame::truncate_width(
        app.filenames[img_idx].as_str(),
        max_label,
    ))
}

#[cfg(test)]
mod tests;
