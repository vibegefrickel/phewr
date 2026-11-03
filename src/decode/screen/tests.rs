//! The tests of the screen layouts against the RGBA reference path.

use super::{ScreenImage, load_for_screen};
use crate::decode::color::{self, ColorTarget, converts};
use crate::decode::hdr::Peak;
use crate::decode::resize::{Layout, fit_rgba_within, resize_region};
use crate::decode::{Decoded, EmbeddedThumbnail, decode_image};
use crate::encoder::Pixels;
use crate::test_support::{fixture, test_dir};
use image::{DynamicImage, Rgb, RgbImage, Rgba, RgbaImage};
use std::path::PathBuf;

/// Every checked-in SDR still that the build decodes. The HDR files resize in 16 bit before the
/// tone map, and `decode::hdr` tests them.
fn still_fixtures() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = ["color", "png", "jxl"]
        .iter()
        .flat_map(|dir| std::fs::read_dir(fixture(dir)).unwrap())
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file() && path.extension().is_none_or(|e| e != "sh"))
        .chain(["still.png", "still.gif", "still.webp", "still.avif"].map(fixture))
        .filter(|path| {
            decode_image(path, None, EmbeddedThumbnail::Deny)
                .is_ok_and(|decoded| !crate::decode::hdr::is_hdr(&decoded.color))
        })
        .collect();
    paths.sort();
    paths
}

/// The alpha flag and the bytes that the Kitty write sends.
fn wire(pixels: Pixels<'_>) -> (bool, Vec<u8>) {
    match pixels.opaque_rgb() {
        Some(rgb) => (false, rgb),
        None => (pixels.alpha, pixels.data.to_vec()),
    }
}

/// The fullscreen image of the RGBA reference path.
fn rgba_fullscreen(decoded: Decoded, (w, h): (u32, u32), target: ColorTarget) -> RgbaImage {
    let fitted = fit_rgba_within(decoded.image.into_rgba8(), w, h);
    color::convert(fitted, &decoded.color, target, Peak::DEFAULT)
}

fn screen_fullscreen(decoded: Decoded, (w, h): (u32, u32), target: ColorTarget) -> ScreenImage {
    ScreenImage::of_decode(decoded.image, decoded.opaque)
        .fit_within(w, h)
        .convert(&decoded.color, target, Peak::DEFAULT)
        .opaque_as_rgb()
}

/// The zoom crop of the RGBA reference path: the crop copied into RGBA, then resized.
fn rgba_zoom(
    decoded: Decoded,
    (x, y, w, h): (u32, u32, u32, u32),
    dst: (u32, u32),
    target: ColorTarget,
) -> RgbaImage {
    let display = if converts(&decoded.color, target) {
        DynamicImage::ImageRgba8(color::convert(
            decoded.image.into_rgba8(),
            &decoded.color,
            target,
            Peak::DEFAULT,
        ))
    } else {
        decoded.image
    };
    let crop = display.crop_imm(x, y, w, h).to_rgba8();
    if dst == (w, h) {
        return crop;
    }
    let resized = resize_region(crop.as_raw(), (w, h), (0, 0, w, h), dst, Layout::Rgba);
    RgbaImage::from_raw(dst.0, dst.1, resized.unwrap().data).unwrap()
}

fn screen_zoom(
    decoded: Decoded,
    crop: (u32, u32, u32, u32),
    dst: (u32, u32),
    target: ColorTarget,
) -> ScreenImage {
    ScreenImage::of_decode(decoded.image, decoded.opaque)
        .convert(&decoded.color, target, Peak::DEFAULT)
        .for_crops()
        .crop_resized(crop, dst)
}

/// The grid picture keeps the pixels of the RGBA reference path, and the thumbnail cache keeps
/// its version.
#[test]
fn grid_picture_matches_rgba_path() {
    let decode = |path: &PathBuf, box_px| {
        decode_image(path, Some(box_px), EmbeddedThumbnail::Allow).unwrap()
    };
    let mut checked = 0;
    for path in still_fixtures() {
        for box_px in [(16, 16), (40, 30), (7, 5), (200, 200)] {
            let before = rgba_fullscreen(decode(&path, box_px), box_px, color::target());
            let after = decode(&path, box_px).fit_within(box_px.0, box_px.1);
            assert_eq!(after, before, "{path:?} {box_px:?}");
            checked += 1;
        }
    }
    let least = if cfg!(feature = "heic") { 150 } else { 140 };
    assert!(checked >= least, "{checked} fixtures and boxes");
}

