//! The fullscreen layouts against the RGBA layout. Each step runs both ways in turn.

use crate::app::App;
use crate::decode::color::{self, ColorTarget};
use crate::decode::resize::{rgb_resize_is_fastest, scaled_down_size};
use crate::decode::screen::ScreenImage;
use crate::decode::{EmbeddedThumbnail, decode_image};
use crate::encoder::{KittyBackend, Transfer, kitty};
use crate::image_list::SharedImageList;
use crate::kitty_output::render_fullscreen_image_to;
use image::{DynamicImage, RgbaImage};
use ratatui::layout::Rect;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The fullscreen box in pixels, as `fullscreen_rect` and `CELL_PX` of `bench/mod.rs` give it.
const BOX_PX: (u32, u32) = (3392, 2052);
const RUNS: usize = 9;

fn medians(
    mut before: impl FnMut() -> Duration,
    mut after: impl FnMut() -> Duration,
) -> (f64, f64) {
    let (mut old, mut new) = (Vec::new(), Vec::new());
    for _ in 0..RUNS {
        old.push(before());
        new.push(after());
    }
    let median = |times: &mut Vec<Duration>| {
        times.sort();
        times[RUNS / 2].as_secs_f64() * 1000.0
    };
    (median(&mut old), median(&mut new))
}

fn report(step: &str, (before, after): (f64, f64)) {
    // clippy.toml allows output in tests. `just bench` shows the lines.
    eprintln!("bench layout {step}: before {before:.1} ms, after {after:.1} ms");
}

fn time<T>(work: impl FnOnce() -> T) -> Duration {
    let start = Instant::now();
    let result = work();
    let elapsed = start.elapsed();
    drop(result);
    elapsed
}

/// The fullscreen write of `image` through the temp file medium.
fn write(app: &mut App, image: ScreenImage) -> Duration {
    app.loaded = Some(image);
    let mut out = Vec::new();
    let elapsed = time(|| render_fullscreen_image_to(app, &mut out).unwrap());
    for path in kitty::temp_file_paths(&out) {
        std::fs::remove_file(path).unwrap();
    }
    elapsed
}

fn fullscreen_app() -> App {
    let mut app = App::new((32, 54), SharedImageList::new());
    app.graphics = KittyBackend::new(Transfer::TempFile);
    app.image_rect = Rect::new(0, 0, 106, 38);
    app
}

/// The fit of the full decode and the color conversion: RGBA with the alpha multiply against
/// `rgb_resize_is_fastest`.
fn measure_fit(name: &str, full: &RgbaImage, color: &color::SourceColor) {
    let size = full.dimensions();
    let dst = scaled_down_size(size, BOX_PX).unwrap_or(size);
    let after_input = if rgb_resize_is_fastest(size, Some(dst)) {
        ScreenImage::Rgb(DynamicImage::ImageRgba8(full.clone()).into_rgb8())
    } else {
        ScreenImage::Opaque(full.clone())
    };
    let before_input = ScreenImage::Rgba(full.clone());
    for target in [ColorTarget::DisplayP3, ColorTarget::Srgb] {
        let times = medians(
            || {
                time(|| {
                    before_input
                        .crop_resized((0, 0, size.0, size.1), dst)
                        .convert(color, target)
                })
            },
            || {
                let input = after_input.clone();
                time(|| input.fit_within(BOX_PX.0, BOX_PX.1).convert(color, target))
            },
        );
        report(
            &format!(
                "{name} fit {}x{} into {}x{} and convert into {target:?}",
                size.0, size.1, dst.0, dst.1
            ),
            times,
        );
    }
}

