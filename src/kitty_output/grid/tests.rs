//! The tests of the thumbnail writes: the passes, the placements, the deletes and the budget.
use super::emit_new_thumbnails_to;
use super::{place_gallery_images_to, render_gallery_images};
use crate::app::App;
use crate::app::thumbnails::pad_for_grid;
use crate::app::thumbnails::thumb_target_px;
use crate::gallery::GridPicture;
use crate::image_list::SharedImageList;
use crate::kitty_output::tests::{
    BEGIN_SYNC, BOX_PX, DELETE_ALL, END_SYNC, TRANSMIT, grid_app, sharp_thumbnail, thumbnail_app,
};
use crate::test_support::char_key;
use crate::test_support::kitty::FakeTerminal;
use crate::test_support::text;
use image::RgbaImage;
use ratatui::layout::Rect;
use std::path::PathBuf;

const PLACE: &str = "\x1b_Ga=p,";

const CLEAR_PLACEMENTS: &str = "\x1b_Ga=d,d=a,";

const BOX_BYTES: usize = 160 * 96 * 4;

fn place_visible(app: &mut App, out: &mut Vec<u8>) {
    place_gallery_images_to(app, out).unwrap();
}

#[test]
fn delete_all_takes_stale_loop_ids() {
    let mut app = thumbnail_app(1);
    app.graphics_storage_dirty = true;
    app.grid_loops.stale_ids.push(900);
    let mut out = Vec::new();

    place_visible(&mut app, &mut out);

    assert!(app.grid_loops.stale_ids.is_empty());
    assert!(!text(&out).contains("d=I,i=900"));
}

