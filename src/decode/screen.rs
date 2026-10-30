//! The pixels of the fullscreen view and of the zoom. An opaque image reaches the Kitty write
//! with 3 bytes per pixel, so no write scans or copies it.
//!
//! The layout of each step is the fastest one that `fast_image_resize` has for it (measured
//! 2026-10-30, `rgb_resize_is_fastest`): an opaque image resizes as RGBA without the alpha
//! multiply and is copied into RGB after, at the size of the screen. Only a downscale to 1/2
//! or less resizes as RGB, without a copy.

use super::color::{self, ColorTarget, SourceColor};
use super::resize::{Layout, Resized, resize_region, rgb_resize_is_fastest, scaled_down_size};
use super::{EmbeddedThumbnail, decode_image};
use crate::encoder::Pixels;
use image::{DynamicImage, RgbImage, RgbaImage};
use std::io;
use std::path::Path;

/// Decode `path` for the fullscreen box `target` in pixels, as `load_and_resize_px` does for
/// the grid, but in the pixel layout of the decode. A zero box keeps the decoded size.
pub(crate) fn load_for_screen(
    path: &Path,
    (target_w, target_h): (u32, u32),
) -> io::Result<ScreenImage> {
    let hint = (target_w > 0 && target_h > 0).then_some((target_w, target_h));
    Ok(decode_image(path, hint, EmbeddedThumbnail::Deny)?.fit_for_screen(target_w, target_h))
}

/// A fullscreen image, a zoom source or a zoom crop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScreenImage {
    /// Every pixel is opaque. The Kitty write sends it as `f=24` without a scan or a copy.
    Rgb(RgbImage),
    /// An image with an alpha channel. The Kitty write sends it as RGB when every pixel is
    /// opaque, so the bytes on the wire match the RGB layout.
    Rgba(RgbaImage),
    /// RGBA with the alpha at 255 in every pixel: a decode without an alpha channel, or the
    /// zoom source. It resizes without the alpha multiply, and the result is RGB.
    Opaque(RgbaImage),
}

impl ScreenImage {
    /// The layout of a decode: 8-bit RGB and RGBA stay as they are, and `opaque` marks an RGBA
    /// decode without an alpha channel. The other pixel types turn into 8-bit RGBA, which is
    /// `Opaque` without an alpha channel.
    pub(super) fn of_decode(image: DynamicImage, opaque: bool) -> Self {
        match image {
            DynamicImage::ImageRgb8(rgb) => Self::Rgb(rgb),
            DynamicImage::ImageRgba8(rgba) if opaque => Self::Opaque(rgba),
            DynamicImage::ImageRgba8(rgba) => Self::Rgba(rgba),
            other if other.color().has_alpha() => Self::Rgba(other.into_rgba8()),
            other => Self::Opaque(other.into_rgba8()),
        }
    }

    /// The width in pixels, as `RgbaImage::width`, so the readers of `App::loaded` keep their code.
    pub(crate) fn width(&self) -> u32 {
        self.dimensions().0
    }

    /// The height in pixels, as `RgbaImage::height`.
    pub(crate) fn height(&self) -> u32 {
        self.dimensions().1
    }

    /// The size in pixels in every layout.
    pub(crate) fn dimensions(&self) -> (u32, u32) {
        match self {
            Self::Rgb(rgb) => rgb.dimensions(),
            Self::Rgba(rgba) | Self::Opaque(rgba) => rgba.dimensions(),
        }
    }

    /// The pixels as RGBA, for the grid, whose padding needs the alpha channel.
    pub(crate) fn into_rgba(self) -> RgbaImage {
        match self {
            Self::Rgb(rgb) => DynamicImage::ImageRgb8(rgb).into_rgba8(),
            Self::Rgba(rgba) | Self::Opaque(rgba) => rgba,
        }
    }

    /// The image with RGB pixels when every pixel is opaque. The check runs one time after the
    /// decode or the crop, so the writes of the image need none.
    pub(super) fn opaque_as_rgb(self) -> Self {
        let (Self::Rgba(rgba) | Self::Opaque(rgba)) = &self else {
            return self;
        };
        let (width, height) = rgba.dimensions();
        match Pixels::from(rgba)
            .opaque_rgb()
            .and_then(|rgb| RgbImage::from_raw(width, height, rgb))
        {
            Some(rgb) => Self::Rgb(rgb),
            None => self,
        }
    }

