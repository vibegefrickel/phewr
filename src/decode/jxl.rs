//! JPEG XL through `jxl-oxide`, in pure Rust.
//!
//! `jxl-oxide` has no scaled decode, so the grid decodes the full image and resizes it. The
//! thumbnail disk cache keeps the result. An animated JPEG XL shows its first frame.
//!
//! The decoder renders into the color space of the terminal (`ColorTarget`): sRGB or Display
//! P3. `jxl-oxide` converts the enum color encodings and the XYB samples itself and tone-maps PQ
//! and HLG to SDR. Samples in the space of an ICC profile go through one `moxcms` transform
//! (`IccToTarget`), 16-bit for a deeper file.

use crate::decode::color::ColorTarget;
use image::{DynamicImage, GrayAlphaImage, GrayImage, RgbImage, RgbaImage};
use jxl_oxide::color::ColourEncoding;
use jxl_oxide::{
    AllocTracker, AuxBoxData, EnumColourEncoding, JxlImage, Moxcms, PixelFormat, RenderingIntent,
};
use moxcms::{
    CmsError, ColorProfile, Layout, TransferCharacteristics, Transform8BitExecutor,
    Transform16BitExecutor, TransformOptions,
};
use rayon::prelude::*;
use std::io;
use std::path::Path;
use std::sync::Arc;

/// The limit for the internal buffers of `jxl-oxide`. A larger header size fails with an error
/// and does not abort the process. A 24 MP RGB image needs about 300 MB.
const DECODER_MEMORY_LIMIT: usize = 2 * 1024 * 1024 * 1024;

/// The rows of one task of the parallel ICC transform.
const ROWS_PER_TASK: usize = 64;

/// Decode the first frame of `path` in the color space `target`, upright: the stream applies
/// the codestream orientation. The EXIF orientation does not apply, because the codestream wins.
pub(crate) fn decode(path: &Path, target: ColorTarget) -> io::Result<DynamicImage> {
    let image = open_for_render(path)?;
    let format = image.pixel_format();
    if let Some(icc) = IccToTarget::for_image(&image, format, target) {
        reserve(&image, icc.bytes_per_pixel())?;
        return decode_icc_or_enum(path, &image, &icc, target);
    }
    reserve(&image, format.channels())?;
    decode_enum(image, target)
}

/// Open `path` with the memory limit, and refuse CMYK.
fn open_for_render(path: &Path) -> io::Result<JxlImage> {
    let image = JxlImage::builder()
        .alloc_tracker(AllocTracker::with_limit(DECODER_MEMORY_LIMIT))
        .open(path)
        .map_err(invalid)?;
    if matches!(image.pixel_format(), PixelFormat::Cmyk | PixelFormat::Cmyka) {
        return Err(invalid("CMYK JPEG XL images are not supported"));
    }
    Ok(image)
}

/// Check the buffers of viewr against the allocation limit of the `image` route:
/// `bytes_per_pixel` counts each buffer that exists at the same time.
fn reserve(image: &JxlImage, bytes_per_pixel: usize) -> io::Result<()> {
    let bytes = u64::from(image.width())
        .checked_mul(u64::from(image.height()))
        .and_then(|pixels| pixels.checked_mul(bytes_per_pixel as u64))
        .ok_or_else(|| invalid("JPEG XL image size overflows"))?;
    image::Limits::default().reserve(bytes).map_err(invalid)
}

/// Let `jxl-oxide` render into `target`: XYB, the enum encodings and the ICC fallback. Gray
/// has the white point and the curve of sRGB in both targets.
fn decode_enum(mut image: JxlImage, target: ColorTarget) -> io::Result<DynamicImage> {
    let format = image.pixel_format();
    let intent = RenderingIntent::Relative;
    image.set_cms(Moxcms);
    image.request_color_encoding(match target {
        _ if format.is_grayscale() => EnumColourEncoding::gray_srgb(intent),
        ColorTarget::Srgb => EnumColourEncoding::srgb(intent),
        ColorTarget::DisplayP3 => EnumColourEncoding::display_p3(intent),
    });
    let render = image.render_frame(0).map_err(invalid)?;
    let mut stream = render.stream();
    let (width, height) = (stream.width(), stream.height());
    let channels = stream.channels() as usize;
    let mut pixels = vec![0u8; width as usize * height as usize * channels];
    stream.write_to_buffer(&mut pixels);
    into_image(width, height, channels, pixels)
}

/// Convert through `icc`. A transform error leaves the conversion to `jxl-oxide`, with a new
/// decoder, because the first one keeps the render without a color request.
fn decode_icc_or_enum(
    path: &Path,
    image: &JxlImage,
    icc: &IccToTarget,
    target: ColorTarget,
) -> io::Result<DynamicImage> {
    icc.decode(image)
        .or_else(|_| decode_enum(open_for_render(path)?, target))
}

