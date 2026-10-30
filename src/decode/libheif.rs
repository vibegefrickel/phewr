//! The libheif route: HEIC, HEIF and AVIF, the embedded thumbnails and the core share.

use super::resize::{Layout, resize_region, rgb_resize_is_fastest, scaled_down_size};
use super::{Decoded, EmbeddedThumbnail, FOREGROUND};
use crate::decode;
use crate::decode::color::{self, SourceColor};
use crate::scanner;
use image::{DynamicImage, RgbImage, RgbaImage};
use std::io;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::thread;

/// Send HEIC, HEIF and AVIF to libheif, because the `image` crate decodes none of them.
pub(crate) fn decodes_with_libheif(path: &Path) -> bool {
    scanner::has_extension(path, scanner::LIBHEIF_EXTENSIONS)
}

pub(crate) fn libheif_dimensions(path: &Path) -> Option<(u32, u32)> {
    let ctx = open_heif_context(path).ok()?;
    let handle = ctx.primary_image_handle().ok()?;
    Some((handle.width(), handle.height()))
}

/// Open `path` for libheif through a buffered stream. `HeifContext::read_from_file` reads and
/// copies the whole file first, 30 ms to 40 ms for a 24 MP iPhone photo. The stream lets libheif
/// read the boxes and the one item that it decodes.
fn open_heif_context(path: &Path) -> io::Result<libheif_rs::HeifContext<'static>> {
    let file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let reader =
        libheif_rs::StreamReader::new(io::BufReader::with_capacity(HEIF_READ_BUFFER, file), size);
    libheif_rs::HeifContext::read_from_reader(Box::new(reader))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// The read buffer of a libheif stream. The image data of a 24 MP photo is a few MB.
const HEIF_READ_BUFFER: usize = 1 << 20;

pub(super) fn decode_with_libheif(
    path: &Path,
    target: Option<(u32, u32)>,
    embedded: EmbeddedThumbnail,
) -> io::Result<Decoded> {
    use libheif_rs::{ColorSpace, LibHeif, RgbChroma};

    let invalid = |e: libheif_rs::HeifError| io::Error::new(io::ErrorKind::InvalidData, e);
    let mut ctx = open_heif_context(path)?;
    let _slot = LibheifSlot::take(&mut ctx);
    let primary = ctx.primary_image_handle().map_err(invalid)?;
    let primary_color = source_color(&primary);
    // An embedded thumbnail (iPhone: 320x240) saves the decode of the full frame.
    let handle = target
        .filter(|_| embedded == EmbeddedThumbnail::Allow)
        .and_then(|target| usable_thumbnail(path, &primary, target))
        .unwrap_or(primary);
    let color = thumbnail_color(&handle, primary_color);
    let layout = decode_layout(&handle, target);
    let chroma = if layout == Layout::Rgb {
        RgbChroma::Rgb
    } else {
        RgbChroma::Rgba
    };
    let image = LibHeif::new()
        .decode(&handle, ColorSpace::Rgb(chroma), None)
        .map_err(invalid)?;
    let image = if layout == Layout::Rgb {
        heif_image_to_rgb(&image, target).map(DynamicImage::ImageRgb8)?
    } else {
        heif_image_to_rgba(&image, target, layout).map(DynamicImage::ImageRgba8)?
    };
    Ok(Decoded {
        image,
        color,
        opaque: layout != Layout::Rgba,
    })
}

/// The layout of the decode of `handle` for `target`. libheif writes the same values in RGB
/// and in RGBA, and RGB about 5 % faster, so an image without alpha decodes as RGB when RGB
/// is the fastest layout of the resize after (`rgb_resize_is_fastest`): the grid and a 24 MP
/// photo in the fullscreen box. Else it decodes as RGBA, which resizes without the alpha
/// multiply. The zoom source (no target) is RGBA too, because its crops resize at a mild scale.
fn decode_layout(handle: &libheif_rs::ImageHandle, target: Option<(u32, u32)>) -> Layout {
    if handle.has_alpha_channel() {
        return Layout::Rgba;
    }
    let size = (handle.width(), handle.height());
    if target.is_some_and(|box_px| rgb_resize_is_fastest(size, scaled_down_size(size, box_px))) {
        Layout::Rgb
    } else {
        Layout::OpaqueRgba
    }
}