    /// The zoom source: an opaque image as `Opaque` RGBA, because each crop resizes fastest
    /// without the alpha multiply. An RGBA decode pays one alpha check here for all its crops.
    pub(super) fn for_crops(self) -> Self {
        match self {
            Self::Rgb(rgb) => Self::Opaque(DynamicImage::ImageRgb8(rgb).into_rgba8()),
            Self::Rgba(rgba) if is_opaque(&rgba) => Self::Opaque(rgba),
            other => other,
        }
    }

    /// The image in `target`, in its own layout.
    pub(super) fn convert(self, source: &SourceColor, target: ColorTarget) -> Self {
        match self {
            Self::Rgb(rgb) => Self::Rgb(color::convert_rgb(rgb, source, target)),
            Self::Rgba(rgba) => Self::Rgba(color::convert(rgba, source, target)),
            Self::Opaque(rgba) => Self::Opaque(color::convert(rgba, source, target)),
        }
    }

    /// The image scaled down into `max_w` x `max_h` with its aspect ratio, and never up. An
    /// opaque image comes back as RGB.
    pub(crate) fn fit_within(self, max_w: u32, max_h: u32) -> Self {
        let (width, height) = self.dimensions();
        match scaled_down_size((width, height), (max_w, max_h)) {
            Some(dst) => self
                .resized_region((0, 0, width, height), dst)
                .unwrap_or(self),
            None => self,
        }
        .opaque_as_rgb_if_known()
    }

    /// The region `crop` (x, y, width, height) scaled to exactly `dst`, also up. The crop of an
    /// opaque source is RGB. The resize reads the region in the image and copies no crop first.
    pub(crate) fn crop_resized(&self, crop: (u32, u32, u32, u32), dst: (u32, u32)) -> Self {
        let resized = if dst == (crop.2, crop.3) || dst.0 == 0 || dst.1 == 0 {
            None
        } else {
            self.resized_region(crop, dst)
        };
        resized
            .unwrap_or_else(|| self.crop(crop))
            .opaque_as_rgb_if_known()
    }

    /// An `Opaque` image as RGB. The copy checks the alpha in the same pass, so a pixel that
    /// is not opaque keeps the image RGBA. The other layouts stay.
    fn opaque_as_rgb_if_known(self) -> Self {
        match self {
            Self::Opaque(_) => self.opaque_as_rgb(),
            other => other,
        }
    }

    /// The region `crop` copied out at its size. An `Opaque` region goes into RGB in the same
    /// pass, so a zoom step at the scale 1 pays one copy.
    fn crop(&self, crop: (u32, u32, u32, u32)) -> Self {
        use image::imageops::crop_imm;
        let (x, y, width, height) = crop;
        match self {
            Self::Rgb(rgb) => Self::Rgb(crop_imm(rgb, x, y, width, height).to_image()),
            Self::Rgba(rgba) => Self::Rgba(crop_imm(rgba, x, y, width, height).to_image()),
            Self::Opaque(rgba) => rgb_region(rgba, crop).map_or_else(
                || Self::Opaque(crop_imm(rgba, x, y, width, height).to_image()),
                Self::Rgb,
            ),
        }
    }

    /// The region `crop` scaled to `dst` in the fastest layout. An RGB image that resizes
    /// faster as RGBA turns into RGBA first. `None` when the resize fails.
    fn resized_region(&self, crop: (u32, u32, u32, u32), dst: (u32, u32)) -> Option<Self> {
        let rgba =
            |resized: Resized| RgbaImage::from_raw(resized.width, resized.height, resized.data);
        match self {
            Self::Rgb(rgb) if rgb_resize_is_fastest((crop.2, crop.3), Some(dst)) => {
                let resized =
                    resize_region(rgb.as_raw(), rgb.dimensions(), crop, dst, Layout::Rgb)?;
                RgbImage::from_raw(resized.width, resized.height, resized.data).map(Self::Rgb)
            }
            Self::Rgb(rgb) => {
                let region = rgba_region(rgb, crop)?;
                let (width, height) = region.dimensions();
                let full = (0, 0, width, height);
                let resized = resize_region(
                    region.as_raw(),
                    (width, height),
                    full,
                    dst,
                    Layout::OpaqueRgba,
                )?;
                rgba(resized).map(Self::Opaque)
            }
            Self::Rgba(image) => {
                let resized =
                    resize_region(image.as_raw(), image.dimensions(), crop, dst, Layout::Rgba)?;
                rgba(resized).map(Self::Rgba)
            }
            Self::Opaque(image) => {
                let resized = resize_region(
                    image.as_raw(),
                    image.dimensions(),
                    crop,
                    dst,
                    Layout::OpaqueRgba,
                )?;
                rgba(resized).map(Self::Opaque)
            }
        }
    }

