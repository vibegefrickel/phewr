//! The color conversion of a decoded image into the color space of the terminal.
//!
//! Ghostty on macOS draws into a Display P3 surface. Its image shader copies the pixels of a
//! Kitty image without a conversion, also with `window-colorspace = srgb`, which converts only
//! the text colors. So Ghostty shows each image pixel as Display P3, and viewr converts every
//! image into Display P3 (`ColorTarget`). `[terminal] colorspace = "srgb"` is for a terminal that
//! shows the pixels as sRGB.
//!
//! Each decoder reports the color space of its pixels (`SourceColor`): an ICC profile, the CICP
//! code points of a HEIF `nclx` box or a PNG `cICP` chunk, or the `gAMA` and `cHRM` chunks of a
//! PNG. An image without them is sRGB. The conversion runs after the resize, on the small
//! image, as one `moxcms` transform in parallel rows. A source in the target space converts
//! nothing.
//!
//! # From the AGENTS.md gotcha "Image colors"
//!
//! Ghostty on macOS draws into a Display P3 surface, and its image shader copies the pixels of a
//! Kitty image without a conversion, also with `window-colorspace = srgb` (that setting converts
//! only the text colors). So viewr converts each image into Display P3 (`[terminal] colorspace`,
//! default `display-p3`, or `srgb`). Each decoder reports the color of its pixels: ICC from JPEG
//! APP2 (also over several segments), PNG iCCP, WebP ICCP, TIFF tag 34675 and HEIF `prof`/`rICC`,
//! CICP from HEIF `nclx` and PNG `cICP`, PNG `gAMA`/`cHRM`, and the Adobe RGB mark of a camera
//! JPEG without a profile (EXIF `ColorSpace` 0xFFFF with the interoperability index `R03`,
//! `color::jpeg_color`). The conversion runs after the resize, in the layout of the image (RGB
//! through `color::convert_rgb`, or RGBA), as one cached `moxcms` transform with the rows in
//! parallel. A source in the target space converts nothing, so iPhone photos (Display P3) cost
//! nothing. A source that cannot convert (an unreadable or non-RGB ICC profile, unknown CICP
//! codes) converts as sRGB. PQ and HLG sources keep their values. `main` sets the target, and
//! tests keep sRGB. The terminal player converts with a `lut3d` cube (`player/tone_map.rs`): an
//! SDR video or animation gets a 17-point cube from its `color_primaries` (`Primaries::of_name`:
//! BT.709, SMPTE 170M, BT.470 BG, Display P3, DCI-P3 with a Bradford adaptation, BT.2020) into the
//! target. An untagged stream and GIF count as BT.709 with the sRGB curve. ffprobe reports no
//! profile for WebP and APNG, so the probe reads their ICC profile or cICP chunk
//! (`animation::embedded_primaries`) and maps a profile with the colorants of sRGB, Display P3 or
//! BT.2020 to its primaries (`color::icc_known_space`). Any other profile counts as BT.709. A
//! source in the target space gets no cube. The SDR cube runs in 8 bit (`rgb24`, `rgba`), and its
//! values hold half a level, because `lut3d` truncates to 8 bit. A real ffmpeg test fails if
//! `lut3d` starts to round. At 1080p, the SDR cube costs 1.4 ms to 2 ms per frame.

use image::{RgbImage, RgbaImage};
use moxcms::{
    Chromaticity, CicpColorPrimaries, CicpProfile, ColorPrimaries, ColorProfile, DataColorSpace,
    Layout, MatrixCoefficients, RenderingIntent, ToneReprCurve, TransferCharacteristics,
    Transform8BitExecutor, TransformOptions, XyY,
};
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// The pixels of one task of the parallel conversion: 256 KB of RGBA.
const PIXELS_PER_TASK: usize = 64 * 1024;

/// The largest number of transforms that the cache keeps. A folder of one camera needs one or
/// two.
const MAX_CACHED_TRANSFORMS: usize = 64;

/// The color space in which the terminal shows the pixels of an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) enum ColorTarget {
    /// Ghostty on macOS: the pixels are Display P3.
    #[default]
    DisplayP3,
    /// A terminal that shows the pixels as sRGB.
    Srgb,
}

impl ColorTarget {
    /// Read the config value. An unknown value returns `None`, so the rest of the file still
    /// counts.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "display-p3" | "p3" => Some(Self::DisplayP3),
            "srgb" => Some(Self::Srgb),
            _ => None,
        }
    }

    /// The `moxcms` profile of the target.
    pub(crate) fn profile(self) -> ColorProfile {
        match self {
            Self::DisplayP3 => ColorProfile::new_display_p3(),
            Self::Srgb => ColorProfile::new_srgb(),
        }
    }
}

/// True when the decodes convert into Display P3. `main` sets it from the config before the
/// first decode. Until then, and in the tests, the target is sRGB, so that an image without a
/// color profile keeps its values.
static DISPLAY_P3: AtomicBool = AtomicBool::new(false);

/// Set the target of all later decodes.
pub(crate) fn set_target(target: ColorTarget) {
    DISPLAY_P3.store(target == ColorTarget::DisplayP3, Ordering::Relaxed);
}

/// The target of the decodes.
pub(crate) fn target() -> ColorTarget {
    if DISPLAY_P3.load(Ordering::Relaxed) {
        ColorTarget::DisplayP3
    } else {
        ColorTarget::Srgb
    }
}