/// The `image` type for the channel count.
fn into_image(
    width: u32,
    height: u32,
    channels: usize,
    pixels: Vec<u8>,
) -> io::Result<DynamicImage> {
    let decoded = match channels {
        1 => GrayImage::from_raw(width, height, pixels).map(DynamicImage::ImageLuma8),
        2 => GrayAlphaImage::from_raw(width, height, pixels).map(DynamicImage::ImageLumaA8),
        3 => RgbImage::from_raw(width, height, pixels).map(DynamicImage::ImageRgb8),
        4 => RgbaImage::from_raw(width, height, pixels).map(DynamicImage::ImageRgba8),
        _ => None,
    };
    decoded.ok_or_else(|| invalid(format!("JPEG XL image with {channels} channels")))
}

/// The conversion of samples in the space of an ICC profile into the target.
///
/// `jxl-oxide` 0.12 converts the samples of an ICC v2 profile wrong: Adobe RGB 1998 blue
/// comes out as (39, 19, 255), not (0, 0, 255). One `moxcms` transform from the profile to
/// sRGB gives the values of `djxl` and `magick`.
struct IccToTarget {
    transform: IccTransform,
    /// The channels of the rendered samples: gray or RGB, with or without alpha.
    input_channels: usize,
    /// The output channels: RGB, or RGBA with alpha. A gray profile also gives RGB.
    channels: usize,
}

/// The `moxcms` transform for the sample depth of the file.
enum IccTransform {
    /// A file with 8 bits per sample.
    Eight(Arc<Transform8BitExecutor>),
    /// A deeper file. The samples stay 16-bit up to the output, so they round only one time.
    Sixteen(Arc<Transform16BitExecutor>),
}

impl IccToTarget {
    /// The transform for a file whose samples are in the space of its ICC profile. An XYB
    /// file has no such samples: `jxl-oxide` renders XYB into sRGB without the profile. A
    /// profile that `moxcms` cannot read, and a PQ or HLG profile, stay with `jxl-oxide`.
    fn for_image(image: &JxlImage, format: PixelFormat, target: ColorTarget) -> Option<Self> {
        let metadata = &image.image_header().metadata;
        if metadata.xyb_encoded
            || !matches!(metadata.colour_encoding, ColourEncoding::IccProfile(_))
        {
            return None;
        }
        let (layout, output, input_channels) = match format {
            PixelFormat::Gray => (Layout::Gray, Layout::Rgb, 1),
            PixelFormat::Graya => (Layout::GrayAlpha, Layout::Rgba, 2),
            PixelFormat::Rgb => (Layout::Rgb, Layout::Rgb, 3),
            PixelFormat::Rgba => (Layout::Rgba, Layout::Rgba, 4),
            PixelFormat::Cmyk | PixelFormat::Cmyka => return None,
        };
        let profile = ColorProfile::new_from_slice(image.original_icc()?).ok()?;
        // moxcms does not tone-map. jxl-oxide 0.12 does not tone-map these profiles either,
        // but it is the place for an upstream fix.
        let hdr = profile.cicp.as_ref().is_some_and(|cicp| {
            matches!(
                cicp.transfer_characteristics,
                TransferCharacteristics::Smpte2084 | TransferCharacteristics::Hlg
            )
        });
        if hdr {
            return None;
        }
        let options = TransformOptions {
            rendering_intent: moxcms::RenderingIntent::RelativeColorimetric,
            ..TransformOptions::default()
        };
        let destination = target.profile();
        let transform = if metadata.bit_depth.bits_per_sample() > 8 {
            IccTransform::Sixteen(
                profile
                    .create_transform_16bit(layout, &destination, output, options)
                    .ok()?,
            )
        } else {
            IccTransform::Eight(
                profile
                    .create_transform_8bit(layout, &destination, output, options)
                    .ok()?,
            )
        };
        let channels = if output == Layout::Rgba { 4 } else { 3 };
        Some(Self {
            transform,
            input_channels,
            channels,
        })
    }

    /// The bytes per pixel of the rendered samples and of the output, which exist at once.
    fn bytes_per_pixel(&self) -> usize {
        let sample_bytes = match self.transform {
            IccTransform::Eight(_) => 1,
            IccTransform::Sixteen(_) => 2,
        };
        self.input_channels * sample_bytes + self.channels
    }

