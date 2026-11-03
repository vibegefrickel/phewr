//! The embedded thumbnails of a HEIF file: the grid preview and the stand-in for the primary
//! decode.

use super::plane::heif_image_to_rgba;
use super::{LibheifSlot, open_heif_context, source_color, thumbnail_color};
use crate::decode;
use crate::decode::color;
use crate::decode::resize::{Layout, scaled_down_size};
use image::RgbaImage;
use std::io;
use std::path::Path;

/// The thumbnails with the `irot` and `imir` of the primary image. libheif applies the
/// properties of the thumbnail item. A file without a thumbnail reads no HEIF boxes.
fn matching_thumbnails(
    path: &Path,
    primary: &libheif_rs::ImageHandle,
) -> Vec<libheif_rs::ImageHandle> {
    let mut ids = vec![0; primary.number_of_thumbnails()];
    let count = primary.thumbnail_ids(&mut ids);
    ids.truncate(count);
    if ids.is_empty() {
        return Vec::new();
    }
    let Some(items) = decode::heif_boxes::HeifItems::read(path) else {
        return Vec::new();
    };
    ids.into_iter()
        .filter(|&id| items.thumbnail_matches_primary(id))
        .filter_map(|id| primary.thumbnail(id).ok())
        .collect()
}

/// The smallest thumbnail that covers the fitted primary image.
pub(super) fn usable_thumbnail(
    path: &Path,
    primary: &libheif_rs::ImageHandle,
    target: (u32, u32),
) -> Option<libheif_rs::ImageHandle> {
    let primary_size = (primary.width(), primary.height());
    matching_thumbnails(path, primary)
        .into_iter()
        .filter(|thumb| thumbnail_is_usable((thumb.width(), thumb.height()), primary_size, target))
        .min_by_key(|thumb| u64::from(thumb.width()) * u64::from(thumb.height()))
}

/// The largest thumbnail with the shape and transform of the primary image. It can be smaller
/// than the cell.
fn preview_thumbnail(
    path: &Path,
    primary: &libheif_rs::ImageHandle,
) -> Option<libheif_rs::ImageHandle> {
    let primary_size = (primary.width(), primary.height());
    matching_thumbnails(path, primary)
        .into_iter()
        .filter(|thumb| same_shape((thumb.width(), thumb.height()), primary_size))
        .max_by_key(|thumb| u64::from(thumb.width()) * u64::from(thumb.height()))
}

/// The embedded thumbnail at its own size, at most the fitted primary size, in the terminal
/// color space. The terminal scales it into the cell, and a cold cell sends a few kilobytes.
pub(crate) fn decode_heif_preview(path: &Path, target: (u32, u32)) -> io::Result<RgbaImage> {
    use libheif_rs::{ColorSpace, LibHeif, RgbChroma};

    let invalid = |e: libheif_rs::HeifError| io::Error::new(io::ErrorKind::InvalidData, e);
    let mut ctx = open_heif_context(path)?;
    let _slot = LibheifSlot::take(&mut ctx);
    let primary = ctx.primary_image_handle().map_err(invalid)?;
    let thumbnail = preview_thumbnail(path, &primary).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no embedded thumbnail with the transform of the primary image",
        )
    })?;
    let sharp_size = fitted_size((primary.width(), primary.height()), target)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty cell"))?;
    let color = thumbnail_color(&thumbnail, source_color(&primary));
    let image = LibHeif::new()
        .decode(&thumbnail, ColorSpace::Rgb(RgbChroma::Rgba), None)
        .map_err(invalid)?;
    let layout = if thumbnail.has_alpha_channel() {
        Layout::Rgba
    } else {
        Layout::OpaqueRgba
    };
    let preview = heif_image_to_rgba(&image, Some(sharp_size), layout)?;
    Ok(color::convert(preview, &color, color::target()))
}

/// A usable thumbnail covers the fitted primary image and has its portrait or landscape shape.
/// `usable_thumbnail` compares `irot` and `imir` first. The shape check is a second guard.
pub(super) fn thumbnail_is_usable(
    thumbnail: (u32, u32),
    primary: (u32, u32),
    target: (u32, u32),
) -> bool {
    let Some(fitted) = fitted_size(primary, target) else {
        return false;
    };
    let covers = thumbnail.0 >= fitted.0 && thumbnail.1 >= fitted.1;
    covers && same_shape(thumbnail, primary)
}

fn same_shape(thumbnail: (u32, u32), primary: (u32, u32)) -> bool {
    (thumbnail.0 >= thumbnail.1) == (primary.0 >= primary.1)
}

/// `scaled_down_size` without the `None` for an image that fits. `None` for an empty side.
pub(super) fn fitted_size(size: (u32, u32), target: (u32, u32)) -> Option<(u32, u32)> {
    if size.0 == 0 || size.1 == 0 || target.0 == 0 || target.1 == 0 {
        return None;
    }
    Some(scaled_down_size(size, target).unwrap_or(size))
}
