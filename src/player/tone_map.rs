//! The color cubes of the terminal player: the HDR tone mapping and the SDR color conversion.
//!
//! The `lut3d` filter of ffmpeg reads a cube and maps each frame into the color space of the
//! terminal (`ColorTarget`), as `decode::color` does for a still image. An HLG or PQ signal
//! goes to SDR in the target space, because the terminal shows only SDR pixels. An SDR signal
//! of other primaries goes into the target space. An SDR signal in the target space needs no
//! cube.

use crate::decode::color::{ColorTarget, SourceColor};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Numbers the temp files of the cube, so that two threads never write the same file.
static PARTIAL_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// The transfer curve of an HDR video, which the terminal cannot show without a tone mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hdr {
    /// Hybrid log-gamma (ARIB STD-B67): iPhone videos, also below Dolby Vision profile 8.4.
    Hlg,
    /// Perceptual quantizer (SMPTE ST 2084): HDR10 and Dolby Vision profile 8.1.
    Pq,
}

impl Hdr {
    /// The HDR curve behind a `color_transfer` name of `ffprobe`, or `None` for SDR.
    pub(crate) fn of_transfer(transfer: &str) -> Option<Self> {
        match transfer {
            "arib-std-b67" => Some(Self::Hlg),
            "smpte2084" => Some(Self::Pq),
            _ => None,
        }
    }
}

/// The primaries of an SDR video or animation. The cube reads the signal with the sRGB curve,
/// as `decode::color` reads an untagged still image and a CICP image with a video curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Primaries {
    /// BT.709 and sRGB. An untagged stream counts as BT.709 too.
    #[default]
    Bt709,
    /// BT.601 for 525 lines (SMPTE 170M and 240M, SMPTE C): NTSC video.
    Smpte170m,
    /// BT.601 for 625 lines (BT.470 BG, EBU): PAL video.
    Bt470Bg,
    /// Display P3 (SMPTE EG 432-1).
    DisplayP3,
    /// DCI-P3 (SMPTE RP 431-2): the P3 primaries with the DCI white point.
    DciP3,
    /// BT.2020 in SDR. The HDR curves always use these primaries.
    Bt2020,
}

impl Primaries {
    /// The primaries behind a `color_primaries` name of `ffprobe`. An unknown name is BT.709.
    pub(crate) fn of_name(name: &str) -> Self {
        match name {
            "smpte170m" | "smpte240m" => Self::Smpte170m,
            "bt470bg" => Self::Bt470Bg,
            "smpte432" => Self::DisplayP3,
            "smpte431" => Self::DciP3,
            "bt2020" => Self::Bt2020,
            _ => Self::Bt709,
        }
    }

    /// The primaries of a still color source, for an animation whose profile ffprobe does not
    /// report. `None` for a source without a cube of its own, for example Adobe RGB.
    pub(crate) fn of_source_color(source: &SourceColor) -> Option<Self> {
        match source {
            SourceColor::Srgb => Some(Self::Bt709),
            SourceColor::DisplayP3 => Some(Self::DisplayP3),
            SourceColor::Cicp { primaries, .. } => match primaries {
                1 => Some(Self::Bt709),
                5 => Some(Self::Bt470Bg),
                6 | 7 => Some(Self::Smpte170m),
                9 => Some(Self::Bt2020),
                11 => Some(Self::DciP3),
                12 => Some(Self::DisplayP3),
                _ => None,
            },
            SourceColor::Icc(data) => {
                Self::of_source_color(&crate::decode::color::icc_known_space(data)?)
            }
            SourceColor::AdobeRgb | SourceColor::PngGamma { .. } => None,
        }
    }

    /// The color of the pixels for `decode::color`, the same source as in the cube.
    pub(crate) fn source_color(self) -> SourceColor {
        // The CICP code points of ITU-T H.273, with the sRGB curve (13).
        let cicp = |primaries| SourceColor::Cicp {
            primaries,
            transfer: 13,
        };
        match self {
            Self::Bt709 => SourceColor::Srgb,
            Self::Smpte170m => cicp(6),
            Self::Bt470Bg => cicp(5),
            Self::DisplayP3 => SourceColor::DisplayP3,
            Self::DciP3 => cicp(11),
            Self::Bt2020 => cicp(9),
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
            Self::Smpte170m => [[0.630, 0.340], [0.310, 0.595], [0.155, 0.070]],
            Self::Bt470Bg => [[0.64, 0.33], [0.29, 0.60], [0.15, 0.06]],
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

    /// The name of the primaries in the file name of a cube.
    fn file_part(self) -> &'static str {
        match self {
            Self::Bt709 => "bt709",
            Self::Smpte170m => "smpte170m",
            Self::Bt470Bg => "bt470bg",
            Self::DisplayP3 => "p3",
            Self::DciP3 => "dcip3",
            Self::Bt2020 => "bt2020",
        }
    }
}

/// The signal that a cube reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CubeSource {
    /// An SDR signal with the sRGB curve.
    Sdr(Primaries),
    /// An HDR signal with BT.2020 primaries.
    Hdr(Hdr),
}

