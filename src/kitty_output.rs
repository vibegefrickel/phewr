//! The Kitty graphics writes per frame: the fullscreen image, the video frame and the grid.

use crate::app::App;
use crate::encoder::{self, GraphicsBackend};
use crossterm::{cursor, queue};
use image::RgbaImage;
use lru::LruCache;
use ratatui::prelude::*;
use std::collections::HashSet;
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
            .transmit(out, img.into(), &encoder::DisplayOptions::default())?;
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

/// The one image id of the terminal player. Each frame replaces the last one under this id,
/// so the terminal never shows an empty box between two frames. The id sits at the top of
/// the range, far from the ids of the grid, which count up from 1 (`ThumbnailCache::next_id`).
#[cfg(feature = "video")]
const VIDEO_IMAGE_ID: u32 = u32::MAX - 1;

#[cfg(feature = "video")]
pub(crate) fn render_video_frame(app: &mut App) -> io::Result<()> {
    use crossterm::terminal::BeginSynchronizedUpdate;

    let clear = app.clear_before_video
        && !app.help_visible
        && app
            .video
            .as_ref()
            .is_some_and(|v| v.current_frame.is_some());
    app.with_graphics_out(|app, out| {
        if clear {
            // The same synchronized update holds the delete and the frame, so nothing flashes.
            queue!(out, BeginSynchronizedUpdate)?;
            app.graphics.clear_placements_to(out)?;
            app.clear_before_video = false;
        }
        render_video_frame_to(app, out)
    })
}

/// Write the current video frame to `out`, so that the tests can read the escape sequences.
///
/// The frame is small. The terminal scales it to the cell box (`c=`, `r=`) in the middle of
/// the view, in whole cells and without a pixel offset, inside a synchronized update. The
/// help popup gets no frame, because Kitty draws a picture above the text. The key that
/// closes the help writes the frame again.
#[cfg(feature = "video")]
pub(crate) fn render_video_frame_to(app: &App, out: &mut impl Write) -> io::Result<()> {
    use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};

    if !app.help_visible
        && let Some(v) = &app.video
        && let Some(frame) = &v.current_frame
    {
        let pixels = frame.pixels();
        let cells = crate::player::cell_box(pixels.dimensions(), app.image_rect, app.cell_px);
        if cells.width == 0 || cells.height == 0 {
            return Ok(());
        }
        queue!(out, BeginSynchronizedUpdate)?;
        queue!(out, cursor::MoveTo(cells.x, cells.y))?;
        app.graphics.transmit(
            out,
            pixels,
            &encoder::DisplayOptions {
                id: Some(VIDEO_IMAGE_ID),
                cell_box: Some((cells.width, cells.height)),
            },
        )?;
        queue!(out, EndSynchronizedUpdate)?;
    }

    out.flush()
}

/// The bytes of RGBA pixels that viewr leaves in the storage of the terminal, at most. Ghostty
/// evicts images at 320 MB, and herdr stores each image twice, so the budget stays well below
/// half of that. Above the budget, the grid deletes its oldest images that no cell shows.
pub(crate) const TERMINAL_IMAGE_BUDGET: usize = 128 * 1024 * 1024;

/// The images that viewr transmitted and the terminal still stores, newest last, with their
/// pixel bytes. A stored image can be placed again by its id (`a=p`) instead of a transmit.
pub(crate) struct TerminalImages {
    stored: LruCache<u32, usize>,
    bytes: usize,
    /// The bytes that the stored images may take. A test sets a small one.
    pub(crate) budget: usize,
}

impl TerminalImages {
    pub(crate) fn new() -> Self {
        Self {
            stored: LruCache::unbounded(),
            bytes: 0,
            budget: TERMINAL_IMAGE_BUDGET,
        }
    }

    /// Record a transmitted image of `bytes` pixel bytes as the newest one.
    pub(crate) fn insert(&mut self, id: u32, bytes: usize) {
        self.remove(id);
        self.stored.put(id, bytes);
        self.bytes += bytes;
    }

    pub(crate) fn remove(&mut self, id: u32) {
        if let Some(bytes) = self.stored.pop(&id) {
            self.bytes -= bytes;
        }
    }

    pub(crate) fn contains(&self, id: u32) -> bool {
        self.stored.contains(&id)
    }

    /// Make `id` the newest image, because a cell shows it again.
    pub(crate) fn touch(&mut self, id: u32) {
        self.stored.promote(&id);
    }

