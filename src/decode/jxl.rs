//! JPEG XL through `jxl-oxide`, in pure Rust.
//!
//! `jxl-oxide` has no scaled decode, so the grid decodes the full image and resizes it. The
//! thumbnail disk cache keeps the result. An animated JPEG XL shows its first frame.
//!
//! viewr shows sRGB, so the decoder renders into sRGB. `jxl-oxide` converts the enum color
//! encodings itself and tone-maps PQ and HLG to SDR. An ICC profile goes through `moxcms`.

use image::{DynamicImage, GrayAlphaImage, GrayImage, RgbImage, RgbaImage};
use jxl_oxide::{
    AllocTracker, AuxBoxData, EnumColourEncoding, JxlImage, Moxcms, PixelFormat, RenderingIntent,
};
use std::io;
use std::path::Path;

/// The limit for the internal buffers of `jxl-oxide`. A larger header size fails with an error
/// and does not abort the process. A 24 MP RGB image needs about 300 MB.
const DECODER_MEMORY_LIMIT: usize = 2 * 1024 * 1024 * 1024;

/// Decode the first frame of `path` as sRGB, upright: the stream applies the codestream
/// orientation.
pub(crate) fn decode(path: &Path) -> io::Result<DynamicImage> {
    let mut image = JxlImage::builder()
        .alloc_tracker(AllocTracker::with_limit(DECODER_MEMORY_LIMIT))
        .open(path)
        .map_err(invalid)?;
    let format = image.pixel_format();
    if matches!(format, PixelFormat::Cmyk | PixelFormat::Cmyka) {
        return Err(invalid("CMYK JPEG XL images are not supported"));
    }
    // The 8-bit output must stay inside the allocation limit of the `image` route.
    let output_bytes = u64::from(image.width())
        .checked_mul(u64::from(image.height()))
        .and_then(|pixels| pixels.checked_mul(format.channels() as u64))
        .ok_or_else(|| invalid("JPEG XL image size overflows"))?;
    image::Limits::default()
        .reserve(output_bytes)
        .map_err(invalid)?;
    let intent = RenderingIntent::Relative;
    image.set_cms(Moxcms);
    image.request_color_encoding(if format.is_grayscale() {
        EnumColourEncoding::gray_srgb(intent)
    } else {
        EnumColourEncoding::srgb(intent)
    });
    let render = image.render_frame(0).map_err(invalid)?;
    let mut stream = render.stream();
    let (width, height, channels) = (stream.width(), stream.height(), stream.channels());
    let mut pixels = vec![0u8; width as usize * height as usize * channels as usize];
    stream.write_to_buffer(&mut pixels);
    let decoded = match channels {
        1 => GrayImage::from_raw(width, height, pixels).map(DynamicImage::ImageLuma8),
        2 => GrayAlphaImage::from_raw(width, height, pixels).map(DynamicImage::ImageLumaA8),
        3 => RgbImage::from_raw(width, height, pixels).map(DynamicImage::ImageRgb8),
        4 => RgbaImage::from_raw(width, height, pixels).map(DynamicImage::ImageRgba8),
        _ => None,
    };
    decoded.ok_or_else(|| invalid(format!("JPEG XL image with {channels} channels")))
}

/// The upright size for the info panel. The header is enough, no frame is decoded.
pub(crate) fn dimensions(path: &Path) -> Option<(u32, u32)> {
    let image = open(path).ok()?;
    Some((image.width(), image.height()))
}

/// The TIFF bytes of the Exif box, for the EXIF rows of the info panel.
pub(crate) fn exif(path: &Path) -> Option<Vec<u8>> {
    let image = open(path).ok()?;
    let AuxBoxData::Data(exif) = image.aux_boxes().first_exif().ok()? else {
        return None;
    };
    let start = usize::try_from(exif.tiff_header_offset()).ok()?;
    exif.payload().get(start..).map(<[u8]>::to_vec)
}

fn open(path: &Path) -> io::Result<JxlImage> {
    JxlImage::builder().open(path).map_err(invalid)
}

fn invalid(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[cfg(test)]
mod tests {
    use super::{decode, dimensions};
    use crate::app::{EmbeddedThumbnail, decode_image_with_hint};

    #[test]
    fn a_broken_file_reports_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.jxl");
        std::fs::write(&path, b"this is not a JPEG XL file").unwrap();
        assert!(decode(&path).is_err());
        assert!(dimensions(&path).is_none());
        assert!(decode_image_with_hint(&path, None, EmbeddedThumbnail::Deny).is_err());
    }

    /// Decode a real JPEG XL file: `VIEWR_TEST_JXL=/path/to/image.jxl`.
    #[test]
    #[ignore = "needs VIEWR_TEST_JXL, a real JPEG XL file"]
    fn decodes_jxl_from_env() {
        let Some(path) = crate::decode::real_test_file("VIEWR_TEST_JXL") else {
            return;
        };
        let path = path.as_path();
        let full =
            decode_image_with_hint(path, None, EmbeddedThumbnail::Deny).expect("full decode");
        assert_eq!(dimensions(path), Some((full.width(), full.height())));
        let rect = ratatui::layout::Rect::new(0, 0, 10, 10);
        let thumb = crate::app::load_and_resize(path, rect, (20, 20), EmbeddedThumbnail::Allow)
            .expect("grid decode");
        assert!(thumb.width() <= 200 && thumb.height() <= 200);
        let info = crate::info::load(path);
        assert!(
            info.rows.iter().any(|(label, _)| label == "Dimensions"),
            "rows: {:?}",
            info.rows
        );
    }
}