/// The color space of a thumbnail. A thumbnail without a color profile of its own shows the
/// colors of the primary image.
fn thumbnail_color(thumbnail: &libheif_rs::ImageHandle, primary: SourceColor) -> SourceColor {
    match source_color(thumbnail) {
        SourceColor::Srgb => primary,
        color => color,
    }
}

/// The color space of the RGB values that libheif decodes from `handle`: the ICC profile
/// (`prof` or `rICC`) first, then the CICP code points of the `nclx` box. libheif converts
/// YCbCr into RGB with the matrix of the `nclx` box, but it does not convert colors.
fn source_color(handle: &libheif_rs::ImageHandle) -> SourceColor {
    if let Some(icc) = handle.color_profile_raw() {
        return SourceColor::icc(icc.data);
    }
    handle
        .color_profile_nclx()
        .map_or(SourceColor::Srgb, |nclx| SourceColor::Cicp {
            primaries: nclx.color_primaries() as u8,
            transfer: nclx.transfer_characteristics() as u8,
        })
}

static LIBHEIF_DECODES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// One running libheif decode. It gives the context its share of the cores on `take`, and it
/// returns the share on drop, also after an error.
struct LibheifSlot;

impl LibheifSlot {
    fn take(ctx: &mut libheif_rs::HeifContext<'_>) -> Self {
        let running = LIBHEIF_DECODES.fetch_add(1, Ordering::Relaxed);
        let cores = thread::available_parallelism().map_or(4, NonZeroUsize::get);
        let share = if FOREGROUND.get() { 0 } else { running };
        ctx.set_max_decoding_threads(libheif_tile_threads(cores, share));
        Self
    }
}

impl Drop for LibheifSlot {
    fn drop(&mut self) {
        LIBHEIF_DECODES.fetch_sub(1, Ordering::Relaxed);
    }
}

/// The tile threads of one libheif decode: all cores for a single decode and for a foreground
/// decode, a share when the grid decodes several files at once. With the default of 4 threads,
/// 10 grid decodes ran 40 threads on 10 cores, and each decode took twice as long.
fn libheif_tile_threads(cores: usize, running: usize) -> u32 {
    u32::try_from(cores / (running + 1)).unwrap_or(1).max(1)
}

/// The embedded thumbnails of the primary image with its `irot` and `imir` properties. libheif
/// applies the properties of the thumbnail item, so a thumbnail with other properties would
/// show another orientation. A file without a thumbnail does not read the HEIF boxes.
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

/// The smallest embedded thumbnail that covers the fitted primary image, or `None` when the
/// grid must decode the primary image.
fn usable_thumbnail(
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

/// The largest embedded thumbnail with the shape and the transform of the primary image. It
/// may be smaller than the cell, because the preview is only a stand-in for the sharp decode.
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

/// Decode the embedded thumbnail of `path` at its own resolution, at most the size that the
/// primary image gets in `target`. The grid pads it to the shape of the cell and lets the
/// terminal scale it, so a cold cell sends a few kilobytes instead of a full-size image. The
/// preview is in the color space of the terminal.
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

/// Accept a thumbnail only if it is at least as large as the primary image fitted into
/// `target`, so that the grid never upscales it. It must also have the portrait or
/// landscape shape of the primary image.
///
/// libheif applies the irot and imir properties of the thumbnail item. `usable_thumbnail`
/// compares these properties with the primary image first (`HeifItems`). The shape check
/// stays as a second guard.
fn thumbnail_is_usable(thumbnail: (u32, u32), primary: (u32, u32), target: (u32, u32)) -> bool {
    let Some(fitted) = fitted_size(primary, target) else {
        return false;
    };
    let covers = thumbnail.0 >= fitted.0 && thumbnail.1 >= fitted.1;
    covers && same_shape(thumbnail, primary)
}

/// True when both sizes are landscape, or both are portrait or square.
fn same_shape(thumbnail: (u32, u32), primary: (u32, u32)) -> bool {
    (thumbnail.0 >= thumbnail.1) == (primary.0 >= primary.1)
}

/// The size of `size` fitted into `target` without upscale, rounded down as `scaled_down_size`
/// does. `None` for an empty size or target.
fn fitted_size(size: (u32, u32), target: (u32, u32)) -> Option<(u32, u32)> {
    if size.0 == 0 || size.1 == 0 || target.0 == 0 || target.1 == 0 {
        return None;
    }
    Some(scaled_down_size(size, target).unwrap_or(size))
}

/// Turn the interleaved RGBA plane of a libheif image into an image that fits `target`.
///
/// libheif decodes the selected image (primary image or thumbnail) at its full size. Its
/// `scale` is nearest neighbour, so `fast_image_resize` scales, as for the other formats. A plane
/// without row padding goes to the resize directly, so a full-size frame is not copied. A plane
/// with padding is copied row by row first.
fn heif_image_to_rgba(
    image: &libheif_rs::Image,
    target: Option<(u32, u32)>,
    layout: Layout,
) -> io::Result<RgbaImage> {
    let ((w, h), pixels) = heif_plane_pixels(image, target, layout)?;
    RgbaImage::from_raw(w, h, pixels).ok_or_else(invalid_dimensions)
}

/// The RGB twin of `heif_image_to_rgba`, for an image without an alpha channel.
fn heif_image_to_rgb(
    image: &libheif_rs::Image,
    target: Option<(u32, u32)>,
) -> io::Result<RgbImage> {
    let ((w, h), pixels) = heif_plane_pixels(image, target, Layout::Rgb)?;
    RgbImage::from_raw(w, h, pixels).ok_or_else(invalid_dimensions)
}

/// The error of a plane whose pixels do not match its size.
fn invalid_dimensions() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid libheif image dimensions",
    )
}

