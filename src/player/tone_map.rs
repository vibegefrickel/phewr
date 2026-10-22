//! The HDR probe and the tone mapping cube of the terminal player.
//!
//! The `lut3d` filter of ffmpeg reads the cube and maps an HLG or PQ signal to SDR, because
//! the terminal shows only SDR pixels.

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

/// The edge length of the tone mapping cube: 33 points per channel, as in common `.cube` files.
const LUT_SIZE: u32 = 33;
/// The peak of the HDR picture in nits: the HLG reference display and a common PQ master.
const HDR_PEAK_NITS: f64 = 1000.0;
/// The HDR level in nits that becomes SDR white before the highlight roll-off. 500 nits put
/// the result closest to the SDR export of macOS (`avconvert`) on three iPhone HLG clips.
const SDR_WHITE_NITS: f64 = 500.0;
/// BT.2020 to BT.709 primaries, in linear light.
const BT2020_TO_BT709: [[f64; 3]; 3] = [
    [1.6605, -0.5876, -0.0728],
    [-0.1246, 1.1329, -0.0083],
    [-0.0182, -0.1006, 1.1187],
];

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

/// One SDR output value per channel (BT.709, gamma 2.4) for one HDR signal value.
///
/// The highlights roll off with an extended Reinhard curve on the luminance, so the hue stays.
/// A color outside BT.709 moves toward its gray of the same luminance, not to a clipped hue.
fn tone_map(hdr: Hdr, signal: [f64; 3]) -> [f64; 3] {
    let nits = hdr_nits(hdr, signal);
    let mut linear = BT2020_TO_BT709
        .map(|row| (row[0] * nits[0] + row[1] * nits[1] + row[2] * nits[2]) / SDR_WHITE_NITS);
    let luma = 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2];
    if luma > 0.0 {
        let peak = HDR_PEAK_NITS / SDR_WHITE_NITS;
        let scale = (1.0 + luma / (peak * peak)) / (1.0 + luma);
        linear = linear.map(|value| value * scale);
        let mapped_luma = luma * scale;
        let low = linear[0].min(linear[1]).min(linear[2]);
        if low < 0.0 {
            let toward_gray = -low / (mapped_luma - low);
            linear = linear.map(|value| value + (mapped_luma - value) * toward_gray);
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

/// The tone mapping cube in the `.cube` text format for the `lut3d` filter.
pub(crate) fn tone_map_cube(hdr: Hdr) -> String {
    let last = f64::from(LUT_SIZE - 1);
    let mut cube = format!("LUT_3D_SIZE {LUT_SIZE}\n");
    // The red index runs fastest in a `.cube` file.
    for blue in 0..LUT_SIZE {
        for green in 0..LUT_SIZE {
            for red in 0..LUT_SIZE {
                let signal = [red, green, blue].map(|index| f64::from(index) / last);
                let [r, g, b] = tone_map(hdr, signal);
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

/// The cube file for `hdr` in the first of `dirs` that works. viewr writes it when it is missing.
///
/// A directory fails when viewr cannot write there, or when its path holds a character that
/// the syntax of a filter graph uses. The caller passes a temp directory as the last choice.
/// Without a directory that works, the video plays without the tone mapping.
pub(crate) fn tone_map_file(dirs: &[&Path], hdr: Hdr) -> Option<PathBuf> {
    dirs.iter().find_map(|dir| tone_map_file_in(dir, hdr))
}

/// The cube file for `hdr` in `dir`, see [`tone_map_file`].
fn tone_map_file_in(dir: &Path, hdr: Hdr) -> Option<PathBuf> {
    let name = match hdr {
        Hdr::Hlg => "tonemap-hlg-v2.cube",
        Hdr::Pq => "tonemap-pq-v2.cube",
    };
    let file = dir.join(name);
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
        std::fs::write(&partial, tone_map_cube(hdr)).ok()?;
        std::fs::rename(&partial, &file).ok()?;
    }
    Some(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The output value of the cube for one signal value on all three channels.
    fn gray(hdr: Hdr, signal: f64) -> [f64; 3] {
        tone_map(hdr, [signal; 3])
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
        let [r, g, b] = tone_map(Hdr::Hlg, [1.0, 0.0, 0.0]);
        assert!(
            r > 0.9 && g < 0.2 && b < 0.4,
            "BT.2020 red stays red: {r} {g} {b}"
        );
        let [r, g, b] = tone_map(Hdr::Pq, [0.0, 0.7, 0.0]);
        assert!(g > r && g > b, "BT.2020 green stays green: {r} {g} {b}");
        // A red highlight far above the PQ peak stays red at full red, not white.
        let [r, g, b] = tone_map(Hdr::Pq, [0.95, 0.6, 0.6]);
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
    fn the_cube_has_a_header_and_one_line_per_point() {
        let cube = tone_map_cube(Hdr::Hlg);
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
    fn the_cube_file_is_written_once() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let file = tone_map_file(&[&cache], Hdr::Pq).unwrap();
        assert_eq!(file, cache.join("tonemap-pq-v2.cube"));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            tone_map_cube(Hdr::Pq)
        );
        std::fs::write(&file, "kept").unwrap();
        assert_eq!(tone_map_file(&[&cache], Hdr::Pq), Some(file.clone()));
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
        for first in [odd.as_path(), unwritable] {
            assert_eq!(
                tone_map_file(&[first, &fallback], Hdr::Hlg),
                Some(fallback.join("tonemap-hlg-v2.cube")),
                "{}",
                first.display()
            );
        }
        assert!(!odd.exists());
        assert_eq!(tone_map_file(&[unwritable], Hdr::Hlg), None);
    }

    #[test]
    fn a_path_with_letters_outside_ascii_keeps_the_tone_mapping() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("Jürgen Ö/Caches");
        let file = tone_map_file(&[&cache], Hdr::Hlg).unwrap();
        assert!(file.is_file());
    }
}