#[test]
fn transmits_once_then_places_by_id() {
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
fn picture_starts_at_cell_border() {
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
fn wide_cell_centers_in_whole_cells() {
    let mut app = thumbnail_app(6);
    // 108 columns: 102 for 5 columns leave 2 wide columns. 31 rows: 26 for 4 rows leave 2.
    app.gallery.update_grid(Rect::new(0, 0, 108, 31));
    assert_eq!((app.gallery.extra_cols, app.gallery.extra_rows), (2, 2));
    assert_eq!(app.gallery.thumb_rect(0), Rect::new(1, 1, 21, 7));
    let mut out = Vec::new();

    place_visible(&mut app, &mut out);

    let text = text_of(&out);
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

    // A smaller picture of 16x4 cells (128x64 px) in the wide and tall first cell of 21x7: 2
    // columns and 1 row in, with its own cell count. The base cell of 20x6 gives the same offsets.
    // A scroll to a base cell keeps the picture in whole cells.
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

/// `test_support::text` under a name that no local `text` shadows.
fn text_of(out: &[u8]) -> String {
    text(out)
}

#[test]
fn terminal_scales_preview_to_cell_box() {
    let mut app = grid_app(1);
    // A 40x30 preview in a 160x96 box of 8x16 px cells. The sharp image is 128x96, 16x6 cells.
    // The canvas gets that shape, 40x30, and the picture its cells.
    let preview = pad_for_grid(RgbaImage::new(40, 30), BOX_PX, (8, 16), false);
    assert_eq!(preview.image.dimensions(), (40, 30));
    assert_eq!(preview.cells, (16, 6));
    assert!(app.store_thumbnail(0, preview, false));
    let mut out = Vec::new();

    emit_new_thumbnails_to(&mut app, &[0], &mut out).unwrap();

    let text = text_of(&out);
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
fn skips_cell_without_thumbnail() {
    let mut app = thumbnail_app(2);
    app.images.push(PathBuf::from("/photos/missing.jpg"));
    app.gallery.filtered_indices.push(2);
    let mut out = Vec::new();

    place_visible(&mut app, &mut out);

    assert_eq!(text(&out).matches(TRANSMIT).count(), 2);
    assert_eq!(app.terminal_images.len(), 2);
}

#[test]
fn new_thumbnails_need_no_delete() {
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
fn pass_stops_at_byte_budget() {
    let mut app = thumbnail_app(8);
    let mut out = Vec::new();
    let left = emit_new_thumbnails_to(&mut app, &[7, 0, 1, 2, 3, 4, 5, 6, 30], &mut out);
    assert!(
        left.unwrap().is_empty(),
        "8 cells of 60 KB fit into one pass"
    );
    let text = text_of(&out);
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
    app.stale_image_ids = vec![(0, 91), (2, 92)];
    app.terminal_images.insert(91, 16);
    app.terminal_images.insert(92, 16);
    let mut out = Vec::new();
    let left = emit_new_thumbnails_to(&mut app, &[2, 0, 1], &mut out).unwrap();
    assert_eq!(left, vec![2], "the cells go out in grid order");
    assert!(text_of(&out).contains("i=91,"));
    assert!(!text_of(&out).contains("i=92,"));
    assert_eq!(
        app.stale_image_ids,
        vec![(2, 92)],
        "the cut cell keeps its preview"
    );
    assert_eq!(text_of(&out).matches(TRANSMIT).count(), 2);
    let mut out = Vec::new();
    let left = emit_new_thumbnails_to(&mut app, &left, &mut out).unwrap();
    assert!(left.is_empty());
    assert_eq!(text_of(&out).matches(TRANSMIT).count(), 1);
    assert_eq!(app.terminal_images.len(), 3);
}

#[test]
fn sharp_thumbnail_deletes_preview() {
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
        text.find(&delete).unwrap() > text.find(TRANSMIT).unwrap(),
        "the preview goes after the sharp image, in the same update: {text}"
    );
    assert!(text.find(&delete).unwrap() < text.find(END_SYNC).unwrap());
    assert_eq!(text.matches(TRANSMIT).count(), 1);
    assert!(text.contains(&format!(",i={sharp_id}")));
    assert!(!app.terminal_images.contains(preview_id));
    assert!(app.terminal_images.contains(sharp_id));
    assert!(app.stale_image_ids.is_empty());
}

#[test]
fn redraw_deletes_replaced_previews() {
    let mut app = thumbnail_app(2);
    app.stale_image_ids.push((5, 99));
    app.terminal_images.insert(99, 16);
    let mut out = Vec::new();
    place_visible(&mut app, &mut out);
    assert!(text(&out).contains("\x1b_Ga=d,d=I,i=99,"));
    assert!(app.stale_image_ids.is_empty());
    assert!(!app.terminal_images.contains(99));

    // A full redraw deletes every image. The list only empties.
    app.stale_image_ids.push((5, 98));
    app.graphics_storage_dirty = true;
    let mut out = Vec::new();
    place_visible(&mut app, &mut out);
    assert!(text(&out).contains(DELETE_ALL));
    assert!(!text(&out).contains("d=I"));
    assert!(app.stale_image_ids.is_empty());
}

#[test]
fn stale_pass_deletes_picked_previews() {
    let mut app = thumbnail_app(1);
    app.stale_image_ids = vec![(1, 91), (2, 92), (1, 93), (3, 94)];
    for id in [91, 92, 93, 94] {
        app.terminal_images.insert(id, 16);
    }
    let mut out = Vec::new();
    super::delete_stale_images_to(&mut app, &mut out, |cell| cell == 1).unwrap();
    let written = text(&out);
    let first = written.find("i=91,").unwrap();
    let second = written.find("i=93,").unwrap();
    assert!(first < second, "the order of the list stays");
    assert!(!written.contains("i=92,") && !written.contains("i=94,"));
    assert_eq!(app.stale_image_ids, vec![(2, 92), (3, 94)]);
}

#[test]
fn deletes_evicted_thumbnail() {
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
fn trims_old_images_but_not_visible() {
    let mut app = thumbnail_app(40);
    app.terminal_images.budget = 25 * BOX_BYTES;
    let mut out = Vec::new();
    place_visible(&mut app, &mut out);
    assert_eq!(app.terminal_images.len(), 20);
    assert!(!text(&out).contains("d=I"));

    // The second page: 40 images are over the budget. The 15 oldest of page 1 go.
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

/// Scroll `pages` down and up. Each page gets its thumbnails as the decodes deliver them.
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
fn scrolling_leaves_no_black_cell() {
    let pages = 10;
    let mut app = grid_app(20 * pages);
    app.terminal_images.budget = 40 * BOX_BYTES;
    let mut terminal = FakeTerminal::with_limit(60 * BOX_BYTES);

    scroll_through(&mut app, &mut terminal, pages);

    assert!(terminal.stored_bytes() <= 40 * BOX_BYTES);
    assert!(app.thumb_cache.capacity() >= 40);
}

#[test]
fn without_budget_terminal_drops_pages() {
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

/// The maintainer's gallery: a 5K screen in Ghostty, cells of 18x43 px, a grid of 280x50 cells,
/// 3:2 photos and 4 rows.
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

/// A sharp thumbnail for the base cell: a portrait photo in the odd cells, a landscape one in
/// the even cells.
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

/// The run loop passes after a key: the grid write, then the visible decodes in as many passes
/// as their bytes need. Then every visible cell shows a stored image.
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

/// Half of the 320 MB that Ghostty stores: herdr stores each image twice.
const RELAYED_TERMINAL_LIMIT: usize = 160 * 1024 * 1024;

/// The zoom keys from 4 rows to a limit and one key past it. The terminal evicts above the
/// relayed Ghostty limit.
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
fn zoom_in_leaves_no_black_cell() {
    zoom_to_the_limit('+', 2);
}

#[test]
fn zoom_out_leaves_no_black_cell() {
    zoom_to_the_limit('-', 12);
}

#[test]
fn cache_capacity_follows_grid() {
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

/// Poll until the cell 0 gets a new thumbnail, for at most 2 s.
#[cfg(feature = "heic")]
fn wait_for_thumbnail(app: &mut App) {
    crate::test_support::wait_until(|| app.poll_thumbnails() == vec![0]);
}

/// A grid app with one 256x128 HEIC whose embedded thumbnail has the aspect of `source`. The
/// cell is 20x6 cells of 8x16 px, a box of 160x96 px, and the sharp image fits as 160x80 px.
#[cfg(feature = "heic")]
fn heic_app(dir: &std::path::Path, source: (u32, u32)) -> App {
    use crate::test_support::write_heif_sized;
    use libheif_rs::{CompressionFormat, ImageOrientation};
    let path = dir.join("photo.heic");
    let normal = ImageOrientation::Normal;
    write_heif_sized(
        &path,
        CompressionFormat::Hevc,
        normal,
        Some((normal, source)),
    );
    let mut app = grid_app(1);
    app.images = vec![path];
    app.disk_cache_dir = dir.join("cache");
    std::fs::create_dir(&app.disk_cache_dir).unwrap();
    app.spawn_thumb_decode(0, app.gallery.thumb_rect(0));
    app
}

/// The text of the emit pass of cell 0 and of the placement that follows it.
#[cfg(feature = "heic")]
fn emit_and_place(app: &mut App) -> (String, String) {
    let mut out = Vec::new();
    emit_new_thumbnails_to(app, &[0], &mut out).unwrap();
    let emitted = text_of(&out);
    out.clear();
    place_visible(app, &mut out);
    (emitted, text_of(&out))
}

/// The cursor move that puts a picture of `cells` in the middle of the cell 0.
#[cfg(feature = "heic")]
fn moved_to(app: &App, cells: (u16, u16)) -> String {
    let rect = crate::gallery::centered(app.gallery.thumb_rect(0), cells.0, cells.1);
    format!("\x1b[{};{}H", rect.y + 1, rect.x + 1)
}

#[cfg(feature = "heic")]
#[test]
fn square_preview_of_wide_photo_keeps_its_shape() {
    use crate::test_support::{BLUE, RED, assert_close, test_dir};
    let dir = test_dir();
    let mut app = heic_app(dir.path(), (128, 128));
    // The preview scales up to 96x96 px, 12x6 cells. Its canvas has that shape already.
    wait_for_thumbnail(&mut app);
    assert!(!app.thumb_cache.is_sharp(0));
    let (preview, _) = app.thumb_cache.peek(0).unwrap();
    assert_eq!(preview.cells, (12, 6));
    assert_eq!(preview.image.dimensions(), (64, 64));
    assert_close(preview.image.get_pixel(0, 0).0, BLUE);
    assert_close(preview.image.get_pixel(63, 63).0, BLUE);
    let (emitted, placed) = emit_and_place(&mut app);
    assert!(emitted.contains(",s=64,v=64,i=1,c=12,r=6,"), "{emitted}");
    assert!(emitted.contains(&moved_to(&app, (12, 6))), "{emitted}");
    assert!(placed.contains("\x1b_Ga=p,q=2,C=1,i=1,c=12,r=6\x1b\\"));

    // The sharp picture takes the cells of its own aspect in the same cell.
    wait_for_thumbnail(&mut app);
    assert!(app.thumb_cache.is_sharp(0));
    let (sharp, _) = app.thumb_cache.peek(0).unwrap();
    assert_eq!(sharp.cells, (20, 5));
    assert_eq!(sharp.image.dimensions(), (160, 80));
    assert_close(sharp.image.get_pixel(80, 40).0, RED);
    let mut out = Vec::new();
    emit_new_thumbnails_to(&mut app, &[0], &mut out).unwrap();
    let emitted = text_of(&out);
    assert!(emitted.contains(",s=160,v=80,i=2,c=20,r=5,"), "{emitted}");
    assert!(emitted.contains(&moved_to(&app, (20, 5))), "{emitted}");
}

#[cfg(feature = "heic")]
#[test]
fn padded_preview_keeps_the_aspect_of_its_cells() {
    use crate::test_support::{BLUE, assert_close, test_dir};
    let dir = test_dir();
    // A 16:9 thumbnail of 64x36 px scales up to 160x90 px, 20x6 cells. The cells have the shape
    // 160x96, so the canvas needs 3 transparent rows: 64x39 px.
    let mut app = heic_app(dir.path(), (128, 72));
    wait_for_thumbnail(&mut app);
    let (preview, _) = app.thumb_cache.peek(0).unwrap();
    assert_eq!(preview.cells, (20, 6));
    let image = &preview.image;
    assert_eq!(image.dimensions(), (64, 39));
    assert_eq!(image.get_pixel(32, 0).0[3], 0, "transparent top margin");
    assert_eq!(image.get_pixel(32, 38).0[3], 0, "transparent bottom margin");
    assert_close(image.get_pixel(32, 19).0, BLUE);
    assert_close(image.get_pixel(0, 19).0, BLUE);
    // The canvas has the shape of the cells in pixels, so the terminal scales it by one factor.
    let canvas = f64::from(image.width()) / f64::from(image.height());
    let cells = f64::from(preview.cells.0 * 8) / f64::from(preview.cells.1 * 16);
    assert!((canvas - cells).abs() < 0.03, "{canvas} against {cells}");
    let (emitted, _) = emit_and_place(&mut app);
    assert!(emitted.contains(",s=64,v=39,i=1,c=20,r=6,"), "{emitted}");
}