/// The packed pixels of the interleaved plane of `image` in `layout`, fitted into `target`,
/// and their size.
fn heif_plane_pixels(
    image: &libheif_rs::Image,
    target: Option<(u32, u32)>,
    layout: Layout,
) -> io::Result<((u32, u32), Vec<u8>)> {
    let planes = image.planes();
    let plane = planes.interleaved.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "libheif decode produced no interleaved RGB plane",
        )
    })?;
    let (w, h) = (plane.width, plane.height);
    let row_bytes = w as usize * layout.pixel_type().size();
    let fitted = target.and_then(|max| scaled_down_size((w, h), max));
    let resize = |pixels: &[u8], dst| {
        resize_region(pixels, (w, h), (0, 0, w, h), dst, layout).map(|resized| resized.data)
    };
    if let Some(dst) = fitted
        && plane.stride == row_bytes
        && let Some(resized) = plane
            .data
            .get(..row_bytes * h as usize)
            .and_then(|pixels| resize(pixels, dst))
    {
        return Ok((dst, resized));
    }

    let mut pixels = Vec::with_capacity(row_bytes * h as usize);
    for y in 0..h as usize {
        let start = y * plane.stride;
        pixels.extend_from_slice(&plane.data[start..start + row_bytes]);
    }
    Ok(fitted
        .and_then(|dst| resize(&pixels, dst).map(|resized| (dst, resized)))
        .unwrap_or(((w, h), pixels)))
}

#[cfg(test)]
mod tests {
    use super::{
        decodes_with_libheif, fitted_size, libheif_dimensions, thumbnail_is_usable,
        usable_thumbnail,
    };
    use crate::decode::ScreenImage;
    use crate::decode::heif_boxes::HeifItems;
    use crate::decode::resize::fit_rgba_within;
    use crate::decode::{EmbeddedThumbnail, decode_image_with_hint};
    use crate::test_support::{BLUE, RED, assert_close, solid_image, test_dir, write_heif};
    use image::GenericImageView;
    use libheif_rs::{
        ColorSpace, CompressionFormat, EncoderQuality, HeifContext, ImageOrientation, LibHeif,
        RgbChroma,
    };

    use std::path::Path;

    /// Write a red AVIF of `width` x `height` without a thumbnail.
    fn write_red_avif(path: &Path, width: u32, height: u32) {
        let lib_heif = LibHeif::new();
        let mut ctx = HeifContext::new().unwrap();
        let mut encoder = lib_heif.encoder_for_format(CompressionFormat::Av1).unwrap();
        encoder.set_quality(EncoderQuality::Lossy(90)).unwrap();
        ctx.encode_image(&solid_image(width, height, RED), &mut encoder, None)
            .unwrap();
        ctx.write_to_file(path.to_str().unwrap()).unwrap();
    }

    /// True when the decoded RGBA plane of `path` has padding at the end of each row.
    fn rows_have_padding(path: &Path) -> bool {
        let ctx = HeifContext::read_from_file(path.to_str().unwrap()).unwrap();
        let handle = ctx.primary_image_handle().unwrap();
        let image = LibHeif::new()
            .decode(&handle, ColorSpace::Rgb(RgbChroma::Rgba), None)
            .unwrap();
        let plane = image.planes().interleaved.unwrap();
        plane.stride != plane.width as usize * 4
    }

