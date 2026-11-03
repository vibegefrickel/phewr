//! The color cubes of the terminal player: the HDR tone mapping and the SDR color conversion.
//!
//! The ffmpeg `lut3d` filter reads a cube and maps each frame into the terminal color space
//! (`ColorTarget`), as `decode::color` does for a still image. The terminal shows only SDR
//! pixels: an HLG or PQ signal goes to SDR in the target space. An SDR signal in the target
//! space needs no cube.

mod math;

use crate::decode::color::{ColorTarget, SourceColor};
use math::{conversion, convert_sdr, rgb_to_xyz, tone_map};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Two threads never write the same temp file.
static PARTIAL_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// The transfer curve of an HDR video.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hdr {
    /// Hybrid log-gamma (ARIB STD-B67): iPhone videos, and the base of Dolby Vision profile 8.4.
    Hlg,
    /// Perceptual quantizer (SMPTE ST 2084): HDR10 and Dolby Vision profile 8.1.
    Pq,
}

impl Hdr {
    /// `None` for an SDR `color_transfer` name.
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
    /// BT.709 and sRGB. An untagged stream counts as BT.709.
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
    /// An unknown `color_primaries` name is BT.709.
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

    /// For an animation whose profile ffprobe does not report. `None` for a source without a cube,
    /// for example Adobe RGB.
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

    /// The same source as in the cube, for `decode::color`.
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

    fn white(self) -> [f64; 2] {
        match self {
            Self::DciP3 => [0.314, 0.351],
            _ => D65,
        }
    }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CubeSource {
    /// An SDR signal with the sRGB curve.
    Sdr(Primaries),
    /// An HDR signal with BT.2020 primaries.
    Hdr(Hdr),
}

/// One cube from its source into the terminal target space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cube {
    pub source: CubeSource,
    pub target: ColorTarget,
}

impl Cube {
    /// `None` for an SDR video in the target space.
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

    /// Change [`CUBE_VERSION`] when the output of a cube changes.
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

    /// The output per channel for one signal value per channel, both from 0 to 1.
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

    /// An SDR cube runs on 8-bit pixels, an HDR cube on 16-bit pixels.
    pub(crate) fn runs_in_8_bit(self) -> bool {
        matches!(self.source, CubeSource::Sdr(_))
    }

    /// ffmpeg parses the cube text at each start: 33 points cost about 42 ms, 17 points about
    /// 16 ms. The smooth SDR conversion stays within 1 level at 17 points.
    fn size(self) -> u32 {
        if self.runs_in_8_bit() {
            SDR_LUT_SIZE
        } else {
            HDR_LUT_SIZE
        }
    }

    /// The cube in the `.cube` text format.
    pub(crate) fn text(self) -> String {
        let mapping = self.mapping();
        let size = self.size();
        let last = f64::from(size - 1);
        // `lut3d` truncates its 8-bit output. Half a level more makes the truncation round.
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

const CUBE_VERSION: u32 = 3;
const SDR_LUT_SIZE: u32 = 17;
/// 33 points per channel, as in common `.cube` files.
const HDR_LUT_SIZE: u32 = 33;
/// The HLG reference display and a common PQ master.
const HDR_PEAK_NITS: f64 = 1000.0;
/// The HDR level that becomes SDR white before the roll-off. 500 nits come closest to the
/// macOS SDR export (`avconvert`) of three iPhone HLG clips.
const SDR_WHITE_NITS: f64 = 500.0;
const D65: [f64; 2] = [0.3127, 0.3290];

/// The file of `cube` in the first of `dirs` that works. viewr writes it when it is missing.
///
/// A directory fails when viewr cannot write there, or when its path holds filter graph
/// syntax. Without a directory that works, the video plays without the cube.
pub(crate) fn cube_file(dirs: &[&Path], cube: Cube) -> Option<PathBuf> {
    dirs.iter().find_map(|dir| cube_file_in(dir, cube))
}

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
        // A temp file and a rename: a second viewr or a parallel thumbnail job never reads half a
        // cube.
        let partial = dir.join(format!(
            "{name}.{}.{}.partial",
            std::process::id(),
            PARTIAL_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        if std::fs::write(&partial, cube.text())
            .and_then(|()| std::fs::rename(&partial, &file))
            .is_err()
        {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "the prune removes a stale partial file"
            )]
            let _ = std::fs::remove_file(&partial);
            return None;
        }
    }
    Some(file)
}

#[cfg(test)]
mod tests;
