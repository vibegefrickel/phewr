//! The mouse: the clicks, the wheel and the drag, routed like the keys. An overlay wins over the
//! view mode.

use crate::app::{App, Drag, ViewMode};
use crate::ui;
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use std::io;
use std::time::Instant;

/// The share of the fullscreen width at each side where a click steps to the neighbor.
const EDGE_SHARE: u16 = 6;

/// Route a mouse event. `now` is the double-click clock.
pub(crate) fn handle_mouse(app: &mut App, mouse: MouseEvent, now: Instant) -> io::Result<bool> {
    let at = Position::new(mouse.column, mouse.row);
    if app.help_visible {
        if is_press(mouse.kind) {
            app.help_visible = false;
            app.needs_render = true;
        }
        return Ok(false);
    }
    match app.mode {
        ViewMode::Gallery => gallery_mouse(app, mouse, at, now),
        ViewMode::Fullscreen => fullscreen_mouse(app, mouse, at),
        #[cfg(feature = "video")]
        ViewMode::Video => video_mouse(app, mouse, at),
    }
}

fn is_press(kind: MouseEventKind) -> bool {
    matches!(kind, MouseEventKind::Down(_))
}

/// The areas of the last draw.
fn regions(app: &App) -> ui::Panes {
    ui::panes(app.layout.screen, app)
}

/// A left click on the status row opens the help, and a left click on the info pane or on the
/// omnibox does nothing. Returns `true` for such a click.
fn shared_click(app: &mut App, regions: &ui::Panes, at: Position) -> io::Result<bool> {
    if regions.status.contains(at) {
        app.graphics_delete_all()?;
        app.help_visible = true;
        app.needs_render = true;
        return Ok(true);
    }
    Ok(regions.omnibox.contains(at) || regions.info.is_some_and(|info| info.contains(at)))
}

fn gallery_mouse(app: &mut App, mouse: MouseEvent, at: Position, now: Instant) -> io::Result<bool> {
    let regions = regions(app);
    let shift = mouse.modifiers.contains(KeyModifiers::SHIFT);
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if shared_click(app, &regions, at)? {
                app.mouse.last_click = None;
            } else {
                gallery_click(app, at, now)?;
            }
        }
        MouseEventKind::Down(MouseButton::Right) => {
            app.mouse.last_click = None;
            gallery_right_click(app, at)?;
        }
        MouseEventKind::ScrollUp if shift => app.thumb_zoom_in(),
        MouseEventKind::ScrollDown if shift => app.thumb_zoom_out(),
        MouseEventKind::ScrollUp => scroll_grid(app, -1),
        MouseEventKind::ScrollDown => scroll_grid(app, 1),
        _ => {}
    }
    Ok(false)
}

fn scroll_grid(app: &mut App, rows: isize) {
    let before = app.gallery.scroll_offset;
    app.gallery.scroll_rows(rows);
    if app.gallery.scroll_offset != before {
        app.pre_decode_hovered();
        app.needs_render = true;
    }
}

/// A click puts the cursor on the cell, and a double-click opens it. A click on a gap breaks a
/// double-click.
fn gallery_click(app: &mut App, at: Position, now: Instant) -> io::Result<()> {
    let Some(position) = select_cell(app, at) else {
        app.mouse.last_click = None;
        return Ok(());
    };
    if !app.mouse.double_click(position, now) {
        return Ok(());
    }
    if app.selected_is_folder() {
        app.enter_selected_folder();
    } else {
        app.graphics_delete_all()?;
        app.enter_fullscreen_selected();
    }
    Ok(())
}

/// A right click on a cell opens the info pane. If the pane is open, a right click anywhere in
/// the gallery closes it without moving the cursor. If the panel is closed, a right click on a gap
/// does nothing.
fn gallery_right_click(app: &mut App, at: Position) -> io::Result<()> {
    if !app.info_visible && select_cell(app, at).is_none() {
        return Ok(());
    }
    app.toggle_info()
}

/// Put the cursor on the cell under `at`. A click ends the search typing and keeps the filter.
/// `None` on a gap.
fn select_cell(app: &mut App, at: Position) -> Option<usize> {
    let position = app.gallery.cell_at(at.x, at.y)?;
    app.gallery.search_active = false;
    app.gallery.cursor = position;
    app.gallery.ensure_cursor_visible();
    app.pre_decode_hovered();
    Some(position)
}

fn fullscreen_mouse(app: &mut App, mouse: MouseEvent, at: Position) -> io::Result<bool> {
    let regions = regions(app);
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if !shared_click(app, &regions, at)? {
                app.mouse.drag = Some(Drag {
                    last: at,
                    moved: false,
                });
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => drag_pan(app, at),
        MouseEventKind::Up(MouseButton::Left) => {
            let Some(drag) = app.mouse.drag.take() else {
                return Ok(false);
            };
            if !drag.moved && app.zoom <= 1.0 && regions.main.contains(at) {
                edge_step(app, regions.main, at);
            }
        }
        MouseEventKind::Down(MouseButton::Right) => app.toggle_info()?,
        MouseEventKind::ScrollUp => app.zoom_in(),
        MouseEventKind::ScrollDown => app.zoom_out(),
        _ => {}
    }
    Ok(false)
}

/// Move the zoomed image with the pointer: one cell of drag moves it by one cell.
fn drag_pan(app: &mut App, at: Position) {
    let Some(drag) = app.mouse.drag.as_mut() else {
        return;
    };
    if drag.last == at {
        return;
    }
    let dx = f64::from(at.x) - f64::from(drag.last.x);
    let dy = f64::from(at.y) - f64::from(drag.last.y);
    drag.last = at;
    drag.moved = true;
    let (range_x, range_y) = app.pan_range_px();
    let fx = share(-dx * f64::from(app.cell_px.0), range_x);
    let fy = share(-dy * f64::from(app.cell_px.1), range_y);
    app.pan_fraction(fx, fy);
}

fn share(pixels: f64, range: f64) -> f64 {
    if range > 0.0 { pixels / range } else { 0.0 }
}

/// A click near the left edge steps back, near the right edge forward. A click on the middle
/// does nothing.
fn edge_step(app: &mut App, content: Rect, at: Position) {
    let edge = (content.width / EDGE_SHARE).max(1);
    if at.x < content.x + edge {
        app.prev();
    } else if at.x >= content.right().saturating_sub(edge) {
        app.next();
    } else {
        return;
    }
    app.slideshow_touch();
}

/// A click on the picture does nothing. A click beside it or a right click goes back like `q`.
/// The wheel does nothing: the player has no seek.
#[cfg(feature = "video")]
fn video_mouse(app: &mut App, mouse: MouseEvent, at: Position) -> io::Result<bool> {
    use super::video::leave_player;

    let regions = regions(app);
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) if shared_click(app, &regions, at)? => {}
        MouseEventKind::Down(MouseButton::Left) if video_picture(app).contains(at) => {}
        MouseEventKind::Down(MouseButton::Left | MouseButton::Right) => leave_player(app)?,
        _ => {}
    }
    Ok(false)
}

/// The cells of the video frame. Before the first frame, the whole box counts.
#[cfg(feature = "video")]
fn video_picture(app: &App) -> Rect {
    let frame = app
        .video
        .as_ref()
        .and_then(|video| video.current_frame.as_ref())
        .map(crate::player::Frame::dimensions);
    let Some(frame) = frame else {
        return app.image_rect;
    };
    tuilibr::pictures::motion::frame_box::cell_box(frame, app.image_rect, app.cell_px)
}

#[cfg(test)]
mod tests;
