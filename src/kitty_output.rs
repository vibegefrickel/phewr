//! The Kitty graphics writes per frame: the fullscreen image, the video frame and the grid.

use crate::app::App;
use crate::encoder::{self, GraphicsBackend};
use crossterm::{cursor, queue};
use image::RgbaImage;
use ratatui::prelude::*;
use std::io::{self, Write};

pub(crate) fn render_fullscreen_image(app: &mut App) -> io::Result<()> {
    app.with_graphics_out(render_fullscreen_image_to)
}

/// Write the fullscreen image to `out`, so that the tests can read the escape sequences.
pub(crate) fn render_fullscreen_image_to(app: &mut App, out: &mut impl Write) -> io::Result<()> {
    use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};

    // Wrap the delete + retransmit in a synchronized update so the terminal swaps
    // atomically instead of briefly showing a blank viewport (visible as flicker
    // when zooming or panning, which redraw rapidly).
    queue!(out, BeginSynchronizedUpdate)?;
    app.graphics_delete_all_to(out)?;

    if let Some(img) = &app.loaded {
        let (column, row) = centered_cell(app.image_rect, app.cell_px, img);
        queue!(out, cursor::MoveTo(column, row))?;
        app.graphics
            .transmit(out, img, &encoder::DisplayOptions::default())?;
    }

    queue!(out, EndSynchronizedUpdate)?;
    out.flush()
}

/// The cell where `img` starts when it sits in the middle of `rect`, in whole cells.
fn centered_cell(rect: Rect, cell_px: (u32, u32), img: &RgbaImage) -> (u16, u16) {
    let cols = u16::try_from(img.width().div_ceil(cell_px.0)).unwrap_or(u16::MAX);
    let rows = u16::try_from(img.height().div_ceil(cell_px.1)).unwrap_or(u16::MAX);
    (
        rect.x + rect.width.saturating_sub(cols) / 2,
        rect.y + rect.height.saturating_sub(rows) / 2,
    )
}

#[cfg(feature = "video")]
const VIDEO_IMAGE_ID: u32 = 900;

#[cfg(feature = "video")]
pub(crate) fn render_video_frame(app: &mut App) -> io::Result<()> {
    app.with_graphics_out(|app, out| render_video_frame_to(app, out))
}

/// Write the current video frame to `out`, so that the tests can read the escape sequences.
#[cfg(feature = "video")]
fn render_video_frame_to(app: &App, out: &mut impl Write) -> io::Result<()> {
    use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};

    if let Some(v) = &app.video
        && let Some(img) = &v.current_frame
    {
        let (column, row) = centered_cell(app.image_rect, app.cell_px, img);
        queue!(out, BeginSynchronizedUpdate)?;
        queue!(out, cursor::MoveTo(column, row))?;
        app.graphics.transmit(
            out,
            img,
            &encoder::DisplayOptions {
                id: Some(VIDEO_IMAGE_ID),
                ..encoder::DisplayOptions::default()
            },
        )?;
        queue!(out, EndSynchronizedUpdate)?;
    }

    out.flush()
}

pub(crate) fn emit_new_thumbnails(app: &mut App, new_indices: &[usize]) -> io::Result<()> {
    app.with_graphics_out(|app, out| emit_new_thumbnails_to(app, new_indices, out))
}

/// Write the new thumbnails to `out`, so that the tests can read the escape sequences.
fn emit_new_thumbnails_to(
    app: &mut App,
    new_indices: &[usize],
    out: &mut impl Write,
) -> io::Result<()> {
    let mut newly_transmitted: Vec<u32> = Vec::new();
    for (vis_idx, position) in app.gallery.visible_range().enumerate() {
        let img_idx = app.gallery.filtered_indices[position];
        if !new_indices.contains(&img_idx) {
            continue;
        }
        delete_stale_images_to(app, out, |cell| cell == img_idx)?;
        if let Some((img, id)) = app.thumb_cache.peek(img_idx) {
            transmit_thumbnail(app, out, vis_idx, img, id)?;
            newly_transmitted.push(id);
        }
    }
    out.flush()?;
    app.transmitted_image_ids.extend(newly_transmitted);
    Ok(())
}

