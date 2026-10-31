//! The tone mapping of HDR stills: a PQ or HLG signal in AVIF, HEIC, JPEG XL or PNG.
//!
//! The terminal shows only SDR pixels, so an HDR still maps to SDR in the target space
//! (`ColorTarget`). The map is the math of the HDR cube of the terminal player
//! (`player/tone_map.rs`): the same curves, the same SDR white ([`SDR_WHITE_NITS`]), the same
//! peak, the same primaries conversion and the same roll-off. So a still and a video of one
//! scene show the same colors. The cube samples the map at 33 points per channel, and `lut3d`
//! interpolates between them. A still runs the map per pixel, with one table for the signal
//! curve and one for the output curve.
//!
//! The decoders keep the depth of an HDR file: libheif decodes 16-bit RGB, `jxl-oxide` writes
//! its 16-bit stream, and a 16-bit PNG stays 16-bit. `Decoded` resizes the 16-bit image and maps
//! the small image, with the rows in parallel. An 8-bit HDR image, for example the embedded
//! thumbnail of an HDR HEIC file, maps in `color::convert`.
//!
//! The source is a CICP transfer of 16 (PQ) or 18 (HLG): from a HEIF `nclx` box, a PNG `cICP`
//! chunk, the `cicp` tag of an ICC profile (`SourceColor::icc`) or the enum encoding of a JPEG
//! XL file. The primaries are BT.2020, Display P3, DCI-P3 (with the Bradford adaptation of its
//! white point, as in the SDR cubes of the player) or BT.709. Other or unknown primaries count as
//! BT.2020, the primaries of BT.2100. The float formats (EXR, Radiance HDR) go through
//! `decode::tonemap`.

use crate::decode::color::{ColorTarget, SourceColor};
use image::{DynamicImage, RgbImage, RgbaImage};
use rayon::prelude::*;
use std::sync::OnceLock;

/// The peak of the HDR picture in nits: the HLG reference display and a common PQ master. The
/// value of `player/tone_map.rs`.
const HDR_PEAK_NITS: f64 = 1000.0;
/// The HDR level in nits that becomes SDR white before the highlight roll-off. The value of
/// `player/tone_map.rs`, so that a still matches a video.
const SDR_WHITE_NITS: f64 = 500.0;
/// The steps of the output table, over the square root of the linear value. Near black one
/// step moves the output by less than 0.25 levels.
const ENCODE_STEPS: usize = 4096;
/// The pixels of one task of the parallel map.
const PIXELS_PER_TASK: usize = 64 * 1024;

/// The transfer curve of an HDR signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Curve {
    /// Perceptual quantizer (SMPTE ST 2084), CICP transfer 16.
    Pq,
    /// Hybrid log-gamma (ARIB STD-B67), CICP transfer 18.
    Hlg,
}

impl Curve {
    /// The curve of a CICP transfer code, or `None` for an SDR curve.
    pub(crate) fn of_transfer(transfer: u8) -> Option<Self> {
        match transfer {
            16 => Some(Self::Pq),
            18 => Some(Self::Hlg),
            _ => None,
        }
    }

    /// The light of one signal value from 0 to 1: display light in nits for PQ, scene light
    /// from 0 to 1 for HLG. The HLG display light needs all three channels, see
    /// [`ToneMap::linear`].
    fn light(self, signal: f64) -> f64 {
        match self {
            Self::Pq => {
                let (m1, m2) = (0.159_301_757_812_5, 78.843_75);
                let (c1, c2, c3) = (0.835_937_5, 18.851_562_5, 18.6875);
                let power = signal.powf(1.0 / m2);
                10_000.0 * ((power - c1).max(0.0) / (c2 - c3 * power)).powf(1.0 / m1)
            }
            Self::Hlg => {
                // The inverse OETF to scene light.
                let (a, b, c) = (0.178_832_77, 0.284_668_92, 0.559_910_73);
                if signal <= 0.5 {
                    signal * signal / 3.0
                } else {
                    (((signal - c) / a).exp() + b) / 12.0
                }
            }
        }
    }

    /// The light of each 16-bit signal value, built at the first use.
    fn table(self) -> &'static [f32] {
        static PQ: OnceLock<Vec<f32>> = OnceLock::new();
        static HLG: OnceLock<Vec<f32>> = OnceLock::new();
        let table = match self {
            Self::Pq => &PQ,
            Self::Hlg => &HLG,
        };
        table.get_or_init(|| {
            (0..=u16::MAX)
                .map(|signal| self.light(f64::from(signal) / 65_535.0) as f32)
                .collect()
        })
    }
}

