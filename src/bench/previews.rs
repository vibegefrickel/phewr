//! The embedded JPEG previews of RAW and PSD files.

use super::{CELL_PX, COLS, ROWS, millis, report, scanned_app, test_dir};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The embedded JPEG previews of RAW and PSD files against the sips decode.
#[test]
#[ignore = "reads the folder PHEWR_TEST_DIR"]
fn raw_and_psd() {
    use crate::pictures::decode::embedded_jpeg;
    use crate::test_support::{write_cr3_with_preview, write_tiff_raw_with_preview};

    let Some(dir) = test_dir() else {
        report("skipped: PHEWR_TEST_DIR is not set");
        return;
    };
    let synthetic = tempfile::tempdir().unwrap();
    let full = synthetic.path().join("full-size-preview.dng");
    write_tiff_raw_with_preview(&full, (6000, 4000));
    let cr3 = synthetic.path().join("camera.cr3");
    write_cr3_with_preview(&cr3, 1, (1620, 1080));
    let mut files = vec![full, cr3];
    let bench = scanned_app(&dir);
    let mut seen: Vec<String> = Vec::new();
    for path in &bench.app.images {
        let Some(extension) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        let extension = extension.to_ascii_lowercase();
        if embedded_jpeg::may_have_embedded_jpeg(path) && !seen.contains(&extension) {
            seen.push(extension);
            files.push(path.clone());
        }
    }
    if files.len() == 2 {
        report("embedded: no RAW or PSD file in the folder, synthetic files only");
    }
    let grid_box =
        crate::pictures::resize::padding::thumb_target_px(bench.app.gallery.thumb_box(), CELL_PX);
    let fullscreen_box = (u32::from(COLS) * CELL_PX.0, u32::from(ROWS - 1) * CELL_PX.1);
    for path in &files {
        measure_file(path, grid_box, fullscreen_box);
    }
}

/// The real camera RAW files of `PHEWR_TEST_RAW_DIR`: the route, the times and the sizes.
#[test]
#[ignore = "reads the folder PHEWR_TEST_RAW_DIR"]
fn raw_files() {
    raw_dir_files();
}

/// The previews of one file: the embedded JPEG against the sips decode.
fn measure_file(path: &Path, grid_box: (u32, u32), fullscreen_box: (u32, u32)) {
    use crate::pictures::decode::embedded_jpeg;
    use crate::pictures::decode::sips;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let start = Instant::now();
    let Some(jpeg) = embedded_jpeg::find(path) else {
        report(&format!(
            "embedded {name}: no JPEG found in {}",
            millis(start.elapsed())
        ));
        return;
    };
    let find = start.elapsed();
    let start = Instant::now();
    let grid = embedded_jpeg::decode(path, &jpeg, Some(grid_box)).ok();
    let grid_time = start.elapsed();
    let start = Instant::now();
    let fullscreen = embedded_jpeg::decode(path, &jpeg, Some(fullscreen_box)).ok();
    let fullscreen_time = start.elapsed();
    report(&format!(
        "embedded {name}: JPEG {}x{} ({} bytes) found in {:.2} ms, grid decode {} to {:?} \
         (covers the cell: {}), fullscreen decode {} to {:?} (covers: {})",
        jpeg.width,
        jpeg.height,
        jpeg.length,
        find.as_secs_f64() * 1000.0,
        millis(grid_time),
        grid.map(|decoded| (decoded.image.width(), decoded.image.height())),
        jpeg.covers(grid_box),
        millis(fullscreen_time),
        fullscreen.map(|decoded| (decoded.image.width(), decoded.image.height())),
        jpeg.covers(fullscreen_box),
    ));
    let start = Instant::now();
    let result = sips::decode(path, Some(grid_box));
    report(&format!(
        "embedded {name}: sips decode for the grid cell {} ({})",
        millis(start.elapsed()),
        result.map_or_else(|e| format!("failed: {e}"), |_| "ok".to_owned())
    ));
}

/// The real RAW files of `PHEWR_TEST_RAW_DIR`: each route, and the size that a decode returns.
fn raw_dir_files() {
    use crate::pictures::decode::{EmbeddedThumbnail, decode_image_with_hint};

    let Some(dir) = std::env::var_os("PHEWR_TEST_RAW_DIR").map(PathBuf::from) else {
        report("raw files: PHEWR_TEST_RAW_DIR is not set");
        return;
    };
    let Ok(read) = std::fs::read_dir(&dir) else {
        report("raw files: PHEWR_TEST_RAW_DIR is not a folder");
        return;
    };
    let mut files: Vec<PathBuf> = read
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| {
            let hidden = path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'));
            !hidden && path.is_file() && crate::scanner::is_supported(path)
        })
        .collect();
    files.sort();
    let bench = scanned_app(&dir);
    let grid_box =
        crate::pictures::resize::padding::thumb_target_px(bench.app.gallery.thumb_box(), CELL_PX);
    let fullscreen_box = (u32::from(COLS) * CELL_PX.0, u32::from(ROWS - 1) * CELL_PX.1);
    for path in &files {
        measure_file(path, grid_box, fullscreen_box);
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let size = |target, embedded| {
            decode_image_with_hint(path, target, embedded)
                .map(|image| (image.width(), image.height()))
                .map_err(|error| error.to_string())
        };
        report(&format!(
            "raw {name}: stored {:?}, grid decode {:?}, fullscreen decode {:?}, full decode {:?}",
            crate::pictures::decode::sips::dimensions(path),
            size(Some(grid_box), EmbeddedThumbnail::Allow),
            size(Some(fullscreen_box), EmbeddedThumbnail::Deny),
            size(None, EmbeddedThumbnail::Deny),
        ));
    }
}