/// Send the thumbnail `img` with the Kitty id `id` into the visible cell `vis_idx`. The caller
/// records the id after the flush, because `img` borrows the cache of `app`.
fn transmit_thumbnail(
    app: &App,
    out: &mut impl Write,
    vis_idx: usize,
    img: &RgbaImage,
    id: u32,
) -> io::Result<()> {
    let origin = gallery_image_origin(app, app.gallery.cell_rect(vis_idx), img);
    queue!(out, cursor::MoveTo(origin.cell.0, origin.cell.1))?;
    app.graphics.transmit(
        out,
        img,
        &encoder::DisplayOptions {
            id: Some(id),
            offset: origin.offset,
            ..encoder::DisplayOptions::default()
        },
    )
}

/// Delete the previews that a sharp thumbnail replaced, for the cells that `select` picks. The
/// sharp image comes with a new id, so the terminal would keep the preview and its pixels.
fn delete_stale_images_to(
    app: &mut App,
    out: &mut impl Write,
    select: impl Fn(usize) -> bool,
) -> io::Result<()> {
    let mut kept = Vec::new();
    for (cell, id) in std::mem::take(&mut app.stale_image_ids) {
        if select(cell) {
            app.graphics.delete_image_to(out, id)?;
            app.transmitted_image_ids.remove(&id);
        } else {
            kept.push((cell, id));
        }
    }
    app.stale_image_ids = kept;
    Ok(())
}

/// Where a thumbnail starts: the cell of the grid and the pixel offset inside that cell.
struct ImageOrigin {
    cell: (u16, u16),
    offset: (u32, u32),
}

/// Center the fitted thumbnail in its grid cell, to the pixel. The thumbnail is never larger
/// than the cell, so the offset stays inside the first cell.
fn gallery_image_origin(app: &App, cell_rect: Rect, img: &RgbaImage) -> ImageOrigin {
    let (cell_width, cell_height) = (app.cell_px.0.max(1), app.cell_px.1.max(1));
    let gap_x = (u32::from(cell_rect.width) * cell_width).saturating_sub(img.width()) / 2;
    let gap_y = (u32::from(cell_rect.height) * cell_height).saturating_sub(img.height()) / 2;
    let cell_x = cell_rect.x + u16::try_from(gap_x / cell_width).unwrap_or(u16::MAX);
    let cell_y = cell_rect.y + u16::try_from(gap_y / cell_height).unwrap_or(u16::MAX);
    ImageOrigin {
        cell: (cell_x, cell_y),
        offset: (gap_x % cell_width, gap_y % cell_height),
    }
}

pub(crate) fn render_gallery_images(app: &mut App) -> io::Result<()> {
    // The cells that left the screen need no decode. The visible ones queue again below. The
    // first screen waits for the cell shape, so that no cell decodes for two sizes.
    app.clear_thumb_queue();
    app.request_cell_shape();
    if !app.cell_shape_pending() {
        for position in app.gallery.visible_range() {
            let img_idx = app.gallery.filtered_indices[position];
            app.spawn_thumb_decode(img_idx, app.gallery.thumb_box());
        }
    }

    app.with_graphics_out(place_gallery_images_to)
}