/// True when `source` holds a PQ or HLG signal, which `moxcms` cannot convert.
pub(crate) fn is_hdr(source: &SourceColor) -> bool {
    matches!(source, SourceColor::Cicp { transfer, .. } if Curve::of_transfer(*transfer).is_some())
}

/// The primaries of an HDR signal and of the targets. All but DCI-P3 have the white point D65.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Primaries {
    Bt709,
    DisplayP3,
    /// DCI-P3 (SMPTE RP 431-2): the P3 primaries with the DCI white point.
    DciP3,
    Bt2020,
}

impl Primaries {
    /// The primaries of a CICP code. Other codes count as BT.2020, the primaries of BT.2100.
    fn of_cicp(primaries: u8) -> Self {
        match primaries {
            1 => Self::Bt709,
            11 => Self::DciP3,
            12 => Self::DisplayP3,
            _ => Self::Bt2020,
        }
    }

    /// The primaries of the color space of the terminal.
    fn of_target(target: ColorTarget) -> Self {
        match target {
            ColorTarget::DisplayP3 => Self::DisplayP3,
            ColorTarget::Srgb => Self::Bt709,
        }
    }

    /// The CIE xy chromaticities of red, green and blue.
    fn chromaticities(self) -> [[f64; 2]; 3] {
        match self {
            Self::Bt709 => [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]],
            Self::DisplayP3 | Self::DciP3 => [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]],
            Self::Bt2020 => [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]],
        }
    }

    /// The CIE xy chromaticity of the white point.
    fn white(self) -> [f64; 2] {
        match self {
            Self::DciP3 => [0.314, 0.351],
            _ => D65,
        }
    }
}

/// The CIE xy chromaticity of the D65 white point.
const D65: [f64; 2] = [0.3127, 0.3290];

/// A 3x3 matrix, row by row.
type Matrix = [[f64; 3]; 3];

/// The matrix product `matrix * vector`.
fn apply(matrix: &Matrix, vector: [f64; 3]) -> [f64; 3] {
    matrix.map(|row| row[0] * vector[0] + row[1] * vector[1] + row[2] * vector[2])
}

/// The matrix product `left * right`.
fn multiply(left: &Matrix, right: &Matrix) -> Matrix {
    left.map(|row| {
        [0, 1, 2].map(|column| {
            row[0] * right[0][column] + row[1] * right[1][column] + row[2] * right[2][column]
        })
    })
}

/// The cross product of two vectors.
fn cross(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

/// The inverse of `matrix`. The matrices of real primaries are never singular.
fn invert(matrix: &Matrix) -> Matrix {
    let [first, second, third] = *matrix;
    let columns = [
        cross(second, third),
        cross(third, first),
        cross(first, second),
    ];
    let determinant =
        first[0] * columns[0][0] + first[1] * columns[0][1] + first[2] * columns[0][2];
    [0, 1, 2].map(|row| columns.map(|column| column[row] / determinant))
}

/// The CIE XYZ of a chromaticity at Y = 1.
fn xyz([x, y]: [f64; 2]) -> [f64; 3] {
    [x / y, 1.0, (1.0 - x - y) / y]
}

/// The matrix from linear RGB of `primaries` to CIE XYZ, with its white at Y = 1.
fn rgb_to_xyz(primaries: Primaries) -> Matrix {
    let [red, green, blue] = primaries.chromaticities().map(xyz);
    let columns = [0, 1, 2].map(|row| [red[row], green[row], blue[row]]);
    let scale = apply(&invert(&columns), xyz(primaries.white()));
    columns.map(|row| [0, 1, 2].map(|column| row[column] * scale[column]))
}

/// The Bradford cone response matrix, as in `player/tone_map.rs`.
const BRADFORD: Matrix = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

/// The Bradford adaptation in CIE XYZ from the white `source` to the white `target`.
fn adaptation(source: [f64; 2], target: [f64; 2]) -> Matrix {
    let (from, to) = (apply(&BRADFORD, xyz(source)), apply(&BRADFORD, xyz(target)));
    let gains = [0, 1, 2].map(|row| {
        let mut line = [0.0; 3];
        line[row] = to[row] / from[row];
        line
    });
    multiply(&invert(&BRADFORD), &multiply(&gains, &BRADFORD))
}

/// The matrix from linear RGB of `source` to linear RGB of `target`. The white of the source
/// becomes the white of the target, as in the cubes of the player.
fn conversion(source: Primaries, target: Primaries) -> Matrix {
    let adapted = multiply(
        &adaptation(source.white(), target.white()),
        &rgb_to_xyz(source),
    );
    multiply(&invert(&rgb_to_xyz(target)), &adapted)
}

/// The map of one HDR source into one target, in single precision for the pixel loop.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ToneMap {
    curve: Curve,
    /// Linear light of the source in nits to linear light of the target, with SDR white at 1.
    matrix: [[f32; 3]; 3],
    /// The Y row of the target, so the roll-off sees the same luminance in each target.
    luma: [f32; 3],
    /// The Y row of the source, for the HLG system gamma.
    scene_luma: [f32; 3],
}

