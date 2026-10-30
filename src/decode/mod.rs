//! The decoders and the decode routes: `image` with libjpeg-turbo, libheif, JPEG XL,
//! macOS `sips`, the tone map of float images, the HEIF box reader, the resize, the color
//! conversion and the folder shape from the file headers.

pub(crate) mod color;
#[cfg(all(feature = "sips", target_os = "macos"))]
pub(crate) mod embedded_jpeg;
#[cfg(feature = "heic")]
pub(crate) mod heif_boxes;
pub(crate) mod jxl;
#[cfg(feature = "heic")]
pub(crate) mod libheif;
pub(crate) mod raster;
pub(crate) mod resize;
pub(crate) mod shape;
#[cfg(all(feature = "sips", target_os = "macos"))]
pub(crate) mod sips;
pub(crate) mod tonemap;
#[cfg(feature = "turbo")]
pub(crate) mod turbo;

use crate::scanner;
use color::SourceColor;
use image::{DynamicImage, RgbaImage};
#[cfg(feature = "heic")]
use libheif::{decode_with_libheif, decodes_with_libheif};
use raster::decode_raster;
use std::cell::Cell;
use std::io;
use std::path::Path;
use std::sync::mpsc;

/// The decoder of a file outside libheif. libheif has its own check (`decodes_with_libheif`),
/// because the info panel uses it too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    /// The `image` crate, with libjpeg-turbo for JPEG. Unknown extensions go here too, so that
    /// the content decides.
    Raster,
    Jxl,
    #[cfg(all(feature = "sips", target_os = "macos"))]
    Sips,
}

impl Route {
    /// Pick the decoder from the extension. The check compares without an allocation and tests
    /// the `image` formats first, so the JPEG and PNG paths pay almost nothing.
    pub(crate) fn for_path(path: &Path) -> Self {
        let listed = |list: &[&str]| scanner::has_extension(path, list);
        if listed(scanner::IMAGE_EXTENSIONS) {
            return Self::Raster;
        }
        if listed(scanner::JXL_EXTENSIONS) {
            return Self::Jxl;
        }
        #[cfg(all(feature = "sips", target_os = "macos"))]
        if listed(scanner::SIPS_EXTENSIONS) {
            return Self::Sips;
        }
        Self::Raster
    }
}

/// The real file that the ignored test reads from `variable`. Without the variable the test
/// skips with a note, so that `just test-real` runs with only some of the variables set.
#[cfg(test)]
pub(crate) fn real_test_file(variable: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os(variable).map(std::path::PathBuf::from);
    if path.is_none() {
        // clippy.toml allows output in tests. `just test-real` shows it.
        eprintln!("skipped: {variable} is not set");
    }
    path
}

/// Only grid thumbnails can use an embedded HEIF thumbnail. A camera preview can differ
/// in color or crop from the primary image, so the fullscreen view does not use it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmbeddedThumbnail {
    Allow,
    Deny,
}

/// A decoded image and the color space of its pixels. The conversion into the color space of
/// the terminal waits for the resize, so that it runs on the small image.
#[derive(Debug)]
pub(crate) struct Decoded {
    pub(crate) image: DynamicImage,
    pub(crate) color: SourceColor,
}

impl Decoded {
    /// The image at its decoded size in the color space of the terminal. An image that needs no
    /// conversion keeps its pixel type.
    pub(crate) fn into_display(self) -> DynamicImage {
        let target = color::target();
        if !color::converts(&self.color, target) {
            return self.image;
        }
        DynamicImage::ImageRgba8(color::convert(self.image.into_rgba8(), &self.color, target))
    }

    /// The image fitted into `max_w` x `max_h`, in the color space of the terminal.
    pub(crate) fn fit_within(self, max_w: u32, max_h: u32) -> RgbaImage {
        let fitted = resize::fit_rgba_within(self.image.into_rgba8(), max_w, max_h);
        color::convert(fitted, &self.color, color::target())
    }
}

/// Decode `path` in the color space of the terminal, at the decoded size.
pub(crate) fn decode_image_with_hint(
    path: &Path,
    target: Option<(u32, u32)>,
    embedded: EmbeddedThumbnail,
) -> io::Result<DynamicImage> {
    decode_image(path, target, embedded).map(Decoded::into_display)
}

