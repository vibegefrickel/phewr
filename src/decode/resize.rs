//! The resize of a decoded image into its box, with Catmull-Rom.

use super::{EmbeddedThumbnail, decode_image};
use fast_image_resize as fir;
use image::{DynamicImage, EncodableLayout, ImageBuffer, RgbaImage};
use std::io;
use std::path::Path;

/// Decode `path` with the size hint `target` in pixels and fit the image into it, in the color
/// space of the terminal.
pub(crate) fn load_and_resize_px(
    path: &Path,
    (target_w, target_h): (u32, u32),
    embedded: EmbeddedThumbnail,
) -> io::Result<RgbaImage> {
    let hint = if target_w > 0 && target_h > 0 {
        Some((target_w, target_h))
    } else {
        None
    };
    Ok(decode_image(path, hint, embedded)?.fit_within(target_w, target_h))
}

/// Scale `rgba` down into `max_w` x `max_h` with its aspect ratio, and never up. An owned image
/// needs no second copy.
pub(crate) fn fit_rgba_within(rgba: RgbaImage, max_w: u32, max_h: u32) -> RgbaImage {
    fit_within_as(rgba, max_w, max_h, Layout::Rgba)
}

/// `fit_rgba_within` for an image whose alpha is 255 in every pixel: the resize skips the
/// alpha multiply.
pub(super) fn fit_opaque_rgba_within(rgba: RgbaImage, max_w: u32, max_h: u32) -> RgbaImage {
    fit_within_as(rgba, max_w, max_h, Layout::OpaqueRgba)
}

/// The fit of both RGBA layouts, so the alpha multiply is the only difference.
fn fit_within_as(rgba: RgbaImage, max_w: u32, max_h: u32, layout: Layout) -> RgbaImage {
    let size = rgba.dimensions();
    scaled_down_size(size, (max_w, max_h))
        .and_then(|dst| resize_region(rgba.as_raw(), size, (0, 0, size.0, size.1), dst, layout))
        .and_then(|pixels| RgbaImage::from_raw(pixels.width, pixels.height, pixels.data))
        .unwrap_or(rgba)
}

/// Scale a 16-bit RGB or RGBA image down into `max_w` x `max_h` as `fit_rgba_within` does, for
/// an HDR image before its tone map. Each other image comes back unchanged.
pub(super) fn fit_deep_within(image: DynamicImage, max_w: u32, max_h: u32) -> DynamicImage {
    let size = (image.width(), image.height());
    let Some(dst) = scaled_down_size(size, (max_w, max_h)) else {
        return image;
    };
    let whole = (0, 0, size.0, size.1);
    let resized = match &image {
        DynamicImage::ImageRgb16(rgb) => {
            resize_region(rgb.as_raw().as_bytes(), size, whole, dst, Layout::Rgb16)
                .and_then(|pixels| {
                    ImageBuffer::from_raw(dst.0, dst.1, native_samples(&pixels.data))
                })
                .map(DynamicImage::ImageRgb16)
        }
        DynamicImage::ImageRgba16(rgba) => {
            resize_region(rgba.as_raw().as_bytes(), size, whole, dst, Layout::Rgba16)
                .and_then(|pixels| {
                    ImageBuffer::from_raw(dst.0, dst.1, native_samples(&pixels.data))
                })
                .map(DynamicImage::ImageRgba16)
        }
        _ => None,
    };
    resized.unwrap_or(image)
}

/// The 16-bit samples of the bytes of a resize, in the byte order of the machine, as
/// `fast_image_resize` writes them.
fn native_samples(bytes: &[u8]) -> Vec<u16> {
    let (pairs, _) = bytes.as_chunks::<2>();
    pairs.iter().map(|&pair| u16::from_ne_bytes(pair)).collect()
}

/// The size of `size` scaled down into `max` with its aspect ratio, rounded down. `None` when
/// no scale is necessary: an empty size or box, or an image that fits already.
pub(crate) fn scaled_down_size(size: (u32, u32), max: (u32, u32)) -> Option<(u32, u32)> {
    let ((orig_w, orig_h), (max_w, max_h)) = (size, max);
    if max_w == 0 || max_h == 0 || orig_w == 0 || orig_h == 0 {
        return None;
    }
    let scale = f64::min(
        f64::from(max_w) / f64::from(orig_w),
        f64::from(max_h) / f64::from(orig_h),
    );
    if scale >= 1.0 {
        return None;
    }
    let dst_w = ((f64::from(orig_w) * scale) as u32).max(1);
    let dst_h = ((f64::from(orig_h) * scale) as u32).max(1);
    Some((dst_w, dst_h))
}

/// The filter of every resize: thumbnails, previews, fullscreen and zoom. Catmull-Rom costs
/// two thirds of Lanczos3 on a 24 MP frame and keeps the edges sharp.
fn resize_options() -> fir::ResizeOptions {
    fir::ResizeOptions::new().resize_alg(fir::ResizeAlg::Convolution(fir::FilterType::CatmullRom))
}