impl ToneMap {
    /// The map of `source` into `target`, or `None` for an SDR source.
    pub(crate) fn of_source(source: &SourceColor, target: ColorTarget) -> Option<Self> {
        let SourceColor::Cicp {
            primaries,
            transfer,
        } = *source
        else {
            return None;
        };
        let curve = Curve::of_transfer(transfer)?;
        Some(Self::new(curve, Primaries::of_cicp(primaries), target))
    }

    /// Build the matrices in double precision, as the cube does.
    fn new(curve: Curve, source: Primaries, target: ColorTarget) -> Self {
        let target = Primaries::of_target(target);
        let conversion = conversion(source, target);
        let single = |row: [f64; 3]| row.map(|value| value as f32);
        Self {
            curve,
            matrix: conversion.map(|row| single(row.map(|value| value / SDR_WHITE_NITS))),
            luma: single(rgb_to_xyz(target)[1]),
            scene_luma: single(rgb_to_xyz(source)[1]),
        }
    }

    /// Linear SDR light from 0 to 1 in the target for the [`Curve::light`] of each channel.
    ///
    /// The highlights roll off with an extended Reinhard curve on the luminance, so the hue
    /// stays. A color outside the target moves toward its gray of the same luminance, and a
    /// channel above white scales the whole color down. The steps of `tone_map` in
    /// `player/tone_map.rs`.
    fn linear(&self, light: [f32; 3]) -> [f32; 3] {
        let nits = match self.curve {
            Curve::Pq => light,
            Curve::Hlg => {
                // The OOTF of a 1000 nit display: system gamma 1.2 on the scene luminance.
                let luma = dot(self.scene_luma, light);
                let gain = HDR_PEAK_NITS as f32 * luma.max(0.0).powf(0.2);
                light.map(|value| value * gain)
            }
        };
        let mut linear = self.matrix.map(|row| dot(row, nits));
        let luminance = dot(self.luma, linear);
        if luminance > 0.0 {
            let peak = (HDR_PEAK_NITS / SDR_WHITE_NITS) as f32;
            let scale = (1.0 + luminance / (peak * peak)) / (1.0 + luminance);
            linear = linear.map(|value| value * scale);
            let mapped = luminance * scale;
            let low = linear[0].min(linear[1]).min(linear[2]);
            if low < 0.0 {
                let toward_gray = -low / (mapped - low);
                linear = linear.map(|value| value + (mapped - value) * toward_gray);
            }
        }
        let high = linear[0].max(linear[1]).max(linear[2]);
        if high > 1.0 {
            linear = linear.map(|value| value / high);
        }
        linear.map(|value| value.clamp(0.0, 1.0))
    }

    /// The 8-bit output of one 16-bit signal per channel.
    fn pixel(&self, table: &[f32], encode: &[u8], signal: [u16; 3]) -> [u8; 3] {
        let light = signal.map(|value| table[usize::from(value)]);
        self.linear(light).map(|value| {
            let step = (value.sqrt() * (ENCODE_STEPS - 1) as f32 + 0.5) as usize;
            encode[step.min(ENCODE_STEPS - 1)]
        })
    }

    /// Map 16-bit pixels with `channels` (3 or 4) samples into 8-bit pixels, rows in parallel.
    /// The alpha rounds to 8 bit.
    fn map_deep(&self, samples: &[u16], channels: usize) -> Vec<u8> {
        let (table, encode) = (self.curve.table(), encode_table());
        let mut out = vec![0; samples.len()];
        let chunk = PIXELS_PER_TASK * channels;
        out.par_chunks_mut(chunk)
            .zip(samples.par_chunks(chunk))
            .for_each(|(out, samples)| {
                let pixels = out.chunks_exact_mut(channels);
                for (pixel, signal) in pixels.zip(samples.chunks_exact(channels)) {
                    let rgb = self.pixel(table, encode, [signal[0], signal[1], signal[2]]);
                    pixel[..3].copy_from_slice(&rgb);
                    if let (Some(alpha), Some(&sample)) = (pixel.get_mut(3), signal.get(3)) {
                        *alpha = to_8bit(sample);
                    }
                }
            });
        out
    }