    fn center(image: &image::DynamicImage) -> [u8; 4] {
        image
            .to_rgba8()
            .get_pixel(image.width() / 2, image.height() / 2)
            .0
    }

    #[test]
    fn tile_threads_share_the_cores_between_the_running_decodes() {
        use super::libheif_tile_threads;
        assert_eq!(libheif_tile_threads(10, 0), 10);
        assert_eq!(libheif_tile_threads(10, 1), 5);
        assert_eq!(libheif_tile_threads(10, 3), 2);
        assert_eq!(libheif_tile_threads(10, 20), 1);
        assert_eq!(libheif_tile_threads(1, 0), 1);
    }

    #[test]
    fn libheif_routing_covers_heic_and_avif() {
        assert!(decodes_with_libheif(Path::new("IMG_0001.HEIC")));
        assert!(decodes_with_libheif(Path::new("photo.hif")));
        assert!(decodes_with_libheif(Path::new("photo.AVIF")));
        assert!(!decodes_with_libheif(Path::new("photo.webp")));
        assert!(!decodes_with_libheif(Path::new("avif")));
    }

    #[test]
    fn thumbnail_must_cover_the_fitted_primary_and_keep_the_shape() {
        // iPhone: 320x240 thumbnail of a 4032x3024 photo. The default grid cell on a
        // Retina display is 18x6 cells of 17x37 px: the photo fits as 296x222.
        assert!(thumbnail_is_usable((320, 240), (4032, 3024), (306, 222)));
        // A wide box: the photo fits as 320x240, so the thumbnail is enough.
        assert!(thumbnail_is_usable((320, 240), (4032, 3024), (400, 240)));
        // The photo fits as 391x293: the thumbnail is too small.
        assert!(!thumbnail_is_usable((320, 240), (4032, 3024), (391, 296)));
        // Portrait photo, rotated thumbnail, tall box: the photo fits as 200x266.
        assert!(thumbnail_is_usable((240, 320), (3024, 4032), (200, 400)));
        // The thumbnail misses the rotation of the primary image.
        assert!(!thumbnail_is_usable((320, 240), (3024, 4032), (200, 200)));
        // A zero target never uses a thumbnail.
        assert!(!thumbnail_is_usable((320, 240), (4032, 3024), (0, 100)));
    }

    #[test]
    fn fitted_size_does_not_upscale() {
        assert_eq!(fitted_size((4032, 3024), (306, 222)), Some((296, 222)));
        assert_eq!(fitted_size((100, 50), (400, 400)), Some((100, 50)));
        assert_eq!(fitted_size((100, 50), (0, 400)), None);
        assert_eq!(fitted_size((0, 50), (400, 400)), None);
    }

    #[test]
    fn avif_opens_through_libheif() {
        let dir = test_dir();
        let path = dir.path().join("red.avif");
        write_heif(
            &path,
            CompressionFormat::Av1,
            ImageOrientation::Normal,
            None,
        );
        let full = decode_image_with_hint(&path, None, EmbeddedThumbnail::Deny).unwrap();
        let thumb = decode_image_with_hint(&path, Some((64, 64)), EmbeddedThumbnail::Deny).unwrap();
        let dims = libheif_dimensions(&path);

        assert_eq!(full.dimensions(), (256, 128));
        assert_close(center(&full), RED);
        assert_eq!(thumb.dimensions(), (64, 32));
        assert_eq!(dims, Some((256, 128)));
    }

    #[test]
    fn libheif_output_has_the_fitted_size_of_the_other_decoders() {
        let dir = test_dir();
        let path = dir.path().join("red.avif");
        write_heif(
            &path,
            CompressionFormat::Av1,
            ImageOrientation::Normal,
            None,
        );
        for target in [(64, 64), (77, 77), (100, 30), (500, 500)] {
            let decoded =
                decode_image_with_hint(&path, Some(target), EmbeddedThumbnail::Deny).unwrap();
            assert_eq!(
                Some(decoded.dimensions()),
                fitted_size((256, 128), target),
                "{target:?}"
            );
            // load_and_resize resizes again: the fitted decode must stay as it is.
            let resized = fit_rgba_within(decoded.to_rgba8(), target.0, target.1);
            assert_eq!(resized.dimensions(), decoded.dimensions(), "{target:?}");
            assert_close(center(&decoded), RED);
        }
        let sizes: Vec<_> = [(64, 64), (100, 30), (500, 500)]
            .into_iter()
            .map(|target| fitted_size((256, 128), target).unwrap())
            .collect();
        assert_eq!(sizes, [(64, 32), (60, 30), (256, 128)]);
    }

