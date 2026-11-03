//! JPEG XL through `jxl-oxide`, in pure Rust.
//!
//! `jxl-oxide` has no scaled decode: the grid decodes the full image, and the disk cache keeps
//! the result. An animated file shows its first frame. `jxl-oxide` renders XYB and the enum
//! encodings into the target itself. ICC samples go through `moxcms` (`IccToTarget`). A PQ or
//! HLG file keeps its 16-bit signal for the tone map of `Decoded` (`HdrSignal`).

use crate::decode::Decoded;
use crate::decode::color::{ColorTarget, SourceColor};
use crate::decode::hdr;
use image::{DynamicImage, GrayAlphaImage, GrayImage, ImageBuffer, RgbImage, RgbaImage};
use jxl_oxide::color::{ColourEncoding, Primaries, TransferFunction, WhitePoint};
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

/// A larger header fails with an error instead of an abort. A 24 MP RGB image needs 300 MB.
const DECODER_MEMORY_LIMIT: usize = 2 * 1024 * 1024 * 1024;

const ROWS_PER_TASK: usize = 64;

/// Decode the first frame of `path` into `target`. The codestream orientation applies, the EXIF
/// orientation does not.
pub(crate) fn decode(path: &Path, target: ColorTarget) -> io::Result<Decoded> {
    let image = open_for_render(path)?;
    let format = image.pixel_format();
    if let Some(signal) = HdrSignal::for_image(&image, format) {
        reserve(&image, format.channels() * 2)?;
        return signal.decode(image);
    }
    let in_target = |image| Decoded {
        image,
        color: SourceColor::of_target(target),
        opaque: false,
    };
    if let Some(icc) = IccToTarget::for_image(&image, format, target) {
        reserve(&image, icc.bytes_per_pixel())?;
        return decode_icc_or_enum(path, &image, &icc, target).map(in_target);
    }
    reserve(&image, format.channels())?;
    decode_enum(image, target).map(in_target)
}

/// The 16-bit signal of a PQ or HLG file and its CICP code points.
struct HdrSignal {
    color: SourceColor,
    /// The enum encoding that turns XYB into the signal. `None` for ICC samples.
    /// `jxl-oxide` 0.12 clips XYB to the sRGB gamut before the primaries conversion, and a lossy
    /// file loses saturation there.
    encoding: Option<EnumColourEncoding>,
}

impl HdrSignal {
    /// The signal of an RGB file with a PQ or HLG curve. Gray, custom primaries, other white points
    /// and XYB with an ICC profile stay with `jxl-oxide`.
    fn for_image(image: &JxlImage, format: PixelFormat) -> Option<Self> {
        if !matches!(format, PixelFormat::Rgb | PixelFormat::Rgba) {
            return None;
        }
        let metadata = &image.image_header().metadata;
        match &metadata.colour_encoding {
            ColourEncoding::Enum(encoding) => {
                let transfer = match encoding.tf {
                    TransferFunction::Pq => 16,
                    TransferFunction::Hlg => 18,
                    _ => return None,
                };
                let primaries = match (&encoding.primaries, &encoding.white_point) {
                    (Primaries::Srgb, WhitePoint::D65) => 1,
                    (Primaries::Bt2100, WhitePoint::D65) => 9,
                    (Primaries::P3, WhitePoint::Dci) => 11,
                    (Primaries::P3, WhitePoint::D65) => 12,
                    _ => return None,
                };
                Some(Self {
                    color: SourceColor::Cicp {
                        primaries,
                        transfer,
                    },
                    encoding: Some(EnumColourEncoding {
                        rendering_intent: RenderingIntent::Relative,
                        ..encoding.clone()
                    }),
                })
            }
            ColourEncoding::IccProfile(_) if !metadata.xyb_encoded => {
                let color = SourceColor::icc(image.original_icc()?.to_vec());
                hdr::is_hdr(&color).then_some(Self {
                    color,
                    encoding: None,
                })
            }
            ColourEncoding::IccProfile(_) => None,
        }
    }