/// One cube of the terminal player: from its source into the target space of the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cube {
    pub source: CubeSource,
    pub target: ColorTarget,
}

impl Cube {
    /// The cube of a video with `hdr` and `primaries` for `target`. `None` for an SDR video in
    /// the target space, because its pixels need no change.
    pub(crate) fn for_video(
        hdr: Option<Hdr>,
        primaries: Primaries,
        target: ColorTarget,
    ) -> Option<Self> {
        let source = match hdr {
            Some(hdr) => CubeSource::Hdr(hdr),
            None if primaries == Primaries::of_target(target) => return None,
            None => CubeSource::Sdr(primaries),
        };
        Some(Self { source, target })
    }

    /// The file name of the cube. Change [`CUBE_VERSION`] when the output of a cube changes.
    fn file_name(self) -> String {
        let source = match self.source {
            CubeSource::Sdr(primaries) => format!("color-{}", primaries.file_part()),
            CubeSource::Hdr(Hdr::Hlg) => "tonemap-hlg".to_owned(),
            CubeSource::Hdr(Hdr::Pq) => "tonemap-pq".to_owned(),
        };
        let target = match self.target {
            ColorTarget::DisplayP3 => "p3",
            ColorTarget::Srgb => "srgb",
        };
        format!("{source}-{target}-v{CUBE_VERSION}.cube")
    }

    /// The output value per channel for one signal value per channel, both from 0 to 1.
    fn mapping(self) -> Box<dyn Fn([f64; 3]) -> [f64; 3]> {
        let target = Primaries::of_target(self.target);
        match self.source {
            CubeSource::Sdr(source) => {
                let matrix = conversion(source, target);
                Box::new(move |signal| convert_sdr(&matrix, signal))
            }
            CubeSource::Hdr(hdr) => {
                let matrix = conversion(Primaries::Bt2020, target);
                let luma = rgb_to_xyz(target)[1];
                Box::new(move |signal| tone_map(hdr, &matrix, luma, signal))
            }
        }
    }

    /// True when `lut3d` runs this cube on 8 bit pixels: an SDR cube. An HDR cube runs on
    /// 16 bit pixels, see `cube_format` in the decoder.
    pub(crate) fn runs_in_8_bit(self) -> bool {
        matches!(self.source, CubeSource::Sdr(_))
    }

    /// The edge length of the cube. ffmpeg parses the text of a cube at each start: 33 points
    /// cost about 42 ms, 17 points about 16 ms. The smooth SDR conversion stays within 1 level
    /// at 17 points, and the steep HDR curves keep 33.
    fn size(self) -> u32 {
        if self.runs_in_8_bit() {
            SDR_LUT_SIZE
        } else {
            HDR_LUT_SIZE
        }
    }

    /// The cube in the `.cube` text format for the `lut3d` filter.
    pub(crate) fn text(self) -> String {
        let mapping = self.mapping();
        let size = self.size();
        let last = f64::from(size - 1);
        // `lut3d` truncates its output to 8 bit, so an 8 bit cube adds half a level, and the
        // truncation rounds.
        let offset = if self.runs_in_8_bit() {
            0.5 / 255.0
        } else {
            0.0
        };
        let mut cube = format!("LUT_3D_SIZE {size}\n");
        // The red index runs fastest in a `.cube` file.
        for blue in 0..size {
            for green in 0..size {
                for red in 0..size {
                    let signal = [red, green, blue].map(|index| f64::from(index) / last);
                    let [r, g, b] = mapping(signal).map(|value| value + offset);
                    #[expect(
                        clippy::let_underscore_must_use,
                        reason = "a write to a String cannot fail"
                    )]
                    let _ = writeln!(cube, "{r:.6} {g:.6} {b:.6}");
                }
            }
        }
        cube
    }
}