    #[test]
    fn libheif_scales_planes_with_and_without_row_padding() {
        let dir = test_dir();
        let mut padding = Vec::new();
        // libheif 1.23 adds 64 bytes to a row of a multiple of 512 bytes, so a 256 px row has
        // padding too. A 320 px row (1280 bytes) has none.
        for (width, height) in [(320, 128), (250, 126)] {
            let path = dir.path().join(format!("red-{width}.avif"));
            write_red_avif(&path, width, height);
            padding.push(rows_have_padding(&path));
            for target in [Some((50, 50)), None] {
                let decoded =
                    decode_image_with_hint(&path, target, EmbeddedThumbnail::Deny).unwrap();
                let expected = target.map_or(Some((width, height)), |target| {
                    fitted_size((width, height), target)
                });
                assert_eq!(Some(decoded.dimensions()), expected, "{width} {target:?}");
                assert_close(center(&decoded), RED);
            }
        }
        assert_eq!(padding, [false, true], "the test covers both plane layouts");
    }

    #[test]
    fn heic_grid_uses_a_large_enough_thumbnail_with_its_rotation() {
        let dir = test_dir();
        let path = dir.path().join("rotated.heic");
        let rotation = ImageOrientation::Rotate90Cw;
        write_heif(&path, CompressionFormat::Hevc, rotation, Some(rotation));
        let decode = |target, embedded| decode_image_with_hint(&path, target, embedded).unwrap();
        let full = decode(None, EmbeddedThumbnail::Allow);
        let small = decode(Some((32, 32)), EmbeddedThumbnail::Allow);
        let wide = decode(Some((64, 64)), EmbeddedThumbnail::Allow);
        let large = decode(Some((100, 100)), EmbeddedThumbnail::Allow);
        let fullscreen = decode(Some((32, 32)), EmbeddedThumbnail::Deny);

        assert_eq!(full.dimensions(), (128, 256));
        assert_close(center(&full), RED);
        // The 32x64 thumbnail covers the fitted 16x32: blue, and upright like the primary.
        assert_eq!(small.dimensions(), (16, 32));
        assert_close(center(&small), BLUE);
        // In a 64x64 box the primary image fits as 32x64: the thumbnail is just enough.
        assert_eq!(wide.dimensions(), (32, 64));
        assert_close(center(&wide), BLUE);
        // In a 100x100 box the primary image fits as 50x100: libheif decodes the primary.
        assert_eq!(large.dimensions(), (50, 100));
        assert_close(center(&large), RED);
        // The fullscreen view never uses the embedded thumbnail.
        assert_eq!(fullscreen.dimensions(), (16, 32));
        assert_close(center(&fullscreen), RED);
    }

    #[test]
    fn heic_thumbnail_without_the_rotation_is_not_used() {
        let dir = test_dir();
        let path = dir.path().join("rotated.heic");
        write_heif(
            &path,
            CompressionFormat::Hevc,
            ImageOrientation::Rotate90Cw,
            Some(ImageOrientation::Normal),
        );
        let small =
            decode_image_with_hint(&path, Some((32, 32)), EmbeddedThumbnail::Allow).unwrap();

        assert_eq!(small.dimensions(), (16, 32));
        assert_close(center(&small), RED);
    }

    /// The grid decode of a 64x64 box with the thumbnail and the primary image of `path`
    /// (the thumbnail is blue, the primary image red).
    fn grid_center(path: &Path) -> [u8; 4] {
        let small = decode_image_with_hint(path, Some((64, 64)), EmbeddedThumbnail::Allow).unwrap();
        center(&small)
    }