    fn decode(self, mut image: JxlImage) -> io::Result<Decoded> {
        if let Some(encoding) = self.encoding {
            image.set_cms(Moxcms);
            image.request_color_encoding(encoding);
        }
        let render = image.render_frame(0).map_err(invalid)?;
        let mut stream = render.stream();
        let (width, height) = (stream.width(), stream.height());
        let channels = stream.channels() as usize;
        let mut samples = vec![0u16; width as usize * height as usize * channels];
        stream.write_to_buffer(&mut samples);
        let decoded = match channels {
            3 => ImageBuffer::from_raw(width, height, samples).map(DynamicImage::ImageRgb16),
            4 => ImageBuffer::from_raw(width, height, samples).map(DynamicImage::ImageRgba16),
            _ => None,
        };
        Ok(Decoded {
            image: decoded
                .ok_or_else(|| invalid(format!("HDR JPEG XL image with {channels} channels")))?,
            color: self.color,
            opaque: false,
        })
    }
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

/// Check the buffers against the allocation limit of the `image` route.
fn reserve(image: &JxlImage, bytes_per_pixel: usize) -> io::Result<()> {
    let bytes = u64::from(image.width())
        .checked_mul(u64::from(image.height()))
        .and_then(|pixels| pixels.checked_mul(bytes_per_pixel as u64))
        .ok_or_else(|| invalid("JPEG XL image size overflows"))?;
    image::Limits::default().reserve(bytes).map_err(invalid)
}

/// Gray keeps the white point and the curve of sRGB in both targets.
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

/// A transform error falls back to `jxl-oxide` with a new decoder. The first decoder has
/// rendered without a color request.
fn decode_icc_or_enum(
    path: &Path,
    image: &JxlImage,
    icc: &IccToTarget,
    target: ColorTarget,
) -> io::Result<DynamicImage> {
    icc.decode(image)
        .or_else(|_| decode_enum(open_for_render(path)?, target))
}

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

/// The conversion of ICC samples into the target.
///
/// `jxl-oxide` 0.12 converts ICC v2 samples wrong: Adobe RGB blue comes out as (39, 19, 255).
/// `moxcms` gives the values of `djxl` and `magick`.
struct IccToTarget {
    transform: IccTransform,
    input_channels: usize,
    /// RGB or RGBA. A gray profile gives RGB.
    channels: usize,
}

enum IccTransform {
    Eight(Arc<Transform8BitExecutor>),
    /// Deeper files stay 16-bit up to the output and round only once.
    Sixteen(Arc<Transform16BitExecutor>),
}

impl IccToTarget {
    /// `None` for XYB, for a profile that `moxcms` cannot read and for a PQ or HLG profile.
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
        // moxcms does not tone-map. An RGB file with such a profile takes `HdrSignal` first.
        // jxl-oxide 0.12 does not tone-map gray or XYB with such a profile either.
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

    /// The rendered samples and the output exist at the same time.
    fn bytes_per_pixel(&self) -> usize {
        let sample_bytes = match self.transform {
            IccTransform::Eight(_) => 1,
            IccTransform::Sixteen(_) => 2,
        };
        self.input_channels * sample_bytes + self.channels
    }

    /// Render the first frame in the profile space and convert it to 8 bits in the target.
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

fn to_8bit(sample: u16) -> u8 {
    u8::try_from((u32::from(sample) * 255 + 32_767) / 65_535).unwrap_or(u8::MAX)
}

/// The upright size from the header.
pub(crate) fn dimensions(path: &Path) -> Option<(u32, u32)> {
    let image = open(path).ok()?;
    Some((image.width(), image.height()))
}

/// The TIFF bytes of the Exif box.
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
        HdrSignal, IccToTarget, decode, decode_enum, decode_icc_or_enum, dimensions,
        open_for_render,
    };
    use crate::decode::color::{ColorTarget, SourceColor};
    use crate::decode::{EmbeddedThumbnail, decode_image_with_hint};
    use crate::test_support::{assert_close, fixture};
    use image::{DynamicImage, RgbaImage};

    /// The largest difference in 8-bit levels from a djxl patch.
    const REFERENCE_TOLERANCE: f32 = 4.0;

    fn decode_fixture(name: &str) -> DynamicImage {
        let path = fixture(&format!("jxl/{name}.jxl"));
        decode_image_with_hint(&path, None, EmbeddedThumbnail::Deny).unwrap()
    }

    /// The mean of the 4x4 center of each 8x8 patch. The center skips the lossy edge ringing.
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
    fn fixtures_match_libjxl() {
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
        assert_eq!(checked, 14, "the fixtures with a reference");
    }

