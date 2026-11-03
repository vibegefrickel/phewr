//! The grid cells: the borders in the gaps, the labels and the icons of the cells without a
//! thumbnail. Kitty draws the thumbnails above them.

use crate::app::App;
use crate::gallery::LABEL_ROWS;
use crate::icons::{self, Icon};
use crate::styles::STYLES;
use ratatui::{
    prelude::*,
    widgets::{Block, BorderType, Borders, Paragraph},
};
use std::borrow::Cow;
use unicode_width::UnicodeWidthStr;

/// The mark of a Live Photo still in the gap above its cell.
const LIVE_MARK: &str = "\u{25ce} Live";

#[derive(Clone, Copy)]
struct CellLook {
    style: Style,
    marked: bool,
}

/// The grid cells with their optional labels. The selected border comes last and wins on a gap
/// that two cells share.
pub(super) fn draw_thumbnail_cells(frame: &mut Frame<'_>, app: &App, grid_rect: Rect) {
    let first_visible = app.gallery.scroll_offset * app.gallery.grid_cols;
    let mut selected_border = None;

    for (vis_idx, img_idx) in app.gallery.visible_items() {
        let is_selected = vis_idx + first_visible == app.gallery.cursor;
        let is_marked = app.is_marked(img_idx);
        let border = app.gallery.border_rect(vis_idx);
        if is_selected {
            selected_border = Some((border, img_idx));
        } else if is_marked {
            draw_cell_border(frame, BorderType::Plain, STYLES.marked, border, grid_rect);
        }
        let look = CellLook {
            style: if is_selected {
                STYLES.cursor_cell
            } else if is_marked {
                STYLES.marked
            } else {
                STYLES.text
            },
            marked: is_marked,
        };
        if app.is_folder(img_idx) {
            draw_folder_cell(frame, app, vis_idx, img_idx, look, grid_rect);
        } else if !app.thumb_cache.contains(img_idx) {
            let icon = if app.is_video(img_idx) {
                Icon::Video
            } else {
                Icon::Loading
            };
            draw_cell_icon(frame, app, app.gallery.thumb_rect(vis_idx), icon, grid_rect);
        }
        if app.gallery.show_filenames && !is_parent(app, img_idx) {
            draw_label(frame, app, vis_idx, img_idx, look, grid_rect);
        }
        if app.is_live(img_idx) {
            draw_live_mark(frame, border, look.style, grid_rect);
        }
    }

    if let Some((border, img_idx)) = selected_border {
        draw_cell_border(
            frame,
            BorderType::Thick,
            STYLES.cursor_cell,
            border,
            grid_rect,
        );
        // The thick border covers the mark of the cursor cell.
        if app.is_live(img_idx) {
            draw_live_mark(frame, border, STYLES.cursor_cell, grid_rect);
        }
    }
}

/// The Live mark of a Live Photo still: `◎ Live` in the gap above the picture, where the top
/// border lives. Kitty draws the picture inside the cell, so the mark stays visible. A border
/// is at least 10 columns wide, and the grid edge cuts the mark.
fn draw_live_mark(frame: &mut Frame<'_>, border: Rect, style: Style, grid_rect: Rect) {
    let at = Position::new(border.x + 2, border.y);
    if !grid_rect.contains(at) {
        return;
    }
    let room = usize::from(grid_rect.right() - at.x);
    frame
        .buffer_mut()
        .set_stringn(at.x, at.y, LIVE_MARK, room, style);
}

/// A large dim icon in the middle of `thumb`, or the one-cell symbol when the area is small or
/// the grid edge cuts the icon. Kitty draws the thumbnail above it, and the icon needs no
/// redraw. Returns the row below the icon.
fn draw_cell_icon(
    frame: &mut Frame<'_>,
    app: &App,
    thumb: Rect,
    icon: Icon,
    grid_rect: Rect,
) -> Option<u16> {
    let shape =
        icons::icon_rect(thumb, app.cell_px).filter(|area| grid_rect.intersection(*area) == *area);
    if let Some(area) = shape {
        if !app.help_visible {
            icons::draw_icon(frame.buffer_mut(), area, icon, STYLES.dim);
        }
        return Some(area.bottom());
    }
    let center = Position::new(thumb.x + thumb.width / 2, thumb.y + thumb.height / 2);
    if !grid_rect.contains(center) {
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

/// A folder icon with the name below it, or two dots for `..`. Without the label row, the name
/// goes inside the cell.
fn draw_folder_cell(
    frame: &mut Frame<'_>,
    app: &App,
    vis_idx: usize,
    img_idx: usize,
    look: CellLook,
    grid_rect: Rect,
) {
    let thumb = app.gallery.thumb_rect(vis_idx);
    let icon = if is_parent(app, img_idx) {
        Icon::Parent
    } else {
        Icon::Folder
    };
    let Some(below) = draw_cell_icon(frame, app, thumb, icon, grid_rect) else {
        return;
    };
    if app.gallery.show_filenames || icon == Icon::Parent {
        return;
    }
    let name_rect = Rect {
        y: below,
        height: LABEL_ROWS,
        ..thumb
    }
    .intersection(grid_rect);
    draw_centered_text(
        frame,
        label_text(app, img_idx, look.marked, name_rect.width),
        look.style,
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

/// The filename below the thumbnail of `vis_idx`, cut to the cell width.
fn draw_label(
    frame: &mut Frame<'_>,
    app: &App,
    vis_idx: usize,
    img_idx: usize,
    look: CellLook,
    grid_rect: Rect,
) {
    let thumb = app.gallery.thumb_rect(vis_idx).intersection(grid_rect);
    let label_y = thumb.y + thumb.height;
    if label_y >= grid_rect.bottom() {
        return;
    }
    let label_rect = Rect {
        y: label_y,
        height: LABEL_ROWS,
        ..thumb
    };
    draw_centered_text(
        frame,
        label_text(app, img_idx, look.marked, thumb.width),
        look.style,
        label_rect,
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

/// The longest head of `text` that fits `room` columns. The width of a prefix grows with its
/// length, so a binary search over the char boundaries gives the same head as a left to right scan.
fn head_within(text: &str, room: usize) -> &str {
    let boundaries: Vec<usize> = text
        .char_indices()
        .map(|(index, _)| index)
        .chain([text.len()])
        .collect();
    let fitting =
        boundaries.partition_point(|&end| text.get(..end).is_some_and(|head| head.width() <= room));
    let end = boundaries
        .get(fitting.saturating_sub(1))
        .copied()
        .unwrap_or(0);
    text.get(..end).unwrap_or(text)
}

/// The label of `img_idx`, cut to `width` with a margin, with a dot for a mark.
fn label_text(app: &App, img_idx: usize, is_marked: bool, width: u16) -> Cow<'_, str> {
    let raw_filename = app.filenames[img_idx].as_str();
    let prefix_len = 2 * usize::from(is_marked);
    let max_label = usize::from(width).saturating_sub(2 + prefix_len);
    let trimmed = if raw_filename.width() > max_label {
        let room = max_label.saturating_sub(1);
        let head = head_within(raw_filename, room);
        Cow::Owned(format!("{head}\u{2026}"))
    } else {
        Cow::Borrowed(raw_filename)
    };
    if is_marked {
        Cow::Owned(format!("\u{25cf} {trimmed}"))
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests;