/// The color space of the pixels that a decoder returns.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub(crate) enum SourceColor {
    /// sRGB, or no color information.
    #[default]
    Srgb,
    /// Display P3: the target of a decoder that converts itself.
    DisplayP3,
    /// Adobe RGB (1998) without a profile: a camera JPEG that marks it in its EXIF block.
    AdobeRgb,
    /// An ICC profile.
    Icc(Arc<[u8]>),
    /// The CICP code points (ITU-T H.273) of a HEIF `nclx` box or a PNG `cICP` chunk.
    Cicp { primaries: u8, transfer: u8 },
    /// The `gAMA` and `cHRM` chunks of a PNG, in units of 1/100000. The chromaticities are
    /// white x, white y, red x, red y, green x, green y, blue x, blue y.
    PngGamma {
        gamma: Option<u32>,
        chromaticities: Option<[u32; 8]>,
    },
}

impl SourceColor {
    /// The space of the pixels of a decoder that converts into `target` itself.
    pub(crate) fn of_target(target: ColorTarget) -> Self {
        match target {
            ColorTarget::DisplayP3 => Self::DisplayP3,
            ColorTarget::Srgb => Self::Srgb,
        }
    }

    /// An ICC profile. An empty profile is no profile.
    pub(crate) fn icc(data: Vec<u8>) -> Self {
        if data.is_empty() {
            Self::Srgb
        } else {
            Self::Icc(data.into())
        }
    }
}

/// Convert `image` from `source` into `target`. A source in the target space, and a source that
/// `moxcms` cannot read, keep their values.
pub(crate) fn convert(
    mut image: RgbaImage,
    source: &SourceColor,
    target: ColorTarget,
) -> RgbaImage {
    let width = image.width();
    convert_pixels(&mut image, width, Channels::Rgba, source, target);
    image
}

/// Convert the RGB `image` as [`convert`] does, without an alpha channel. An opaque image
/// keeps 3 bytes per pixel from the decode to the terminal.
pub(crate) fn convert_rgb(
    mut image: RgbImage,
    source: &SourceColor,
    target: ColorTarget,
) -> RgbImage {
    let width = image.width();
    convert_pixels(&mut image, width, Channels::Rgb, source, target);
    image
}

/// Convert the packed `pixels` of an image `width` pixels wide in parallel rows.
fn convert_pixels(
    pixels: &mut [u8],
    width: u32,
    channels: Channels,
    source: &SourceColor,
    target: ColorTarget,
) {
    let Some(transform) = transform_for(source, target, channels) else {
        return;
    };
    let rows = (PIXELS_PER_TASK / width.max(1) as usize).max(1);
    let chunk = rows * width as usize * channels.count();
    let result = pixels.par_chunks_mut(chunk).try_for_each(|pixels| {
        let source = pixels.to_vec();
        transform.transform(&source, pixels)
    });
    // The chunks hold whole pixels, so the transform cannot fail on their length. A failed
    // chunk keeps its values.
    #[expect(
        clippy::let_underscore_must_use,
        reason = "a failed chunk keeps its source values, which is the best picture left"
    )]
    let _ = result;
}

/// True when `source` needs a conversion into `target`, for the tests and the bench.
#[cfg(test)]
pub(crate) fn converts(source: &SourceColor, target: ColorTarget) -> bool {
    transform(source, target).is_some()
}

/// The pixel layout of a transform: 3 or 4 bytes per pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Channels {
    Rgb,
    Rgba,
}

impl Channels {
    /// The `moxcms` layout of the source and of the target pixels.
    fn layout(self) -> Layout {
        match self {
            Self::Rgb => Layout::Rgb,
            Self::Rgba => Layout::Rgba,
        }
    }

    /// The bytes per pixel, for the size of a parallel task.
    fn count(self) -> usize {
        match self {
            Self::Rgb => 3,
            Self::Rgba => 4,
        }
    }
}

/// The built transforms, and `None` for a source that converts nothing.
type TransformCache =
    HashMap<(SourceColor, ColorTarget, Channels), Option<Arc<Transform8BitExecutor>>>;

static TRANSFORMS: OnceLock<Mutex<TransformCache>> = OnceLock::new();

/// The RGBA transform of the source into the target, from the cache. `None` when the source is
/// the target or cannot convert. The tests read it.
#[cfg(test)]
fn transform(source: &SourceColor, target: ColorTarget) -> Option<Arc<Transform8BitExecutor>> {
    transform_for(source, target, Channels::Rgba)
}

/// The transform for `channels`. The RGBA transform decides whether a source converts, so
/// the RGB and the RGBA image of one source always get the same answer.
fn transform_for(
    source: &SourceColor,
    target: ColorTarget,
    channels: Channels,
) -> Option<Arc<Transform8BitExecutor>> {
    if *source == SourceColor::of_target(target) {
        return None;
    }
    let rgba = cached_transform(source, target, Channels::Rgba);
    match channels {
        Channels::Rgba => rgba,
        Channels::Rgb => rgba.and_then(|_| cached_transform(source, target, Channels::Rgb)),
    }
}

/// The transform of the key from the cache, built at the first request.
fn cached_transform(
    source: &SourceColor,
    target: ColorTarget,
    channels: Channels,
) -> Option<Arc<Transform8BitExecutor>> {
    let cache = TRANSFORMS.get_or_init(Mutex::default);
    let key = (source.clone(), target, channels);
    let cached = cache
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
        .cloned();
    if let Some(cached) = cached {
        return cached;
    }
    // Two threads can build the same transform at once. Both results are equal.
    let built = build_transform(source, target, channels);
    let mut cache = cache.lock().unwrap_or_else(PoisonError::into_inner);
    if cache.len() >= MAX_CACHED_TRANSFORMS {
        cache.clear();
    }
    cache.insert(key, built.clone());
    built
}