    /// Set one pixel, as `RgbaImage::put_pixel` does. A transparent pixel turns an RGB image
    /// into RGBA, so the bench can write the image in both layouts.
    #[cfg(test)]
    pub(crate) fn put_pixel(&mut self, x: u32, y: u32, pixel: image::Rgba<u8>) {
        use image::Pixel;
        if let Self::Rgb(rgb) = self {
            if pixel[3] == u8::MAX {
                rgb.put_pixel(x, y, pixel.to_rgb());
                return;
            }
            *self = Self::Rgba(DynamicImage::ImageRgb8(std::mem::take(rgb)).into_rgba8());
        }
        if let Self::Opaque(rgba) = self {
            *self = Self::Rgba(std::mem::take(rgba));
        }
        if let Self::Rgba(rgba) = self {
            rgba.put_pixel(x, y, pixel);
        }
    }
}

/// The region `crop` (x, y, width, height) of `rgb` as RGBA, in one pass. `None` for a region
/// outside the image.
fn rgba_region(rgb: &RgbImage, (x, y, width, height): (u32, u32, u32, u32)) -> Option<RgbaImage> {
    if x.checked_add(width)? > rgb.width() || y.checked_add(height)? > rgb.height() {
        return None;
    }
    let (x, width) = (x as usize, width as usize);
    let mut data = vec![u8::MAX; width * height as usize * 4];
    let rows = rgb
        .as_raw()
        .chunks_exact(rgb.width() as usize * 3)
        .skip(y as usize)
        .take(height as usize);
    for (source, target) in rows.zip(data.chunks_exact_mut(width * 4)) {
        let (source, _) = source[x * 3..(x + width) * 3].as_chunks::<3>();
        // The array stores let the compiler turn the loop into vector code, as in
        // `Pixels::opaque_rgb`.
        for (&[red, green, blue], pixel) in source.iter().zip(target.as_chunks_mut::<4>().0) {
            *pixel = [red, green, blue, u8::MAX];
        }
    }
    RgbaImage::from_raw(u32::try_from(width).ok()?, height, data)
}

/// The region `crop` (x, y, width, height) of the opaque `rgba` as RGB, in one pass. `None` for
/// a region outside the image.
fn rgb_region(rgba: &RgbaImage, (x, y, width, height): (u32, u32, u32, u32)) -> Option<RgbImage> {
    if x.checked_add(width)? > rgba.width() || y.checked_add(height)? > rgba.height() {
        return None;
    }
    let (x, width) = (x as usize, width as usize);
    let mut data = vec![0; width * height as usize * 3];
    let rows = rgba
        .as_raw()
        .chunks_exact(rgba.width() as usize * 4)
        .skip(y as usize)
        .take(height as usize);
    for (source, target) in rows.zip(data.chunks_exact_mut(width * 3)) {
        let (source, _) = source[x * 4..(x + width) * 4].as_chunks::<4>();
        for (&[red, green, blue, _], pixel) in source.iter().zip(target.as_chunks_mut::<3>().0) {
            *pixel = [red, green, blue];
        }
    }
    RgbImage::from_raw(u32::try_from(width).ok()?, height, data)
}

/// True when the alpha of every pixel is 255. The check reads the image once and stops at the
/// first chunk with a transparent pixel.
fn is_opaque(rgba: &RgbaImage) -> bool {
    let (pixels, _) = rgba.as_raw().as_chunks::<4>();
    pixels
        .chunks(4096)
        .all(|chunk| chunk.iter().fold(u8::MAX, |all, pixel| all & pixel[3]) == u8::MAX)
}

impl From<RgbImage> for ScreenImage {
    fn from(image: RgbImage) -> Self {
        Self::Rgb(image)
    }
}

impl From<RgbaImage> for ScreenImage {
    fn from(image: RgbaImage) -> Self {
        Self::Rgba(image)
    }
}