    /// Map 8-bit pixels with `channels` (3 or 4) samples in place, rows in parallel. The alpha
    /// stays.
    pub(crate) fn map_in_place(&self, pixels: &mut [u8], channels: usize) {
        let (table, encode) = (self.curve.table(), encode_table());
        pixels
            .par_chunks_mut(PIXELS_PER_TASK * channels)
            .for_each(|pixels| {
                for pixel in pixels.chunks_exact_mut(channels) {
                    let signal = [pixel[0], pixel[1], pixel[2]].map(|value| u16::from(value) * 257);
                    pixel[..3].copy_from_slice(&self.pixel(table, encode, signal));
                }
            });
    }

    /// Map a 16-bit RGB or RGBA image into an 8-bit image of the same layout. Each other image
    /// comes back unchanged.
    pub(crate) fn map_image(&self, image: DynamicImage) -> DynamicImage {
        let (width, height) = (image.width(), image.height());
        let mapped = match &image {
            DynamicImage::ImageRgb16(rgb) => {
                RgbImage::from_raw(width, height, self.map_deep(rgb.as_raw(), 3))
                    .map(DynamicImage::ImageRgb8)
            }
            DynamicImage::ImageRgba16(rgba) => {
                RgbaImage::from_raw(width, height, self.map_deep(rgba.as_raw(), 4))
                    .map(DynamicImage::ImageRgba8)
            }
            _ => None,
        };
        mapped.unwrap_or(image)
    }
}

/// The dot product of two vectors.
fn dot(left: [f32; 3], right: [f32; 3]) -> f32 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}

/// The 8-bit output (gamma 2.4, as the cube writes) of each step of the square root of the
/// linear value, built at the first use. The square root spreads the steps over the dark
/// values, where the output curve is steep.
fn encode_table() -> &'static [u8] {
    static TABLE: OnceLock<Vec<u8>> = OnceLock::new();
    TABLE.get_or_init(|| {
        (0..ENCODE_STEPS)
            .map(|step| {
                let root = step as f64 / (ENCODE_STEPS - 1) as f64;
                (255.0 * (root * root).powf(1.0 / 2.4)).round() as u8
            })
            .collect()
    })
}

/// Round a 16-bit sample to 8 bits.
fn to_8bit(sample: u16) -> u8 {
    u8::try_from((u32::from(sample) * 255 + 32_767) / 65_535).unwrap_or(u8::MAX)
}

#[cfg(test)]
mod tests {
    use super::{Curve, ENCODE_STEPS, Primaries, ToneMap, encode_table, is_hdr, rgb_to_xyz};
    use crate::decode::color::{ColorTarget, SourceColor, convert};
    use crate::decode::resize::load_and_resize_px;
    use crate::decode::screen::load_for_screen;
    use crate::decode::{Decoded, EmbeddedThumbnail, ScreenImage, decode_image};
    use crate::scanner;
    use crate::test_support::fixture;
    use image::{DynamicImage, Rgb, RgbImage, RgbaImage};

    const TARGETS: [ColorTarget; 2] = [ColorTarget::DisplayP3, ColorTarget::Srgb];

    /// The exact output from 0 to 1 of `map` for one signal value per channel from 0 to 1.
    fn exact(map: &ToneMap, signal: [f64; 3]) -> [f64; 3] {
        let light = signal.map(|value| map.curve.light(value) as f32);
        map.linear(light)
            .map(|value| f64::from(value).powf(1.0 / 2.4))
    }

    fn cicp(primaries: u8, transfer: u8) -> SourceColor {
        SourceColor::Cicp {
            primaries,
            transfer,
        }
    }

    #[test]
    fn only_pq_and_hlg_are_hdr() {
        assert!(is_hdr(&cicp(9, 16)));
        assert!(is_hdr(&cicp(9, 18)));
        assert!(is_hdr(&cicp(2, 16)), "unknown primaries");
        assert!(!is_hdr(&cicp(9, 13)));
        assert!(!is_hdr(&SourceColor::Srgb));
        assert!(ToneMap::of_source(&SourceColor::DisplayP3, ColorTarget::Srgb).is_none());
    }

    #[test]
    fn the_bt2020_luma_row_is_the_one_of_bt2100() {
        let luma = rgb_to_xyz(Primaries::Bt2020)[1];
        for (computed, known) in luma.into_iter().zip([0.2627, 0.6780, 0.0593]) {
            assert!((computed - known).abs() < 1e-4, "{luma:?}");
        }
    }