    #[test]
    fn heic_thumbnail_needs_the_half_turn_and_the_mirror_of_the_primary() {
        let dir = test_dir();
        let cases = [
            (ImageOrientation::Rotate180, ImageOrientation::Normal, RED),
            (
                ImageOrientation::Rotate180,
                ImageOrientation::Rotate180,
                BLUE,
            ),
            (
                ImageOrientation::FlipHorizontally,
                ImageOrientation::Normal,
                RED,
            ),
            (
                ImageOrientation::FlipVertically,
                ImageOrientation::FlipVertically,
                BLUE,
            ),
            (
                ImageOrientation::Normal,
                ImageOrientation::FlipVertically,
                RED,
            ),
        ];
        for (index, (primary, thumbnail, expected)) in cases.into_iter().enumerate() {
            let path = dir.path().join(format!("case-{index}.heic"));
            write_heif(&path, CompressionFormat::Hevc, primary, Some(thumbnail));
            // The shape guard alone accepts each of these thumbnails.
            assert_close(grid_center(&path), expected);
        }
    }

    #[test]
    fn heif_boxes_of_a_synthetic_file_list_its_thumbnail() {
        let dir = test_dir();
        let path = dir.path().join("rotated.heic");
        let rotation = ImageOrientation::Rotate90Cw;
        write_heif(&path, CompressionFormat::Hevc, rotation, Some(rotation));
        let ctx = HeifContext::read_from_file(path.to_str().unwrap()).unwrap();
        let primary = ctx.primary_image_handle().unwrap();
        let mut ids = vec![0; primary.number_of_thumbnails()];
        primary.thumbnail_ids(&mut ids);
        let items = HeifItems::read(&path).expect("the meta box");
        assert_eq!(ids.len(), 1);
        assert!(items.thumbnail_matches_primary(ids[0]));
        assert!(!items.thumbnail_matches_primary(primary.item_id()));
    }

    /// Decode a real AVIF file: `VIEWR_TEST_AVIF=/path/to/image.avif cargo test -- --ignored`.
    #[test]
    #[ignore = "needs VIEWR_TEST_AVIF, a real AVIF file"]
    fn decodes_avif_from_env() {
        let Some(path) = crate::decode::real_test_file("VIEWR_TEST_AVIF") else {
            return;
        };
        let path = path.as_path();
        let full =
            decode_image_with_hint(path, None, EmbeddedThumbnail::Deny).expect("full decode");
        assert_eq!(libheif_dimensions(path), Some(full.dimensions()));
        let thumb = decode_image_with_hint(path, Some((200, 200)), EmbeddedThumbnail::Deny)
            .expect("scaled decode");
        assert!(thumb.width() <= 200 && thumb.height() <= 200);
    }

    /// The grid decode of a real iPhone HEIC uses the embedded thumbnail and keeps the
    /// shape of the full image: `VIEWR_TEST_HEIC=/path/to/photo.heic`.
    #[test]
    #[ignore = "needs VIEWR_TEST_HEIC, a real photo"]
    fn heic_thumbnail_from_env() {
        let Some(path) = crate::decode::real_test_file("VIEWR_TEST_HEIC") else {
            return;
        };
        let path = path.as_path();
        let ctx = HeifContext::read_from_file(path.to_str().unwrap()).unwrap();
        let primary = ctx.primary_image_handle().unwrap();
        let thumbnail = usable_thumbnail(path, &primary, (160, 160))
            .expect("an embedded thumbnail of at least 160x160");
        let shape = |w: u32, h: u32| w >= h;
        assert_eq!(
            shape(thumbnail.width(), thumbnail.height()),
            shape(primary.width(), primary.height())
        );
        let thumb = decode_image_with_hint(path, Some((160, 160)), EmbeddedThumbnail::Allow)
            .expect("thumbnail decode");
        assert_eq!(
            shape(thumb.width(), thumb.height()),
            shape(primary.width(), primary.height())
        );
        // A shape check misses a 180 degree turn: compare the pixels with the primary image.
        let full =
            decode_image_with_hint(path, None, EmbeddedThumbnail::Deny).expect("full decode");
        let size = (full.width(), full.height());
        let reference = ScreenImage::of_decode(full, false)
            .crop_resized((0, 0, size.0, size.1), (thumb.width(), thumb.height()))
            .into_rgba();
        let thumb = thumb.to_rgba8();
        let total: u64 = thumb
            .as_raw()
            .iter()
            .zip(reference.as_raw())
            .map(|(&a, &b)| u64::from(a.abs_diff(b)))
            .sum();
        let mean = total / thumb.as_raw().len() as u64;
        assert!(mean < 20, "mean pixel difference {mean}");
    }
}