impl<'a> From<&'a ScreenImage> for Pixels<'a> {
    fn from(image: &'a ScreenImage) -> Self {
        match image {
            ScreenImage::Rgb(rgb) => Self::from(rgb),
            ScreenImage::Rgba(rgba) | ScreenImage::Opaque(rgba) => Self::from(rgba),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ScreenImage, load_for_screen};
    use crate::decode::color::{self, ColorTarget, converts};
    use crate::decode::resize::{Layout, fit_rgba_within, resize_region};
    use crate::decode::{Decoded, EmbeddedThumbnail, decode_image};
    use crate::encoder::Pixels;
    use crate::test_support::{fixture, test_dir};
    use image::{DynamicImage, Rgb, RgbImage, Rgba, RgbaImage};
    use std::path::PathBuf;

    /// Every checked-in still: the color charts, the PNG layouts, the JPEG XL charts and the
    /// still images of the animation fixtures.
    fn still_fixtures() -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = ["color", "png", "jxl"]
            .iter()
            .flat_map(|dir| std::fs::read_dir(fixture(dir)).unwrap())
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_file() && path.extension().is_none_or(|e| e != "sh"))
            .collect();
        paths.extend(["still.png", "still.gif", "still.webp", "still.avif"].map(fixture));
        paths.sort();
        paths
    }

    /// The bytes and the alpha flag that the Kitty write sends for `pixels`.
    fn wire(pixels: Pixels<'_>) -> (bool, Vec<u8>) {
        match pixels.opaque_rgb() {
            Some(rgb) => (false, rgb),
            None => (pixels.alpha, pixels.data.to_vec()),
        }
    }

    /// The fullscreen image of the RGBA path before the change: `Decoded::fit_within`.
    fn rgba_fullscreen(decoded: Decoded, (w, h): (u32, u32), target: ColorTarget) -> RgbaImage {
        let fitted = fit_rgba_within(decoded.image.into_rgba8(), w, h);
        color::convert(fitted, &decoded.color, target)
    }

    /// `Decoded::fit_for_screen` with the target as an argument, so the test needs no global.
    fn screen_fullscreen(decoded: Decoded, (w, h): (u32, u32), target: ColorTarget) -> ScreenImage {
        ScreenImage::of_decode(decoded.image, decoded.opaque)
            .fit_within(w, h)
            .convert(&decoded.color, target)
            .opaque_as_rgb()
    }

    /// The zoom crop of the RGBA path before the change: `into_display`, `crop_imm` and
    /// `resize_to_exact`, which copied the crop into RGBA and resized it.
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

    /// `Decoded::into_screen` and `App::build_zoom_view` with the target as an argument.
    fn screen_zoom(
        decoded: Decoded,
        crop: (u32, u32, u32, u32),
        dst: (u32, u32),
        target: ColorTarget,
    ) -> ScreenImage {
        ScreenImage::of_decode(decoded.image, decoded.opaque)
            .convert(&decoded.color, target)
            .for_crops()
            .crop_resized(crop, dst)
    }

    /// The grid picture, `Decoded::fit_within`, as `load_and_resize_px` and the thumbnail jobs
    /// call it: the same pixels as the RGBA path with the alpha multiply before the change, so
    /// the thumbnail cache keeps its version.
    #[test]
    fn every_fixture_gives_the_grid_picture_of_the_rgba_path() {
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
        assert!(checked >= 150, "{checked} fixtures and boxes");
    }

    #[test]
    fn every_fixture_sends_the_bytes_of_the_rgba_path_in_both_targets() {
        let decode =
            |path: &PathBuf, hint| decode_image(path, hint, EmbeddedThumbnail::Deny).unwrap();
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

    /// The JPEG layouts of libjpeg-turbo: its DCT scaling decides the scale of the resize.
    #[cfg(feature = "turbo")]
    mod jpeg {
        use super::*;
        use crate::app::App;
        use crate::encoder::{KittyBackend, Transfer};
        use crate::image_list::SharedImageList;
        use crate::kitty_output::render_fullscreen_image_to;
        use ratatui::layout::Rect;
        use std::path::Path;

        /// A 640x480 JPEG with a gradient, written into `dir`.
        fn gradient_jpeg(dir: &Path) -> PathBuf {
            let path = dir.join("gradient.jpg");
            RgbImage::from_fn(640, 480, |x, y| Rgb([(x % 256) as u8, (y % 256) as u8, 99]))
                .save(&path)
                .unwrap();
            path
        }

        /// Write `image` as the fullscreen image of a test app through the direct medium.
        fn written(image: ScreenImage) -> Vec<u8> {
            let mut app = App::new((8, 16), SharedImageList::new());
            app.graphics = KittyBackend::new(Transfer::Direct);
            app.image_rect = Rect::new(0, 0, 40, 20);
            app.loaded = Some(image);
            let mut out = Vec::new();
            render_fullscreen_image_to(&mut app, &mut out).unwrap();
            out
        }

        /// The bytes on the wire of the RGBA path before the change, for the same file and box.
        fn written_before(path: &Path, box_px: (u32, u32)) -> Vec<u8> {
            let decoded = decode_image(path, Some(box_px), EmbeddedThumbnail::Deny).unwrap();
            written(rgba_fullscreen(decoded, box_px, color::target()).into())
        }

        #[test]
        fn a_jpeg_that_shrinks_to_half_or_less_stays_rgb_from_the_decode_to_the_wire() {
            let dir = test_dir();
            let path = gradient_jpeg(dir.path());
            // libjpeg-turbo scales to 1/2 (320x240), and the fit to 300x40 scales to 1/6 after.
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
            let converted = fitted.convert(&source, color::target());
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
        fn a_jpeg_at_a_mild_scale_resizes_as_rgba_and_reaches_the_wire_as_rgb() {
            let dir = test_dir();
            let path = gradient_jpeg(dir.path());
            // libjpeg-turbo scales to 1/2 (320x240), and the fit to 200x200 scales to 0.625 after.
            let box_px = (200, 200);
            let decoded = decode_image(&path, Some(box_px), EmbeddedThumbnail::Deny).unwrap();
            assert!(decoded.opaque, "a JPEG has no alpha channel");
            let source = decoded.color.clone();
            let screen = ScreenImage::of_decode(decoded.image, decoded.opaque);
            assert!(
                matches!(&screen, ScreenImage::Opaque(rgba) if rgba.dimensions() == (320, 240)),
                "RGBA without the alpha multiply resizes fastest at this scale"
            );
            // The fit copies the RGBA result into RGB one time, on the decode thread.
            let fitted = screen.fit_within(box_px.0, box_px.1);
            assert!(matches!(fitted, ScreenImage::Rgb(_)), "the fit gives RGB");
            let loaded = load_for_screen(&path, box_px).unwrap();
            assert_eq!(loaded, fitted.convert(&source, color::target()));
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
    fn the_zoom_source_of_an_opaque_image_is_rgba_and_its_crops_are_rgb() {
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
        // At the scale 1 the crop is a copy of the region, straight into RGB.
        let expected = RgbImage::from_fn(40, 20, |x, y| {
            Rgb([(x + 8) as u8 * 4, (y + 4) as u8 * 8, 7])
        });
        assert_eq!(
            source.crop_resized((8, 4, 40, 20), (40, 20)),
            expected.into()
        );
    }

    #[test]
    fn a_transparent_pixel_keeps_the_fullscreen_image_rgba() {
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
    fn the_other_pixel_types_turn_into_8_bit_by_their_alpha_channel() {
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
    fn the_crop_reads_the_region_and_scales_up_and_down() {
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
            // The view reads only the region, as a copy of it does.
            let copy = ScreenImage::from(
                image::imageops::crop_imm(&image, crop.0, crop.1, crop.2, crop.3).to_image(),
            );
            assert_eq!(from_rgb, copy.crop_resized((0, 0, crop.2, crop.3), dst));
        }
        assert_eq!(rgb.clone().fit_within(20, 20).dimensions(), (20, 15));
        assert_eq!(rgb.fit_within(80, 80).dimensions(), (40, 30), "never up");
    }

    #[test]
    fn a_transparent_pixel_turns_an_rgb_image_into_rgba() {
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
    fn libjpeg_turbo_gives_the_same_values_as_rgb_and_as_rgba() {
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
    fn libheif_gives_the_same_values_as_rgb_and_as_rgba() {
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
}

/// The layouts of this module against the RGBA layout before them, step by step, in one
/// process: each step runs both ways in turn, so the load of the machine hits both the same.
#[cfg(test)]
#[cfg(feature = "turbo")]
mod layout_bench {
    use super::ScreenImage;
    use crate::app::App;
    use crate::decode::color::{self, ColorTarget};
    use crate::decode::resize::{Layout, resize_region, rgb_resize_is_fastest, scaled_down_size};
    use crate::decode::{EmbeddedThumbnail, decode_image};
    use crate::encoder::{KittyBackend, Transfer, kitty};
    use crate::image_list::SharedImageList;
    use crate::kitty_output::render_fullscreen_image_to;
    use image::{DynamicImage, RgbaImage};
    use ratatui::layout::Rect;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    /// The fullscreen box of `bench.rs`: 106x38 cells of 32x54 px.
    const BOX_PX: (u32, u32) = (3392, 2052);
    const RUNS: usize = 9;

    /// The median of `RUNS` runs of the RGBA way and of the new way, in turns.
    fn medians(
        mut before: impl FnMut() -> Duration,
        mut after: impl FnMut() -> Duration,
    ) -> (f64, f64) {
        let (mut old, mut new) = (Vec::new(), Vec::new());
        for _ in 0..RUNS {
            old.push(before());
            new.push(after());
        }
        let median = |times: &mut Vec<Duration>| {
            times.sort();
            times[RUNS / 2].as_secs_f64() * 1000.0
        };
        (median(&mut old), median(&mut new))
    }

    fn report(step: &str, (before, after): (f64, f64)) {
        // clippy.toml allows output in tests. `just bench` shows the lines.
        eprintln!("bench layout {step}: before {before:.1} ms, after {after:.1} ms");
    }

    fn time<T>(work: impl FnOnce() -> T) -> Duration {
        let start = Instant::now();
        let result = work();
        let elapsed = start.elapsed();
        drop(result);
        elapsed
    }

    /// The fullscreen write of `image` through the temp file medium, without the temp file.
    fn write(app: &mut App, image: ScreenImage) -> Duration {
        app.loaded = Some(image);
        let mut out = Vec::new();
        let elapsed = time(|| render_fullscreen_image_to(app, &mut out).unwrap());
        for path in kitty::temp_file_paths(&out) {
            std::fs::remove_file(path).unwrap();
        }
        elapsed
    }

    fn fullscreen_app() -> App {
        let mut app = App::new((32, 54), SharedImageList::new());
        app.graphics = KittyBackend::new(Transfer::TempFile);
        app.image_rect = Rect::new(0, 0, 106, 38);
        app
    }

    /// The resize of the RGBA way: with the alpha multiply.
    fn rgba_resize(rgba: &RgbaImage, crop: (u32, u32, u32, u32), dst: (u32, u32)) -> RgbaImage {
        let resized = resize_region(rgba.as_raw(), rgba.dimensions(), crop, dst, Layout::Rgba);
        RgbaImage::from_raw(dst.0, dst.1, resized.unwrap().data).unwrap()
    }

    /// The fit of the full decode into the box and the color conversion: RGBA with the alpha
    /// multiply before, the layout of `rgb_resize_is_fastest` now.
    fn measure_fit(name: &str, full: &RgbaImage, color: &color::SourceColor) {
        let size = full.dimensions();
        let dst = scaled_down_size(size, BOX_PX).unwrap_or(size);
        let after_input = if rgb_resize_is_fastest(size, Some(dst)) {
            ScreenImage::Rgb(DynamicImage::ImageRgba8(full.clone()).into_rgb8())
        } else {
            ScreenImage::Opaque(full.clone())
        };
        for target in [ColorTarget::DisplayP3, ColorTarget::Srgb] {
            let times = medians(
                || {
                    time(|| {
                        color::convert(
                            rgba_resize(full, (0, 0, size.0, size.1), dst),
                            color,
                            target,
                        )
                    })
                },
                || {
                    let input = after_input.clone();
                    time(|| input.fit_within(BOX_PX.0, BOX_PX.1).convert(color, target))
                },
            );
            report(
                &format!(
                    "{name} fit {}x{} into {}x{} and convert into {target:?}",
                    size.0, size.1, dst.0, dst.1
                ),
                times,
            );
        }
    }

    fn measure(path: &Path) {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let decoded = decode_image(path, None, EmbeddedThumbnail::Deny).unwrap();
        let color = decoded.color.clone();
        let full = decoded.image.to_rgba8();
        measure_fit(&name, &full, &color);

        let shown = ScreenImage::Opaque(full.clone()).fit_within(BOX_PX.0, BOX_PX.1);
        let rgba = ScreenImage::Rgba(shown.clone().into_rgba());
        let (mut rgba_app, mut rgb_app) = (fullscreen_app(), fullscreen_app());
        let writes = medians(
            || write(&mut rgba_app, rgba.clone()),
            || write(&mut rgb_app, shown.clone()),
        );
        report(
            &format!(
                "{name} fullscreen write {}x{}",
                shown.width(),
                shown.height()
            ),
            writes,
        );

        let (w, h) = full.dimensions();
        let old_source = DynamicImage::ImageRgba8(full.clone());
        let new_source = ScreenImage::Opaque(full).for_crops();
        for zoom in [1.25, 2.0, 4.0] {
            // The crop of `build_zoom_view` in the middle of the picture.
            let eff = f64::min(
                f64::from(BOX_PX.0) / f64::from(w),
                f64::from(BOX_PX.1) / f64::from(h),
            )
            .min(1.0)
                * zoom;
            let crop_w = ((f64::from(BOX_PX.0) / eff).round() as u32).clamp(1, w);
            let crop_h = ((f64::from(BOX_PX.1) / eff).round() as u32).clamp(1, h);
            let crop = ((w - crop_w) / 2, (h - crop_h) / 2, crop_w, crop_h);
            let dst_w = ((f64::from(crop_w) * eff).round() as u32).clamp(1, BOX_PX.0);
            let dst_h = ((f64::from(crop_h) * eff).round() as u32).clamp(1, BOX_PX.1);
            let (mut rgba_app, mut rgb_app) = (fullscreen_app(), fullscreen_app());
            let times = medians(
                || {
                    // Before: a crop copy, an RGBA copy of it, the resize with the alpha
                    // multiply and the write with the scan and the RGB copy.
                    let start = Instant::now();
                    let copy = old_source
                        .crop_imm(crop.0, crop.1, crop_w, crop_h)
                        .to_rgba8();
                    let resized = rgba_resize(&copy, (0, 0, crop_w, crop_h), (dst_w, dst_h));
                    write(&mut rgba_app, ScreenImage::Rgba(resized));
                    start.elapsed()
                },
                || {
                    let start = Instant::now();
                    let crop = new_source.crop_resized(crop, (dst_w, dst_h));
                    write(&mut rgb_app, crop);
                    start.elapsed()
                },
            );
            report(
                &format!(
                    "{name} zoom {zoom} pan step {crop_w}x{crop_h} into {dst_w}x{dst_h}, crop and write"
                ),
                times,
            );
        }
    }

    /// The decoders alone: libjpeg-turbo and libheif at full size in both layouts.
    fn measure_decoders(jpeg: Option<&PathBuf>, heic: Option<&PathBuf>) {
        if let Some(path) = jpeg {
            let data = std::fs::read(path).unwrap();
            let decode = |format| time(|| turbojpeg::decompress(&data, format).unwrap());
            let times = medians(
                || decode(turbojpeg::PixelFormat::RGBA),
                || decode(turbojpeg::PixelFormat::RGB),
            );
            report("libjpeg-turbo full decode, RGBA before, RGB after", times);
        }
        #[cfg(feature = "heic")]
        if let Some(path) = heic {
            use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};
            let ctx = HeifContext::read_from_file(path.to_str().unwrap()).unwrap();
            let handle = ctx.primary_image_handle().unwrap();
            let decode = |chroma| {
                time(|| {
                    LibHeif::new()
                        .decode(&handle, ColorSpace::Rgb(chroma), None)
                        .unwrap()
                })
            };
            let times = medians(|| decode(RgbChroma::Rgba), || decode(RgbChroma::Rgb));
            report("libheif full decode, RGBA before, RGB after", times);
        }
        #[cfg(not(feature = "heic"))]
        let _ = heic;
    }

    #[test]
    #[ignore = "reads the folder VIEWR_TEST_DIR"]
    fn bench_the_layouts_against_the_rgba_layout() {
        let Some(dir) = std::env::var_os("VIEWR_TEST_DIR").map(PathBuf::from) else {
            eprintln!("skipped: VIEWR_TEST_DIR is not set");
            return;
        };
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        files.sort();
        let first = |extension: &str| {
            files.iter().find(|path| {
                path.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case(extension))
            })
        };
        for path in ["heic", "jpg", "png"].into_iter().filter_map(first) {
            measure(path);
        }
        measure_decoders(first("jpg"), first("heic"));
    }
}