/// The version in the file names of the cubes. Version 3 adds the target space.
const CUBE_VERSION: u32 = 3;
/// The edge length of an SDR cube, see [`Cube::size`].
const SDR_LUT_SIZE: u32 = 17;
/// The edge length of an HDR cube: 33 points per channel, as in common `.cube` files.
const HDR_LUT_SIZE: u32 = 33;
/// The peak of the HDR picture in nits: the HLG reference display and a common PQ master.
const HDR_PEAK_NITS: f64 = 1000.0;
/// The HDR level in nits that becomes SDR white before the highlight roll-off. 500 nits put
/// the result closest to the SDR export of macOS (`avconvert`) on three iPhone HLG clips.
const SDR_WHITE_NITS: f64 = 500.0;
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

/// The inverse of `matrix`. Its columns are the cross products of the rows over the
/// determinant. The matrices of real primaries are never singular.
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

/// The Bradford cone response matrix, as in the ICC profiles that `decode::color` reads.
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
/// becomes the white of the target.
fn conversion(source: Primaries, target: Primaries) -> Matrix {
    let adapted = multiply(
        &adaptation(source.white(), target.white()),
        &rgb_to_xyz(source),
    );
    multiply(&invert(&rgb_to_xyz(target)), &adapted)
}

/// Linear light of one sRGB signal value.
fn srgb_to_linear(value: f64) -> f64 {
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// The sRGB signal value of linear light from 0 to 1.
fn linear_to_srgb(value: f64) -> f64 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

/// One SDR signal value per channel in the target primaries. `matrix` is the [`conversion`]
/// from the source. A color outside the target clips per channel, as in `decode::color`.
fn convert_sdr(matrix: &Matrix, signal: [f64; 3]) -> [f64; 3] {
    apply(matrix, signal.map(srgb_to_linear)).map(|value| linear_to_srgb(value.clamp(0.0, 1.0)))
}

/// Display light in nits for one HDR signal value per channel, BT.2020 primaries.
fn hdr_nits(hdr: Hdr, signal: [f64; 3]) -> [f64; 3] {
    match hdr {
        Hdr::Hlg => {
            // The inverse OETF to scene light, then the OOTF of a 1000 nit display (gamma 1.2).
            let (a, b, c) = (0.178_832_77, 0.284_668_92, 0.559_910_73);
            let scene = signal.map(|value| {
                if value <= 0.5 {
                    value * value / 3.0
                } else {
                    (((value - c) / a).exp() + b) / 12.0
                }
            });
            let luma = 0.2627 * scene[0] + 0.6780 * scene[1] + 0.0593 * scene[2];
            let gain = HDR_PEAK_NITS * luma.max(0.0).powf(0.2);
            scene.map(|value| value * gain)
        }
        Hdr::Pq => {
            let (m1, m2) = (0.159_301_757_812_5, 78.843_75);
            let (c1, c2, c3) = (0.835_937_5, 18.851_562_5, 18.6875);
            signal.map(|value| {
                let power = value.powf(1.0 / m2);
                10_000.0 * ((power - c1).max(0.0) / (c2 - c3 * power)).powf(1.0 / m1)
            })
        }
    }
}

/// One SDR output value per channel (gamma 2.4) in the target primaries for one HDR signal
/// value. `matrix` is the [`conversion`] from BT.2020 to the target, `luma` the Y row of the
/// target, so the roll-off sees the same luminance in each target.
///
/// The highlights roll off with an extended Reinhard curve on the luminance, so the hue stays.
/// A color outside the target moves toward its gray of the same luminance, not to a clipped hue.
fn tone_map(hdr: Hdr, matrix: &Matrix, luma: [f64; 3], signal: [f64; 3]) -> [f64; 3] {
    let nits = hdr_nits(hdr, signal);
    let mut linear = apply(matrix, nits).map(|value| value / SDR_WHITE_NITS);
    let luminance = luma[0] * linear[0] + luma[1] * linear[1] + luma[2] * linear[2];
    if luminance > 0.0 {
        let peak = HDR_PEAK_NITS / SDR_WHITE_NITS;
        let scale = (1.0 + luminance / (peak * peak)) / (1.0 + luminance);
        linear = linear.map(|value| value * scale);
        let mapped = luminance * scale;
        let low = linear[0].min(linear[1]).min(linear[2]);
        if low < 0.0 {
            let toward_gray = -low / (mapped - low);
            linear = linear.map(|value| value + (mapped - value) * toward_gray);
        }
    }
    // A channel above white (a saturated highlight, or PQ above the peak) scales the whole
    // color down, so that the clip keeps the hue.
    let high = linear[0].max(linear[1]).max(linear[2]);
    if high > 1.0 {
        linear = linear.map(|value| value / high);
    }
    linear.map(|value| value.clamp(0.0, 1.0).powf(1.0 / 2.4))
}

/// The file of `cube` in the first of `dirs` that works. viewr writes it when it is missing.
///
/// A directory fails when viewr cannot write there, or when its path holds a character that
/// the syntax of a filter graph uses. The caller passes a temp directory as the last choice.
/// Without a directory that works, the video plays without the cube.
pub(crate) fn cube_file(dirs: &[&Path], cube: Cube) -> Option<PathBuf> {
    dirs.iter().find_map(|dir| cube_file_in(dir, cube))
}

/// The file of `cube` in `dir`, see [`cube_file`].
fn cube_file_in(dir: &Path, cube: Cube) -> Option<PathBuf> {
    let name = cube.file_name();
    let file = dir.join(&name);
    // The filter graph of ffmpeg reads these characters as syntax.
    let safe = file.to_str()?.chars().all(|character| {
        !character.is_control()
            && !matches!(character, '\\' | '\'' | ':' | ',' | ';' | '[' | ']' | '=')
    });
    if !safe {
        return None;
    }
    if !file.is_file() {
        std::fs::create_dir_all(dir).ok()?;
        // Write to a temp file and rename it, so that a second viewr or a parallel thumbnail
        // job never reads half a cube.
        let partial = dir.join(format!(
            "{name}.{}.{}.partial",
            std::process::id(),
            PARTIAL_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&partial, cube.text()).ok()?;
        std::fs::rename(&partial, &file).ok()?;
    }
    Some(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::color;
    use image::RgbaImage;

    const ALL_PRIMARIES: [Primaries; 6] = [
        Primaries::Bt709,
        Primaries::Smpte170m,
        Primaries::Bt470Bg,
        Primaries::DisplayP3,
        Primaries::DciP3,
        Primaries::Bt2020,
    ];

    /// The tone mapping of `hdr` into `target`.
    fn hdr_cube(hdr: Hdr, target: ColorTarget) -> Cube {
        Cube {
            source: CubeSource::Hdr(hdr),
            target,
        }
    }

    /// The output value of the sRGB tone mapping for one signal value on all three channels.
    fn gray(hdr: Hdr, signal: f64) -> [f64; 3] {
        hdr_cube(hdr, ColorTarget::Srgb).mapping()([signal; 3])
    }

    /// The output of `cube` for `signal`, in 8 bit levels.
    fn levels(cube: Cube, signal: [u8; 3]) -> [f64; 3] {
        cube.mapping()(signal.map(|value| f64::from(value) / 255.0)).map(|value| value * 255.0)
    }

    /// The still conversion of `decode::color` for one pixel.
    fn still(signal: [u8; 3], source: &SourceColor, target: ColorTarget) -> [f64; 3] {
        let [r, g, b] = signal;
        let image = RgbaImage::from_raw(1, 1, vec![r, g, b, 255]).unwrap();
        let pixel = color::convert(image, source, target).into_raw();
        [pixel[0], pixel[1], pixel[2]].map(f64::from)
    }

    #[test]
    fn the_tone_mapping_keeps_black_and_gray_and_rolls_off_the_highlights() {
        for hdr in [Hdr::Hlg, Hdr::Pq] {
            assert_eq!(gray(hdr, 0.0), [0.0; 3]);
            let mut last = 0.0;
            for step in 1..=20 {
                let [r, g, b] = gray(hdr, f64::from(step) / 20.0);
                assert!(
                    (r - g).abs() < 1e-3 && (g - b).abs() < 1e-3,
                    "gray stays gray"
                );
                // PQ above the 1000 nit peak stays at white.
                assert!(r >= last && r <= 1.0, "brighter in, never darker out");
                last = r;
            }
        }
        // HLG reference white (75 %, 203 nits) lands in the upper midtones, not at the clip.
        let [white, ..] = gray(Hdr::Hlg, 0.75);
        assert!(
            (0.55..0.7).contains(&white),
            "HLG reference white maps to {white}"
        );
        // A PQ value of 1000 nits is the peak, and the peak is the SDR white.
        let [peak, ..] = gray(Hdr::Pq, 0.751_827_5);
        assert!(peak > 0.99, "PQ 1000 nits maps to {peak}");
    }

    #[test]
    fn a_color_outside_bt709_keeps_its_hue_and_stays_in_range() {
        let srgb = |hdr, signal| hdr_cube(hdr, ColorTarget::Srgb).mapping()(signal);
        let [r, g, b] = srgb(Hdr::Hlg, [1.0, 0.0, 0.0]);
        assert!(
            r > 0.9 && g < 0.2 && b < 0.4,
            "BT.2020 red stays red: {r} {g} {b}"
        );
        let [r, g, b] = srgb(Hdr::Pq, [0.0, 0.7, 0.0]);
        assert!(g > r && g > b, "BT.2020 green stays green: {r} {g} {b}");
        // A red highlight far above the PQ peak stays red at full red, not white.
        let [r, g, b] = srgb(Hdr::Pq, [0.95, 0.6, 0.6]);
        assert!(
            (r - 1.0).abs() < 1e-9,
            "the brightest channel is at white: {r}"
        );
        assert!(
            g < 0.95 && b < 0.95,
            "the other channels keep the hue: {g} {b}"
        );
    }

    #[test]
    fn the_tone_mapping_into_display_p3_keeps_gray_and_widens_saturated_colors() {
        for hdr in [Hdr::Hlg, Hdr::Pq] {
            let p3 = hdr_cube(hdr, ColorTarget::DisplayP3).mapping();
            let srgb = hdr_cube(hdr, ColorTarget::Srgb).mapping();
            for step in 0..=10 {
                let signal = [f64::from(step) / 10.0; 3];
                let (wide, narrow) = (p3(signal), srgb(signal));
                for channel in 0..3 {
                    assert!((wide[channel] - narrow[channel]).abs() < 1e-6, "{signal:?}");
                }
            }
            // BT.2020 red is outside both targets. In Display P3, it keeps more of its
            // saturation: the other channels stay lower.
            let (wide, narrow) = (p3([0.9, 0.2, 0.2]), srgb([0.9, 0.2, 0.2]));
            assert!(
                wide[1] <= narrow[1] && wide[2] < narrow[2],
                "{wide:?} {narrow:?}"
            );
        }
    }

    #[test]
    fn the_computed_primaries_give_the_known_matrices() {
        let to_bt709 = conversion(Primaries::Bt2020, Primaries::Bt709);
        let expected = [
            [1.6605, -0.5876, -0.0728],
            [-0.1246, 1.1329, -0.0083],
            [-0.0182, -0.1006, 1.1187],
        ];
        for (row, expected) in to_bt709.iter().zip(expected) {
            for (value, expected) in row.iter().zip(expected) {
                assert!((value - expected).abs() < 1e-4, "{to_bt709:?}");
            }
        }
        let luma = rgb_to_xyz(Primaries::Bt709)[1];
        assert!((luma[0] - 0.2126).abs() < 1e-4 && (luma[1] - 0.7152).abs() < 1e-4);
        let round_trip = multiply(
            &conversion(Primaries::DisplayP3, Primaries::Bt709),
            &conversion(Primaries::Bt709, Primaries::DisplayP3),
        );
        for (row, line) in round_trip.iter().enumerate() {
            for (column, value) in line.iter().enumerate() {
                let identity = if row == column { 1.0 } else { 0.0 };
                assert!((value - identity).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn the_sdr_cube_matches_the_still_conversion() {
        let into_p3 = Cube::for_video(None, Primaries::Bt709, ColorTarget::DisplayP3).unwrap();
        // sRGB red is (234, 51, 35) in Display P3.
        let red = levels(into_p3, [255, 0, 0]);
        assert!(
            (red[0] - 234.0).abs() < 1.0 && (red[1] - 51.0).abs() < 1.0 && red[2] < 36.0,
            "{red:?}"
        );
        for signal in [
            [255, 0, 0],
            [0, 255, 0],
            [0, 0, 255],
            [255, 255, 255],
            [128, 128, 128],
            [200, 120, 40],
            [30, 90, 160],
        ] {
            for primaries in ALL_PRIMARIES {
                for target in [ColorTarget::DisplayP3, ColorTarget::Srgb] {
                    let cube = Cube {
                        source: CubeSource::Sdr(primaries),
                        target,
                    };
                    let video = levels(cube, signal);
                    let image = still(signal, &primaries.source_color(), target);
                    for channel in 0..3 {
                        assert!(
                            (video[channel] - image[channel]).abs() <= 1.0,
                            "{signal:?} {primaries:?} into {target:?}: cube {video:?}, still {image:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn only_a_video_outside_the_target_gets_an_sdr_cube() {
        let target = ColorTarget::DisplayP3;
        assert_eq!(Cube::for_video(None, Primaries::DisplayP3, target), None);
        assert_eq!(
            Cube::for_video(None, Primaries::Bt709, ColorTarget::Srgb),
            None
        );
        assert_eq!(
            Cube::for_video(None, Primaries::Bt709, target),
            Some(Cube {
                source: CubeSource::Sdr(Primaries::Bt709),
                target
            })
        );
        // An HDR video gets the tone mapping in each target.
        for target in [ColorTarget::Srgb, ColorTarget::DisplayP3] {
            assert_eq!(
                Cube::for_video(Some(Hdr::Pq), Primaries::Bt2020, target),
                Some(hdr_cube(Hdr::Pq, target))
            );
        }
        assert_eq!(Primaries::of_name("bt709"), Primaries::Bt709);
        assert_eq!(Primaries::of_name("smpte170m"), Primaries::Smpte170m);
        assert_eq!(Primaries::of_name("smpte240m"), Primaries::Smpte170m);
        assert_eq!(Primaries::of_name("bt470bg"), Primaries::Bt470Bg);
        assert_eq!(Primaries::of_name("smpte431"), Primaries::DciP3);
        assert_eq!(Primaries::of_name("bt470m"), Primaries::Bt709);
        assert_eq!(Primaries::of_name(""), Primaries::Bt709);
        assert_eq!(Primaries::of_name("smpte432"), Primaries::DisplayP3);
        assert_eq!(Primaries::of_name("bt2020"), Primaries::Bt2020);
    }

    #[test]
    fn the_cube_has_a_header_and_one_line_per_point() {
        let cube = hdr_cube(Hdr::Hlg, ColorTarget::DisplayP3).text();
        let mut lines = cube.lines();
        assert_eq!(lines.next(), Some("LUT_3D_SIZE 33"));
        assert_eq!(lines.next(), Some("0.000000 0.000000 0.000000"));
        assert_eq!(cube.lines().count(), 1 + 33 * 33 * 33);
        // Red runs fastest: the second point is a dark red.
        let second: Vec<f64> = cube
            .lines()
            .nth(2)
            .unwrap()
            .split(' ')
            .map(|value| value.parse().unwrap())
            .collect();
        assert!(second[0] > 0.0 && second[1] < second[0]);
    }

    #[test]
    fn an_sdr_cube_has_17_points_and_rounds_the_8_bit_output_of_lut3d() {
        let cube = Cube::for_video(None, Primaries::Bt709, ColorTarget::DisplayP3).unwrap();
        assert!(cube.runs_in_8_bit());
        assert!(!hdr_cube(Hdr::Pq, ColorTarget::DisplayP3).runs_in_8_bit());
        let text = cube.text();
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some("LUT_3D_SIZE 17"));
        // Black gets half a level, so that the truncation of `lut3d` rounds.
        assert_eq!(lines.next(), Some("0.001961 0.001961 0.001961"));
        assert_eq!(text.lines().count(), 1 + 17 * 17 * 17);
        let white = text.lines().last().unwrap();
        assert_eq!(white, "1.001961 1.001961 1.001961");
    }

    /// The real `lut3d` truncates its 8 bit output, and the SDR cube adds half a level, so that
    /// the output rounds. If a later `FFmpeg` rounds itself, the mean error grows to about half
    /// a level, and this test fails. Without ffmpeg on PATH, the test skips with a note.
    #[test]
    fn the_real_lut3d_rounds_the_8_bit_output_of_an_sdr_cube() {
        let Some(program) = crate::test_support::video::real_ffmpeg() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let cube = Cube::for_video(None, Primaries::Bt709, ColorTarget::DisplayP3).unwrap();
        let file = cube_file(&[dir.path()], cube).unwrap();
        // Every 5th level per channel: 52 x 52 x 52 colors.
        let steps: Vec<u8> = (0..=255).step_by(5).collect();
        let mut colors = Vec::new();
        for &blue in &steps {
            for &green in &steps {
                for &red in &steps {
                    colors.push([red, green, blue]);
                }
            }
        }
        let input = dir.path().join("colors.rgb");
        std::fs::write(&input, colors.concat()).unwrap();
        let size = format!("{}x{}", steps.len(), steps.len() * steps.len());
        let graph = format!(
            "format=rgb24,lut3d=file={}:interp=tetrahedral",
            file.display()
        );
        let output = std::process::Command::new(&program)
            .args(["-hide_banner", "-loglevel", "error", "-f", "rawvideo"])
            .args(["-pix_fmt", "rgb24", "-s", &size, "-i"])
            .arg(&input)
            .args([
                "-vf", &graph, "-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1",
            ])
            .output()
            .unwrap();
        assert_eq!(output.stdout.len(), colors.len() * 3);
        let mut largest = 0.0_f64;
        let mut sum = 0.0;
        for (color, pixel) in colors.iter().zip(output.stdout.as_chunks::<3>().0) {
            let exact = levels(cube, *color);
            for (expected, &actual) in exact.iter().zip(pixel) {
                let error = (f64::from(actual) - expected.round()).abs();
                largest = largest.max(error);
                sum += error;
            }
        }
        let mean = sum / (colors.len() * 3) as f64;
        assert!(largest <= 1.0, "largest error {largest} levels");
        assert!(mean < 0.1, "mean error {mean} levels: lut3d rounds now?");
    }

    #[test]
    fn each_cube_has_its_own_file_name() {
        let names: Vec<String> = [
            hdr_cube(Hdr::Hlg, ColorTarget::DisplayP3),
            hdr_cube(Hdr::Hlg, ColorTarget::Srgb),
            hdr_cube(Hdr::Pq, ColorTarget::DisplayP3),
            Cube::for_video(None, Primaries::Bt709, ColorTarget::DisplayP3).unwrap(),
            Cube::for_video(None, Primaries::DisplayP3, ColorTarget::Srgb).unwrap(),
            Cube::for_video(None, Primaries::Bt2020, ColorTarget::DisplayP3).unwrap(),
            Cube::for_video(None, Primaries::Smpte170m, ColorTarget::Srgb).unwrap(),
            Cube::for_video(None, Primaries::DciP3, ColorTarget::DisplayP3).unwrap(),
        ]
        .map(Cube::file_name)
        .into();
        assert_eq!(
            names,
            [
                "tonemap-hlg-p3-v3.cube",
                "tonemap-hlg-srgb-v3.cube",
                "tonemap-pq-p3-v3.cube",
                "color-bt709-p3-v3.cube",
                "color-p3-srgb-v3.cube",
                "color-bt2020-p3-v3.cube",
                "color-smpte170m-srgb-v3.cube",
                "color-dcip3-p3-v3.cube",
            ]
        );
    }

    #[test]
    fn the_cube_file_is_written_once() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let cube = hdr_cube(Hdr::Pq, ColorTarget::DisplayP3);
        let file = cube_file(&[&cache], cube).unwrap();
        assert_eq!(file, cache.join("tonemap-pq-p3-v3.cube"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), cube.text());
        std::fs::write(&file, "kept").unwrap();
        assert_eq!(cube_file(&[&cache], cube), Some(file.clone()));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "kept");
        assert_eq!(
            std::fs::read_dir(&cache).unwrap().count(),
            1,
            "no partial file"
        );
    }

    #[test]
    fn a_path_with_syntax_or_without_write_access_falls_back_to_the_next_directory() {
        let dir = tempfile::tempdir().unwrap();
        let fallback = dir.path().join("fallback");
        let odd = dir.path().join("a:b,c");
        let unwritable = Path::new("/dev/null/cache");
        let cube = hdr_cube(Hdr::Hlg, ColorTarget::Srgb);
        for first in [odd.as_path(), unwritable] {
            assert_eq!(
                cube_file(&[first, &fallback], cube),
                Some(fallback.join("tonemap-hlg-srgb-v3.cube")),
                "{}",
                first.display()
            );
        }
        assert!(!odd.exists());
        assert_eq!(cube_file(&[unwritable], cube), None);
    }

    #[test]
    fn a_path_with_letters_outside_ascii_keeps_the_cube() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("Jürgen Ö/Caches");
        let file = cube_file(&[&cache], hdr_cube(Hdr::Hlg, ColorTarget::DisplayP3)).unwrap();
        assert!(file.is_file());
    }
}