    /// Render the first frame without a color request, in the space of the profile, and
    /// convert it to 8 bits in the target. The rows convert in parallel.
    fn decode(&self, image: &JxlImage) -> io::Result<DynamicImage> {
        let render = image.render_frame(0).map_err(invalid)?;
        let mut stream = render.stream();
        if stream.channels() as usize != self.input_channels {
            return Err(invalid("JPEG XL channels do not match the ICC transform"));
        }
        let (width, height) = (stream.width(), stream.height());
        let row = width as usize;
        let samples = row * height as usize * self.input_channels;
        let mut pixels = vec![0u8; row * height as usize * self.channels];
        let input_chunk = row * self.input_channels * ROWS_PER_TASK;
        let output_chunk = row * self.channels * ROWS_PER_TASK;
        match &self.transform {
            IccTransform::Eight(transform) => {
                let mut source = vec![0u8; samples];
                stream.write_to_buffer(&mut source);
                source
                    .par_chunks(input_chunk)
                    .zip(pixels.par_chunks_mut(output_chunk))
                    .try_for_each(|(input, output)| transform.transform(input, output))
                    .map_err(invalid)?;
            }
            IccTransform::Sixteen(transform) => {
                let mut source = vec![0u16; samples];
                stream.write_to_buffer(&mut source);
                source
                    .par_chunks(input_chunk)
                    .zip(pixels.par_chunks_mut(output_chunk))
                    .try_for_each(|(input, output)| {
                        let mut deep = vec![0u16; output.len()];
                        transform.transform(input, &mut deep)?;
                        for (value, sample) in output.iter_mut().zip(deep) {
                            *value = to_8bit(sample);
                        }
                        Ok::<(), CmsError>(())
                    })
                    .map_err(invalid)?;
            }
        }
        into_image(width, height, self.channels, pixels)
    }
}