    pub(crate) fn clear(&mut self) {
        self.stored.clear();
        self.bytes = 0;
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.stored.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.stored.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    /// The oldest ids that must go so that the rest fits the budget. The ids in `keep` stay,
    /// because a cell shows them. They are the newest ones, so the search stops at the first.
    fn over_budget(&mut self, keep: &HashSet<u32>) -> Vec<u32> {
        let mut dropped = Vec::new();
        while self.bytes > self.budget {
            let Some((&id, _)) = self.stored.peek_lru() else {
                break;
            };
            if keep.contains(&id) {
                break;
            }
            self.remove(id);
            dropped.push(id);
        }
        dropped
    }
}

/// The pixel bytes of new thumbnails that one pass of the run loop writes, about 9 sharp cells
/// of the Retina grid of the bench. The write of 36 cells takes about 11 ms, and one write
/// would hold the first picture and the keys for that time. The loop writes the rest in the
/// next passes, without a wait.
///
/// The count is the RGBA bytes of the cache, not the bytes on the wire: an opaque picture
/// goes out as RGB (3/4 of the count), and the direct medium adds base64 (4/3 of its data).
/// The first picture of a pass always goes out, so a pass can end one picture above the
/// limit. The temp file medium writes one file per picture.
const EMIT_PASS_BYTES: usize = 4 * 1024 * 1024;

/// Write the new thumbnails of `pending` that fit into one pass, and keep the visible rest in
/// `pending` for the next pass.
pub(crate) fn emit_new_thumbnails(app: &mut App, pending: &mut Vec<usize>) -> io::Result<()> {
    app.with_graphics_out(|app, out| {
        *pending = emit_new_thumbnails_to(app, pending, out)?;
        Ok(())
    })
}

/// Write the new thumbnails to `out` in grid order, so that the tests can read the escape
/// sequences. The synchronized update swaps the preview for the sharp image without a blank
/// cell between. After `EMIT_PASS_BYTES`, the visible cells that are left come back for the
/// next pass. A cell that left the screen gets its picture from the next render of the grid.
fn emit_new_thumbnails_to(
    app: &mut App,
    new_indices: &[usize],
    out: &mut impl Write,
) -> io::Result<Vec<usize>> {
    use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};

    queue!(out, BeginSynchronizedUpdate)?;
    delete_evicted_images_to(app, out)?;
    let new_indices: HashSet<usize> = new_indices.iter().copied().collect();
    let mut shown = HashSet::new();
    let mut written = 0;
    let mut left = Vec::new();
    for (vis_idx, position) in app.gallery.visible_range().enumerate() {
        let img_idx = app.gallery.filtered_indices[position];
        if let Some((_, id)) = app.thumb_cache.peek(img_idx) {
            shown.insert(id);
        }
        if !new_indices.contains(&img_idx) {
            continue;
        }
        if written >= EMIT_PASS_BYTES {
            left.push(img_idx);
            continue;
        }
        delete_stale_images_to(app, out, |cell| cell == img_idx)?;
        written += transmit_cell(app, out, vis_idx, img_idx)?;
    }
    trim_to_budget(app, out, &shown)?;
    queue!(out, EndSynchronizedUpdate)?;
    out.flush()?;
    Ok(left)
}

/// Transmit the thumbnail of the cell `img_idx` into the visible cell `vis_idx`, and record its
/// id and its bytes. Returns the bytes. A cell without a thumbnail writes nothing.
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
    let (origin, cell_box) = grid_placement(app, vis_idx, picture.cells);
    queue!(out, cursor::MoveTo(origin.0, origin.1))?;
    app.graphics.transmit(
        out,
        (&picture.image).into(),
        &encoder::DisplayOptions {
            id: Some(id),
            cell_box,
        },
    )?;
    app.terminal_images.insert(id, bytes);
    Ok(bytes)
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
            delete_image_to(app, out, id)?;
        } else {
            kept.push((cell, id));
        }
    }
    app.stale_image_ids = kept;
    Ok(())
}

/// Delete the images of the thumbnails that the cache dropped. Their cells show nothing, and
/// a cell that comes back gets a new decode and a new id.
fn delete_evicted_images_to(app: &mut App, out: &mut impl Write) -> io::Result<()> {
    for id in app.thumb_cache.take_evicted_ids() {
        delete_image_to(app, out, id)?;
    }
    Ok(())
}

/// Delete the oldest images that no visible cell shows, until the stored bytes fit the budget.
/// A cell that shows one of them later transmits its thumbnail again from the cache.
fn trim_to_budget(app: &mut App, out: &mut impl Write, shown: &HashSet<u32>) -> io::Result<()> {
    for id in app.terminal_images.over_budget(shown) {
        app.graphics.delete_image_to(out, id)?;
    }
    Ok(())
}

/// Delete the image `id` in the terminal, if viewr transmitted it, and forget it.
fn delete_image_to(app: &mut App, out: &mut impl Write, id: u32) -> io::Result<()> {
    if app.terminal_images.contains(id) {
        app.graphics.delete_image_to(out, id)?;
        app.terminal_images.remove(id);
    }
    Ok(())
}

/// Where the thumbnail of the visible cell `vis_idx` goes: its `cells` in the middle of the
/// grid cell, and the cell box that the terminal scales it to.
///
/// Every grid picture is padded to whole cells, at most the base cell box: a sharp thumbnail
/// to its pixel size, a preview to the shape of those cells at its own size. `c=` and `r=` fit
/// both to the same cells, centered in the grid cell. A sharp picture of the exact size scales
/// 1:1, and one off by a pixel, after a rounded cell size, stays in its cells. No picture needs
/// a pixel offset, which a relaying multiplexer gets wrong.
fn grid_placement(
    app: &App,
    vis_idx: usize,
    cells: (u16, u16),
) -> ((u16, u16), Option<(u16, u16)>) {
    let cell = app.gallery.cell_rect(vis_idx);
    let cols = cells.0.clamp(1, app.gallery.thumb_cols.max(1));
    let rows = cells.1.clamp(1, app.gallery.thumb_rows.max(1));
    let origin = (
        cell.x + cell.width.saturating_sub(cols) / 2,
        cell.y + cell.height.saturating_sub(rows) / 2,
    );
    (origin, Some((cols, rows)))
}

