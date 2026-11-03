//! The prefetch thread: the first screen of the folder and its decode into the disk cache.

use crate::app::cell_shape::SHAPE_SAMPLE;
use crate::app::thumbnails::{thumb_target_px, warm_disk_cache};
use crate::decode::shape;
use crate::gallery::GalleryState;
use crate::scanner;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

/// The wait of a paused prefetch between two looks at its flags. An unpark ends it early.
const PAUSE_POLL: Duration = Duration::from_millis(50);

/// The flags that the UI thread sets and the thread reads before each file.
#[derive(Default)]
pub(super) struct Control {
    pub(super) cancel: AtomicBool,
    pub(super) paused: AtomicBool,
}

impl Control {
    /// Wait while the prefetch is paused. `false` once it is cancelled.
    pub(super) fn proceed(&self) -> bool {
        while self.paused.load(Ordering::Acquire) && !self.cancelled() {
            thread::park_timeout(PAUSE_POLL);
        }
        !self.cancelled()
    }

    pub(super) fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
}

/// What the thread needs: the folder, the folder of the grid and the grid that the switch
/// starts from.
pub(super) struct Request {
    pub(super) folder: PathBuf,
    /// The cursor of a step to the parent lands on this folder.
    pub(super) from: PathBuf,
    pub(super) grid: GalleryState,
    pub(super) zoom_rows: usize,
    pub(super) cell_px: (u32, u32),
    pub(super) cache_dir: PathBuf,
}

/// The files of the first screen and their decode box.
#[derive(Debug, Default, PartialEq)]
pub(super) struct FirstScreen {
    pub(super) files: Vec<PathBuf>,
    pub(super) target: (u32, u32),
}

/// The first screen that a switch to `request.folder` shows. A folder with fewer than 2 files
/// gets no prefetch: its grid saves one decode at most.
pub(super) fn first_screen(request: &Request) -> FirstScreen {
    let (folders, files, names) = scanner::folder_listing(&request.folder);
    if files.len() < 2 {
        return FirstScreen::default();
    }
    // The scan samples the first files by name, and the files come sorted.
    let aspect = shape::folder_aspect(&files[..files.len().min(SHAPE_SAMPLE)]);
    let media: Vec<(PathBuf, String)> = files.into_iter().zip(names).collect();
    let folder_count = folders.len();
    let entries: Vec<PathBuf> = folders
        .into_iter()
        .map(|folder| folder.path)
        .chain(media.into_iter().map(|(path, _)| path))
        .collect();
    // As `enter_selected_folder`: a step to the parent puts the cursor on the folder before.
    let cursor = (request.from.parent() == Some(request.folder.as_path()))
        .then(|| entries.iter().position(|entry| *entry == request.from))
        .flatten()
        .unwrap_or(0);
    // The cursor moves at the end of the scan, and the shape comes when its header read ends.
    // Either can come first, with another scroll offset. Both screens count.
    let base = request.grid.for_folder(entries.len(), request.zoom_rows);
    let mut cursor_first = base.clone();
    cursor_first.move_cursor_to(cursor);
    cursor_first.set_cell_shape(aspect, request.cell_px);
    let mut shape_first = base;
    shape_first.set_cell_shape(aspect, request.cell_px);
    shape_first.move_cursor_to(cursor);
    let positions: BTreeSet<usize> = cursor_first
        .visible_range()
        .chain(shape_first.visible_range())
        .filter(|&position| position >= folder_count)
        .collect();
    FirstScreen {
        files: positions
            .into_iter()
            .map(|position| entries[position].clone())
            .collect(),
        target: thumb_target_px(cursor_first.thumb_box(), request.cell_px),
    }
}

/// One file after the other, never two in parallel.
pub(super) fn prefetch(request: &Request, control: &Control) {
    if !control.proceed() {
        return;
    }
    let screen = first_screen(request);
    for path in &screen.files {
        if !control.proceed() {
            return;
        }
        warm_disk_cache(&request.cache_dir, path, screen.target);
    }
}
