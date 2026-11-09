//! The Kitty writes per frame: the fullscreen image, the video frame, the grid and its loops.

mod grid;
mod grid_icons;
mod loops;
mod terminal_images;

use crate::app::App;
use crate::decode::ScreenImage;
use crate::encoder::{self, MAX_PAYLOAD_BYTES};
use crate::gallery::{box_px, centered};
use crossterm::{cursor, queue};
pub(crate) use grid::{emit_new_thumbnails, render_gallery_images};
pub(crate) use grid_icons::{GridIcons, IconCell, sync_grid_icons};
pub(crate) use loops::play_grid_loops;
use ratatui::prelude::*;
use std::borrow::Cow;
use std::io::{self, Write};
pub(crate) use terminal_images::TerminalImages;

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
            &encoder::DisplayOptions {
                columns: placement.columns,
                rows: placement.rows,
                ..encoder::DisplayOptions::default()
            },
        )?;
    }

    queue!(out, EndSynchronizedUpdate)?;
    out.flush()
}

/// `img`, or a copy shrunk until its payload fits [`MAX_PAYLOAD_BYTES`]: Ghostty 1.4.0 refuses
/// a larger payload. The decode box keeps the image below the cap, and this is a fallback.
fn within_payload_cap(img: &ScreenImage) -> Cow<'_, ScreenImage> {
    let (width, height) = img.dimensions();
    let pixels = u64::from(width) * u64::from(height);
    // Only an RGBA image above the cap as RGBA needs the scan.
    let transparent = matches!(img, ScreenImage::Rgba(_))
        && pixels * 4 > MAX_PAYLOAD_BYTES as u64
        && encoder::Pixels::from(img).opaque_rgb().is_none();
    let bytes_per_pixel = if transparent { 4 } else { 3 };
    let wire_bytes = pixels * bytes_per_pixel;
    if wire_bytes <= MAX_PAYLOAD_BYTES as u64 {
        return Cow::Borrowed(img);
    }
    // The side scales with the square root of the area.
    let scale = (MAX_PAYLOAD_BYTES as f64 / wire_bytes as f64).sqrt();
    let side = |value: u32| ((f64::from(value) * scale) as u32).max(1);
    let (width, height) = (side(width), side(height));
    Cow::Owned(img.crop_resized((0, 0, img.width(), img.height()), (width, height)))
}

/// The cells of the fullscreen image and the one side that the terminal scales it to. With one
/// side, the terminal takes the other side from the aspect ratio, so the image never stretches to
/// a rounded box.
#[derive(Debug, PartialEq, Eq)]
struct Placement {
    cells: Rect,
    columns: Option<u16>,
    rows: Option<u16>,
}

/// The image at its own pixel size, centered. The terminal scales a copy that the payload cap
/// shrank back to the width of `img` in cells.
fn native_placement(
    rect: Rect,
    cell_px: (u32, u32),
    img: &ScreenImage,
    fitted: &ScreenImage,
) -> Placement {
    let columns = (fitted.dimensions() != img.dimensions()).then(|| {
        let columns = (f64::from(img.width()) / f64::from(cell_px.0))
            .round()
            .max(1.0);
        columns as u16
    });
    Placement {
        cells: centered_image(rect, cell_px, img.dimensions()),
        columns,
        rows: None,
    }
}

/// The image scaled up or down to fill `rect` with its aspect, for the slideshow. A portrait
/// image reaches the top and the bottom of `rect`, a landscape image the left and the right.
fn filled_placement(rect: Rect, cell_px: (u32, u32), size: (u32, u32)) -> Placement {
    let (box_w, box_h) = box_px(rect, cell_px);
    let (width, height) = (u64::from(size.0), u64::from(size.1));
    if width == 0 || height == 0 || box_w == 0 || box_h == 0 {
        return Placement {
            cells: centered_image(rect, cell_px, size),
            columns: None,
            rows: None,
        };
    }
    let side = |value: u64| u32::try_from(value).unwrap_or(u32::MAX);
    if width * u64::from(box_h) >= height * u64::from(box_w) {
        let cells = centered_image(
            rect,
            cell_px,
            (box_w, side(u64::from(box_w) * height / width)),
        );
        Placement {
            cells,
            columns: Some(cells.width),
            rows: None,
        }
    } else {
        let cells = centered_image(
            rect,
            cell_px,
            (side(u64::from(box_h) * width / height), box_h),
        );
        Placement {
            cells,
            columns: None,
            rows: Some(cells.height),
        }
    }
}

/// The cells of an image of `size` pixels centered in `rect`.
fn centered_image(rect: Rect, cell_px: (u32, u32), size: (u32, u32)) -> Rect {
    let cols = u16::try_from(size.0.div_ceil(cell_px.0)).unwrap_or(u16::MAX);
    let rows = u16::try_from(size.1.div_ceil(cell_px.1)).unwrap_or(u16::MAX);
    centered(rect, cols, rows)
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
            app.grid_icons.forget_placements();
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
        encoder::kitty::clear_placements_to(out)?;
    }
    queue!(out, cursor::MoveTo(cells.x, cells.y))?;
    app.graphics.transmit(
        out,
        pixels,
        &encoder::DisplayOptions {
            id: Some(VIDEO_IMAGE_ID),
            cell_box: Some((cells.width, cells.height)),
            transmit_only: false,
            columns: None,
            rows: None,
            transient: true,
        },
    )?;
    queue!(out, EndSynchronizedUpdate)?;
    out.flush()?;
    Ok(true)
}

#[cfg(test)]
mod tests;