#[test]
fn wire_bytes_match_rgba_path() {
    let decode = |path: &PathBuf, hint| decode_image(path, hint, EmbeddedThumbnail::Deny).unwrap();
    let mut checked = 0;
    for path in still_fixtures() {
        for target in [ColorTarget::DisplayP3, ColorTarget::Srgb] {
            for box_px in [(0, 0), (40, 40), (7, 5)] {
                let hint = (box_px.0 > 0).then_some(box_px);
                let before = rgba_fullscreen(decode(&path, hint), box_px, target);
                let after = screen_fullscreen(decode(&path, hint), box_px, target);
                assert_eq!(
                    after.dimensions(),
                    before.dimensions(),
                    "{path:?} {box_px:?}"
                );
                let opaque = Pixels::from(&before).opaque_rgb().is_some();
                assert_eq!(matches!(after, ScreenImage::Rgb(_)), opaque, "{path:?}");
                assert_eq!(
                    wire((&after).into()),
                    wire((&before).into()),
                    "{path:?} {box_px:?}"
                );
            }
            let image = decode(&path, None).image;
            let (w, h) = (image.width(), image.height());
            let crops = [(0, 0, w, h), (w / 4, h / 4, w / 2, h / 2), (1, 0, w - 1, h)];
            for crop in crops {
                for dst in [
                    (crop.2, crop.3),
                    (crop.2 * 3, crop.3 * 2),
                    ((crop.2 / 3).max(1), (crop.3 / 2).max(1)),
                ] {
                    let before = rgba_zoom(decode(&path, None), crop, dst, target);
                    let after = screen_zoom(decode(&path, None), crop, dst, target);
                    assert_eq!(after.dimensions(), dst, "{path:?}");
                    assert_eq!(
                        wire((&after).into()),
                        wire((&before).into()),
                        "{path:?} {crop:?} {dst:?}"
                    );
                }
            }
            checked += 1;
        }
    }
    assert!(checked >= 60, "{checked} fixtures and targets");
}

