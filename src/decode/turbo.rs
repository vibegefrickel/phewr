//! The libjpeg-turbo decode with its DCT scaling.

use crate::decode::Decoded;
use crate::decode::color::jpeg_color;
use crate::decode::resize::{rgb_resize_is_fastest, scaled_down_size};
use crate::info;
use crate::scanner;
use image::metadata::Orientation;
use image::{DynamicImage, RgbImage, RgbaImage};
use std::io;
use std::path::Path;

pub(super) fn is_jpeg(path: &Path) -> bool {
    scanner::has_extension(path, &["jpg", "jpeg"])
}

fn pick_scaling_factor(
    orig_w: usize,
    orig_h: usize,
    target_w: u32,
    target_h: u32,
) -> turbojpeg::ScalingFactor {
    let candidates = [
        turbojpeg::ScalingFactor::ONE_EIGHTH,
        turbojpeg::ScalingFactor::ONE_QUARTER,
        turbojpeg::ScalingFactor::ONE_HALF,
        turbojpeg::ScalingFactor::ONE,
    ];
    let tw = target_w as usize;
    let th = target_h as usize;
    for &sf in &candidates {
        let sw = sf.scale(orig_w);
        let sh = sf.scale(orig_h);
        if sw >= tw && sh >= th {
            return sf;
        }
    }
    turbojpeg::ScalingFactor::ONE
}

pub(super) fn decode_jpeg_turbo(path: &Path, target: Option<(u32, u32)>) -> io::Result<Decoded> {
    let data = std::fs::read(path)?;
    decode_jpeg_turbo_bytes(&data, target, None)
}

/// Decode the JPEG `data` with libjpeg-turbo. The DCT scaling picks the smallest size that
/// still covers `target`. `container_orientation` is the EXIF orientation of the file that
/// holds the JPEG (a RAW file). It wins over the EXIF orientation of the JPEG itself. The color
/// is the ICC profile of the APP2 segments.
pub(super) fn decode_jpeg_turbo_bytes(
    data: &[u8],
    target: Option<(u32, u32)>,
    container_orientation: Option<u8>,
) -> io::Result<Decoded> {
    let mut decompressor = turbojpeg::Decompressor::new()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let header = decompressor
        .read_header(data)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    // libjpeg-turbo ignores the EXIF orientation, so phone photos need it here. The
    // container orientation of a RAW file wins over the EXIF of its preview.
    let orientation = container_orientation
        .or_else(|| info::orientation_from_bytes(data))
        .and_then(Orientation::from_exif);
    let target = target.map(|box_px| turned_target(box_px, orientation));
    let scaling = match target {
        Some((tw, th)) if tw > 0 && th > 0 && !header.is_lossless => {
            pick_scaling_factor(header.width, header.height, tw, th)
        }
        _ => turbojpeg::ScalingFactor::ONE,
    };

    if scaling != turbojpeg::ScalingFactor::ONE {
        decompressor
            .set_scaling_factor(scaling)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    }

    // A JPEG has no alpha channel. libjpeg-turbo writes RGB or RGBA at the same speed, so the
    // layout is the one of the fastest resize after: RGB without a resize or for a downscale
    // to 1/2 or less, else RGBA, which resizes without the alpha multiply.
    let scaled = header.scaled(scaling);
    let size = (scaled.width as u32, scaled.height as u32);
    let rgb =
        target.is_some_and(|box_px| rgb_resize_is_fastest(size, scaled_down_size(size, box_px)));
    let mut img = decompress(&mut decompressor, data, scaled, rgb)?;
    if let Some(orientation) = orientation {
        img.apply_orientation(orientation);
    }
    Ok(Decoded {
        image: img,
        color: jpeg_color(data),
        opaque: true,
    })
}

/// Decompress `data` at the `scaled` size into RGB or RGBA pixels.
fn decompress(
    decompressor: &mut turbojpeg::Decompressor,
    data: &[u8],
    scaled: turbojpeg::DecompressHeader,
    rgb: bool,
) -> io::Result<DynamicImage> {
    let format = if rgb {
        turbojpeg::PixelFormat::RGB
    } else {
        turbojpeg::PixelFormat::RGBA
    };
    let pitch = scaled.width * format.size();
    let mut image = turbojpeg::Image {
        pixels: vec![0u8; scaled.height * pitch],
        width: scaled.width,
        pitch,
        height: scaled.height,
        format,
    };
    decompressor
        .decompress(data, image.as_deref_mut())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let (w, h) = (image.width as u32, image.height as u32);
    let image = if rgb {
        RgbImage::from_raw(w, h, image.pixels).map(DynamicImage::ImageRgb8)
    } else {
        RgbaImage::from_raw(w, h, image.pixels).map(DynamicImage::ImageRgba8)
    };
    image.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid image dimensions"))
}

/// The box in the stored orientation: a quarter turn swaps its sides, so the DCT scaling keeps
/// enough pixels for the turned picture.
fn turned_target(target: (u32, u32), orientation: Option<Orientation>) -> (u32, u32) {
    match orientation {
        Some(
            Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH,
        ) => (target.1, target.0),
        _ => target,
    }
}