/// Build the transform, and drop an RGBA transform that changes no value: an sRGB ICC profile
/// into sRGB, a Display P3 profile into Display P3. A profile that `moxcms` cannot convert from
/// RGB, for example a CMYK profile on a decoded RGB JPEG, converts as sRGB: the values unchanged
/// would show too saturated in a Display P3 terminal. An RGB transform is built only after the
/// RGBA transform of its source passed the identity check.
fn build_transform(
    source: &SourceColor,
    target: ColorTarget,
    channels: Channels,
) -> Option<Arc<Transform8BitExecutor>> {
    let options = TransformOptions {
        rendering_intent: RenderingIntent::RelativeColorimetric,
        ..TransformOptions::default()
    };
    let destination = target.profile();
    let layout = channels.layout();
    let create = |profile: &ColorProfile| {
        profile.create_transform_8bit(layout, &destination, layout, options)
    };
    let transform = create(&source_profile(source)?)
        .or_else(|_| create(&ColorProfile::new_srgb()))
        .ok()?;
    (channels == Channels::Rgb || !is_identity(transform.as_ref())).then_some(transform)
}

/// True when `transform` moves no sample of a 9x9x9 grid of the RGB cube by more than 1.
fn is_identity(transform: &Transform8BitExecutor) -> bool {
    let levels = (0..=8).map(|step| u8::try_from(step * 32).unwrap_or(u8::MAX));
    let probe: Vec<u8> = levels
        .clone()
        .flat_map(|r| {
            let levels = levels.clone();
            levels
                .clone()
                .flat_map(move |g| levels.clone().map(move |b| [r, g, b, 255]))
        })
        .flatten()
        .collect();
    let mut out = vec![0; probe.len()];
    transform.transform(&probe, &mut out).is_ok()
        && probe.iter().zip(&out).all(|(a, b)| a.abs_diff(*b) <= 1)
}

/// The `moxcms` profile of a source. A profile that `moxcms` cannot read is sRGB. `None` only
/// for HDR: `moxcms` does not tone-map PQ and HLG, so these values stay as they are.
fn source_profile(source: &SourceColor) -> Option<ColorProfile> {
    let profile = match source {
        SourceColor::Srgb => ColorProfile::new_srgb(),
        SourceColor::DisplayP3 => ColorProfile::new_display_p3(),
        SourceColor::AdobeRgb => ColorProfile::new_adobe_rgb(),
        // The decoders give RGB values. moxcms reads RGBA samples as CMYK for a CMYK
        // profile, so a profile of another color space counts as sRGB too.
        SourceColor::Icc(data) => ColorProfile::new_from_slice(data)
            .ok()
            .filter(|profile| profile.color_space == DataColorSpace::Rgb)
            .unwrap_or_else(ColorProfile::new_srgb),
        SourceColor::Cicp {
            primaries,
            transfer,
        } => cicp_profile(*primaries, *transfer)?,
        SourceColor::PngGamma {
            gamma,
            chromaticities,
        } => png_profile(*gamma, chromaticities.as_ref()),
    };
    let hdr = profile.cicp.as_ref().is_some_and(|cicp| {
        matches!(
            cicp.transfer_characteristics,
            TransferCharacteristics::Smpte2084 | TransferCharacteristics::Hlg
        )
    });
    (!hdr).then_some(profile)
}

/// The profile of the CICP code points. The SDR curves of video (BT.709, BT.601, BT.2020)
/// decode with the sRGB curve: a still image with these codes is a photo for an sRGB display,
/// and the curves differ only in the shadows. An unknown or unspecified curve is the sRGB
/// curve, unknown or unspecified primaries are BT.709. `None` for PQ (16) and HLG (18).
fn cicp_profile(primaries: u8, transfer: u8) -> Option<ColorProfile> {
    let transfer = match transfer {
        16 | 18 => return None,
        4 => TransferCharacteristics::Bt470M,
        5 => TransferCharacteristics::Bt470Bg,
        8 => TransferCharacteristics::Linear,
        _ => TransferCharacteristics::Srgb,
    };
    let primaries = match primaries {
        1 | 4..=7 | 9..=12 | 22 => {
            CicpColorPrimaries::try_from(primaries).unwrap_or(CicpColorPrimaries::Bt709)
        }
        _ => CicpColorPrimaries::Bt709,
    };
    Some(ColorProfile::new_from_cicp(CicpProfile {
        color_primaries: primaries,
        transfer_characteristics: transfer,
        matrix_coefficients: MatrixCoefficients::Bt709,
        full_range: true,
    }))
}

/// The profile of the `gAMA` and `cHRM` chunks of a PNG. A missing chunk keeps the value of
/// sRGB.
fn png_profile(gamma: Option<u32>, chromaticities: Option<&[u32; 8]>) -> ColorProfile {
    let mut profile = ColorProfile::new_srgb();
    if let Some(&[wx, wy, rx, ry, gx, gy, bx, by]) = chromaticities {
        let xy = |x: u32, y: u32| Chromaticity::new(x as f32 / 1e5, y as f32 / 1e5);
        profile.update_rgb_colorimetry(
            XyY::new(f64::from(wx) / 1e5, f64::from(wy) / 1e5, 1.0),
            ColorPrimaries {
                red: xy(rx, ry),
                green: xy(gx, gy),
                blue: xy(bx, by),
            },
        );
    }
    if let Some(gamma) = gamma.filter(|&gamma| gamma > 0) {
        // gAMA holds the encode exponent: 45455 is 1/2.2.
        let curve = ToneReprCurve::Parametric(vec![1e5 / gamma as f32]);
        profile.red_trc = Some(curve.clone());
        profile.green_trc = Some(curve.clone());
        profile.blue_trc = Some(curve);
    }
    profile.cicp = None;
    profile
}

