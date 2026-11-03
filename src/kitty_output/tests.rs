//! The tests of the fullscreen image and the video frame writes.
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

/// The pixel box of a `thumbnail_app` cell.
pub(super) const BOX_PX: (u32, u32) = (160, 96);

/// A gallery of `count` photos without thumbnails: 5 columns and 4 rows of 160x96 px cells.
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

/// `grid_app` with a sharp thumbnail in each cell. No decode starts.
pub(super) fn thumbnail_app(count: usize) -> App {
    let mut app = grid_app(count);
    for index in 0..count {
        app.thumb_cache.insert(index, sharp_thumbnail(160, 96));
    }
    app
}

/// A sharp thumbnail of opaque pixels, padded to whole cells of 8x16 px.
pub(super) fn sharp_thumbnail(width: u32, height: u32) -> GridPicture {
    let image = RgbaImage::from_pixel(width, height, image::Rgba([9, 9, 9, 255]));
    pad_for_grid(image, BOX_PX, (8, 16), true)
}

#[test]
fn opaque_image_goes_out_as_rgb() {
    let mut app = App::new((8, 16), SharedImageList::new());
    app.image_rect = Rect::new(0, 0, 10, 10);
    let mut image = RgbaImage::from_pixel(16, 32, image::Rgba([9, 9, 9, 255]));
    app.loaded = Some(image.clone().into());
    let mut out = Vec::new();
    render_fullscreen_image_to(&mut app, &mut out).unwrap();
    assert!(text(&out).contains(",f=24,s=16,v=32,"), "{}", text(&out));

    image.put_pixel(3, 3, image::Rgba([9, 9, 9, 0]));
    app.loaded = Some(image.into());
    let mut out = Vec::new();
    render_fullscreen_image_to(&mut app, &mut out).unwrap();
    assert!(text(&out).contains(",f=32,s=16,v=32,"), "{}", text(&out));
}

