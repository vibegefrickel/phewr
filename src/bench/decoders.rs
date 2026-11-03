//! The resize filters and the color conversion.

use super::{millis, report, test_dir};
use crate::decode::color;
use crate::decode::color::ColorTarget;
use crate::decode::hdr::Peak;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

/// The resize filters on a 24 MP frame.
#[test]
#[ignore = "reads the folder PHEWR_TEST_DIR"]
fn resize_filters_24_mp() {
    use fast_image_resize as fir;
    let (w, h) = (5712u32, 4284u32);
    let pixels: Vec<u8> = (0..w * h * 4).map(|i| (i % 251) as u8).collect();
    let src = fir::images::ImageRef::new(w, h, &pixels, fir::PixelType::U8x4).unwrap();
    let algs = [
        (
            "lanczos3",
            fir::ResizeAlg::Convolution(fir::FilterType::Lanczos3),
        ),
        (
            "catmull_rom",
            fir::ResizeAlg::Convolution(fir::FilterType::CatmullRom),
        ),
        (
            "bilinear",
            fir::ResizeAlg::Convolution(fir::FilterType::Bilinear),
        ),
        ("box", fir::ResizeAlg::Convolution(fir::FilterType::Box)),
        (
            "supersampling_bilinear_2",
            fir::ResizeAlg::SuperSampling(fir::FilterType::Bilinear, 2),
        ),
        ("nearest", fir::ResizeAlg::Nearest),
    ];
    for (name, alg) in algs {
        let mut dst = fir::images::Image::new(2688, 2016, fir::PixelType::U8x4);
        let options = fir::ResizeOptions::new().resize_alg(alg);
        let start = Instant::now();
        fir::Resizer::new()
            .resize(&src, &mut dst, &options)
            .unwrap();
        let full = start.elapsed();
        let mut dst = fir::images::Image::new(528, 396, fir::PixelType::U8x4);
        let start = Instant::now();
        fir::Resizer::new()
            .resize(&src, &mut dst, &options)
            .unwrap();
        report(&format!(
            "resize {name}: to 2688x2016 {}, to 528x396 {}",
            millis(full),
            millis(start.elapsed())
        ));
    }
}

/// The color conversion alone, at the fullscreen box, a grid cell and a 24 MP frame (median of 5).
#[test]
#[ignore = "reads the folder PHEWR_TEST_DIR"]
fn color_conversion() {
    use crate::decode::resize::fit_rgba_within;
    use crate::decode::{EmbeddedThumbnail, decode_image};
    let Some(dir) = test_dir() else { return };
    let median = |work: &dyn Fn() -> Duration| {
        let mut times: Vec<Duration> = (0..5).map(|_| work()).collect();
        times.sort();
        times[2]
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    files.sort();
    for extension in ["heic", "jpg", "png"] {
        let Some(path) = files.iter().find(|path| {
            path.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(extension))
        }) else {
            continue;
        };
        for (label, box_px) in [("fullscreen", (2688, 2016)), ("grid", (528, 396))] {
            let decoded = decode_image(path, Some(box_px), EmbeddedThumbnail::Deny).unwrap();
            let source = decoded.color.clone();
            let fitted = fit_rgba_within(decoded.image.into_rgba8(), box_px.0, box_px.1);
            let time = |target: ColorTarget| {
                median(&|| {
                    let copy = fitted.clone();
                    let start = Instant::now();
                    let converted = color::convert(copy, &source, target, Peak::DEFAULT);
                    let elapsed = start.elapsed();
                    assert_eq!(converted.dimensions(), fitted.dimensions());
                    elapsed
                })
            };
            let kind = match &source {
                color::SourceColor::Icc(data) => format!("ICC of {} bytes", data.len()),
                other => format!("{other:?}"),
            };
            report(&format!(
                "color {extension} {label} {}x{} ({kind}): into Display P3 {:.2} ms, into sRGB {:.2} ms",
                fitted.width(),
                fitted.height(),
                time(ColorTarget::DisplayP3).as_secs_f64() * 1000.0,
                time(ColorTarget::Srgb).as_secs_f64() * 1000.0,
            ));
        }
    }
    let (w, h) = (5712u32, 4284u32);
    let frame = image::RgbaImage::from_fn(w, h, |x, y| {
        image::Rgba([(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8, 255])
    });
    let full = median(&|| {
        let copy = frame.clone();
        let start = Instant::now();
        let converted = color::convert(
            copy,
            &color::SourceColor::Srgb,
            ColorTarget::DisplayP3,
            Peak::DEFAULT,
        );
        let elapsed = start.elapsed();
        assert_eq!(converted.width(), w);
        elapsed
    });
    report(&format!(
        "color 24 MP sRGB frame into Display P3 (zoom source): {:.1} ms",
        full.as_secs_f64() * 1000.0
    ));
}