/// The DCT scaling of libjpeg-turbo decides the scale of the resize.
#[cfg(feature = "turbo")]
mod jpeg {
    use super::*;
    use crate::app::App;
    use crate::encoder::{KittyBackend, Transfer};
    use crate::image_list::SharedImageList;
    use crate::kitty_output::render_fullscreen_image_to;
    use ratatui::layout::Rect;
    use std::path::Path;

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
        // libjpeg-turbo scales to 320x240, and the fit to 300x40 scales to 1/6 after.
        let box_px = (300, 40);
        let decoded = decode_image(&path, Some(box_px), EmbeddedThumbnail::Deny).unwrap();
        let DynamicImage::ImageRgb8(rgb) = &decoded.image else {
            panic!("the decode is RGB: {:?}", decoded.image.color())
        };
        assert_eq!(rgb.dimensions(), (320, 240));
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
        assert!(String::from_utf8_lossy(&after).contains(",f=24,s=53,v=40,"));
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

#[test]
fn opaque_zoom_crops_are_rgb() {
    let dir = test_dir();
    let path = dir.path().join("wide.png");
    RgbImage::from_fn(64, 32, |x, y| Rgb([x as u8 * 4, y as u8 * 8, 7]))
        .save(&path)
        .unwrap();
    let source = decode_image(&path, None, EmbeddedThumbnail::Deny)
        .unwrap()
        .into_screen();
    assert!(matches!(source, ScreenImage::Opaque(_)), "{source:?}");
    for dst in [(20, 10), (100, 50), (40, 30), (40, 20)] {
        let crop = source.crop_resized((8, 4, 40, 20), dst);
        assert!(matches!(crop, ScreenImage::Rgb(_)), "{dst:?}");
        assert_eq!(crop.dimensions(), dst);
    }
    let expected = RgbImage::from_fn(40, 20, |x, y| {
        Rgb([(x + 8) as u8 * 4, (y + 4) as u8 * 8, 7])
    });
    assert_eq!(
        source.crop_resized((8, 4, 40, 20), (40, 20)),
        expected.into()
    );
}

#[test]
fn transparent_pixel_keeps_rgba() {
    let dir = test_dir();
    let path = dir.path().join("alpha.png");
    let mut image = RgbaImage::from_pixel(32, 16, Rgba([200, 10, 10, 255]));
    image.put_pixel(3, 3, Rgba([0, 0, 0, 0]));
    image.save(&path).unwrap();
    assert!(matches!(
        load_for_screen(&path, (0, 0)).unwrap(),
        ScreenImage::Rgba(_)
    ));
    RgbaImage::from_pixel(32, 16, Rgba([200, 10, 10, 255]))
        .save(&path)
        .unwrap();
    assert!(matches!(
        load_for_screen(&path, (16, 16)).unwrap(),
        ScreenImage::Rgb(_)
    ));
}

#[test]
fn other_pixel_types_turn_8_bit() {
    let gray =
        DynamicImage::ImageLuma16(image::ImageBuffer::from_pixel(2, 2, image::Luma([257 * 9])));
    let gray = ScreenImage::of_decode(gray, false);
    assert!(matches!(gray, ScreenImage::Opaque(_)), "{gray:?}");
    assert_eq!(
        gray.opaque_as_rgb(),
        RgbImage::from_pixel(2, 2, Rgb([9; 3])).into()
    );
    let gray_alpha =
        DynamicImage::ImageLumaA8(image::ImageBuffer::from_pixel(2, 2, image::LumaA([9, 255])));
    assert_eq!(
        ScreenImage::of_decode(gray_alpha, false).opaque_as_rgb(),
        RgbImage::from_pixel(2, 2, Rgb([9; 3])).into()
    );
}

#[test]
fn crop_scales_region_both_ways() {
    let image = RgbImage::from_fn(40, 30, |x, y| Rgb([x as u8 * 6, y as u8 * 8, 99]));
    let rgb = ScreenImage::from(image.clone());
    let rgba = ScreenImage::from(DynamicImage::ImageRgb8(image.clone()).into_rgba8());
    let ScreenImage::Rgba(opaque) = rgba.clone() else {
        panic!("RGBA")
    };
    let opaque = ScreenImage::Opaque(opaque);
    for (crop, dst) in [
        ((5, 4, 20, 10), (60, 30)),
        ((5, 4, 20, 10), (7, 3)),
        ((0, 0, 40, 30), (40, 30)),
    ] {
        let from_rgb = rgb.crop_resized(crop, dst);
        assert!(
            matches!(from_rgb, ScreenImage::Rgb(_)),
            "an RGB source gives an RGB crop"
        );
        assert_eq!(from_rgb.dimensions(), dst);
        assert_eq!(
            from_rgb,
            rgba.crop_resized(crop, dst).opaque_as_rgb(),
            "{crop:?}"
        );
        assert_eq!(from_rgb, opaque.crop_resized(crop, dst), "{crop:?}");
        let copy = ScreenImage::from(
            image::imageops::crop_imm(&image, crop.0, crop.1, crop.2, crop.3).to_image(),
        );
        assert_eq!(from_rgb, copy.crop_resized((0, 0, crop.2, crop.3), dst));
    }
    assert_eq!(rgb.clone().fit_within(20, 20).dimensions(), (20, 15));
    assert_eq!(rgb.fit_within(80, 80).dimensions(), (40, 30), "never up");
}

#[test]
fn transparent_pixel_turns_rgb_into_rgba() {
    let mut image = ScreenImage::from(RgbImage::from_pixel(2, 1, Rgb([9, 9, 9])));
    image.put_pixel(0, 0, Rgba([1, 2, 3, 255]));
    assert_eq!(
        image,
        RgbImage::from_raw(2, 1, vec![1, 2, 3, 9, 9, 9])
            .unwrap()
            .into()
    );
    image.put_pixel(1, 0, Rgba([0, 0, 0, 0]));
    assert_eq!(
        image,
        RgbaImage::from_raw(2, 1, vec![1, 2, 3, 255, 0, 0, 0, 0])
            .unwrap()
            .into()
    );
}

#[cfg(feature = "turbo")]
#[test]
fn turbo_rgb_matches_rgba() {
    for name in [
        "color/srgb.jpg",
        "color/p3-icc.jpg",
        "color/adobe-rgb-icc.jpg",
    ] {
        let data = std::fs::read(fixture(name)).unwrap();
        let rgb = turbojpeg::decompress(&data, turbojpeg::PixelFormat::RGB).unwrap();
        let rgba = turbojpeg::decompress(&data, turbojpeg::PixelFormat::RGBA).unwrap();
        let rows = |image: &turbojpeg::Image<Vec<u8>>, size: usize| -> Vec<[u8; 3]> {
            (0..image.height)
                .flat_map(|y| {
                    let row = &image.pixels[y * image.pitch..][..image.width * size];
                    row.chunks(size)
                        .map(|pixel| [pixel[0], pixel[1], pixel[2]])
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        assert_eq!(rows(&rgb, 3), rows(&rgba, 4), "{name}");
    }
}

#[cfg(feature = "heic")]
#[test]
fn libheif_rgb_matches_rgba() {
    use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};
    for name in [
        "color/p3-icc.heic",
        "color/p3-nclx.heic",
        "color/p3-nclx.avif",
        "still.avif",
    ] {
        let path = fixture(name);
        let ctx = HeifContext::read_from_file(path.to_str().unwrap()).unwrap();
        let handle = ctx.primary_image_handle().unwrap();
        let rows = |chroma, size: usize| -> Vec<[u8; 3]> {
            let image = LibHeif::new()
                .decode(&handle, ColorSpace::Rgb(chroma), None)
                .unwrap();
            let plane = image.planes().interleaved.unwrap();
            (0..plane.height as usize)
                .flat_map(|y| {
                    let row = &plane.data[y * plane.stride..][..plane.width as usize * size];
                    row.chunks(size)
                        .map(|pixel| [pixel[0], pixel[1], pixel[2]])
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        assert_eq!(rows(RgbChroma::Rgb, 3), rows(RgbChroma::Rgba, 4), "{name}");
    }
}
