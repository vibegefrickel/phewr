//! The `image` route, with libjpeg-turbo for JPEG, and the EXIF orientation.

#[cfg(feature = "turbo")]
use super::turbo::{decode_jpeg_turbo, is_jpeg};
use crate::decode;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader};
use std::io;
use std::path::Path;

/// Decode a JPEG that sits inside another file (a RAW preview, a PSD thumbnail). libjpeg-turbo
/// scales it down to `target` during the decode. `container_orientation` is the EXIF
/// orientation of the outer file. It wins over the orientation of the JPEG itself, the same
/// rule as on the turbo path.
#[cfg(all(feature = "sips", target_os = "macos"))]
pub(crate) fn decode_jpeg_bytes(
    data: &[u8],
    target: Option<(u32, u32)>,
    container_orientation: Option<u8>,
) -> io::Result<DynamicImage> {
    #[cfg(feature = "turbo")]
    if let Ok(image) = super::turbo::decode_jpeg_turbo_bytes(data, target, container_orientation) {
        return Ok(image);
    }
    #[cfg(not(feature = "turbo"))]
    let _ = target;
    let reader = ImageReader::new(io::Cursor::new(data))
        .with_guessed_format()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let orientation = match container_orientation.and_then(Orientation::from_exif) {
        Some(orientation) => orientation,
        None => decoder.orientation().unwrap_or(Orientation::NoTransforms),
    };
    let mut image = DynamicImage::from_decoder(decoder)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    image.apply_orientation(orientation);
    Ok(image)
}

pub(super) fn decode_raster(path: &Path, target: Option<(u32, u32)>) -> io::Result<DynamicImage> {
    // libjpeg-turbo is the fast path, but it rejects some JPEG variants the
    // pure-Rust decoder accepts (and vice versa), so a turbo failure falls
    // through to `image` rather than aborting. Remember turbo's error so that if
    // the fallback also fails we can report *both* reasons instead of only the
    // fallback's — otherwise a genuine turbo diagnosis is lost.
    #[cfg(not(feature = "turbo"))]
    let _ = target;
    #[cfg(feature = "turbo")]
    let turbo_error = if is_jpeg(path) {
        match decode_jpeg_turbo(path, target) {
            Ok(img) => return Ok(img),
            Err(error) => Some(error),
        }
    } else {
        None
    };

    let reader = ImageReader::open(path)
        .map_err(|e| io::Error::new(io::ErrorKind::NotFound, e))?
        .with_guessed_format()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    match decode_upright(reader) {
        Ok(img) => Ok(img),
        Err(fallback_error) => {
            #[cfg(feature = "turbo")]
            if let Some(turbo_error) = turbo_error {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "failed to decode image (libjpeg-turbo: {turbo_error}; image: {fallback_error})"
                    ),
                ));
            }
            Err(io::Error::new(io::ErrorKind::InvalidData, fallback_error))
        }
    }
}

/// Decode with `image` and apply the EXIF orientation that its decoder reads (JPEG, PNG,
/// TIFF, WebP), so that phone photos show upright. A float image (EXR, HDR) is tone-mapped to
/// 8 bit.
pub(crate) fn decode_upright<R: io::BufRead + io::Seek>(
    reader: ImageReader<R>,
) -> image::ImageResult<DynamicImage> {
    let mut decoder = reader.into_decoder()?;
    // A broken EXIF block must not stop the decode of the pixels.
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    // The same allocation limit as `ImageReader::decode`.
    image::Limits::default().reserve(decoder.total_bytes())?;
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok(decode::tonemap::to_display(img))
}

#[cfg(test)]
mod tests {
    use super::decode_raster;
    use crate::test_support::fixture;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const BLACK: [u8; 4] = [0, 0, 0, 255];
    const GRAY: [u8; 4] = [200, 200, 200, 255];

    /// The RGBA pixels in the middle of the left half and of the right half of a 32x16 fixture
    /// of `tests/fixtures/png`.
    fn halves(name: &str) -> ([u8; 4], [u8; 4]) {
        let decoded = decode_raster(&fixture(&format!("png/{name}")), None)
            .unwrap()
            .to_rgba8();
        assert_eq!(decoded.dimensions(), (32, 16), "{name}");
        (decoded.get_pixel(8, 8).0, decoded.get_pixel(24, 8).0)
    }

    #[test]
    fn every_opaque_png_layout_decodes_to_its_colors() {
        for name in ["rgb16.png", "palette.png", "interlaced.png"] {
            assert_eq!(halves(name), (RED, BLUE), "{name}");
        }
        for name in ["gray8.png", "gray16.png"] {
            assert_eq!(halves(name), (BLACK, GRAY), "{name}");
        }
    }

    #[test]
    fn the_transparent_half_of_a_png_stays_transparent() {
        for (name, left) in [
            ("rgba8.png", RED),
            ("rgba16.png", RED),
            ("palette-trns.png", RED),
            ("gray-alpha.png", BLACK),
        ] {
            let (opaque, transparent) = halves(name);
            assert_eq!(opaque, left, "{name}");
            assert_eq!(transparent[3], 0, "{name}");
        }
    }

    #[test]
    fn a_png_with_exif_orientation_6_decodes_upright() {
        let decoded = decode_raster(&fixture("png/orientation-6.png"), None)
            .unwrap()
            .to_rgba8();
        assert_eq!(decoded.dimensions(), (16, 32));
        assert_eq!(decoded.get_pixel(8, 4).0, RED);
        assert_eq!(decoded.get_pixel(8, 27).0, BLUE);
    }
}
