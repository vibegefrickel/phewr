//! The Kitty writes per frame: the fullscreen image, the video frame, the grid and its loops.

mod grid;
mod grid_icons;
mod loops;

use crate::app::App;
use crossterm::{cursor, queue};
pub(crate) use grid::{emit_new_thumbnails, render_gallery_images};
pub(crate) use grid_icons::{GridIcons, IconCell, sync_grid_icons};
pub(crate) use loops::play_grid_loops;
use std::io::{self, Write};
use tuilibr::pictures::kitty;
use tuilibr::pictures::kitty::placement::{filled_placement, native_placement, within_payload_cap};

pub(crate) fn render_fullscreen_image(app: &mut App) -> io::Result<()> {
    app.with_graphics_out(render_fullscreen_image_to)
}

/// Write the fullscreen image. An opaque image is RGB already and needs no scan.
pub(crate) fn render_fullscreen_image_to(app: &mut App, out: &mut impl Write) -> io::Result<()> {
    use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};

    // The synchronized update swaps the images without a blank view between.
    queue!(out, BeginSynchronizedUpdate)?;
    app.graphics_delete_all_to(out)?;

    if let Some(img) = &app.loaded {
        let fitted = within_payload_cap(img);
        let placement = if app.slideshow_running() {
            filled_placement(app.image_rect, app.cell_px, img.dimensions())
        } else {
            native_placement(app.image_rect, app.cell_px, img, &fitted)
        };
        queue!(out, cursor::MoveTo(placement.cells.x, placement.cells.y))?;
        app.graphics.transmit(
            out,
            fitted.as_ref().into(),
            &kitty::DisplayOptions {
                columns: placement.columns,
                rows: placement.rows,
                // The slideshow badge sits on top of the picture.
                below_backgrounds: app.slideshow_running(),
                ..kitty::DisplayOptions::default()
            },
        )?;
    }

    queue!(out, EndSynchronizedUpdate)?;
    out.flush()
}

/// The image id of the terminal player. Each frame replaces the last one under this id, and no
/// empty box shows between two frames. The grid ids count up from 1.
#[cfg(feature = "video")]
const VIDEO_IMAGE_ID: u32 = u32::MAX - 1;

#[cfg(feature = "video")]
pub(crate) fn render_video_frame(app: &mut App) -> io::Result<()> {
    let clear = app.clear_before_video
        && !app.help_visible
        && app
            .video
            .as_ref()
            .is_some_and(|v| v.current_frame.is_some());
    app.with_graphics_out(|app, out| {
        if render_video_frame_to(app, out, clear)? && clear {
            app.clear_before_video = false;
            app.screen.forget_placements();
        }
        Ok(())
    })
}

/// Write the current video frame. Returns `true` when the frame went out.
///
/// The terminal scales the frame to the cell box (`c=`, `r=`), in a synchronized update. With
/// `clear`, the update first removes all placements, and nothing flashes. The help popup gets
/// no frame: Kitty draws a picture above the text.
#[cfg(feature = "video")]
pub(crate) fn render_video_frame_to(
    app: &App,
    out: &mut impl Write,
    clear: bool,
) -> io::Result<bool> {
    use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};

    let frame = app
        .video
        .as_ref()
        .and_then(|v| v.current_frame.as_ref())
        .filter(|_| !app.help_visible);
    let Some(frame) = frame else {
        out.flush()?;
        return Ok(false);
    };
    let pixels = frame.pixels();
    let cells = crate::player::cell_box(pixels.dimensions(), app.image_rect, app.cell_px);
    if cells.width == 0 || cells.height == 0 {
        return Ok(false);
    }
    queue!(out, BeginSynchronizedUpdate)?;
    if clear {
        kitty::protocol::clear_placements_to(out)?;
    }
    queue!(out, cursor::MoveTo(cells.x, cells.y))?;
    app.graphics.transmit(
        out,
        pixels,
        &kitty::DisplayOptions {
            id: Some(VIDEO_IMAGE_ID),
            placement: None,
            cell_box: Some((cells.width, cells.height)),
            transmit_only: false,
            columns: None,
            rows: None,
            below_backgrounds: app.slideshow_running(),
            transient: true,
        },
    )?;
    queue!(out, EndSynchronizedUpdate)?;
    out.flush()?;
    Ok(true)
}

#[cfg(test)]
mod tests;