/// The color chunks of the PNG at `path` that `image` does not read: `cICP`, `sRGB`, `gAMA` and
/// `cHRM`. They count in the order of the PNG specification: `cICP`, then `iCCP`, which
/// `image` reads, then `sRGB`, then `gAMA` and `cHRM`. `None` when the file has none of them,
/// or when the `iCCP` chunk decides.
pub(crate) fn png_color(path: &Path) -> Option<SourceColor> {
    let mut file = BufReader::new(File::open(path).ok()?);
    let mut signature = [0; 8];
    file.read_exact(&mut signature).ok()?;
    let (mut icc, mut srgb, mut gamma, mut chromaticities) = (false, false, None, None);
    let mut header = [0; 8];
    while file.read_exact(&mut header).is_ok() {
        let [l0, l1, l2, l3, k0, k1, k2, k3] = header;
        let length = u32::from_be_bytes([l0, l1, l2, l3]);
        let kind = [k0, k1, k2, k3];
        if matches!(&kind, b"IDAT" | b"IEND") {
            break;
        }
        if !matches!(&kind, b"cICP" | b"sRGB" | b"gAMA" | b"cHRM") || length > 32 {
            icc |= &kind == b"iCCP";
            file.seek_relative(i64::from(length) + 4).ok()?;
            continue;
        }
        let mut body = vec![0; length as usize + 4];
        file.read_exact(&mut body).ok()?;
        let values: Vec<u32> = body
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&value| u32::from_be_bytes(value))
            .collect();
        match &kind {
            b"cICP" => {
                return Some(SourceColor::Cicp {
                    primaries: *body.first()?,
                    transfer: *body.get(1)?,
                });
            }
            b"sRGB" => srgb = true,
            b"gAMA" => gamma = values.first().copied(),
            _ => chromaticities = values.get(..8).and_then(|v| v.try_into().ok()),
        }
    }
    if icc {
        return None;
    }
    if srgb {
        return Some(SourceColor::Srgb);
    }
    (gamma.is_some() || chromaticities.is_some()).then_some(SourceColor::PngGamma {
        gamma,
        chromaticities,
    })
}

/// The largest ICC profile that `tiff_icc_profile` reads.
const MAX_ICC_BYTES: u32 = 16 * 1024 * 1024;

/// The ICC profile of the first image of the TIFF file at `path` (tag 34675). `image` 0.25
/// does not read it: its TIFF decoder asks for the type BYTE, and the tag has the type
/// UNDEFINED.
pub(crate) fn tiff_icc_profile(path: &Path) -> Option<Vec<u8>> {
    let mut file = BufReader::new(File::open(path).ok()?);
    let mut header = [0; 8];
    file.read_exact(&mut header).ok()?;
    let big_endian = match header {
        [b'I', b'I', 42, 0, ..] => false,
        [b'M', b'M', 0, 42, ..] => true,
        _ => return None,
    };
    let u16_of = |b: [u8; 2]| {
        if big_endian {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        }
    };
    let u32_of = |b: [u8; 4]| {
        if big_endian {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        }
    };
    let [_, _, _, _, o0, o1, o2, o3] = header;
    file.seek(SeekFrom::Start(u64::from(u32_of([o0, o1, o2, o3]))))
        .ok()?;
    let mut count = [0; 2];
    file.read_exact(&mut count).ok()?;
    for _ in 0..u16_of(count) {
        let mut entry = [0; 12];
        file.read_exact(&mut entry).ok()?;
        let [t0, t1, _, _, c0, c1, c2, c3, v0, v1, v2, v3] = entry;
        if u16_of([t0, t1]) != 34_675 {
            continue;
        }
        let length = u32_of([c0, c1, c2, c3]);
        // A profile has a header of 128 bytes, so it never fits into the entry.
        if !(128..=MAX_ICC_BYTES).contains(&length) {
            return None;
        }
        file.seek(SeekFrom::Start(u64::from(u32_of([v0, v1, v2, v3]))))
            .ok()?;
        let mut profile = vec![0; length as usize];
        file.read_exact(&mut profile).ok()?;
        return Some(profile);
    }
    None
}

/// The known RGB space whose colorants the ICC profile `data` has: sRGB, Display P3 or
/// BT.2020 (as CICP 9 with the sRGB curve). The terminal player has a color cube for each of
/// them. `None` for another profile.
#[cfg(feature = "video")]
pub(crate) fn icc_known_space(data: &[u8]) -> Option<SourceColor> {
    let profile = ColorProfile::new_from_slice(data).ok()?;
    let close = |known: &ColorProfile| {
        let near = |a: moxcms::Xyzd, b: moxcms::Xyzd| {
            (a.x - b.x).abs() < 0.003 && (a.y - b.y).abs() < 0.003 && (a.z - b.z).abs() < 0.003
        };
        near(profile.red_colorant, known.red_colorant)
            && near(profile.green_colorant, known.green_colorant)
            && near(profile.blue_colorant, known.blue_colorant)
    };
    if close(&ColorProfile::new_srgb()) {
        Some(SourceColor::Srgb)
    } else if close(&ColorProfile::new_display_p3()) {
        Some(SourceColor::DisplayP3)
    } else if close(&ColorProfile::new_bt2020()) {
        Some(SourceColor::Cicp {
            primaries: 9,
            transfer: 13,
        })
    } else {
        None
    }
}