    #[test]
    fn the_output_table_rounds_like_the_exact_curve() {
        let encode = encode_table();
        let mut worst = 0u8;
        for step in 0..=100_000 {
            let linear = f64::from(step) / 100_000.0;
            let exact = (255.0 * linear.powf(1.0 / 2.4)).round() as u8;
            let index = (linear.sqrt() * (ENCODE_STEPS - 1) as f64 + 0.5) as usize;
            worst = worst.max(encode[index].abs_diff(exact));
        }
        assert!(worst <= 1, "the table differs by {worst} levels");
        assert_eq!((encode[0], encode[ENCODE_STEPS - 1]), (0, 255));
    }

    #[test]
    fn the_gray_ramp_stays_neutral_and_in_order() {
        for target in TARGETS {
            for curve in [Curve::Pq, Curve::Hlg] {
                let map = ToneMap::new(curve, Primaries::Bt2020, target);
                let mut last = -1.0;
                for step in 0..=64 {
                    let signal = f64::from(step) / 64.0;
                    let [r, g, b] = exact(&map, [signal; 3]);
                    assert!(
                        (r - g).abs() < 1e-4 && (g - b).abs() < 1e-4,
                        "{curve:?} {signal}"
                    );
                    assert!(g >= last - 1e-6, "{curve:?} {signal}: {g} after {last}");
                    last = g;
                }
                assert!(last > 0.99, "{curve:?}: signal 1 is white");
            }
        }
    }

    #[test]
    fn dci_p3_white_adapts_to_the_white_of_the_target() {
        // Without the Bradford adaptation, DCI-P3 gray shows green-yellow in a D65 target.
        for target in TARGETS {
            let map = ToneMap::of_source(&cicp(11, 16), target).unwrap();
            let [r, g, b] = exact(&map, [0.6; 3]);
            assert!(
                (r - g).abs() < 1e-3 && (g - b).abs() < 1e-3,
                "{target:?}: {r} {g} {b}"
            );
        }
    }

    /// The test and bench helper `into_display` maps an 8-bit HDR image, as `fit_within` does.
    #[test]
    fn the_display_image_of_an_8_bit_hdr_decode_is_mapped() {
        let decoded = Decoded {
            image: DynamicImage::ImageRgb8(RgbImage::from_pixel(2, 2, Rgb([148, 148, 148]))),
            color: cicp(9, 16),
            opaque: true,
        };
        let pixel = decoded.into_display().to_rgba8().get_pixel(0, 0).0;
        assert_eq!(pixel, [158, 158, 158, 255]);
    }

    #[test]
    fn the_8_bit_and_the_16_bit_maps_agree() {
        let map = ToneMap::of_source(&cicp(9, 16), ColorTarget::Srgb).unwrap();
        let mut eight = vec![0, 0, 0, 128, 200, 120, 60, 7, 255, 255, 255, 0];
        let deep: Vec<u16> = eight.iter().map(|&value| u16::from(value) * 257).collect();
        let mapped = map.map_deep(&deep, 4);
        map.map_in_place(&mut eight, 4);
        assert_eq!(eight[..3], mapped[..3]);
        assert_eq!(eight[4..7], mapped[4..7]);
        assert_eq!(eight[8..11], mapped[8..11]);
        assert_eq!((eight[3], mapped[3]), (128, 128), "the alpha stays");
        assert_eq!((eight[11], mapped[11]), (0, 0));
    }

