//! The phases of one HEIC decode.

use super::fullscreen_rect;

use super::{CELL_PX, millis, report, scanned_app, test_dir};
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

/// The phases of one HEIC decode, alone and under the parallel decodes of a grid.
#[test]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn decode_phases() {
    use crate::decode::{EmbeddedThumbnail, decode_image_with_hint};
    use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};

    let Some(dir) = test_dir() else {
        report("skipped: VIEWR_TEST_DIR is not set");
        return;
    };
    let bench = scanned_app(&dir);
    let heics: Vec<PathBuf> = bench
        .app
        .images
        .iter()
        .filter(|path| {
            path.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("heic"))
        })
        .cloned()
        .collect();
    let Some(path) = heics.first() else {
        report("heic phases: no HEIC file in the folder");
        return;
    };
    let box_px = (
        u32::from(fullscreen_rect().width) * CELL_PX.0,
        u32::from(fullscreen_rect().height) * CELL_PX.1,
    );

    let start = Instant::now();
    let ctx = HeifContext::read_from_file(path.to_str().unwrap()).unwrap();
    let primary = ctx.primary_image_handle().unwrap();
    let read = start.elapsed();
    let start = Instant::now();
    let image = LibHeif::new()
        .decode(&primary, ColorSpace::Rgb(RgbChroma::Rgba), None)
        .unwrap();
    let decode = start.elapsed();
    let plane = image.planes().interleaved.unwrap();
    report(&format!(
        "heic phases: read {}, libheif decode {}, plane {}x{} stride {} (row bytes {})",
        millis(read),
        millis(decode),
        plane.width,
        plane.height,
        plane.stride,
        plane.width * 4
    ));
    drop(image);

    for threads in [1u32, 2, 4, 8, 10] {
        let mut ctx = HeifContext::read_from_file(path.to_str().unwrap()).unwrap();
        ctx.set_max_decoding_threads(threads);
        let primary = ctx.primary_image_handle().unwrap();
        let start = Instant::now();
        let image = LibHeif::new()
            .decode(&primary, ColorSpace::Rgb(RgbChroma::Rgba), None)
            .unwrap();
        report(&format!(
            "heic phases: libheif decode with {threads} tile threads {}",
            millis(start.elapsed())
        ));
        drop(image);
    }

    let start = Instant::now();
    let fitted = decode_image_with_hint(path, Some(box_px), EmbeddedThumbnail::Deny).unwrap();
    let total = start.elapsed();
    report(&format!(
        "heic phases: decode_image_with_hint to {}x{} in {} (resize and copy {})",
        fitted.width(),
        fitted.height(),
        millis(total),
        millis(total.saturating_sub(decode))
    ));

    let cell_px = (528, 396);
    let start = Instant::now();
    let ctx = HeifContext::read_from_file(path.to_str().unwrap()).unwrap();
    let primary = ctx.primary_image_handle().unwrap();
    let read = start.elapsed();
    let start = Instant::now();
    let file = std::fs::File::open(path).unwrap();
    let size = file.metadata().unwrap().len();
    let reader = libheif_rs::StreamReader::new(std::io::BufReader::new(file), size);
    let streamed = HeifContext::read_from_reader(Box::new(reader)).unwrap();
    let streamed_read = start.elapsed();
    drop(streamed);
    let start = Instant::now();
    let items = crate::decode::heif_boxes::HeifItems::read(path);
    let boxes = start.elapsed();
    let mut ids = vec![0; primary.number_of_thumbnails()];
    primary.thumbnail_ids(&mut ids);
    report(&format!(
        "heic phases: preview steps: context from file {}, from stream {}, boxes {}",
        millis(read),
        millis(streamed_read),
        millis(boxes)
    ));
    match ids.first().and_then(|&id| primary.thumbnail(id).ok()) {
        Some(thumbnail) => {
            let matches = items.is_some_and(|items| items.thumbnail_matches_primary(ids[0]));
            let start = Instant::now();
            let image = LibHeif::new()
                .decode(&thumbnail, ColorSpace::Rgb(RgbChroma::Rgba), None)
                .unwrap();
            report(&format!(
                "heic phases: preview steps: thumbnail decode {}x{} {} (matches the primary image: {matches})",
                image.width(),
                image.height(),
                millis(start.elapsed())
            ));
        }
        None => report("heic phases: preview steps: no embedded thumbnail"),
    }
    let start = Instant::now();
    match crate::decode::libheif::decode_heif_preview(path, cell_px) {
        Ok(preview) => report(&format!(
            "heic phases: embedded preview {}x{} in {}",
            preview.width(),
            preview.height(),
            millis(start.elapsed())
        )),
        Err(error) => report(&format!("heic phases: no embedded preview ({error})")),
    }

    let count = heics.len().min(10);
    let start = Instant::now();
    let times: Vec<Duration> = {
        use rayon::prelude::*;
        heics[..count]
            .par_iter()
            .map(|path| {
                let one = Instant::now();
                decode_image_with_hint(path, Some(cell_px), EmbeddedThumbnail::Allow).unwrap();
                one.elapsed()
            })
            .collect()
    };
    report(&format!(
        "heic phases: {count} grid decodes in parallel: all done after {}, single decode min {} max {}",
        millis(start.elapsed()),
        millis(times.iter().copied().min().unwrap_or_default()),
        millis(times.iter().copied().max().unwrap_or_default())
    ));
}