    #[test]
    fn icc_v2_keeps_srgb_blue() {
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
    fn sixteen_bit_icc_rounds_once() {
        // The gradient climbs 16 levels over 64 pixels. A transform of 8-bit samples rounds twice
        // and skips levels (the red ramp jumps from 94 to 96). djxl dithers and is no reference per
        // pixel.
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
    fn hdr_keeps_signal_for_tone_map() {
        let open = |name: &str| open_for_render(&fixture(&format!("jxl/{name}.jxl"))).unwrap();
        for (name, transfer) in [("pq", 16), ("hlg", 18), ("pq-icc", 16), ("hlg-icc", 18)] {
            let image = open(name);
            let format = image.pixel_format();
            assert!(IccToTarget::for_image(&image, format, ColorTarget::Srgb).is_none());
            let signal = HdrSignal::for_image(&image, format).unwrap();
            assert_eq!(
                signal.color,
                SourceColor::Cicp {
                    primaries: 9,
                    transfer
                },
                "{name}"
            );
            assert_eq!(signal.encoding.is_some(), !name.ends_with("-icc"), "{name}");
            let decoded = decode(&fixture(&format!("jxl/{name}.jxl")), ColorTarget::Srgb).unwrap();
            assert!(
                matches!(decoded.image, DynamicImage::ImageRgb16(_)),
                "{name}"
            );
        }
        let image = open("adobe-rgb-icc");
        assert!(IccToTarget::for_image(&image, image.pixel_format(), ColorTarget::Srgb).is_some());
        assert!(HdrSignal::for_image(&image, image.pixel_format()).is_none());
    }

    #[test]
    fn failed_icc_uses_jxl_oxide() {
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
    fn codestream_orientation_applies() {
        // Orientation 6 turns the 64x32 chart clockwise: the stored top row becomes the right
        // column.
        for name in ["orientation-6", "jpeg-orientation-6"] {
            let path = fixture(&format!("jxl/{name}.jxl"));
            let decoded = decode(&path, ColorTarget::Srgb).unwrap().image.to_rgba8();
            assert_eq!(decoded.dimensions(), (32, 64), "{name}");
            assert_eq!(dimensions(&path), Some((32, 64)), "{name}");
            assert_eq!(crate::decode::shape::aspect_of(&path), Some(0.5), "{name}");
            assert_close(decoded.get_pixel(28, 4).0, [255, 0, 0]);
            assert_close(decoded.get_pixel(28, 60).0, [0, 0, 0]);
            assert_close(decoded.get_pixel(4, 4).0, [193, 90, 99]);
        }
    }

    #[test]
    fn hdr_gray_ramps_stay_neutral() {
        // viewr puts SDR white at 500 nits, like the player. libjxl puts it at about 250 nits in
        // pq.
        for name in ["pq", "hlg", "pq-icc", "hlg-icc"] {
            assert_gray_ramp(name);
        }
    }

    /// The lossy files have noise near black, where the output curve is steep.
    fn assert_gray_ramp(name: &str) {
        let means = patch_means(&decode_fixture(name).to_rgba8());
        let ramp = &means[8..16];
        for gray in ramp {
            let spread = gray[..3]
                .iter()
                .fold(0.0f32, |high, &value| high.max(value))
                - gray[..3]
                    .iter()
                    .fold(255.0f32, |low, &value| low.min(value));
            assert!(spread <= 3.0, "{name}: gray {gray:?}");
        }
        assert!(
            ramp.windows(2).all(|pair| pair[1][0] > pair[0][0]),
            "{name}: {ramp:?}"
        );
        assert!(ramp[0][0] <= 4.0 && ramp[7][0] >= 160.0, "{name}: {ramp:?}");
        let red = means[0];
        assert!(
            red[0] > red[1] + 60.0 && red[0] > red[2] + 60.0,
            "{name}: red {red:?}"
        );
    }

    #[test]
    fn broken_file_reports_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.jxl");
        std::fs::write(&path, b"this is not a JPEG XL file").unwrap();
        assert!(decode(&path, ColorTarget::Srgb).is_err());
        assert!(dimensions(&path).is_none());
        assert!(decode_image_with_hint(&path, None, EmbeddedThumbnail::Deny).is_err());
    }

    #[test]
    fn enum_file_renders_into_display_p3() {
        let path = fixture("jxl/p3-enum.jxl");
        let p3 = decode(&path, ColorTarget::DisplayP3).unwrap();
        assert_eq!(p3.color, SourceColor::DisplayP3);
        let srgb = decode(&path, ColorTarget::Srgb).unwrap().image.to_rgba8();
        let back = crate::decode::color::convert(p3.image.to_rgba8(), &p3.color, ColorTarget::Srgb);
        let total: u64 = back
            .as_raw()
            .iter()
            .zip(srgb.as_raw())
            .map(|(&a, &b)| u64::from(a.abs_diff(b)))
            .sum();
        let mean = total / back.as_raw().len() as u64;
        assert!(mean <= 1, "mean difference {mean}");
    }

    #[test]
    fn exif_box_reaches_info_panel() {
        let data = super::exif(&fixture("jxl/jpeg-orientation-6.jxl")).unwrap();
        let exif = exif::Reader::new().read_raw(data).unwrap();
        let orientation = exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY);
        assert_eq!(
            orientation.and_then(|field| field.value.get_uint(0)),
            Some(6)
        );
        assert_eq!(super::exif(&fixture("jxl/srgb-lossless.jxl")), None);
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