/// The color of the JPEG `data`: its ICC profile, else Adobe RGB when the EXIF block marks it,
/// else sRGB. Cameras in the Adobe RGB mode often write no profile, only the EXIF mark.
#[cfg(any(feature = "turbo", test))]
pub(crate) fn jpeg_color(data: &[u8]) -> SourceColor {
    if let Some(icc) = jpeg_icc_profile(data) {
        return SourceColor::icc(icc);
    }
    if exif_marks_adobe_rgb(data) {
        SourceColor::AdobeRgb
    } else {
        SourceColor::Srgb
    }
}

/// True when the EXIF block of the JPEG `data` marks Adobe RGB, as the DCF standard defines
/// it: `ColorSpace` 0xFFFF (uncalibrated) and the interoperability index `R03`.
pub(crate) fn exif_marks_adobe_rgb(data: &[u8]) -> bool {
    let Some(payload) = jpeg_exif_payload(data) else {
        return false;
    };
    let Ok(exif) = exif::Reader::new().read_raw(payload.to_vec()) else {
        return false;
    };
    let uncalibrated = exif
        .get_field(exif::Tag::ColorSpace, exif::In::PRIMARY)
        .and_then(|field| field.value.get_uint(0))
        == Some(0xFFFF);
    let option_file = exif
        .get_field(exif::Tag::InteroperabilityIndex, exif::In::PRIMARY)
        .is_some_and(|field| {
            matches!(&field.value, exif::Value::Ascii(values)
                if values.first().is_some_and(|value| value.starts_with(b"R03")))
        });
    uncalibrated && option_file
}

/// The marker segments of the JPEG `data` before the entropy-coded data, as `(marker, payload)`.
/// A segment that the data cuts yields one `None` and ends the walk.
fn jpeg_segments(data: &[u8]) -> impl Iterator<Item = Option<(u8, &[u8])>> {
    let mut pos = 2;
    let mut done = false;
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        let &[0xFF, marker, length_high, length_low, ..] = data.get(pos..)? else {
            return None;
        };
        // SOS starts the entropy-coded data, EOI ends the file: no more APP segments.
        if matches!(marker, 0xDA | 0xD9) {
            return None;
        }
        let length = usize::from(u16::from_be_bytes([length_high, length_low]));
        let segment = data.get(pos + 4..pos + 2 + length);
        done = segment.is_none();
        pos += 2 + length;
        Some(segment.map(|segment| (marker, segment)))
    })
}

/// The TIFF data of the first `Exif` APP1 segment of the JPEG `data`.
fn jpeg_exif_payload(data: &[u8]) -> Option<&[u8]> {
    for segment in jpeg_segments(data) {
        let (marker, segment) = segment?;
        if marker == 0xE1
            && let Some(payload) = segment.strip_prefix(b"Exif\0\0")
        {
            return Some(payload);
        }
    }
    None
}