fn measure(path: &Path) {
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    let decoded = decode_image(path, None, EmbeddedThumbnail::Deny).unwrap();
    let color = decoded.color.clone();
    let full = decoded.image.to_rgba8();
    measure_fit(&name, &full, &color);

    let shown = ScreenImage::Opaque(full.clone()).fit_within(BOX_PX.0, BOX_PX.1);
    let rgba = ScreenImage::Rgba(shown.clone().into_rgba());
    let (mut rgba_app, mut rgb_app) = (fullscreen_app(), fullscreen_app());
    let writes = medians(
        || write(&mut rgba_app, rgba.clone()),
        || write(&mut rgb_app, shown.clone()),
    );
    report(
        &format!(
            "{name} fullscreen write {}x{}",
            shown.width(),
            shown.height()
        ),
        writes,
    );

    let (w, h) = full.dimensions();
    let old_source = DynamicImage::ImageRgba8(full.clone());
    let new_source = ScreenImage::Opaque(full).for_crops();
    for zoom in [1.25, 2.0, 4.0] {
        // The crop of `build_zoom_view` in the middle of the picture.
        let eff = f64::min(
            f64::from(BOX_PX.0) / f64::from(w),
            f64::from(BOX_PX.1) / f64::from(h),
        )
        .min(1.0)
            * zoom;
        let crop_w = ((f64::from(BOX_PX.0) / eff).round() as u32).clamp(1, w);
        let crop_h = ((f64::from(BOX_PX.1) / eff).round() as u32).clamp(1, h);
        let crop = ((w - crop_w) / 2, (h - crop_h) / 2, crop_w, crop_h);
        let dst_w = ((f64::from(crop_w) * eff).round() as u32).clamp(1, BOX_PX.0);
        let dst_h = ((f64::from(crop_h) * eff).round() as u32).clamp(1, BOX_PX.1);
        let (mut rgba_app, mut rgb_app) = (fullscreen_app(), fullscreen_app());
        let times = medians(
            || {
                // Before: a crop copy, an RGBA copy, the alpha multiply resize and the scanning write.
                let start = Instant::now();
                let copy = old_source
                    .crop_imm(crop.0, crop.1, crop_w, crop_h)
                    .to_rgba8();
                let resized =
                    ScreenImage::Rgba(copy).crop_resized((0, 0, crop_w, crop_h), (dst_w, dst_h));
                write(&mut rgba_app, resized);
                start.elapsed()
            },
            || {
                let start = Instant::now();
                let crop = new_source.crop_resized(crop, (dst_w, dst_h));
                write(&mut rgb_app, crop);
                start.elapsed()
            },
        );
        report(
            &format!(
                "{name} zoom {zoom} pan step {crop_w}x{crop_h} into {dst_w}x{dst_h}, crop and write"
            ),
            times,
        );
    }
}

fn measure_decoders(jpeg: Option<&PathBuf>, heic: Option<&PathBuf>) {
    if let Some(path) = jpeg {
        let data = std::fs::read(path).unwrap();
        let decode = |format| time(|| turbojpeg::decompress(&data, format).unwrap());
        let times = medians(
            || decode(turbojpeg::PixelFormat::RGBA),
            || decode(turbojpeg::PixelFormat::RGB),
        );
        report("libjpeg-turbo full decode, RGBA before, RGB after", times);
    }
    #[cfg(feature = "heic")]
    if let Some(path) = heic {
        use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};
        let ctx = HeifContext::read_from_file(path.to_str().unwrap()).unwrap();
        let handle = ctx.primary_image_handle().unwrap();
        let decode = |chroma| {
            time(|| {
                LibHeif::new()
                    .decode(&handle, ColorSpace::Rgb(chroma), None)
                    .unwrap()
            })
        };
        let times = medians(|| decode(RgbChroma::Rgba), || decode(RgbChroma::Rgb));
        report("libheif full decode, RGBA before, RGB after", times);
    }
    #[cfg(not(feature = "heic"))]
    let _ = heic;
}

/// The fullscreen fit, write and decoders in the new layouts against the RGBA layout.
#[test]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn layouts_against_rgba() {
    let Some(dir) = std::env::var_os("VIEWR_TEST_DIR").map(PathBuf::from) else {
        eprintln!("skipped: VIEWR_TEST_DIR is not set");
        return;
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    files.sort();
    let first = |extension: &str| {
        files.iter().find(|path| {
            path.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(extension))
        })
    };
    for path in ["heic", "jpg", "png"].into_iter().filter_map(first) {
        measure(path);
    }
    measure_decoders(first("jpg"), first("heic"));
}