/// Write the visible thumbnails to `out`, so that the tests can read the escape sequences.
fn place_gallery_images_to(app: &mut App, out: &mut impl Write) -> io::Result<()> {
    if app.graphics_storage_dirty {
        app.graphics_delete_all_to(out)?;
        app.graphics_storage_dirty = false;
        app.stale_image_ids.clear();
    } else {
        // Drop visible placements only; keep stored image data so already-transmitted thumbs
        // can be re-placed with a cheap `a=p` instead of a full PNG retransmit.
        app.graphics.clear_placements_to(out)?;
        delete_stale_images_to(app, out, |_| true)?;
    }

    let mut newly_transmitted: Vec<u32> = Vec::new();
    for (vis_idx, position) in app.gallery.visible_range().enumerate() {
        let img_idx = app.gallery.filtered_indices[position];
        let Some((img, id)) = app.thumb_cache.peek(img_idx) else {
            continue;
        };
        if app.transmitted_image_ids.contains(&id) {
            let origin = gallery_image_origin(app, app.gallery.cell_rect(vis_idx), img);
            queue!(out, cursor::MoveTo(origin.cell.0, origin.cell.1))?;
            app.graphics.place_by_id_to(out, id, origin.offset)?;
        } else {
            transmit_thumbnail(app, out, vis_idx, img, id)?;
            newly_transmitted.push(id);
        }
    }

    out.flush()?;
    app.transmitted_image_ids.extend(newly_transmitted);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{emit_new_thumbnails_to, place_gallery_images_to, render_fullscreen_image_to};
    use crate::app::App;
    use crate::image_list::SharedImageList;
    use crate::test_support::text;
    use image::RgbaImage;
    use ratatui::layout::Rect;
    use std::path::PathBuf;

    const TRANSMIT: &str = "\x1b_Ga=T,";
    const PLACE: &str = "\x1b_Ga=p,";
    const DELETE_ALL: &str = "\x1b_Ga=d,d=A,";
    const CLEAR_PLACEMENTS: &str = "\x1b_Ga=d,d=a,";

    /// A gallery of `count` photos with a decoded thumbnail for each, so no decode starts.
    /// The cells are 20x6 cells of 8x16 px, so 160x96 px.
    fn thumbnail_app(count: usize) -> App {
        let mut app = App::new((8, 16), SharedImageList::new());
        app.images = (0..count)
            .map(|index| PathBuf::from(format!("/photos/{index}.jpg")))
            .collect();
        app.gallery.filtered_indices = (0..count).collect();
        // 4 rows of 6 and 5 columns of 20, with 5 and 6 gaps.
        app.gallery.update_grid(Rect::new(0, 0, 106, 29));
        for index in 0..count {
            app.thumb_cache.insert(index, RgbaImage::new(4, 4));
        }
        app
    }

    /// The write half of `render_gallery_images`, for the visible cells of the grid.
    fn place_visible(app: &mut App, out: &mut Vec<u8>) {
        place_gallery_images_to(app, out).unwrap();
    }

    #[test]
    fn fullscreen_centers_the_image_in_a_synchronized_update() {
        let mut app = App::new((8, 16), SharedImageList::new());
        app.loaded = Some(RgbaImage::new(16, 32));
        app.image_rect = Rect::new(2, 1, 10, 10);
        app.transmitted_image_ids.insert(7);
        let mut out = Vec::new();

        render_fullscreen_image_to(&mut app, &mut out).unwrap();

        let text = text(&out);
        assert!(text.starts_with("\x1b[?2026h"));
        assert!(text.ends_with("\x1b[?2026l"));
        assert!(text.contains(DELETE_ALL));
        assert!(text.contains("\x1b[6;7H"), "2 by 2 cells in the middle");
        assert!(text.contains(TRANSMIT));
        assert!(app.transmitted_image_ids.is_empty());
    }

    #[test]
    fn fullscreen_without_an_image_only_deletes() {
        let mut app = App::new((8, 16), SharedImageList::new());
        let mut out = Vec::new();

        render_fullscreen_image_to(&mut app, &mut out).unwrap();

        let text = text(&out);
        assert!(text.contains(DELETE_ALL));
        assert!(!text.contains(TRANSMIT));
    }

    #[test]
    fn gallery_transmits_once_and_then_places_by_id() {
        let mut app = thumbnail_app(3);
        app.graphics_storage_dirty = true;
        let mut out = Vec::new();

        place_visible(&mut app, &mut out);

        let first = text(&out);
        assert!(first.contains(DELETE_ALL));
        assert_eq!(first.matches(TRANSMIT).count(), 3);
        assert!(!first.contains(PLACE));
        assert_eq!(app.transmitted_image_ids.len(), 3);
        assert!(!app.graphics_storage_dirty);

        out.clear();
        place_visible(&mut app, &mut out);

        let second = text(&out);
        assert!(second.starts_with(CLEAR_PLACEMENTS));
        assert!(!second.contains(DELETE_ALL));
        assert!(!second.contains(TRANSMIT));
        assert_eq!(second.matches(PLACE).count(), 3);
    }

    #[test]
    fn gallery_centers_each_thumbnail_to_the_pixel() {
        let mut app = thumbnail_app(1);
        assert_eq!((app.gallery.thumb_cols, app.gallery.thumb_rows), (20, 6));
        let mut out = Vec::new();

        place_visible(&mut app, &mut out);

        // 4x4 px in 160x96 px: a gap of 78 px (9 cells and 6 px) and 46 px (2 rows and 14 px),
        // after the edge gap of 1 cell.
        let first = text(&out);
        assert!(
            first.contains("\x1b[4;11H"),
            "the cell of the origin: {first}"
        );
        assert!(first.contains(",X=6,Y=14,"), "the pixel offset: {first}");

        app.transmitted_image_ids.insert(1);
        out.clear();
        place_visible(&mut app, &mut out);
        assert!(text(&out).contains("\x1b_Ga=p,q=2,C=1,i=1,X=6,Y=14\x1b\\"));
    }

    #[test]
    fn a_thumbnail_as_large_as_its_box_fills_its_cell() {
        let mut app = thumbnail_app(1);
        let (width, height) =
            crate::app::thumbnails::thumb_target_px(app.gallery.thumb_box(), (8, 16));
        assert_eq!((width, height), (160, 96), "the whole cell, no padding");
        app.thumb_cache.insert(0, RgbaImage::new(width, height));
        let mut out = Vec::new();

        place_visible(&mut app, &mut out);

        let text = text(&out);
        assert!(
            text.contains("\x1b[2;2H"),
            "the first cell after the gaps: {text}"
        );
        assert!(!text.contains(",X="), "no pixel offset: {text}");
        assert!(!text.contains(",Y="), "no pixel offset: {text}");
    }

    #[test]
    fn gallery_skips_a_cell_without_a_thumbnail() {
        let mut app = thumbnail_app(2);
        app.images.push(PathBuf::from("/photos/missing.jpg"));
        app.gallery.filtered_indices.push(2);
        let mut out = Vec::new();

        place_visible(&mut app, &mut out);

        assert_eq!(text(&out).matches(TRANSMIT).count(), 2);
        assert_eq!(app.transmitted_image_ids.len(), 2);
    }

    #[test]
    fn new_thumbnails_go_out_without_a_delete() {
        let mut app = thumbnail_app(3);
        let mut out = Vec::new();

        emit_new_thumbnails_to(&mut app, &[1, 2, 5], &mut out).unwrap();

        let text = text(&out);
        assert_eq!(text.matches(TRANSMIT).count(), 2);
        assert!(!text.contains("\x1b_Ga=d"));
        assert_eq!(app.transmitted_image_ids.len(), 2);
        assert!(text.contains("\x1b[4;32H"), "centered in the second cell");
    }

    #[test]
    fn a_sharp_thumbnail_deletes_its_preview_before_the_transmit() {
        let mut app = thumbnail_app(2);
        app.images.push(PathBuf::from("/photos/2.heic"));
        app.gallery.filtered_indices.push(2);
        assert!(app.store_thumbnail(2, RgbaImage::new(4, 4), false));
        let (_, preview_id) = app.thumb_cache.peek(2).unwrap();
        let mut out = Vec::new();
        emit_new_thumbnails_to(&mut app, &[2], &mut out).unwrap();
        assert!(!text(&out).contains("\x1b_Ga=d"));
        assert!(app.transmitted_image_ids.contains(&preview_id));

        assert!(app.store_thumbnail(2, RgbaImage::new(4, 4), true));
        let (_, sharp_id) = app.thumb_cache.peek(2).unwrap();
        let mut out = Vec::new();
        emit_new_thumbnails_to(&mut app, &[2], &mut out).unwrap();

        let text = text(&out);
        let delete = format!("\x1b_Ga=d,d=I,i={preview_id},q=2\x1b\\");
        assert!(
            text.find(&delete).unwrap() < text.find(TRANSMIT).unwrap(),
            "the preview goes before the sharp image comes: {text}"
        );
        assert_eq!(text.matches(TRANSMIT).count(), 1);
        assert!(text.contains(&format!(",i={sharp_id}")));
        assert!(!app.transmitted_image_ids.contains(&preview_id));
        assert!(app.transmitted_image_ids.contains(&sharp_id));
        assert!(app.stale_image_ids.is_empty());
    }

    #[test]
    fn a_redraw_of_the_grid_deletes_the_replaced_previews() {
        let mut app = thumbnail_app(2);
        app.stale_image_ids.push((5, 99));
        app.transmitted_image_ids.insert(99);
        let mut out = Vec::new();
        place_visible(&mut app, &mut out);
        assert!(text(&out).contains("\x1b_Ga=d,d=I,i=99,"));
        assert!(app.stale_image_ids.is_empty());
        assert!(!app.transmitted_image_ids.contains(&99));

        // A full redraw deletes every image, so the list only empties.
        app.stale_image_ids.push((5, 98));
        app.graphics_storage_dirty = true;
        let mut out = Vec::new();
        place_visible(&mut app, &mut out);
        assert!(text(&out).contains(DELETE_ALL));
        assert!(!text(&out).contains("d=I"));
        assert!(app.stale_image_ids.is_empty());
    }

    #[cfg(feature = "video")]
    #[test]
    fn a_video_without_a_frame_writes_nothing() {
        let app = App::new((8, 16), SharedImageList::new());
        let mut out = Vec::new();

        super::render_video_frame_to(&app, &mut out).unwrap();

        assert!(out.is_empty());
    }
}
