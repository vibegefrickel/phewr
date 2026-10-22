//! The resize of a decoded image into its box, with Catmull-Rom.

use super::{EmbeddedThumbnail, decode_image};
use fast_image_resize as fir;
use image::{DynamicImage, RgbaImage};
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

/// Fit `img` into the box. The decodes fit through `Decoded::fit_within`, which also converts
/// the colors.
#[cfg(test)]
pub(crate) fn resize_decoded_to_dims(img: &DynamicImage, max_w: u32, max_h: u32) -> RgbaImage {
    fit_rgba_within(img.to_rgba8(), max_w, max_h)
}

/// Scale `rgba` down into `max_w` x `max_h` with its aspect ratio, and never up. An owned image
/// needs no second copy.
pub(crate) fn fit_rgba_within(rgba: RgbaImage, max_w: u32, max_h: u32) -> RgbaImage {
    let size = rgba.dimensions();
    scaled_down_size(size, (max_w, max_h))
        .and_then(|dst| resize_rgba(rgba.as_raw(), size, dst))
        .unwrap_or(rgba)
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

/// Resize the packed RGBA `pixels` of `size` to `dst` with `fast_image_resize`. The pixels stay
/// borrowed, so a caller can pass a decoder buffer without a copy. `None` when the resize fails.
pub(super) fn resize_rgba(pixels: &[u8], size: (u32, u32), dst: (u32, u32)) -> Option<RgbaImage> {
    let src_image =
        fir::images::ImageRef::new(size.0, size.1, pixels, fir::PixelType::U8x4).ok()?;
    let mut dst_image = fir::images::Image::new(dst.0, dst.1, fir::PixelType::U8x4);
    fir::Resizer::new()
        .resize(&src_image, &mut dst_image, &resize_options())
        .ok()?;
    RgbaImage::from_raw(dst.0, dst.1, dst_image.into_vec())
}

/// Resize `img` to exactly `dst_w` x `dst_h` (used for zoomed crops, which may upscale).
pub(crate) fn resize_to_exact(img: &DynamicImage, dst_w: u32, dst_h: u32) -> RgbaImage {
    let (ow, oh) = (img.width(), img.height());
    if dst_w == 0 || dst_h == 0 || ow == 0 || oh == 0 {
        return img.to_rgba8();
    }
    if dst_w == ow && dst_h == oh {
        return img.to_rgba8();
    }
    let src_rgba = img.to_rgba8();
    let Ok(src_image) =
        fir::images::Image::from_vec_u8(ow, oh, src_rgba.into_raw(), fir::PixelType::U8x4)
    else {
        return img.to_rgba8();
    };
    let mut dst_image = fir::images::Image::new(dst_w, dst_h, fir::PixelType::U8x4);
    let mut resizer = fir::Resizer::new();
    if resizer
        .resize(&src_image, &mut dst_image, &resize_options())
        .is_err()
    {
        return img.to_rgba8();
    }
    RgbaImage::from_raw(dst_w, dst_h, dst_image.into_vec()).unwrap_or_else(|| img.to_rgba8())
}

#[cfg(test)]
mod tests {
    use super::{load_and_resize_px, resize_decoded_to_dims, resize_to_exact};
    use crate::decode::EmbeddedThumbnail;
    use crate::test_support::test_dir;
    use image::{DynamicImage, RgbaImage};

    #[test]
    fn resize_preserves_aspect_ratio_without_upscaling() {
        let image = DynamicImage::new_rgba8(100, 50);
        assert_eq!(
            resize_decoded_to_dims(&image, 20, 20).dimensions(),
            (20, 10)
        );
        assert_eq!(
            resize_decoded_to_dims(&image, 200, 200).dimensions(),
            (100, 50)
        );
    }

    #[test]
    fn resize_to_exact_upscales_to_requested_dimensions() {
        let image = DynamicImage::new_rgba8(10, 10);
        // Zoomed crops may upscale, unlike the fit-to-window path.
        assert_eq!(resize_to_exact(&image, 40, 30).dimensions(), (40, 30));
        assert_eq!(resize_to_exact(&image, 10, 10).dimensions(), (10, 10));
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
        assert_eq!(resize_decoded_to_dims(&image, 4, 4).dimensions(), (10, 0));
    }
}