/// Round a 16-bit sample to 8 bits.
fn to_8bit(sample: u16) -> u8 {
    u8::try_from((u32::from(sample) * 255 + 32_767) / 65_535).unwrap_or(u8::MAX)
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
    use super::{
        IccToTarget, decode, decode_enum, decode_icc_or_enum, dimensions, open_for_render,
    };
    use crate::decode::color::ColorTarget;
    use crate::decode::{EmbeddedThumbnail, decode_image_with_hint};
    use crate::test_support::{assert_close, fixture};
    use image::{DynamicImage, RgbaImage};

    /// The largest difference in 8-bit levels between a patch of viewr and of djxl.
    const REFERENCE_TOLERANCE: f32 = 4.0;

    /// Decode a fixture of `tests/fixtures/jxl/` through the route of the fullscreen view.
    fn decode_fixture(name: &str) -> DynamicImage {
        let path = fixture(&format!("jxl/{name}.jxl"));
        decode_image_with_hint(&path, None, EmbeddedThumbnail::Deny).unwrap()
    }

    /// The mean of the 4x4 center pixels of each 8x8 patch, per channel, row by row. The
    /// center skips the ringing of a lossy patch edge.
    fn patch_means(image: &RgbaImage) -> Vec<[f32; 4]> {
        let (width, height) = image.dimensions();
        let mut means = Vec::new();
        for patch_y in (0..height).step_by(8) {
            for patch_x in (0..width).step_by(8) {
                let mut sum = [0.0f32; 4];
                for y in patch_y + 2..patch_y + 6 {
                    for x in patch_x + 2..patch_x + 6 {
                        for (total, value) in sum.iter_mut().zip(image.get_pixel(x, y).0) {
                            *total += f32::from(value) / 16.0;
                        }
                    }
                }
                means.push(sum);
            }
        }
        means
    }

    #[test]
    fn each_fixture_matches_the_libjxl_reference() {
        let references = std::fs::read_dir(fixture("jxl/ref")).unwrap();
        let mut checked = 0;
        for entry in references {
            let path = entry.unwrap().path();
            let name = path.file_stem().unwrap().to_str().unwrap().to_owned();
            let decoded = decode_fixture(&name).to_rgba8();
            let reference = image::open(&path).unwrap().to_rgba8();
            assert_eq!(decoded.dimensions(), reference.dimensions(), "{name}");
            let worst = patch_means(&decoded)
                .iter()
                .zip(patch_means(&reference))
                .flat_map(|(ours, theirs)| ours.iter().zip(theirs).map(|(a, b)| (a - b).abs()))
                .fold(0.0, f32::max);
            assert!(
                worst <= REFERENCE_TOLERANCE,
                "{name}: a patch differs by {worst} levels"
            );
            checked += 1;
        }
        assert_eq!(checked, 15, "the fixtures with a reference");
    }

    #[test]
    fn an_icc_v2_profile_keeps_the_blue_primary_of_srgb() {
        // Adobe RGB 1998 has the blue primary of sRGB. jxl-oxide alone gives (39, 19, 255).
        for name in ["adobe-rgb-icc", "adobe-rgb-icc-alpha"] {
            let blue = patch_means(&decode_fixture(name).to_rgba8())[2];
            assert!(
                blue[0] <= 1.0 && blue[1] <= 1.0 && blue[2] >= 254.0,
                "{name}: {blue:?}"
            );
        }
    }

    #[test]
    fn a_16_bit_icc_file_rounds_one_time() {
        // The gradient climbs 16 levels over 64 pixels, so most 16-bit samples lie between two
        // 8-bit levels. A transform of 8-bit samples rounds two times and skips levels: the
        // red ramp jumps from 94 to 96. djxl dithers, so it is no reference per pixel.
        let decoded = decode_fixture("adobe-rgb-icc-16bit").to_rgba8();
        for channel in 0..3 {
            let ramp: Vec<u8> = (0..64)
                .map(|x| decoded.get_pixel(x, 0).0[channel])
                .collect();
            assert!(
                ramp.windows(2)
                    .all(|pair| pair[1].checked_sub(pair[0]).is_some_and(|step| step <= 1)),
                "channel {channel}: {ramp:?}"
            );
        }
    }

    #[test]
    fn an_hdr_icc_profile_stays_with_jxl_oxide() {
        let open = |name: &str| open_for_render(&fixture(&format!("jxl/{name}.jxl"))).unwrap();
        for name in ["pq-icc", "hlg-icc"] {
            let image = open(name);
            assert!(
                IccToTarget::for_image(&image, image.pixel_format(), ColorTarget::Srgb).is_none(),
                "{name}"
            );
        }
        let image = open("adobe-rgb-icc");
        assert!(IccToTarget::for_image(&image, image.pixel_format(), ColorTarget::Srgb).is_some());
    }

    #[test]
    fn a_failed_icc_transform_falls_back_to_jxl_oxide() {
        let path = fixture("jxl/adobe-rgb-icc.jxl");
        let image = open_for_render(&path).unwrap();
        let mut icc =
            IccToTarget::for_image(&image, image.pixel_format(), ColorTarget::Srgb).unwrap();
        icc.input_channels = 4;
        assert!(icc.decode(&image).is_err());
        let decoded = decode_icc_or_enum(&path, &image, &icc, ColorTarget::Srgb).unwrap();
        let fallback = decode_enum(open_for_render(&path).unwrap(), ColorTarget::Srgb).unwrap();
        assert_eq!(decoded, fallback);
    }

    #[test]
    fn the_codestream_orientation_turns_the_image_upright() {
        // The stored chart is 64x32. Orientation 6 turns it clockwise: the stored top row
        // becomes the right column, and the stored bottom row becomes the left column.
        for name in ["orientation-6", "jpeg-orientation-6"] {
            let path = fixture(&format!("jxl/{name}.jxl"));
            let decoded = decode(&path, ColorTarget::Srgb).unwrap().to_rgba8();
            assert_eq!(decoded.dimensions(), (32, 64), "{name}");
            assert_eq!(dimensions(&path), Some((32, 64)), "{name}");
            assert_eq!(crate::decode::shape::aspect_of(&path), Some(0.5), "{name}");
            assert_close(decoded.get_pixel(28, 4).0, [255, 0, 0]);
            assert_close(decoded.get_pixel(28, 60).0, [0, 0, 0]);
            assert_close(decoded.get_pixel(4, 4).0, [193, 90, 99]);
        }
    }

    #[test]
    fn hlg_keeps_the_gray_ramp_neutral_and_in_order() {
        // jxl-oxide renders HLG for a 1000 nit display and tone-maps it to SDR. libjxl renders
        // HLG differently in its lossless and lossy paths, so no djxl reference applies.
        let means = patch_means(&decode_fixture("hlg").to_rgba8());
        let ramp = &means[8..16];
        for gray in ramp {
            let spread = gray[..3]
                .iter()
                .fold(0.0f32, |high, &value| high.max(value))
                - gray[..3]
                    .iter()
                    .fold(255.0f32, |low, &value| low.min(value));
            assert!(spread <= 3.0, "gray {gray:?}");
        }
        assert!(
            ramp.windows(2).all(|pair| pair[1][0] >= pair[0][0]),
            "{ramp:?}"
        );
        assert!(ramp[0][0] <= 2.0 && ramp[7][0] >= 250.0, "{ramp:?}");
        let red = means[0];
        assert!(
            red[0] > red[1] + 60.0 && red[0] > red[2] + 60.0,
            "red {red:?}"
        );
    }

    #[test]
    fn a_broken_file_reports_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.jxl");
        std::fs::write(&path, b"this is not a JPEG XL file").unwrap();
        assert!(decode(&path, ColorTarget::Srgb).is_err());
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
        let thumb =
            crate::decode::resize::load_and_resize_px(path, (200, 200), EmbeddedThumbnail::Allow)
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
