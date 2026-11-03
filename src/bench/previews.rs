//! The embedded JPEG previews of RAW and PSD files.

use super::{CELL_PX, COLS, ROWS, millis, report, scanned_app, test_dir};
use std::time::Instant;

/// The embedded JPEG previews of RAW and PSD files against the sips decode.
#[test]
#[ignore = "reads the folder VIEWR_TEST_DIR"]
fn raw_and_psd() {
    use crate::decode::embedded_jpeg;
    use crate::decode::sips;
    use crate::test_support::{write_cr3_with_preview, write_tiff_raw_with_preview};

    let Some(dir) = test_dir() else {
        report("skipped: VIEWR_TEST_DIR is not set");
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
    let grid_box = crate::app::thumbnails::thumb_target_px(bench.app.gallery.thumb_box(), CELL_PX);
    let fullscreen_box = (u32::from(COLS) * CELL_PX.0, u32::from(ROWS - 1) * CELL_PX.1);
    for path in &files {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let start = Instant::now();
        let Some(jpeg) = embedded_jpeg::find(path) else {
            report(&format!(
                "embedded {name}: no JPEG found in {}",
                millis(start.elapsed())
            ));
            continue;
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
}