#[test]
fn fullscreen_centers_image() {
    let mut app = App::new((8, 16), SharedImageList::new());
    app.loaded = Some(RgbaImage::new(16, 32).into());
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

/// A 6016x3384 px image, 81 MB as RGBA, with one transparent pixel when `transparent` is set.
fn six_k_image(transparent: bool) -> RgbaImage {
    let mut image = RgbaImage::from_pixel(6016, 3384, image::Rgba([9, 9, 9, 255]));
    if transparent {
        image.put_pixel(3, 3, image::Rgba([9, 9, 9, 0]));
    }
    image
}

#[test]
fn payload_over_cap_shrinks() {
    use crate::encoder::{KittyBackend, MAX_PAYLOAD_BYTES, Transfer, kitty};
    use crate::test_support::kitty::FakeTerminal;

    let mut app = App::new((8, 16), SharedImageList::new());
    app.graphics = KittyBackend::new(Transfer::TempFile);
    app.image_rect = Rect::new(0, 0, 752, 212);
    app.loaded = Some(six_k_image(true).into());
    let mut out = Vec::new();

    render_fullscreen_image_to(&mut app, &mut out).unwrap();

    let mut terminal = FakeTerminal::new();
    terminal.feed(&out);
    assert_eq!(terminal.synchronized_depth, 0);
    let paths = kitty::temp_file_paths(&out);
    assert_eq!(paths.len(), 1);
    let size = std::fs::metadata(&paths[0]).unwrap().len();
    std::fs::remove_file(&paths[0]).unwrap();
    assert!(size <= MAX_PAYLOAD_BYTES as u64, "{size} bytes");
    assert!(size > MAX_PAYLOAD_BYTES as u64 * 99 / 100, "{size} bytes");
    let text = text(&out);
    let (width, height) = wire_size(&text);
    assert_eq!(u64::from(width) * u64::from(height) * 4, size, "{text}");
    assert!(width < 6016 && height < 3384, "{text}");
    assert!(
        (f64::from(width) / f64::from(height) - 6016.0 / 3384.0).abs() < 0.01,
        "the aspect stays: {text}"
    );
    let transmit = text.split("\x1b_G").find(|c| c.starts_with("a=T")).unwrap();
    assert!(
        transmit.contains(",c=752,"),
        "the width of the full image: {text}"
    );
    assert!(
        !transmit.contains(",r="),
        "the rows follow the aspect: {text}"
    );
}

fn wire_size(text: &str) -> (u32, u32) {
    let keys: Vec<&str> = text
        .split("\x1b_G")
        .find(|command| command.starts_with("a=T"))
        .unwrap()
        .split(',')
        .collect();
    let find = |name: &str| {
        keys.iter()
            .find_map(|key| key.strip_prefix(name))
            .and_then(|value| value.parse().ok())
            .unwrap()
    };
    (find("s="), find("v="))
}

#[test]
fn opaque_6k_image_needs_no_resize() {
    use crate::decode::ScreenImage;

    let rgb = image::RgbImage::from_pixel(6016, 3384, image::Rgb([9, 9, 9]));
    assert!(matches!(
        super::within_payload_cap(&ScreenImage::Rgb(rgb)),
        std::borrow::Cow::Borrowed(_)
    ));
    let rgba = ScreenImage::Rgba(six_k_image(false));
    assert!(
        matches!(
            super::within_payload_cap(&rgba),
            std::borrow::Cow::Borrowed(_)
        ),
        "an RGBA image without a transparent pixel goes out as RGB"
    );
}

#[test]
fn fullscreen_image_is_not_transient() {
    let mut app = App::new((8, 16), SharedImageList::new());
    app.image_rect = Rect::new(0, 0, 10, 10);
    app.loaded = Some(RgbaImage::from_pixel(16, 32, image::Rgba([9, 9, 9, 255])).into());
    let mut out = Vec::new();

    render_fullscreen_image_to(&mut app, &mut out).unwrap();

    assert!(!text(&out).contains("N="), "{}", text(&out));
}

#[test]
fn fullscreen_without_image_only_deletes() {
    let mut app = App::new((8, 16), SharedImageList::new());
    let mut out = Vec::new();

    render_fullscreen_image_to(&mut app, &mut out).unwrap();

    let text = text(&out);
    assert!(text.contains(DELETE_ALL));
    assert!(!text.contains(TRANSMIT));
}

#[cfg(feature = "video")]
#[test]
fn video_without_frame_writes_nothing() {
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
fn first_frame_removes_old_pictures() {
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
fn video_frame_uses_temp_file() {
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
fn video_frame_transient_with_animations() {
    use crate::encoder::Loops;

    for (loops, hinted) in [(Loops::Ticks, false), (Loops::Animation, true)] {
        let mut app = playing_app();
        app.graphics = app.graphics.with_loops(loops);
        let mut out = Vec::new();

        super::render_video_frame_to(&app, &mut out, false).unwrap();

        for path in crate::encoder::kitty::temp_file_paths(&out) {
            std::fs::remove_file(path).unwrap();
        }
        let text = text(&out);
        assert_eq!(text.contains(",r=38,N=1,t=t;"), hinted, "{text}");
        assert_eq!(text.contains("N="), hinted, "{text}");
    }
}

#[cfg(feature = "video")]
#[test]
fn help_popup_gets_no_video_frame() {
    let mut app = playing_app();
    app.help_visible = true;
    let mut out = Vec::new();

    super::render_video_frame_to(&app, &mut out, false).unwrap();

    assert!(out.is_empty());
}

#[cfg(feature = "video")]
#[test]
fn frame_without_cells_writes_nothing() {
    let mut app = playing_app();
    app.image_rect = Rect::default();
    let mut out = Vec::new();

    super::render_video_frame_to(&app, &mut out, false).unwrap();

    assert!(out.is_empty());
}

#[cfg(feature = "video")]
#[test]
fn one_row_terminal_closes_update() {
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