/// Decode `path` with the size hint `target`. The pixels stay in the color space of the file.
pub(crate) fn decode_image(
    path: &Path,
    target: Option<(u32, u32)>,
    embedded: EmbeddedThumbnail,
) -> io::Result<Decoded> {
    #[cfg(feature = "heic")]
    if decodes_with_libheif(path) {
        // libheif applies the irot, imir and clap properties itself, so the EXIF
        // orientation must not apply a second time.
        return decode_with_libheif(path, target, embedded);
    }
    #[cfg(not(feature = "heic"))]
    let _ = embedded;

    match Route::for_path(path) {
        Route::Raster => decode_raster(path, target),
        Route::Jxl => {
            let color = color::target();
            jxl::decode(path, color).map(|image| Decoded {
                image,
                color: SourceColor::of_target(color),
            })
        }
        // The grid and the fullscreen view take the embedded JPEG of a RAW or PSD file when
        // it covers their box, else sips writes a copy at the size of the box. The zoom
        // decodes without a target, so it gets the full image with all detail.
        #[cfg(all(feature = "sips", target_os = "macos"))]
        Route::Sips => match target.and_then(|box_px| embedded_jpeg::decode_covering(path, box_px))
        {
            Some(image) => Ok(image),
            None => sips::decode(path, target),
        },
    }
}

thread_local! {
    /// True while the thread decodes the image that the user waits for.
    static FOREGROUND: Cell<bool> = const { Cell::new(false) };
}

/// Run `work` as the decode that the user waits for: the fullscreen image or the zoom source.
/// A libheif decode inside takes all cores. The thumbnails and the prefetch share them.
pub(crate) fn foreground<T>(work: impl FnOnce() -> T) -> T {
    let before = FOREGROUND.replace(true);
    let result = work();
    FOREGROUND.set(before);
    result
}