    /// The HDR fixtures of `tests/fixtures/hdr/`. AVIF and HEIC need libheif.
    fn fixtures() -> Vec<&'static str> {
        let mut names = vec!["pq.jxl", "hlg.jxl", "pq-icc.jxl", "pq.png", "hlg.png"];
        if cfg!(feature = "heic") {
            names.extend(["pq.avif", "hlg.avif", "pq.heic", "hlg.heic"]);
        }
        names
    }

    /// The rendering of the signal of `name` through the HDR cube of the player.
    fn reference(name: &str, target: ColorTarget) -> RgbaImage {
        let curve = if name.starts_with("pq") { "pq" } else { "hlg" };
        let suffix = match target {
            ColorTarget::DisplayP3 => "p3",
            ColorTarget::Srgb => "srgb",
        };
        image::open(fixture(&format!("hdr/ref/{curve}-{suffix}.png")))
            .unwrap()
            .to_rgba8()
    }

    /// The mean of the center of each patch of the 8x4 chart, per channel, row by row. The
    /// center skips the edges of the resize.
    fn patch_means(image: &RgbaImage) -> Vec<[f32; 3]> {
        let (width, height) = image.dimensions();
        let (patch_w, patch_h) = (width / 8, height / 4);
        let mut means = Vec::new();
        for row in 0..4 {
            for column in 0..8 {
                let (x0, y0) = (column * patch_w + patch_w / 4, row * patch_h + patch_h / 4);
                let mut sum = [0.0f32; 3];
                for y in y0..y0 + patch_h / 2 {
                    for x in x0..x0 + patch_w / 2 {
                        for (total, value) in sum.iter_mut().zip(image.get_pixel(x, y).0) {
                            *total += f32::from(value);
                        }
                    }
                }
                means.push(sum.map(|total| total / (patch_w * patch_h / 4) as f32));
            }
        }
        means
    }

    /// The largest difference in levels per patch between two images, row by row.
    fn differences(image: &RgbaImage, reference: &RgbaImage) -> Vec<f32> {
        patch_means(image)
            .iter()
            .zip(patch_means(reference))
            .map(|(ours, theirs)| {
                ours.iter()
                    .zip(theirs)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0, f32::max)
            })
            .collect()
    }

    /// The grid box (half size), the fullscreen box (twice the size) and the zoom source.
    const BOXES: [Option<(u32, u32)>; 3] = [Some((64, 32)), Some((256, 128)), None];

    /// Each fixture in each view and target matches the rendering of its signal through the HDR
    /// cube of the player. The tolerance in 8-bit levels per patch and channel: the gray ramp
    /// within 1, the mean of the 32 patches within 1.5, at most 3 patches above 2, none above
    /// 16. The cube holds 33 points per channel, and `lut3d` interpolates the output with its
    /// gamma between them. Where the gamut map sets a channel to 0, the exact map gives 0 and
    /// the cube gives up to 15 levels (the pale blue patch of row 1 into Display P3).
    #[test]
    fn each_fixture_matches_the_video_cube_in_each_view() {
        for name in fixtures() {
            let path = fixture(&format!("hdr/{name}"));
            for target in TARGETS {
                let reference = reference(name, target);
                for box_px in BOXES {
                    let decoded = decode_image(&path, box_px, EmbeddedThumbnail::Allow).unwrap();
                    assert!(is_hdr(&decoded.color), "{name}: {:?}", decoded.color);
                    let mapped = decoded.tone_mapped(box_px, target);
                    assert_eq!(mapped.color, SourceColor::of_target(target));
                    let DynamicImage::ImageRgb8(rgb) = mapped.image else {
                        panic!("{name}: an 8-bit RGB image");
                    };
                    let size = if box_px == Some((64, 32)) {
                        (64, 32)
                    } else {
                        (128, 64)
                    };
                    assert_eq!(rgb.dimensions(), size, "{name} {box_px:?}");
                    let rgba = DynamicImage::ImageRgb8(rgb).to_rgba8();
                    assert_matches_cube(
                        &rgba,
                        &reference,
                        1.0,
                        &format!("{name} {target:?} {box_px:?}"),
                    );
                }
            }
        }
    }

    /// The grid and the fullscreen view through their public steps, in the target of the tests.
    #[test]
    fn the_grid_and_the_fullscreen_view_show_the_tone_map() {
        for name in fixtures() {
            let path = fixture(&format!("hdr/{name}"));
            let reference = reference(name, ColorTarget::Srgb);
            let grid = load_and_resize_px(&path, (64, 32), EmbeddedThumbnail::Allow).unwrap();
            assert_eq!(grid.dimensions(), (64, 32), "{name}");
            assert_matches_cube(&grid, &reference, 1.0, name);
            let screen = load_for_screen(&path, (256, 128)).unwrap();
            assert!(matches!(screen, ScreenImage::Rgb(_)), "{name}: opaque RGB");
            assert_matches_cube(&screen.into_rgba(), &reference, 1.0, name);
            let zoom = decode_image(&path, None, EmbeddedThumbnail::Deny)
                .unwrap()
                .into_screen();
            assert_matches_cube(&zoom.into_rgba(), &reference, 1.0, name);
        }
    }

    /// Assert the tolerance of [`each_fixture_matches_the_video_cube_in_each_view`], with the
    /// gray ramp within `gray_levels`.
    fn assert_matches_cube(
        image: &RgbaImage,
        reference: &RgbaImage,
        gray_levels: f32,
        label: &str,
    ) {
        let differences = differences(image, reference);
        let gray = differences[..8].iter().copied().fold(0.0, f32::max);
        let worst = differences.iter().copied().fold(0.0, f32::max);
        let mean = differences.iter().sum::<f32>() / differences.len() as f32;
        let above_two = differences
            .iter()
            .filter(|&&difference| difference > 2.0)
            .count();
        assert!(
            gray <= gray_levels && mean <= 1.5 && above_two <= 3 && worst <= 16.0,
            "{label}: gray {gray}, mean {mean}, {above_two} above 2, max {worst}: {differences:?}"
        );
    }

    #[test]
    fn the_gray_ramp_of_each_fixture_is_neutral_and_in_order() {
        for name in fixtures() {
            let path = fixture(&format!("hdr/{name}"));
            let image = decode_image(&path, None, EmbeddedThumbnail::Deny).unwrap();
            let ramp =
                &patch_means(&image.tone_mapped(None, ColorTarget::Srgb).image.to_rgba8())[..8];
            for gray in ramp {
                let spread = gray.iter().copied().fold(0.0, f32::max)
                    - gray.iter().copied().fold(255.0, f32::min);
                assert!(spread <= 1.0, "{name}: gray {gray:?}");
            }
            assert!(
                ramp.windows(2)
                    .all(|pair| pair[1][1] > pair[0][1] || pair[1][1] >= 254.5),
                "{name}: {ramp:?}"
            );
            assert_eq!((ramp[0][1], ramp[7][1]), (0.0, 255.0), "{name}: {ramp:?}");
        }
    }

    /// The signal of each patch of the chart, from the lossless PNG of `curve`.
    fn chart_signal(curve: &str) -> Vec<[f64; 3]> {
        let path = fixture(&format!("hdr/{curve}.png"));
        let signal = decode_image(&path, None, EmbeddedThumbnail::Deny).unwrap();
        let signal = signal.image.to_rgb16();
        (0..32)
            .map(|patch| {
                let pixel = signal.get_pixel(patch % 8 * 16 + 8, patch / 8 * 16 + 8).0;
                pixel.map(|value| f64::from(value) / 65_535.0)
            })
            .collect()
    }

    /// A lossy JPEG XL file holds XYB samples, and `jxl-oxide` renders them into the PQ or HLG
    /// signal. Inside the sRGB gamut, the file matches the cube within 4 levels per patch, the
    /// tolerance of the lossy files of `jxl/`. `jxl-oxide` 0.12 clips the linear sRGB of the
    /// XYB samples to 0..1 before it converts the primaries (`ColorTransformOp::Clip`), so a
    /// BT.2020 color outside the sRGB gamut loses saturation. libjxl keeps it. The last assert
    /// fails when `jxl-oxide` keeps it too.
    #[test]
    fn a_lossy_jpeg_xl_file_matches_the_cube_inside_the_srgb_gamut() {
        for curve in ["pq", "hlg"] {
            let to_srgb = super::conversion(Primaries::Bt2020, Primaries::Bt709);
            let curve_of = if curve == "pq" { Curve::Pq } else { Curve::Hlg };
            // A patch is inside when no channel of its linear sRGB is below 0.1 % of its peak.
            let inside: Vec<bool> = chart_signal(curve)
                .into_iter()
                .map(|signal| {
                    let light = signal.map(|value| curve_of.light(value));
                    let peak = light.into_iter().fold(0.0, f64::max);
                    super::apply(&to_srgb, light)
                        .into_iter()
                        .all(|value| value >= -1e-3 * peak)
                })
                .collect();
            assert!(
                inside.iter().filter(|&&inside| inside).count() >= 12,
                "{inside:?}"
            );
            let path = fixture(&format!("hdr/{curve}-lossy.jxl"));
            for target in TARGETS {
                let decoded = decode_image(&path, None, EmbeddedThumbnail::Deny).unwrap();
                let mapped = decoded.tone_mapped(None, target).image.to_rgba8();
                let differences = differences(&mapped, &reference(curve, target));
                for (patch, difference) in differences.iter().enumerate() {
                    assert!(
                        !inside[patch] || *difference <= 4.0,
                        "{curve} {target:?} patch {patch}: {differences:?}"
                    );
                }
            }
            let decoded = decode_image(&path, None, EmbeddedThumbnail::Deny).unwrap();
            let red = decoded.image.to_rgb16().get_pixel(8, 24).0;
            assert!(
                red[1] > 10_000,
                "{curve}: BT.2020 red keeps its saturation: {red:?}"
            );
        }
    }

    /// An 8-bit HDR image, for example the embedded thumbnail of an HDR HEIC file, maps in
    /// `color::convert`. 8 bits hold less of the signal, so the gray ramp can differ by 2.
    #[test]
    fn an_8_bit_hdr_image_maps_in_the_color_conversion() {
        for name in ["pq.png", "hlg.png"] {
            let decoded = decode_image(
                &fixture(&format!("hdr/{name}")),
                None,
                EmbeddedThumbnail::Deny,
            )
            .unwrap();
            for target in TARGETS {
                let eight = decoded.image.to_rgba8();
                let mapped = convert(eight, &decoded.color, target);
                let label = format!("{name} {target:?} in 8 bit");
                assert_matches_cube(&mapped, &reference(name, target), 2.0, &label);
            }
        }
    }

    /// An SDR image of each color and PNG fixture keeps its pixels: the tone map passes it on.
    #[test]
    fn an_sdr_fixture_passes_the_tone_map_unchanged() {
        let mut checked = 0;
        for dir in ["color", "png"] {
            for entry in std::fs::read_dir(fixture(dir)).unwrap() {
                let path = entry.unwrap().path();
                // AVIF and HEIC need libheif.
                let libheif = scanner::has_extension(&path, &["avif", "heic"]);
                if !path.is_file()
                    || path.extension().is_some_and(|e| e == "sh")
                    || (libheif && !cfg!(feature = "heic"))
                {
                    continue;
                }
                for target in TARGETS {
                    for box_px in [Some((16, 16)), None] {
                        let before = decode_image(&path, box_px, EmbeddedThumbnail::Allow).unwrap();
                        let after = decode_image(&path, box_px, EmbeddedThumbnail::Allow)
                            .unwrap()
                            .tone_mapped(box_px, target);
                        assert_eq!(after.image, before.image, "{path:?}");
                        assert_eq!(after.color, before.color, "{path:?}");
                        checked += 1;
                    }
                }
            }
        }
        let least = if cfg!(feature = "heic") { 80 } else { 64 };
        assert!(checked >= least, "{checked}");
    }

    /// The map of the still matches the HDR cube of the terminal player at each point of the
    /// cube: the copied math gives the same values, within 1e-3 of the output range.
    #[cfg(feature = "video")]
    #[test]
    fn the_map_matches_each_point_of_the_player_cube() {
        use crate::player::tone_map::{Cube, CubeSource, Hdr};
        for target in TARGETS {
            for (hdr, transfer) in [(Hdr::Pq, 16), (Hdr::Hlg, 18)] {
                let map = ToneMap::of_source(&cicp(9, transfer), target).unwrap();
                let cube = Cube {
                    source: CubeSource::Hdr(hdr),
                    target,
                };
                let text = cube.text();
                let mut lines = text.lines();
                let size: u32 = lines
                    .next()
                    .and_then(|line| line.strip_prefix("LUT_3D_SIZE "))
                    .unwrap()
                    .parse()
                    .unwrap();
                let last = f64::from(size - 1);
                let mut worst = 0.0f64;
                for (index, line) in lines.enumerate() {
                    let index = u32::try_from(index).unwrap();
                    let signal = [index % size, index / size % size, index / (size * size)]
                        .map(|step| f64::from(step) / last);
                    let values: Vec<f64> = line.split(' ').map(|v| v.parse().unwrap()).collect();
                    for (ours, theirs) in exact(&map, signal).into_iter().zip(values) {
                        worst = worst.max((ours - theirs).abs());
                    }
                }
                // Single precision near black, where the output curve is steep: 1e-3 is a
                // quarter of a level.
                assert!(worst < 1e-3, "{hdr:?} {target:?}: {worst}");
            }
        }
    }

    /// Write the four HDR cubes of the player into `VIEWR_HDR_CUBE_DIR`, for the references of
    /// `tests/fixtures/hdr/make.sh`.
    #[cfg(feature = "video")]
    #[test]
    #[ignore = "writes the cubes for tests/fixtures/hdr/make.sh"]
    fn write_the_player_hdr_cubes() {
        use crate::player::tone_map::{Cube, CubeSource, Hdr};
        let Some(dir) = crate::decode::real_test_file("VIEWR_HDR_CUBE_DIR") else {
            return;
        };
        for target in TARGETS {
            for (hdr, name) in [(Hdr::Pq, "pq"), (Hdr::Hlg, "hlg")] {
                let suffix = match target {
                    ColorTarget::DisplayP3 => "p3",
                    ColorTarget::Srgb => "srgb",
                };
                let cube = Cube {
                    source: CubeSource::Hdr(hdr),
                    target,
                };
                std::fs::write(dir.join(format!("{name}-{suffix}.cube")), cube.text()).unwrap();
            }
        }
    }
}
