//! The Kitty graphics writes per frame: the fullscreen image, the video frame, the grid and its
//! loops.

mod grid;
mod loops;
mod terminal_images;

use crate::app::App;
use crate::encoder;
use crate::gallery::centered;
use crossterm::{cursor, queue};
pub(crate) use grid::{emit_new_thumbnails, render_gallery_images};
use image::RgbaImage;
pub(crate) use loops::play_grid_loops;
use ratatui::prelude::*;
use std::io::{self, Write};
pub(crate) use terminal_images::TerminalImages;

pub(crate) fn render_fullscreen_image(app: &mut App) -> io::Result<()> {
    app.with_graphics_out(render_fullscreen_image_to)
}

/// Write the fullscreen image to `out`, so that the tests can read the escape sequences.
pub(crate) fn render_fullscreen_image_to(app: &mut App, out: &mut impl Write) -> io::Result<()> {
    use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};

    // The synchronized update swaps the old image for the new one without a blank view between.
    queue!(out, BeginSynchronizedUpdate)?;
    app.graphics_delete_all_to(out)?;

    if let Some(img) = &app.loaded {
        let start = centered_image(app.image_rect, app.cell_px, img);
        queue!(out, cursor::MoveTo(start.x, start.y))?;
        app.graphics
            .transmit(out, img.into(), &encoder::DisplayOptions::default())?;
    }

    queue!(out, EndSynchronizedUpdate)?;
    out.flush()
}

/// The terminal cells of `img` when it sits in the middle of `rect`, in whole cells.
fn centered_image(rect: Rect, cell_px: (u32, u32), img: &RgbaImage) -> Rect {
    let cols = u16::try_from(img.width().div_ceil(cell_px.0)).unwrap_or(u16::MAX);
    let rows = u16::try_from(img.height().div_ceil(cell_px.1)).unwrap_or(u16::MAX);
    centered(rect, cols, rows)
}

/// The one image id of the terminal player. Each frame replaces the last one under this id,
/// so the terminal never shows an empty box between two frames. The id sits at the top of
/// the range, far from the ids of the grid, which count up from 1 (`ThumbnailCache::next_id`).
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
        }
        Ok(())
    })
}

/// Write the current video frame to `out`, so that the tests can read the escape sequences.
/// Returns true when the frame went out.
///
/// The frame is small. The terminal scales it to the cell box (`c=`, `r=`) in the middle of
/// the view, in whole cells and without a pixel offset, inside a synchronized update. With
/// `clear`, the same update first removes all placements, so the pictures of the view before
/// do not show around the frame and nothing flashes. The help popup gets no frame, because
/// Kitty draws a picture above the text. The key that closes the help writes the frame again.
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
        },
    )?;
    queue!(out, EndSynchronizedUpdate)?;
    out.flush()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::render_fullscreen_image_to;
    use crate::app::App;
    use crate::app::thumbnails::pad_for_grid;
    use crate::gallery::GridPicture;
    use crate::image_list::SharedImageList;
    use crate::test_support::text;
    use image::RgbaImage;
    use ratatui::layout::Rect;
    use std::path::PathBuf;

    pub(super) const TRANSMIT: &str = "\x1b_Ga=T,";

    pub(super) const DELETE_ALL: &str = "\x1b_Ga=d,d=A,";

    pub(super) const BEGIN_SYNC: &str = "\x1b[?2026h";

    pub(super) const END_SYNC: &str = "\x1b[?2026l";

    /// The pixel box of a cell of `thumbnail_app`: 20x6 cells of 8x16 px.
    pub(super) const BOX_PX: (u32, u32) = (160, 96);

    /// A gallery of `count` photos without thumbnails. The cells are 20x6 cells of 8x16 px, so
    /// 160x96 px, in 5 columns and 4 rows.
    pub(super) fn grid_app(count: usize) -> App {
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
    pub(super) fn thumbnail_app(count: usize) -> App {
        let mut app = grid_app(count);
        for index in 0..count {
            app.thumb_cache.insert(index, sharp_thumbnail(160, 96));
        }
        app
    }

    /// A sharp thumbnail of `width` x `height` opaque pixels, padded to whole cells of 8x16 px.
    pub(super) fn sharp_thumbnail(width: u32, height: u32) -> GridPicture {
        let image = RgbaImage::from_pixel(width, height, image::Rgba([9, 9, 9, 255]));
        pad_for_grid(image, BOX_PX, (8, 16), true)
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

    #[cfg(feature = "video")]
    #[test]
    fn a_video_without_a_frame_writes_nothing() {
        let app = App::new((8, 16), SharedImageList::new());
        let mut out = Vec::new();

        super::render_video_frame_to(&app, &mut out, false).unwrap();

        assert!(out.is_empty());
    }

    /// A player on a 640x480 clip with a 64x48 frame, with one frame on the screen.
    #[cfg(feature = "video")]
    pub(super) fn playing_app() -> App {
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

        super::render_video_frame_to(&app, &mut out, false).unwrap();

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

        super::render_video_frame_to(&app, &mut out, false).unwrap();

        assert!(out.is_empty());
    }

    #[cfg(feature = "video")]
    #[test]
    fn a_video_frame_in_a_view_without_cells_writes_nothing() {
        let mut app = playing_app();
        app.image_rect = Rect::default();
        let mut out = Vec::new();

        super::render_video_frame_to(&app, &mut out, false).unwrap();

        assert!(out.is_empty());
    }

    #[cfg(feature = "video")]
    #[test]
    fn a_first_frame_in_a_one_row_terminal_leaves_no_synchronized_update_open() {
        use crate::test_support::kitty::FakeTerminal;

        let mut app = playing_app();
        crate::test_support::draw_frame(&mut app, 100, 1);
        assert_eq!(app.image_rect.height, 0, "the status row takes the one row");
        let mut terminal = FakeTerminal::new();

        super::render_video_frame(&mut app).unwrap();
        terminal.feed(&app.graphics_out.take_bytes());

        assert_eq!(terminal.synchronized_depth, 0);
        assert!(
            app.clear_before_video,
            "the clear waits for the first frame that goes out"
        );
    }
}
