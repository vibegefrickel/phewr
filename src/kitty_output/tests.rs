//! The tests of the fullscreen image and the video frame writes.
use super::render_fullscreen_image_to;
use crate::app::App;
use crate::image_list::SharedImageList;
use crate::pictures::jobs::thumbnail_cache::GridPicture;
use crate::pictures::resize::padding::pad_for_grid;
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
    app.screen.record(7, 4);
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
    assert_eq!(app.screen.stored_count(), 0);
}

/// A slideshow scales the image to the fill of its box with one side, so it never stretches.
#[test]
fn slideshow_fills_box_with_image() {
    let mut app = crate::test_support::fullscreen_app(3, 0);
    app.cell_px = (8, 16);
    // 80x160 px.
    app.image_rect = Rect::new(2, 1, 10, 10);
    let render = |app: &mut App, size: (u32, u32)| {
        app.loaded = Some(RgbaImage::new(size.0, size.1).into());
        let mut out = Vec::new();
        render_fullscreen_image_to(app, &mut out).unwrap();
        text(&out)
    };

    let native = render(&mut app, (10, 40));
    assert!(
        !native.contains(",r=") && !native.contains(",c="),
        "{native}"
    );
    assert!(!native.contains(",z="), "{native}");

    app.start_slideshow();
    let portrait = render(&mut app, (10, 40));
    assert!(
        portrait.contains(",z=-1073741825"),
        "the badge sits on top of the picture: {portrait}"
    );
    assert!(
        portrait.contains(",r=10,"),
        "flush with the top and the bottom: {portrait}"
    );
    assert!(!portrait.contains(",c="), "{portrait}");
    assert!(
        portrait.contains("\x1b[2;5H"),
        "5 of 10 columns, centered: {portrait}"
    );

    let landscape = render(&mut app, (40, 10));
    assert!(
        landscape.contains(",c=10,"),
        "flush with the left and the right: {landscape}"
    );
    assert!(!landscape.contains(",r="), "{landscape}");
    assert!(
        landscape.contains("\x1b[6;3H"),
        "2 of 10 rows, centered: {landscape}"
    );

    let large = render(&mut app, (800, 3200));
    assert!(
        large.contains(",r=10,"),
        "a large image fills the box too: {large}"
    );
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
    use crate::pictures::kitty::{KittyBackend, MAX_PAYLOAD_BYTES, Transfer, protocol};
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
    let paths = protocol::temp_file_paths(&out);
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
    use crate::pictures::decode::ScreenImage;

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

/// In a slideshow, the video frame sits below the badge too.
#[cfg(feature = "video")]
#[test]
fn slideshow_frame_below_badge() {
    let mut app = playing_app();
    let mut before = Vec::new();
    super::render_video_frame_to(&app, &mut before, false).unwrap();

    app.start_slideshow();
    let mut during = Vec::new();
    super::render_video_frame_to(&app, &mut during, false).unwrap();

    for bytes in [&before, &during] {
        for path in crate::pictures::kitty::protocol::temp_file_paths(bytes) {
            std::fs::remove_file(path).unwrap();
        }
    }
    assert!(!text(&before).contains(",z="), "{}", text(&before));
    assert!(
        text(&during).contains(",z=-1073741825"),
        "{}",
        text(&during)
    );
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
        for path in crate::pictures::kitty::protocol::temp_file_paths(bytes) {
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

    for path in crate::pictures::kitty::protocol::temp_file_paths(&out) {
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
    use crate::pictures::kitty::Loops;

    for (loops, hinted) in [(Loops::Ticks, false), (Loops::Animation, true)] {
        let mut app = playing_app();
        app.graphics = app.graphics.with_loops(loops);
        let mut out = Vec::new();

        super::render_video_frame_to(&app, &mut out, false).unwrap();

        for path in crate::pictures::kitty::protocol::temp_file_paths(&out) {
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

/// The DCT scaling of libjpeg-turbo decides the scale of the resize.
#[cfg(feature = "turbo")]
mod jpeg {
    use super::render_fullscreen_image_to;
    use crate::app::App;
    use crate::image_list::SharedImageList;
    use crate::pictures::decode::color;
    use crate::pictures::decode::hdr::Peak;
    use crate::pictures::decode::screen::tests::rgba_fullscreen;
    use crate::pictures::decode::screen::{ScreenImage, load_for_screen};
    use crate::pictures::decode::{EmbeddedThumbnail, decode_image};
    use crate::pictures::kitty::{KittyBackend, Pixels, Transfer};
    use crate::test_support::test_dir;
    use image::{DynamicImage, Rgb, RgbImage};
    use ratatui::layout::Rect;
    use std::path::{Path, PathBuf};

    fn gradient_jpeg(dir: &Path) -> PathBuf {
        let path = dir.join("gradient.jpg");
        RgbImage::from_fn(640, 480, |x, y| Rgb([(x % 256) as u8, (y % 256) as u8, 99]))
            .save(&path)
            .unwrap();
        path
    }

    /// The bytes of `image` as the fullscreen image of a test app, through the direct medium.
    fn written(image: ScreenImage) -> Vec<u8> {
        let mut app = App::new((8, 16), SharedImageList::new());
        app.graphics = KittyBackend::new(Transfer::Direct);
        app.image_rect = Rect::new(0, 0, 40, 20);
        app.loaded = Some(image);
        let mut out = Vec::new();
        render_fullscreen_image_to(&mut app, &mut out).unwrap();
        out
    }

    fn written_before(path: &Path, box_px: (u32, u32)) -> Vec<u8> {
        let decoded = decode_image(path, Some(box_px), EmbeddedThumbnail::Deny).unwrap();
        written(rgba_fullscreen(decoded, box_px, color::target()).into())
    }

    #[test]
    fn half_scale_jpeg_stays_rgb() {
        let dir = test_dir();
        let path = gradient_jpeg(dir.path());
        // libjpeg-turbo scales to 80x60, and the fit to 300x30 scales to 1/2 after.
        let box_px = (300, 30);
        let decoded = decode_image(&path, Some(box_px), EmbeddedThumbnail::Deny).unwrap();
        let DynamicImage::ImageRgb8(rgb) = &decoded.image else {
            panic!("the decode is RGB: {:?}", decoded.image.color())
        };
        assert_eq!(rgb.dimensions(), (80, 60));
        let source = decoded.color.clone();
        let screen = ScreenImage::of_decode(decoded.image, decoded.opaque);
        assert!(matches!(screen, ScreenImage::Rgb(_)), "the layout is RGB");
        let fitted = screen.fit_within(box_px.0, box_px.1);
        assert!(matches!(fitted, ScreenImage::Rgb(_)), "the resize is RGB");
        let converted = fitted.convert(&source, color::target(), Peak::DEFAULT);
        assert!(
            matches!(converted, ScreenImage::Rgb(_)),
            "the conversion is RGB"
        );
        let loaded = load_for_screen(&path, box_px).unwrap();
        assert_eq!(loaded, converted);
        let pixels = Pixels::from(&loaded);
        assert!(
            !pixels.alpha && pixels.opaque_rgb().is_none(),
            "the write has nothing to scan or copy"
        );
        let after = written(loaded);
        assert!(String::from_utf8_lossy(&after).contains(",f=24,s=40,v=30,"));
        assert_eq!(
            after,
            written_before(&path, box_px),
            "the same bytes as before"
        );
    }

    #[test]
    fn mild_scale_jpeg_resizes_as_rgba() {
        let dir = test_dir();
        let path = gradient_jpeg(dir.path());
        // libjpeg-turbo scales to 320x240, and the fit to 200x200 scales to 0.625 after.
        let box_px = (200, 200);
        let decoded = decode_image(&path, Some(box_px), EmbeddedThumbnail::Deny).unwrap();
        assert!(decoded.opaque, "a JPEG has no alpha channel");
        let source = decoded.color.clone();
        let screen = ScreenImage::of_decode(decoded.image, decoded.opaque);
        assert!(
            matches!(&screen, ScreenImage::Opaque(rgba) if rgba.dimensions() == (320, 240)),
            "RGBA without the alpha multiply resizes fastest at this scale"
        );
        let fitted = screen.fit_within(box_px.0, box_px.1);
        assert!(matches!(fitted, ScreenImage::Rgb(_)), "the fit gives RGB");
        let loaded = load_for_screen(&path, box_px).unwrap();
        assert_eq!(
            loaded,
            fitted.convert(&source, color::target(), Peak::DEFAULT)
        );
        let pixels = Pixels::from(&loaded);
        assert!(!pixels.alpha && pixels.opaque_rgb().is_none());
        let after = written(loaded);
        assert!(String::from_utf8_lossy(&after).contains(",f=24,s=200,v=150,"));
        assert_eq!(
            after,
            written_before(&path, box_px),
            "the same bytes as before"
        );
    }
}