/// Run `work` as a foreground decode on the thread pool. The receiver gets the result once. A
/// dropped receiver means that the view moved on, and the result goes nowhere.
pub(crate) fn spawn_foreground<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> mpsc::Receiver<T> {
    let (tx, rx) = mpsc::channel();
    rayon::spawn(move || {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a closed receiver means that nobody waits for the result"
        )]
        let _ = tx.send(foreground(work));
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::{EmbeddedThumbnail, decode_image_with_hint};
    use crate::test_support::{BLUE, RED, assert_close, test_dir};
    use image::metadata::Orientation;
    use image::{DynamicImage, ImageDecoder, ImageEncoder, Rgb, RgbImage};
    use std::fs;

    use super::Route;
    use std::path::Path;

    #[test]
    fn the_extension_picks_the_route() {
        let route = |name: &str| Route::for_path(Path::new(name));
        assert_eq!(route("a.JPG"), Route::Raster);
        assert_eq!(route("a.exr"), Route::Raster);
        assert_eq!(route("a.APNG"), Route::Raster);
        assert_eq!(route("a.JXL"), Route::Jxl);
        assert_eq!(route("unknown.xyz"), Route::Raster);
        assert_eq!(route("no-extension"), Route::Raster);
        #[cfg(all(feature = "sips", target_os = "macos"))]
        assert_eq!(route("a.CR2"), Route::Sips);
    }

    /// A minimal little-endian TIFF block with one IFD entry: Orientation = 6 (rotate 90 CW).
    const EXIF_ORIENTATION_6: [u8; 26] = [
        b'I', b'I', 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x12, 0x01, 0x03, 0x00, 0x01,
        0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    /// A 32x16 image: the left half is red, the right half is blue.
    fn red_left_blue_right() -> RgbImage {
        RgbImage::from_fn(32, 16, |x, _| Rgb(if x < 16 { RED } else { BLUE }))
    }

    /// Orientation 6 turns the stored 32x16 into 16x32 with red on top and blue below.
    fn assert_upright(decoded: &DynamicImage) {
        let rgba = decoded.to_rgba8();
        assert_eq!(rgba.dimensions(), (16, 32));
        assert_close(rgba.get_pixel(8, 4).0, RED);
        assert_close(rgba.get_pixel(8, 27).0, BLUE);
    }

    #[cfg(feature = "turbo")]
    #[test]
    fn corrupt_jpeg_reports_both_decoder_errors() {
        // A file with the JPEG SOI magic but no valid frame: both libjpeg-turbo
        // and the pure-Rust fallback reject it. The error must mention both so a
        // turbo-only diagnosis is not silently lost.
        let dir = test_dir();
        let path = dir.path().join("corrupt.jpg");
        fs::write(&path, [0xFF, 0xD8, 0xFF, 0xD9]).unwrap();
        let err = decode_image_with_hint(&path, None, EmbeddedThumbnail::Deny).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("libjpeg-turbo:"), "message: {message}");
        assert!(message.contains("image:"), "message: {message}");
    }

    #[test]
    fn webp_with_exif_orientation_6_decodes_upright() {
        let dir = test_dir();
        let path = dir.path().join("rotated.webp");
        let file = fs::File::create(&path).unwrap();
        let mut encoder = image::codecs::webp::WebPEncoder::new_lossless(file);
        encoder
            .set_exif_metadata(EXIF_ORIENTATION_6.to_vec())
            .unwrap();
        let stored = red_left_blue_right();
        encoder
            .write_image(stored.as_raw(), 32, 16, image::ExtendedColorType::Rgb8)
            .unwrap();
        let decoded = decode_image_with_hint(&path, None, EmbeddedThumbnail::Deny).unwrap();
        assert_upright(&decoded);
    }

    #[test]
    fn jpeg_with_exif_orientation_6_decodes_upright() {
        let dir = test_dir();
        let path = dir.path().join("rotated.jpg");
        let file = fs::File::create(&path).unwrap();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(file, 95);
        encoder
            .set_exif_metadata(EXIF_ORIENTATION_6.to_vec())
            .unwrap();
        let stored = red_left_blue_right();
        encoder
            .write_image(stored.as_raw(), 32, 16, image::ExtendedColorType::Rgb8)
            .unwrap();
        let decoded = decode_image_with_hint(&path, None, EmbeddedThumbnail::Deny).unwrap();
        let scaled = decode_image_with_hint(&path, Some((8, 8)), EmbeddedThumbnail::Deny).unwrap();
        assert_upright(&decoded);
        assert!(
            scaled.height() > scaled.width(),
            "scaled decodes rotate too"
        );
    }

    /// Decode a real JPEG whose EXIF orientation is 6 or 8 (portrait stored sideways):
    /// `VIEWR_TEST_JPEG=/path/to/photo.jpg cargo test -- --ignored`.
    #[test]
    #[ignore = "needs VIEWR_TEST_JPEG, a real photo"]
    fn decodes_jpeg_upright_from_env() {
        let Some(path) = crate::decode::real_test_file("VIEWR_TEST_JPEG") else {
            return;
        };
        let path = path.as_path();
        let mut decoder = image::ImageReader::open(path)
            .unwrap()
            .into_decoder()
            .unwrap();
        let orientation = decoder.orientation().expect("orientation tag");
        let stored = decoder.dimensions();
        let decoded = decode_image_with_hint(path, None, EmbeddedThumbnail::Deny).expect("decode");
        let expected = if matches!(
            orientation,
            Orientation::Rotate90
                | Orientation::Rotate270
                | Orientation::Rotate90FlipH
                | Orientation::Rotate270FlipH
        ) {
            (stored.1, stored.0)
        } else {
            stored
        };
        assert_eq!((decoded.width(), decoded.height()), expected);
        // Scaled decodes must be rotated the same way.
        let small = decode_image_with_hint(path, Some((300, 300)), EmbeddedThumbnail::Deny)
            .expect("scaled decode");
        assert_eq!(small.width() < small.height(), expected.0 < expected.1);
    }

    /// Decode a real HEIC file: `VIEWR_TEST_HEIC=/path/to/photo.heic cargo test -- --ignored`.
    #[cfg(feature = "heic")]
    #[test]
    #[ignore = "needs VIEWR_TEST_HEIC, a real photo"]
    fn decodes_heic_from_env() {
        let Some(path) = crate::decode::real_test_file("VIEWR_TEST_HEIC") else {
            return;
        };
        let path = path.as_path();
        let full =
            decode_image_with_hint(path, None, EmbeddedThumbnail::Deny).expect("full decode");
        assert!(full.width() > 0 && full.height() > 0);
        let thumb = decode_image_with_hint(path, Some((200, 200)), EmbeddedThumbnail::Deny)
            .expect("scaled decode");
        assert!(
            thumb.width() <= 200 && thumb.height() <= 200,
            "{}x{}",
            thumb.width(),
            thumb.height()
        );
        let dims = super::libheif::libheif_dimensions(path).expect("dimensions");
        assert_eq!(dims, (full.width(), full.height()));
        let info = crate::info::load(path);
        assert!(
            info.rows.iter().any(|(_, v)| v == "EXIF"),
            "rows: {:?}",
            info.rows
        );
    }

    #[test]
    fn a_foreground_decode_marks_its_thread_for_the_duration() {
        use super::{FOREGROUND, foreground};
        assert!(!FOREGROUND.get());
        let inside = foreground(|| {
            let outer = FOREGROUND.get();
            let nested = foreground(|| FOREGROUND.get());
            (outer, nested, FOREGROUND.get())
        });
        assert_eq!(
            inside,
            (true, true, true),
            "the flag stays after a nested call"
        );
        assert!(!FOREGROUND.get(), "the flag goes back after the work");
    }

    /// QOI, TGA, EXR and HDR: the `image` crate writes a synthetic file of each, and the grid
    /// and the fullscreen decode open it.
    mod formats {
        use super::red_left_blue_right;
        use crate::decode::resize::load_and_resize_px;
        use crate::decode::{EmbeddedThumbnail, decode_image_with_hint};
        use crate::test_support::{BLUE, RED, assert_close, test_dir};
        use image::{ImageEncoder, Rgb};
        use std::fs::File;
        use std::path::Path;

        /// Decode `path` in fullscreen (no target) and in a grid cell of 16x16 px, and return
        /// the left and right pixels of the full decode.
        fn open_both_ways(path: &Path) -> ([u8; 4], [u8; 4]) {
            let full = decode_image_with_hint(path, None, EmbeddedThumbnail::Deny).unwrap();
            assert_eq!(
                (full.width(), full.height()),
                (32, 16),
                "{}",
                path.display()
            );
            let grid = load_and_resize_px(path, (16, 16), EmbeddedThumbnail::Allow).unwrap();
            assert_eq!(grid.dimensions(), (16, 8), "{}", path.display());
            let rgba = full.to_rgba8();
            (rgba.get_pixel(4, 8).0, rgba.get_pixel(28, 8).0)
        }

        #[test]
        fn qoi_and_tga_open() {
            let dir = test_dir();
            let pixels = red_left_blue_right();
            let qoi = dir.path().join("synthetic.qoi");
            image::codecs::qoi::QoiEncoder::new(File::create(&qoi).unwrap())
                .write_image(pixels.as_raw(), 32, 16, image::ExtendedColorType::Rgb8)
                .unwrap();
            let tga = dir.path().join("synthetic.TGA");
            image::codecs::tga::TgaEncoder::new(File::create(&tga).unwrap())
                .write_image(pixels.as_raw(), 32, 16, image::ExtendedColorType::Rgb8)
                .unwrap();
            for path in [qoi, tga] {
                let (left, right) = open_both_ways(&path);
                assert_close(left, RED);
                assert_close(right, BLUE);
            }
        }

        /// Linear light: 0.18 (middle gray) on the left, 4.0 (brighter than white) on the right.
        fn linear_samples() -> Vec<f32> {
            (0..16 * 32)
                .flat_map(|i| {
                    if i % 32 < 16 {
                        [0.18; 3]
                    } else {
                        [4.0, 0.0, 0.0]
                    }
                })
                .collect()
        }

        /// After the tone map, middle gray is a mid tone, not black. 4.0 is the white point of
        /// the image, so it becomes full red.
        fn assert_tone_mapped(path: &Path) {
            let (left, right) = open_both_ways(path);
            assert!(
                left[..3].iter().all(|&c| (100..=120).contains(&c)),
                "{left:?}"
            );
            assert!(right[0] == 255 && right[1] == 0, "{right:?}");
            assert_eq!(left[3], 255);
        }

        #[test]
        fn exr_and_hdr_are_tone_mapped_to_8_bit() {
            let dir = test_dir();
            let samples = linear_samples();
            let exr = dir.path().join("synthetic.exr");
            let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_ne_bytes()).collect();
            image::codecs::openexr::OpenExrEncoder::new(File::create(&exr).unwrap())
                .write_image(&bytes, 32, 16, image::ExtendedColorType::Rgb32F)
                .unwrap();
            let hdr = dir.path().join("synthetic.hdr");
            let pixels: Vec<Rgb<f32>> =
                samples.chunks(3).map(|c| Rgb([c[0], c[1], c[2]])).collect();
            image::codecs::hdr::HdrEncoder::new(File::create(&hdr).unwrap())
                .encode(&pixels, 32, 16)
                .unwrap();
            assert_tone_mapped(&exr);
            assert_tone_mapped(&hdr);
        }
    }
}