/// The pixel layout of a resize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout {
    /// 3 bytes per pixel.
    Rgb,
    /// 4 bytes per pixel with transparent pixels: the resize multiplies the colors with the
    /// alpha before and divides after, so a transparent pixel does not darken its neighbors.
    Rgba,
    /// 4 bytes per pixel with the alpha at 255 in every pixel. The multiply changes no value,
    /// so the resize skips it.
    OpaqueRgba,
    /// 3 samples of 16 bits per pixel, in the byte order of the machine: an HDR image before
    /// its tone map.
    Rgb16,
    /// 4 samples of 16 bits per pixel with the alpha multiply: an HDR image with alpha.
    Rgba16,
}

impl Layout {
    /// The pixel type of `fast_image_resize` for the layout.
    pub(super) fn pixel_type(self) -> fir::PixelType {
        match self {
            Self::Rgb => fir::PixelType::U8x3,
            Self::Rgba | Self::OpaqueRgba => fir::PixelType::U8x4,
            Self::Rgb16 => fir::PixelType::U16x3,
            Self::Rgba16 => fir::PixelType::U16x4,
        }
    }
}

/// True when RGB is the fastest layout for a resize of `size` to `dst`, or for no resize
/// (`None`).
///
/// Measured 2026-10-30 into a 3392x2052 box: RGBA without the alpha multiply is the fastest
/// layout at every scale (17.6 ms at 0.75, 9.3 ms at 1.5), and RGB is 35.2 ms and 12.0 ms. Only
/// from a scale of 1/2 down is RGB as fast, and then it needs no copy into RGB after.
pub(crate) fn rgb_resize_is_fastest(size: (u32, u32), dst: Option<(u32, u32)>) -> bool {
    dst.is_none_or(|(w, h)| {
        2 * u64::from(w) <= u64::from(size.0) && 2 * u64::from(h) <= u64::from(size.1)
    })
}

/// The packed pixels of a resize and their size.
pub(super) struct Resized {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) data: Vec<u8>,
}

/// Resize the region `crop` (x, y, width, height) of the packed `pixels` of `size` to `dst`
/// with `fast_image_resize`. The pixels stay borrowed, so a caller can pass a decoder buffer
/// without a copy. A region smaller than the image is a view: the resize reads only its
/// pixels, as from a copy, and the zoom pays no crop copy. `None` when the resize fails.
pub(super) fn resize_region(
    pixels: &[u8],
    size: (u32, u32),
    crop: (u32, u32, u32, u32),
    dst: (u32, u32),
    layout: Layout,
) -> Option<Resized> {
    let pixel_type = layout.pixel_type();
    let source = fir::images::ImageRef::new(size.0, size.1, pixels, pixel_type).ok()?;
    let mut target = fir::images::Image::new(dst.0, dst.1, pixel_type);
    let options = resize_options().use_alpha(matches!(layout, Layout::Rgba | Layout::Rgba16));
    let mut resizer = fir::Resizer::new();
    let (x, y, width, height) = crop;
    if (x, y, width, height) == (0, 0, size.0, size.1) {
        resizer.resize(&source, &mut target, &options).ok()?;
    } else {
        let view = fir::images::CroppedImage::new(&source, x, y, width, height).ok()?;
        resizer.resize(&view, &mut target, &options).ok()?;
    }
    Some(Resized {
        width: dst.0,
        height: dst.1,
        data: target.into_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::{fit_rgba_within, load_and_resize_px};
    use crate::decode::EmbeddedThumbnail;
    use crate::test_support::test_dir;
    use image::{DynamicImage, RgbaImage};

    /// Fit `img` into the box without the color conversion of `Decoded::fit_within`.
    fn fit_decoded_within(img: &DynamicImage, max_w: u32, max_h: u32) -> RgbaImage {
        fit_rgba_within(img.to_rgba8(), max_w, max_h)
    }

    #[test]
    fn resize_preserves_aspect_ratio_without_upscaling() {
        let image = DynamicImage::new_rgba8(100, 50);
        assert_eq!(fit_decoded_within(&image, 20, 20).dimensions(), (20, 10));
        assert_eq!(fit_decoded_within(&image, 200, 200).dimensions(), (100, 50));
    }

    #[test]
    fn load_and_resize_fits_the_box() {
        let dir = test_dir();
        let path = dir.path().join("wide.png");
        RgbaImage::new(64, 32).save(&path).unwrap();
        let fitted = load_and_resize_px(&path, (16, 16), EmbeddedThumbnail::Deny).unwrap();
        assert_eq!(fitted.dimensions(), (16, 8));
        let full = load_and_resize_px(&path, (0, 0), EmbeddedThumbnail::Deny).unwrap();
        assert_eq!(full.dimensions(), (64, 32));
        assert!(
            load_and_resize_px(
                &dir.path().join("missing.png"),
                (8, 8),
                EmbeddedThumbnail::Deny
            )
            .is_err()
        );
        let image = DynamicImage::new_rgba8(10, 0);
        assert_eq!(fit_decoded_within(&image, 4, 4).dimensions(), (10, 0));
    }
}