pub(crate) fn render_gallery_images(app: &mut App) -> io::Result<()> {
    // The cells that left the screen need no decode. The visible ones queue again below. The
    // first screen waits for the cell shape, so that no cell decodes for two sizes.
    app.clear_thumb_queue();
    app.request_cell_shape();
    app.thumb_cache.fit_capacity(app.gallery.visible_cells());
    app.thumb_cache
        .set_visible(app.gallery.visible_items().map(|(_, index)| index));
    for position in app.gallery.visible_range() {
        app.thumb_cache
            .touch(app.gallery.filtered_indices[position]);
    }
    if !app.cell_shape_pending() {
        for position in app.gallery.visible_range() {
            let img_idx = app.gallery.filtered_indices[position];
            app.spawn_thumb_decode(img_idx, app.gallery.thumb_box());
        }
    }

    app.with_graphics_out(place_gallery_images_to)
}

/// Write the visible thumbnails to `out`, so that the tests can read the escape sequences. The
/// synchronized update swaps the old screen for the new one without a blank grid between.
fn place_gallery_images_to(app: &mut App, out: &mut impl Write) -> io::Result<()> {
    use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};

    queue!(out, BeginSynchronizedUpdate)?;
    if app.graphics_storage_dirty {
        app.graphics_delete_all_to(out)?;
        app.graphics_storage_dirty = false;
        app.stale_image_ids.clear();
        app.thumb_cache.take_evicted_ids();
    } else {
        // Drop visible placements only; keep stored image data so already-transmitted thumbs
        // can be re-placed with a cheap `a=p` instead of a full PNG retransmit.
        app.graphics.clear_placements_to(out)?;
        delete_stale_images_to(app, out, |_| true)?;
        delete_evicted_images_to(app, out)?;
    }

    let mut shown = HashSet::new();
    for (vis_idx, position) in app.gallery.visible_range().enumerate() {
        let img_idx = app.gallery.filtered_indices[position];
        let Some((picture, id)) = app.thumb_cache.peek(img_idx) else {
            continue;
        };
        shown.insert(id);
        if app.terminal_images.contains(id) {
            let (origin, cell_box) = grid_placement(app, vis_idx, picture.cells);
            queue!(out, cursor::MoveTo(origin.0, origin.1))?;
            app.graphics.place_by_id_to(out, id, cell_box)?;
            app.terminal_images.touch(id);
        } else {
            transmit_cell(app, out, vis_idx, img_idx)?;
        }
    }
    trim_to_budget(app, out, &shown)?;
    queue!(out, EndSynchronizedUpdate)?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::{
        TerminalImages, emit_new_thumbnails_to, place_gallery_images_to,
        render_fullscreen_image_to, render_gallery_images,
    };
    use crate::app::App;
    use crate::app::thumbnails::{pad_for_grid, thumb_target_px};
    use crate::gallery::GridPicture;
    use crate::image_list::SharedImageList;
    use crate::test_support::kitty::FakeTerminal;
    use crate::test_support::{char_key, text};
    use image::RgbaImage;
    use ratatui::layout::Rect;
    use std::path::PathBuf;

    const TRANSMIT: &str = "\x1b_Ga=T,";
    const PLACE: &str = "\x1b_Ga=p,";
    const DELETE_ALL: &str = "\x1b_Ga=d,d=A,";
    const CLEAR_PLACEMENTS: &str = "\x1b_Ga=d,d=a,";
    const BEGIN_SYNC: &str = "\x1b[?2026h";
    const END_SYNC: &str = "\x1b[?2026l";
    /// The pixel box of a cell of `thumbnail_app`: 20x6 cells of 8x16 px.
    const BOX_PX: (u32, u32) = (160, 96);
    const BOX_BYTES: usize = 160 * 96 * 4;

    /// A gallery of `count` photos without thumbnails. The cells are 20x6 cells of 8x16 px, so
    /// 160x96 px, in 5 columns and 4 rows.
    fn grid_app(count: usize) -> App {
        let mut app = App::new((8, 16), SharedImageList::new());
        app.images = (0..count)
            .map(|index| PathBuf::from(format!("/photos/{index}.jpg")))
            .collect();
        app.filenames = (0..count).map(|index| format!("{index}.jpg")).collect();
        app.gallery.filtered_indices = (0..count).collect();
        app.gallery.zoom_rows = 4;
        // 4 rows of 6 and 5 columns of 20, with 5 and 6 gaps.
        app.gallery.update_grid(Rect::new(0, 0, 106, 29));
        app.scan_complete = true;
        app
    }

    /// `grid_app` with a sharp thumbnail of the whole box for each cell, so no decode starts.
    fn thumbnail_app(count: usize) -> App {
        let mut app = grid_app(count);
        for index in 0..count {
            app.thumb_cache.insert(index, sharp_thumbnail(160, 96));
        }
        app
    }

    /// A sharp thumbnail of `width` x `height` opaque pixels, padded to whole cells of 8x16 px.
    fn sharp_thumbnail(width: u32, height: u32) -> GridPicture {
        let image = RgbaImage::from_pixel(width, height, image::Rgba([9, 9, 9, 255]));
        pad_for_grid(image, BOX_PX, (8, 16), true)
    }

    /// The write half of `render_gallery_images`, for the visible cells of the grid.
    fn place_visible(app: &mut App, out: &mut Vec<u8>) {
        place_gallery_images_to(app, out).unwrap();
    }

    #[test]
    fn an_opaque_fullscreen_image_goes_out_as_rgb_and_a_transparent_one_as_rgba() {
        let mut app = App::new((8, 16), SharedImageList::new());
        app.image_rect = Rect::new(0, 0, 10, 10);
        let mut image = RgbaImage::from_pixel(16, 32, image::Rgba([9, 9, 9, 255]));
        app.loaded = Some(image.clone());
        let mut out = Vec::new();
        render_fullscreen_image_to(&mut app, &mut out).unwrap();
        assert!(text(&out).contains(",f=24,s=16,v=32,"), "{}", text(&out));

        image.put_pixel(3, 3, image::Rgba([9, 9, 9, 0]));
        app.loaded = Some(image);
        let mut out = Vec::new();
        render_fullscreen_image_to(&mut app, &mut out).unwrap();
        assert!(text(&out).contains(",f=32,s=16,v=32,"), "{}", text(&out));
    }

    #[test]
    fn fullscreen_centers_the_image_in_a_synchronized_update() {
        let mut app = App::new((8, 16), SharedImageList::new());
        app.loaded = Some(RgbaImage::new(16, 32));
        app.image_rect = Rect::new(2, 1, 10, 10);
        app.terminal_images.insert(7, 4);
        let mut out = Vec::new();

        render_fullscreen_image_to(&mut app, &mut out).unwrap();

        let text = text(&out);
        assert!(text.starts_with(BEGIN_SYNC));
        assert!(text.ends_with(END_SYNC));
        assert!(text.contains(DELETE_ALL));
        assert!(text.contains("\x1b[6;7H"), "2 by 2 cells in the middle");
        assert!(text.contains(TRANSMIT));
        assert!(!text.contains(",X="), "no pixel offset: {text}");
        assert!(!text.contains(",Y="), "no pixel offset: {text}");
        assert!(app.terminal_images.is_empty());
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
    fn gallery_transmits_once_and_then_places_by_id_in_a_synchronized_update() {
        let mut app = thumbnail_app(3);
        app.graphics_storage_dirty = true;
        let mut out = Vec::new();

        place_visible(&mut app, &mut out);

        let first = text(&out);
        assert!(first.starts_with(BEGIN_SYNC));
        assert!(first.ends_with(END_SYNC));
        assert!(first.contains(DELETE_ALL));
        assert_eq!(first.matches(TRANSMIT).count(), 3);
        assert!(!first.contains(PLACE));
        assert_eq!(app.terminal_images.len(), 3);
        assert_eq!(app.terminal_images.bytes(), 3 * BOX_BYTES);
        assert!(!app.graphics_storage_dirty);

        out.clear();
        place_visible(&mut app, &mut out);

        let second = text(&out);
        assert!(second.starts_with(BEGIN_SYNC));
        assert!(
            second
                .strip_prefix(BEGIN_SYNC)
                .is_some_and(|rest| rest.starts_with(CLEAR_PLACEMENTS))
        );
        assert!(second.ends_with(END_SYNC));
        assert!(!second.contains(DELETE_ALL));
        assert!(!second.contains(TRANSMIT));
        assert_eq!(second.matches(PLACE).count(), 3);
    }

    #[test]
    fn a_grid_picture_starts_at_a_cell_border_without_a_pixel_offset() {
        let mut app = thumbnail_app(1);
        assert_eq!((app.gallery.thumb_cols, app.gallery.thumb_rows), (20, 6));
        let mut out = Vec::new();

        place_visible(&mut app, &mut out);

        let first = text(&out);
        assert!(
            first.contains("\x1b[2;2H"),
            "the first cell after the edge gaps: {first}"
        );
        assert!(first.contains(",s=160,v=96,"), "the padded box: {first}");
        assert!(!first.contains(",X="), "no pixel offset: {first}");
        assert!(!first.contains(",Y="), "no pixel offset: {first}");
        assert!(
            first.contains(",i=1,c=20,r=6,"),
            "the cell box, so a rounded cell size cannot push it into the gap: {first}"
        );

        out.clear();
        place_visible(&mut app, &mut out);
        assert!(text(&out).contains("\x1b_Ga=p,q=2,C=1,i=1,c=20,r=6\x1b\\"));
    }

    #[test]
    fn a_wide_or_tall_cell_centers_the_picture_in_whole_cells() {
        let mut app = thumbnail_app(6);
        // 108 columns: 102 for 5 columns leave 2 wide columns. 31 rows: 26 for 4 rows leave 2.
        app.gallery.update_grid(Rect::new(0, 0, 108, 31));
        assert_eq!((app.gallery.extra_cols, app.gallery.extra_rows), (2, 2));
        assert_eq!(app.gallery.cell_rect(0), Rect::new(1, 1, 21, 7));
        let mut out = Vec::new();

        place_visible(&mut app, &mut out);

        let text = text(&out);
        assert!(
            text.contains("\x1b[2;2H"),
            "a 1-cell rest leaves it top left"
        );
        assert!(
            text.contains("\x1b[2;24H"),
            "the second wide column: {text}"
        );
        assert!(text.contains("\x1b[2;46H"), "the first base column: {text}");
        assert!(text.contains("\x1b[10;2H"), "the second tall row: {text}");
        assert!(!text.contains(",X=") && !text.contains(",Y="));
        assert_eq!(text.matches(",s=160,v=96,").count(), 6, "the base size");

        // A smaller picture of 16x4 cells (128x64 px) in the wide and tall first cell of 21x7:
        // 2 columns and 1 row in, with its own cell count. The base cell of 20x6 gives 2
        // columns and 1 row too, so a scroll that moves the picture to a base cell keeps it
        // in whole cells.
        assert_eq!(app.thumb_cache.insert(0, sharp_thumbnail(128, 64)), Some(1));
        out.clear();
        emit_new_thumbnails_to(&mut app, &[0], &mut out).unwrap();
        let text = text_of(&out);
        assert!(
            text.contains("\x1b[3;4H"),
            "centered in whole cells: {text}"
        );
        assert!(text.contains(",s=128,v=64,i=7,c=16,r=4,"), "{text}");
        assert!(!text.contains(",X=") && !text.contains(",Y="));
        out.clear();
        place_visible(&mut app, &mut out);
        assert!(text_of(&out).contains("\x1b[3;4H\x1b_Ga=p,q=2,C=1,i=7,c=16,r=4\x1b\\"));
    }

    fn text_of(out: &[u8]) -> String {
        text(out)
    }

    #[test]
    fn a_preview_is_scaled_to_the_cell_box_by_the_terminal() {
        let mut app = grid_app(1);
        // A 40x30 preview in a 160x96 box of 8x16 px cells: the sharp image would be 128x96,
        // 16x6 cells. The canvas gets that shape, 40x30, and the picture its cells.
        let preview = pad_for_grid(RgbaImage::new(40, 30), BOX_PX, (8, 16), false);
        assert_eq!(preview.image.dimensions(), (40, 30));
        assert_eq!(preview.cells, (16, 6));
        assert!(app.store_thumbnail(0, preview, false));
        let mut out = Vec::new();

        emit_new_thumbnails_to(&mut app, &[0], &mut out).unwrap();

        let text = text(&out);
        assert!(text.contains(",s=40,v=30,i=1,c=16,r=6,"), "{text}");
        assert!(!text.contains(",X=") && !text.contains(",Y="));
        assert!(text.contains("\x1b[2;4H"), "centered: 2 columns in: {text}");

        out.clear();
        place_visible(&mut app, &mut out);
        assert!(text_of(&out).contains("\x1b_Ga=p,q=2,C=1,i=1,c=16,r=6\x1b\\"));

        // The sharp image of 128x96 takes the same cells without a scale.
        assert!(app.store_thumbnail(0, sharp_thumbnail(128, 96), true));
        out.clear();
        emit_new_thumbnails_to(&mut app, &[0], &mut out).unwrap();
        let text = text_of(&out);
        assert!(text.contains("\x1b_Ga=d,d=I,i=1,q=2\x1b\\"));
        assert!(text.contains("\x1b[2;4H"), "the same origin: {text}");
        assert!(text.contains(",s=128,v=96,i=2,c=16,r=6,m="), "{text}");
    }

    #[test]
    fn gallery_skips_a_cell_without_a_thumbnail() {
        let mut app = thumbnail_app(2);
        app.images.push(PathBuf::from("/photos/missing.jpg"));
        app.gallery.filtered_indices.push(2);
        let mut out = Vec::new();

        place_visible(&mut app, &mut out);

        assert_eq!(text(&out).matches(TRANSMIT).count(), 2);
        assert_eq!(app.terminal_images.len(), 2);
    }

    #[test]
    fn new_thumbnails_go_out_in_a_synchronized_update_without_a_delete() {
        let mut app = thumbnail_app(3);
        let mut out = Vec::new();

        emit_new_thumbnails_to(&mut app, &[1, 2, 5], &mut out).unwrap();

        let text = text(&out);
        assert!(text.starts_with(BEGIN_SYNC) && text.ends_with(END_SYNC));
        assert_eq!(text.matches(TRANSMIT).count(), 2);
        assert!(!text.contains("\x1b_Ga=d"));
        assert_eq!(app.terminal_images.len(), 2);
        assert!(text.contains("\x1b[2;23H"), "the second cell: {text}");
    }

    #[test]
    fn a_pass_writes_the_new_thumbnails_up_to_its_byte_budget_and_keeps_the_rest() {
        let mut app = thumbnail_app(8);
        let mut out = Vec::new();
        let left = emit_new_thumbnails_to(&mut app, &[7, 0, 1, 2, 3, 4, 5, 6, 30], &mut out);
        assert!(
            left.unwrap().is_empty(),
            "8 cells of 60 KB fit into one pass"
        );
        let text = text(&out);
        assert_eq!(text.matches(TRANSMIT).count(), 8);
        assert_eq!(
            text.matches(BEGIN_SYNC).count(),
            1,
            "in one synchronized update"
        );

        // Pictures of 2 MB: the third one waits for the next pass.
        let mut app = grid_app(3);
        for index in 0..3 {
            let picture = GridPicture {
                image: RgbaImage::new(1024, 512),
                cells: (20, 6),
            };
            app.thumb_cache.insert(index, picture);
        }
        let mut out = Vec::new();
        let left = emit_new_thumbnails_to(&mut app, &[2, 0, 1], &mut out).unwrap();
        assert_eq!(left, vec![2], "the cells go out in grid order");
        assert_eq!(text_of(&out).matches(TRANSMIT).count(), 2);
        let mut out = Vec::new();
        let left = emit_new_thumbnails_to(&mut app, &left, &mut out).unwrap();
        assert!(left.is_empty());
        assert_eq!(text_of(&out).matches(TRANSMIT).count(), 1);
        assert_eq!(app.terminal_images.len(), 3);
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
        assert!(app.terminal_images.contains(preview_id));

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
        assert!(!app.terminal_images.contains(preview_id));
        assert!(app.terminal_images.contains(sharp_id));
        assert!(app.stale_image_ids.is_empty());
    }

    #[test]
    fn a_redraw_of_the_grid_deletes_the_replaced_previews() {
        let mut app = thumbnail_app(2);
        app.stale_image_ids.push((5, 99));
        app.terminal_images.insert(99, 16);
        let mut out = Vec::new();
        place_visible(&mut app, &mut out);
        assert!(text(&out).contains("\x1b_Ga=d,d=I,i=99,"));
        assert!(app.stale_image_ids.is_empty());
        assert!(!app.terminal_images.contains(99));

        // A full redraw deletes every image, so the list only empties.
        app.stale_image_ids.push((5, 98));
        app.graphics_storage_dirty = true;
        let mut out = Vec::new();
        place_visible(&mut app, &mut out);
        assert!(text(&out).contains(DELETE_ALL));
        assert!(!text(&out).contains("d=I"));
        assert!(app.stale_image_ids.is_empty());
    }

    #[test]
    fn an_evicted_thumbnail_is_deleted_in_the_terminal() {
        let mut app = thumbnail_app(2);
        let mut out = Vec::new();
        place_visible(&mut app, &mut out);
        let (_, old_id) = app.thumb_cache.peek(0).unwrap();
        assert!(app.terminal_images.contains(old_id));

        // The cache drops the cell 0 when it shrinks to the smallest capacity it has.
        for index in 2..=app.thumb_cache.capacity() {
            app.thumb_cache.insert(index, RgbaImage::new(1, 1));
        }
        assert!(!app.thumb_cache.contains(0));
        out.clear();
        place_visible(&mut app, &mut out);
        assert!(
            text(&out).contains(&format!("\x1b_Ga=d,d=I,i={old_id},q=2\x1b\\")),
            "{}",
            text(&out)
        );
        assert!(!app.terminal_images.contains(old_id));

        // A never transmitted id needs no delete.
        app.thumb_cache.insert(1, RgbaImage::new(1, 1));
        app.thumb_cache.insert(1, RgbaImage::new(1, 1));
        out.clear();
        emit_new_thumbnails_to(&mut app, &[], &mut out).unwrap();
        assert!(!text(&out).contains("d=I"));
    }

    #[test]
    fn the_terminal_images_count_their_bytes_newest_last() {
        let mut images = TerminalImages::new();
        assert_eq!(images.budget, super::TERMINAL_IMAGE_BUDGET);
        images.budget = 10;
        images.insert(1, 4);
        images.insert(2, 4);
        images.insert(3, 4);
        assert_eq!((images.len(), images.bytes()), (3, 12));
        images.touch(1);
        let keep = std::collections::HashSet::from([3]);
        assert_eq!(
            images.over_budget(&keep),
            vec![2],
            "the oldest one that no cell shows"
        );
        assert_eq!(images.bytes(), 8);
        images.insert(1, 6);
        assert_eq!(
            (images.len(), images.bytes()),
            (2, 10),
            "a second insert replaces"
        );
        images.remove(9);
        images.clear();
        assert!(images.is_empty() && images.bytes() == 0);
    }

    #[test]
    fn the_grid_trims_the_old_images_above_the_budget_but_never_a_visible_one() {
        let mut app = thumbnail_app(40);
        app.terminal_images.budget = 25 * BOX_BYTES;
        let mut out = Vec::new();
        place_visible(&mut app, &mut out);
        assert_eq!(app.terminal_images.len(), 20);
        assert!(!text(&out).contains("d=I"));

        // The second page: 40 images are over the budget, so the 15 oldest of page 1 go.
        app.gallery.move_to_last();
        assert_eq!(app.gallery.visible_range(), 20..40);
        out.clear();
        place_visible(&mut app, &mut out);
        assert_eq!(text(&out).matches("\x1b_Ga=d,d=I,").count(), 15);
        assert_eq!(app.terminal_images.len(), 25);
        for index in 20..40 {
            let (_, id) = app.thumb_cache.peek(index).unwrap();
            assert!(app.terminal_images.contains(id), "cell {index} stays");
        }

        // Back on page 1, the 15 trimmed cells transmit again, the 5 kept ones place by id.
        app.gallery.move_to_first();
        out.clear();
        place_visible(&mut app, &mut out);
        let text = text(&out);
        assert_eq!(text.matches(TRANSMIT).count(), 15);
        assert_eq!(text.matches(PLACE).count(), 5);
        assert_eq!(app.terminal_images.len(), 25);
    }

    /// Scroll `pages` down and up through `app` on `terminal`: each page gets its thumbnails,
    /// as the decodes deliver them, and the grid writes go into the fake terminal.
    fn scroll_through(app: &mut App, terminal: &mut FakeTerminal, pages: usize) {
        for step in 0..2 * pages {
            render_gallery_images(app).unwrap();
            terminal.feed(&app.graphics_out.take_bytes());
            let fresh: Vec<usize> = app
                .gallery
                .visible_items()
                .map(|(_, index)| index)
                .filter(|&index| !app.thumb_cache.contains(index))
                .collect();
            for &index in &fresh {
                assert!(app.store_thumbnail(index, sharp_thumbnail(160, 96), true));
            }
            let mut out = Vec::new();
            emit_new_thumbnails_to(app, &fresh, &mut out).unwrap();
            terminal.feed(&out);
            assert!(terminal.black_cells.is_empty(), "step {step}");
            for (_, index) in app.gallery.visible_items() {
                let (_, id) = app.thumb_cache.peek(index).unwrap();
                assert!(
                    terminal
                        .placements
                        .iter()
                        .any(|placement| placement.id == id),
                    "step {step}: the cell {index} shows nothing"
                );
            }
            assert_eq!(terminal.synchronized_depth, 0);
            if step + 1 < pages {
                app.gallery.move_page_down();
            } else {
                app.gallery.move_page_up();
            }
        }
    }

    #[test]
    fn scrolling_a_long_folder_up_and_down_leaves_no_black_cell() {
        let pages = 10;
        let mut app = grid_app(20 * pages);
        app.terminal_images.budget = 40 * BOX_BYTES;
        let mut terminal = FakeTerminal::with_limit(60 * BOX_BYTES);

        scroll_through(&mut app, &mut terminal, pages);

        assert!(terminal.stored_bytes() <= 40 * BOX_BYTES);
        assert!(app.thumb_cache.capacity() >= 40);
    }

    #[test]
    fn without_the_budget_the_terminal_drops_the_images_of_the_first_pages() {
        let pages = 10;
        let mut app = grid_app(20 * pages);
        app.terminal_images.budget = usize::MAX;
        let mut terminal = FakeTerminal::with_limit(60 * BOX_BYTES);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            scroll_through(&mut app, &mut terminal, pages);
        }));

        assert!(
            result.is_err(),
            "the fake terminal evicts, so a cell goes black"
        );
    }

    /// A gallery like the one of the maintainer: a 5K screen in Ghostty, cells of 18x43 px,
    /// a grid area of 280x50 cells, 3:2 photos and the default of 4 rows.
    fn large_screen_app(count: usize) -> App {
        let cell_px = (18, 43);
        let mut app = App::new(cell_px, SharedImageList::new());
        app.images = (0..count)
            .map(|index| PathBuf::from(format!("/photos/{index}.jpg")))
            .collect();
        app.filenames = (0..count).map(|index| format!("{index}.jpg")).collect();
        app.gallery.filtered_indices = (0..count).collect();
        app.gallery.zoom_rows = 4;
        app.gallery.update_grid(Rect::new(0, 0, 280, 50));
        app.gallery.set_cell_shape(1.5, cell_px);
        app.scan_complete = true;
        app.needs_render = false;
        app
    }

    /// A sharp thumbnail for the base cell of `app`: a portrait photo in the odd cells and a
    /// landscape one in the even cells, padded to the pixel box of the cell.
    fn thumbnail_for_grid(app: &App, index: usize) -> GridPicture {
        let target = thumb_target_px(app.gallery.thumb_box(), app.cell_px);
        let (width, height) = if index.is_multiple_of(2) {
            (target.0, target.1 * 2 / 3)
        } else {
            (target.0 * 2 / 3, target.1)
        };
        let image = RgbaImage::from_pixel(width, height, image::Rgba([9, 9, 9, 255]));
        pad_for_grid(image, target, app.cell_px, true)
    }

    /// The passes of the run loop after a key: the grid write when a render is due, then the
    /// decodes of the visible cells arrive and go out, in as many passes as their bytes need.
    /// Then every visible cell must show a stored image.
    fn settle_grid(app: &mut App, terminal: &mut FakeTerminal, step: &str) {
        if app.needs_render {
            render_gallery_images(app).unwrap();
            app.needs_render = false;
        }
        terminal.feed(&app.graphics_out.take_bytes());
        let fresh: Vec<usize> = app
            .gallery
            .visible_items()
            .map(|(_, index)| index)
            .filter(|&index| !app.thumb_cache.is_sharp(index))
            .collect();
        for &index in &fresh {
            let image = thumbnail_for_grid(app, index);
            assert!(app.store_thumbnail(index, image, true));
        }
        let mut pending = fresh;
        while !pending.is_empty() {
            let mut out = Vec::new();
            pending = emit_new_thumbnails_to(app, &pending, &mut out).unwrap();
            terminal.feed(&out);
        }
        assert!(terminal.black_cells.is_empty(), "{step}: black cells");
        assert_eq!(terminal.synchronized_depth, 0, "{step}");
        for (_, index) in app.gallery.visible_items() {
            let (_, id) = app.thumb_cache.peek(index).unwrap();
            assert!(
                terminal
                    .placements
                    .iter()
                    .any(|placement| placement.id == id),
                "{step}: the cell {index} shows nothing"
            );
        }
    }

    /// Half of the 320 MB that Ghostty stores, because herdr stores each image twice.
    const RELAYED_TERMINAL_LIMIT: usize = 160 * 1024 * 1024;

    /// The zoom keys through the real key path, from 4 rows to a limit and one key past it.
    /// The terminal evicts above the relayed Ghostty limit, so the test also covers the
    /// storage of the large pictures of a 2x2 grid.
    fn zoom_to_the_limit(key: char, limit: usize) {
        let mut app = large_screen_app(300);
        let mut terminal = FakeTerminal::with_limit(RELAYED_TERMINAL_LIMIT);
        settle_grid(&mut app, &mut terminal, "start");
        for step in 0..12 {
            char_key(&mut app, key);
            settle_grid(&mut app, &mut terminal, &format!("{key} {step}"));
            if app.gallery.zoom_rows == limit {
                break;
            }
        }
        assert_eq!(app.gallery.zoom_rows, limit);
        let cells = app.gallery.visible_cells();

        // One more key at the limit changes nothing and takes no picture away.
        char_key(&mut app, key);
        settle_grid(&mut app, &mut terminal, "at the limit");
        assert_eq!(app.gallery.zoom_rows, limit);
        assert_eq!(terminal.placements.len(), cells);
    }

    #[test]
    fn a_zoom_in_to_the_largest_cells_and_one_more_leaves_no_black_cell() {
        zoom_to_the_limit('+', 2);
    }

    #[test]
    fn a_zoom_out_to_the_most_rows_and_one_more_leaves_no_black_cell() {
        zoom_to_the_limit('-', 12);
    }

    #[test]
    fn the_cache_capacity_follows_the_grid() {
        let mut app = thumbnail_app(1);
        assert_eq!(app.thumb_cache.capacity(), 200);
        app.gallery.zoom_rows = 12;
        app.gallery.update_grid(Rect::new(0, 0, 1000, 300));
        assert!(
            app.gallery.visible_cells() > 100,
            "{}",
            app.gallery.visible_cells()
        );

        render_gallery_images(&mut app).unwrap();

        assert_eq!(app.thumb_cache.capacity(), 2 * app.gallery.visible_cells());
    }

    #[cfg(feature = "video")]
    #[test]
    fn a_video_without_a_frame_writes_nothing() {
        let app = App::new((8, 16), SharedImageList::new());
        let mut out = Vec::new();

        super::render_video_frame_to(&app, &mut out).unwrap();

        assert!(out.is_empty());
    }

    /// A player on a 640x480 clip with a 64x48 frame, with one frame on the screen.
    #[cfg(feature = "video")]
    fn playing_app() -> App {
        use crate::player::Player;
        use crate::test_support::video::{clip_info, fake_decoder, shared, with_temp_file_medium};
        use std::path::Path;

        let mut app = with_temp_file_medium(App::new((8, 16), SharedImageList::new()));
        app.images = vec![PathBuf::from("/videos/clip.mp4")];
        app.filenames = vec!["clip.mp4".to_owned()];
        app.gallery.filtered_indices = vec![0];
        app.enter_video(0);
        app.image_rect = Rect::new(0, 0, 100, 40);
        let decoder = fake_decoder(clip_info());
        let mut player = Player::open(
            shared(&decoder),
            Path::new("/bin/ffmpeg"),
            Path::new("/videos/clip.mp4"),
            (64, 48),
            &[],
            true,
        )
        .unwrap();
        decoder.feed(vec![7; 64 * 48 * 3]);
        assert!(player.poll());
        app.video = Some(player);
        app
    }

    #[cfg(feature = "video")]
    #[test]
    fn the_first_frame_of_a_new_video_removes_the_pictures_of_the_view_before() {
        let mut app = playing_app();
        assert!(app.clear_before_video, "enter_video asks for the clear");

        super::render_video_frame(&mut app).unwrap();
        let first = app.graphics_out.take_bytes();
        super::render_video_frame(&mut app).unwrap();
        let second = app.graphics_out.take_bytes();

        for bytes in [&first, &second] {
            for path in crate::encoder::kitty::temp_file_paths(bytes) {
                std::fs::remove_file(path).unwrap();
            }
        }
        let (first, second) = (text(&first), text(&second));
        let clear = first
            .find("a=d")
            .expect("the first frame clears the placements");
        let frame = first.find("a=T").unwrap();
        assert!(clear < frame, "{first}");
        assert!(
            first.starts_with("\x1b[?2026h"),
            "in one synchronized update: {first}"
        );
        assert!(
            !second.contains("a=d"),
            "only the first frame clears: {second}"
        );
        assert!(!app.clear_before_video);
    }

    #[cfg(feature = "video")]
    #[test]
    fn a_video_frame_goes_out_as_a_temp_file_scaled_to_the_cell_box() {
        let app = playing_app();
        let mut out = Vec::new();

        super::render_video_frame_to(&app, &mut out).unwrap();

        for path in crate::encoder::kitty::temp_file_paths(&out) {
            assert_eq!(std::fs::metadata(&path).unwrap().len(), 64 * 48 * 3);
            std::fs::remove_file(path).unwrap();
        }
        let text = text(&out);
        assert!(text.starts_with("\x1b[?2026h"), "{text}");
        assert!(text.ends_with("\x1b[?2026l"), "{text}");
        // 64x48 in 100x40 cells of 8x16: the 100 columns take 38 rows, from row 1.
        assert!(text.contains("\x1b[2;1H"), "{text}");
        assert!(
            text.contains("\x1b_Ga=T,q=2,C=1,f=24,s=64,v=48,i=4294967294,c=100,r=38,t=t;"),
            "{text}"
        );
        assert!(!text.contains("X=") && !text.contains("Y="), "{text}");
    }

    #[cfg(feature = "video")]
    #[test]
    fn the_help_popup_gets_no_video_frame_above_it() {
        let mut app = playing_app();
        app.help_visible = true;
        let mut out = Vec::new();

        super::render_video_frame_to(&app, &mut out).unwrap();

        assert!(out.is_empty());
    }

    #[cfg(feature = "video")]
    #[test]
    fn a_video_frame_in_a_view_without_cells_writes_nothing() {
        let mut app = playing_app();
        app.image_rect = Rect::default();
        let mut out = Vec::new();

        super::render_video_frame_to(&app, &mut out).unwrap();

        assert!(out.is_empty());
    }
}