/// The ICC profile of the JPEG `data`: the `ICC_PROFILE` APP2 segments in the order of their
/// sequence numbers. A profile larger than one segment (64 KB) spans several. `None` without
/// a profile, or when a segment is missing.
#[cfg(any(feature = "turbo", test))]
pub(crate) fn jpeg_icc_profile(data: &[u8]) -> Option<Vec<u8>> {
    const SIGNATURE: &[u8] = b"ICC_PROFILE\0";
    let mut chunks: Vec<(u8, u8, &[u8])> = Vec::new();
    for segment in jpeg_segments(data) {
        let (marker, segment) = segment?;
        if marker == 0xE2
            && let Some(rest) = segment.strip_prefix(SIGNATURE)
            && let [sequence, count, chunk @ ..] = rest
        {
            chunks.push((*sequence, *count, chunk));
        }
    }
    chunks.sort_by_key(|&(sequence, _, _)| sequence);
    let count = chunks.first()?.1;
    let complete = usize::from(count) == chunks.len()
        && chunks
            .iter()
            .zip(1..)
            .all(|(&(sequence, total, _), expected)| sequence == expected && total == count);
    complete.then(|| {
        chunks
            .iter()
            .flat_map(|&(_, _, chunk)| chunk)
            .copied()
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ColorTarget, SourceColor, convert, exif_marks_adobe_rgb, jpeg_color, jpeg_exif_payload,
        jpeg_icc_profile, png_color, tiff_icc_profile, transform,
    };
    use crate::decode::{EmbeddedThumbnail, decode_image};
    use crate::test_support::{fixture, test_dir};
    use image::{Rgba, RgbaImage};

    const P3_ICC: &str = "/System/Library/ColorSync/Profiles/Display P3.icc";

    /// One pixel of `rgba`, converted.
    fn pixel(rgba: [u8; 4], source: &SourceColor, target: ColorTarget) -> [u8; 4] {
        let image = RgbaImage::from_pixel(1, 1, Rgba(rgba));
        convert(image, source, target).get_pixel(0, 0).0
    }

    fn assert_near(actual: [u8; 4], expected: [u8; 4]) {
        let near = actual.iter().zip(expected).all(|(a, e)| a.abs_diff(e) <= 1);
        assert!(near, "{actual:?} is not near {expected:?}");
    }

    #[test]
    fn srgb_red_turns_into_display_p3_red() {
        // The Display P3 values of sRGB red, as ColorSync gives them.
        let red = pixel([255, 0, 0, 255], &SourceColor::Srgb, ColorTarget::DisplayP3);
        assert_near(red, [234, 51, 35, 255]);
    }

    #[test]
    fn the_target_space_keeps_every_value() {
        assert!(transform(&SourceColor::Srgb, ColorTarget::Srgb).is_none());
        assert!(transform(&SourceColor::DisplayP3, ColorTarget::DisplayP3).is_none());
        let p3 = SourceColor::Cicp {
            primaries: 12,
            transfer: 13,
        };
        assert!(transform(&p3, ColorTarget::DisplayP3).is_none());
        let unspecified = SourceColor::Cicp {
            primaries: 2,
            transfer: 2,
        };
        assert!(transform(&unspecified, ColorTarget::Srgb).is_none());
    }

    #[test]
    fn a_display_p3_icc_profile_into_display_p3_converts_nothing() {
        let Ok(icc) = std::fs::read(P3_ICC) else {
            return;
        };
        assert!(transform(&SourceColor::icc(icc), ColorTarget::DisplayP3).is_none());
    }

    #[test]
    fn display_p3_green_clips_in_srgb() {
        let green = pixel([0, 255, 0, 128], &SourceColor::DisplayP3, ColorTarget::Srgb);
        assert_eq!(green[3], 128, "alpha stays");
        assert!(green[0] == 0 && green[1] == 255, "{green:?}");
    }

    #[test]
    fn a_source_that_cannot_convert_converts_as_srgb() {
        let srgb_red = pixel([255, 0, 0, 255], &SourceColor::Srgb, ColorTarget::DisplayP3);
        let broken = SourceColor::icc(vec![1, 2, 3]);
        assert_eq!(
            pixel([255, 0, 0, 255], &broken, ColorTarget::DisplayP3),
            srgb_red
        );
        let cmyk = std::fs::read("/System/Library/ColorSync/Profiles/Generic CMYK Profile.icc");
        if let Ok(cmyk) = cmyk {
            let red = pixel(
                [255, 0, 0, 255],
                &SourceColor::icc(cmyk),
                ColorTarget::DisplayP3,
            );
            assert_eq!(red, srgb_red, "a CMYK profile on RGB values");
        }
        for (primaries, transfer) in [(2, 3), (3, 13), (200, 200)] {
            let unknown = SourceColor::Cicp {
                primaries,
                transfer,
            };
            let red = pixel([255, 0, 0, 255], &unknown, ColorTarget::DisplayP3);
            assert_near(red, srgb_red);
        }
    }

    #[test]
    fn hdr_cicp_codes_keep_their_values() {
        let pq = SourceColor::Cicp {
            primaries: 9,
            transfer: 16,
        };
        assert!(transform(&pq, ColorTarget::DisplayP3).is_none());
        let hlg = SourceColor::Cicp {
            primaries: 9,
            transfer: 18,
        };
        assert!(transform(&hlg, ColorTarget::Srgb).is_none());
    }

    #[test]
    fn a_png_gamma_of_one_over_two_point_two_is_close_to_srgb() {
        let gamma = SourceColor::PngGamma {
            gamma: Some(45_455),
            chromaticities: None,
        };
        let gray = pixel([128, 128, 128, 255], &gamma, ColorTarget::Srgb);
        assert!(gray[0].abs_diff(128) <= 3, "{gray:?}");
        let linear = SourceColor::PngGamma {
            gamma: Some(100_000),
            chromaticities: None,
        };
        let brighter = pixel([128, 128, 128, 255], &linear, ColorTarget::Srgb);
        assert!(brighter[0] > 180, "linear 0.5 is sRGB 188: {brighter:?}");
    }

    #[test]
    fn png_chromaticities_of_display_p3_match_the_display_p3_profile() {
        let p3 = SourceColor::PngGamma {
            gamma: None,
            chromaticities: Some([
                31_270, 32_900, 68_000, 32_000, 26_500, 69_000, 15_000, 6_000,
            ]),
        };
        let red = pixel([255, 0, 0, 255], &p3, ColorTarget::Srgb);
        let expected = pixel([255, 0, 0, 255], &SourceColor::DisplayP3, ColorTarget::Srgb);
        assert_near(red, expected);
    }

    /// A JPEG header with the APP2 segments `chunks`, each as (sequence, count, bytes).
    fn jpeg_with_icc(chunks: &[(u8, u8, &[u8])]) -> Vec<u8> {
        let mut data = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0];
        for &(sequence, count, chunk) in chunks {
            let length = u16::try_from(2 + 12 + 2 + chunk.len()).unwrap();
            data.extend([0xFF, 0xE2]);
            data.extend(length.to_be_bytes());
            data.extend(b"ICC_PROFILE\0");
            data.extend([sequence, count]);
            data.extend(chunk);
        }
        data.extend([0xFF, 0xDA, 0, 2, 0xFF, 0xD9]);
        data
    }

    #[test]
    fn a_split_icc_profile_joins_in_sequence_order() {
        let data = jpeg_with_icc(&[(2, 2, b"world"), (1, 2, b"hello ")]);
        assert_eq!(jpeg_icc_profile(&data).unwrap(), b"hello world");
        let missing = jpeg_with_icc(&[(1, 2, b"hello ")]);
        assert_eq!(jpeg_icc_profile(&missing), None);
        assert_eq!(jpeg_icc_profile(&jpeg_with_icc(&[])), None);
        assert_eq!(jpeg_icc_profile(&[0xFF, 0xD8, 0xFF]), None, "a cut file");
    }

    #[test]
    fn the_config_value_names_the_target() {
        assert_eq!(
            ColorTarget::parse(" Display-P3 "),
            Some(ColorTarget::DisplayP3)
        );
        assert_eq!(ColorTarget::parse("srgb"), Some(ColorTarget::Srgb));
        assert_eq!(ColorTarget::parse("adobe"), None);
    }

    /// The fixtures of `tests/fixtures/color/` and the name of their reference.
    fn reference_fixtures() -> Vec<(&'static str, &'static str)> {
        let mut fixtures = vec![
            ("p3-icc.jpg", "p3-icc-jpg"),
            ("p3-icc.png", "p3-icc-png"),
            ("p3-icc.webp", "p3-icc-webp"),
            ("p3-icc.tiff", "p3-icc-tiff"),
            ("p3-cicp.png", "p3-cicp-png"),
            ("p3-chrm.png", "p3-chrm-png"),
            ("adobe-rgb-icc.jpg", "adobe-rgb-icc-jpg"),
            ("srgb.jpg", "srgb-jpg"),
            ("srgb.png", "srgb-png"),
        ];
        if cfg!(feature = "heic") {
            fixtures.extend([
                ("p3-icc.heic", "p3-icc-heic"),
                ("p3-nclx.heic", "p3-nclx-heic"),
                ("p3-nclx.avif", "p3-nclx-avif"),
            ]);
        }
        fixtures
    }

    /// The mean of the 4x4 center pixels of each 8x8 patch, row by row. The center skips the
    /// ringing of a lossy patch edge.
    fn patch_means(image: &RgbaImage) -> Vec<[f64; 3]> {
        let mut means = Vec::new();
        for patch_y in (0..image.height()).step_by(8) {
            for patch_x in (0..image.width()).step_by(8) {
                let mut sum = [0.0; 3];
                for y in patch_y + 2..patch_y + 6 {
                    for x in patch_x + 2..patch_x + 6 {
                        let pixel = image.get_pixel(x, y).0;
                        for (total, value) in sum.iter_mut().zip(pixel) {
                            *total += f64::from(value) / 16.0;
                        }
                    }
                }
                means.push(sum);
            }
        }
        means
    }

    /// CIELAB (D65) of 8-bit values in `space`.
    fn lab(rgb: [f64; 3], space: ColorTarget) -> [f64; 3] {
        let matrix = match space {
            ColorTarget::Srgb => [
                [0.412_456_4, 0.357_576_1, 0.180_437_5],
                [0.212_672_9, 0.715_152_2, 0.072_175_0],
                [0.019_333_9, 0.119_192_0, 0.950_304_1],
            ],
            ColorTarget::DisplayP3 => [
                [0.486_570_9, 0.265_667_7, 0.198_217_3],
                [0.228_974_6, 0.691_738_5, 0.079_286_9],
                [0.0, 0.045_113_4, 1.043_944_4],
            ],
        };
        let linear = rgb.map(|value| {
            let v = value / 255.0;
            if v <= 0.040_45 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        });
        let white = [0.950_47, 1.0, 1.088_83];
        let f = |t: f64| {
            if t > 216.0 / 24_389.0 {
                t.cbrt()
            } else {
                (24_389.0 / 27.0 * t + 16.0) / 116.0
            }
        };
        let [x, y, z] = [0, 1, 2].map(|row| {
            let [a, b, c] = matrix[row];
            f((a * linear[0] + b * linear[1] + c * linear[2]) / white[row])
        });
        [116.0 * y - 16.0, 500.0 * (x - y), 200.0 * (y - z)]
    }

    /// The mean and the largest CIE76 color difference of the patches of two images.
    fn delta_e(image: &RgbaImage, reference: &RgbaImage, space: ColorTarget) -> (f64, f64) {
        let differences: Vec<f64> = patch_means(image)
            .into_iter()
            .zip(patch_means(reference))
            .map(|(a, b)| {
                let (a, b) = (lab(a, space), lab(b, space));
                a.iter()
                    .zip(b)
                    .map(|(a, b)| (a - b).powi(2))
                    .sum::<f64>()
                    .sqrt()
            })
            .collect();
        let mean = differences.iter().sum::<f64>() / differences.len() as f64;
        (mean, differences.into_iter().fold(0.0, f64::max))
    }

    /// Each fixture in each target matches the conversion of `ColorSync` (`sips --matchTo`).
    /// `cargo nextest run each_fixture --no-capture` prints the table: "as is" sends the decoded
    /// values without a conversion, as viewr did before.
    #[test]
    fn each_fixture_matches_the_colorsync_reference() {
        for target in [ColorTarget::DisplayP3, ColorTarget::Srgb] {
            let suffix = match target {
                ColorTarget::DisplayP3 => "p3",
                ColorTarget::Srgb => "srgb",
            };
            for (name, reference) in reference_fixtures() {
                let path = fixture(&format!("color/{name}"));
                let decoded = decode_image(&path, None, EmbeddedThumbnail::Deny).unwrap();
                let as_is = decoded.image.to_rgba8();
                let converted = convert(decoded.image.into_rgba8(), &decoded.color, target);
                let reference = fixture(&format!("color/ref/{reference}-{suffix}.png"));
                let reference = image::open(reference).unwrap().to_rgba8();
                let (before_mean, before_max) = delta_e(&as_is, &reference, target);
                let (mean, max) = delta_e(&converted, &reference, target);
                eprintln!(
                    "{suffix:>4} {name:<18} as is: mean {before_mean:5.2} max {before_max:5.2}   \
                     converted: mean {mean:5.2} max {max:5.2}"
                );
                assert!(max < 1.5, "{name} into {suffix}: max delta E {max:.2}");
            }
        }
    }

    /// An iPhone HEIC carries a Display P3 ICC profile in a `prof` box. The grid and the
    /// fullscreen view convert nothing for it in the default target.
    #[test]
    #[cfg(feature = "heic")]
    fn a_heic_file_with_a_display_p3_profile_converts_nothing_into_display_p3() {
        let path = fixture("color/p3-icc.heic");
        for hint in [None, Some((16, 16))] {
            let decoded = decode_image(&path, hint, EmbeddedThumbnail::Allow).unwrap();
            assert!(
                matches!(decoded.color, SourceColor::Icc(_)),
                "{:?}",
                decoded.color
            );
            assert!(transform(&decoded.color, ColorTarget::DisplayP3).is_none());
            assert!(transform(&decoded.color, ColorTarget::Srgb).is_some());
        }
    }

    #[test]
    fn png_color_chunks_count_in_the_order_of_the_specification() {
        let color = |name: &str| png_color(&fixture(&format!("color/{name}")));
        assert_eq!(
            color("p3-cicp.png"),
            Some(SourceColor::Cicp {
                primaries: 12,
                transfer: 13
            })
        );
        assert_eq!(color("p3-icc.png"), None, "iCCP wins over cHRM");
        assert_eq!(color("srgb.png"), None);
        let Some(SourceColor::PngGamma {
            gamma,
            chromaticities,
        }) = color("p3-chrm.png")
        else {
            panic!("p3-chrm.png has gAMA and cHRM");
        };
        assert_eq!(gamma, Some(45_455));
        assert_eq!(chromaticities.map(|c| c[2]), Some(68_000), "red x");
    }

    #[test]
    fn a_split_icc_profile_in_a_jpeg_file_converts_like_one_segment() {
        let data = std::fs::read(fixture("color/p3-icc.jpg")).unwrap();
        let icc = jpeg_icc_profile(&data).unwrap();
        let (first, second) = icc.split_at(icc.len() / 2);
        let split = jpeg_with_icc(&[(1, 2, first), (2, 2, second)]);
        assert_eq!(jpeg_icc_profile(&split).unwrap(), icc);
    }

    /// A small JPEG whose EXIF block holds `ColorSpace` and the interoperability index.
    fn jpeg_with_color_space(color_space: u16, index: &str) -> Vec<u8> {
        use exif::experimental::Writer;
        use exif::{Field, In, Tag, Value};
        use image::ImageEncoder;
        let fields = [
            Field {
                tag: Tag::ColorSpace,
                ifd_num: In::PRIMARY,
                value: Value::Short(vec![color_space]),
            },
            Field {
                tag: Tag::InteroperabilityIndex,
                ifd_num: In::PRIMARY,
                value: Value::Ascii(vec![index.as_bytes().to_vec()]),
            },
        ];
        let mut writer = Writer::new();
        for field in &fields {
            writer.push_field(field);
        }
        let mut exif = std::io::Cursor::new(Vec::new());
        writer.write(&mut exif, true).unwrap();
        let mut jpeg = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90);
        encoder.set_exif_metadata(exif.into_inner()).unwrap();
        encoder
            .write_image(&[200; 8 * 8 * 3], 8, 8, image::ExtendedColorType::Rgb8)
            .unwrap();
        jpeg
    }

    #[test]
    fn a_camera_jpeg_with_the_adobe_rgb_mark_converts_as_adobe_rgb() {
        let adobe = jpeg_with_color_space(0xFFFF, "R03");
        assert!(exif_marks_adobe_rgb(&adobe));
        assert_eq!(jpeg_color(&adobe), SourceColor::AdobeRgb);
        let srgb = jpeg_with_color_space(1, "R98");
        assert!(!exif_marks_adobe_rgb(&srgb));
        assert_eq!(jpeg_color(&srgb), SourceColor::Srgb);
        let uncalibrated_only = jpeg_with_color_space(0xFFFF, "R98");
        assert!(
            !exif_marks_adobe_rgb(&uncalibrated_only),
            "R03 is the mark of Adobe RGB"
        );
        assert!(!exif_marks_adobe_rgb(&[0xFF, 0xD8, 0xFF]), "a cut file");

        // A mid green of Adobe RGB is more saturated than the same values in sRGB or P3, so
        // the conversion lowers its red. Gray stays gray.
        let tone = RgbaImage::from_pixel(1, 1, Rgba([60, 160, 60, 255]));
        for target in [ColorTarget::Srgb, ColorTarget::DisplayP3] {
            let converted = convert(tone.clone(), &SourceColor::AdobeRgb, target);
            let [red, green, _, _] = converted.get_pixel(0, 0).0;
            assert!(red < 40 && green > 150, "{target:?}: {red} {green}");
        }
        let gray = RgbaImage::from_pixel(1, 1, Rgba([128, 128, 128, 255]));
        let converted = convert(gray, &SourceColor::AdobeRgb, ColorTarget::Srgb);
        let [red, green, blue, _] = converted.get_pixel(0, 0).0;
        assert!(
            red.abs_diff(green) <= 1 && green.abs_diff(blue) <= 1,
            "gray stays gray"
        );
    }

    #[test]
    fn a_cut_tiff_has_no_icc_profile() {
        let dir = test_dir();
        // A little-endian header, one entry for the tag 34675 with 200 bytes at offset 100.
        let mut tiff = vec![b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0];
        tiff.extend([0x73, 0x87, 7, 0, 200, 0, 0, 0, 100, 0, 0, 0]);
        for length in [0, 4, 8, 9, 15, tiff.len()] {
            let path = dir.path().join(format!("cut-{length}.tif"));
            std::fs::write(&path, &tiff[..length]).unwrap();
            assert_eq!(tiff_icc_profile(&path), None, "{length} bytes");
        }
    }

    #[test]
    fn a_cut_jpeg_has_no_exif_payload() {
        let jpeg = [
            0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x0A, b'E', b'x', b'i', b'f', 0, 0, 1, 2,
        ];
        assert_eq!(jpeg_exif_payload(&jpeg), Some(&[1, 2][..]));
        for length in 0..jpeg.len() {
            assert_eq!(jpeg_exif_payload(&jpeg[..length]), None, "{length} bytes");
        }
    }
}
