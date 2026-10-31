//! `App`, the whole view state, and its construction.

mod cell_shape;
mod delete;
mod directory;
mod disk_cache;
mod folder_prefetch;
mod folders;
mod fullscreen;
pub(crate) mod grid_loops;
mod grid_zoom;
mod info_panel;
mod meta;
pub(crate) mod mouse;
mod scan;
mod slideshow;
pub(crate) mod thumbnails;
mod video;
mod zoom;

pub(crate) use delete::{DeleteMode, PendingDelete};
pub(crate) use mouse::{Drag, MouseState, ScreenLayout};
pub(crate) use slideshow::Slideshow;
#[cfg(feature = "video")]
pub(crate) use video::VideoReturn;

use crate::config::{self, Config};
use crate::decode::ScreenImage;
use crate::encoder::{GraphicsOutput, KittyBackend};
use crate::gallery::{GalleryState, ThumbnailCache};
pub(crate) use crate::image_list::EntryKind;
use crate::image_list::SharedImageList;
use crate::info::ImageInfo;
use crate::kitty_output::TerminalImages;
use crate::prefetch::Prefetcher;
use disk_cache::default_disk_cache_dir;
use lru::LruCache;
use ratatui::layout::Rect;
use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;
#[cfg(feature = "video")]
use video::VideoWindow;

const INFO_CACHE_CAPACITY: NonZeroUsize = NonZeroUsize::new(64).unwrap();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewMode {
    Gallery,
    Fullscreen,
    #[cfg(feature = "video")]
    Video,
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "App is the whole view state; each flag is an independent toggle"
)]
pub(crate) struct App {
    /// The entries of the grid: the folders first, then the files. The fullscreen view, the
    /// slideshow, the prefetch, the thumbnails and the delete never touch the folders.
    pub(crate) images: Vec<PathBuf>,
    pub(crate) filenames: Vec<String>,
    /// The first `folder_count` entries of `images` are folders. `..` is the first one.
    pub(crate) folder_count: usize,
    pub(crate) mode: ViewMode,
    pub(crate) cell_px: (u32, u32),
    pub(crate) graphics: KittyBackend,
    /// Where `graphics_delete_all` and the render functions write. A test app writes to a buffer.
    pub(crate) graphics_out: GraphicsOutput,

    // Info panel (gallery + fullscreen)
    pub(crate) info_visible: bool,
    info_cache: LruCache<PathBuf, ImageInfo>,

    /// The state file that remembers the grid zoom. Tests point it into a temp directory.
    pub(crate) state_file: PathBuf,
    /// The directory of the thumbnail disk cache. Tests point it into a temp directory.
    pub(crate) disk_cache_dir: PathBuf,

    // Slideshow (fullscreen)
    pub(crate) slideshow: Option<Slideshow>,
    pub(crate) slideshow_interval: Duration,
    pub(crate) slideshow_loop: bool,
    /// `--slideshow`: start once the scan has produced the image list.
    pub(crate) slideshow_autostart: bool,

    // Gallery state
    pub(crate) gallery: GalleryState,
    pub(crate) thumb_cache: ThumbnailCache,
    pub(crate) selection: HashSet<PathBuf>,
    pub(crate) pending_delete: Option<PendingDelete>,
    pub(crate) delete_error: Option<String>,

    pub(crate) current_dir: Option<PathBuf>,
    pub(crate) initial_dir: Option<PathBuf>,
    /// The entry that the cursor lands on when the next scan ends: the folder that the user
    /// came from after a step to the parent.
    return_to: Option<PathBuf>,
    /// The decode of the first screen of the folder under the cursor into the disk cache.
    folder_prefetch: folder_prefetch::FolderPrefetch,
    /// The facts of the file under the cursor for the status row.
    meta: meta::MetaState,

    // Fullscreen state
    pub(crate) current: usize,
    pub(crate) prefetcher: Prefetcher,
    /// The fullscreen image or zoom crop on the screen, RGB when every pixel is opaque.
    pub(crate) loaded: Option<ScreenImage>,
    pub(crate) error: Option<String>,
    /// The read errors of the scan. Only the status row reads it. Like the status note, the next key press clears it, and a decode does not.
    pub(crate) scan_error: Option<String>,
    pub(crate) needs_render: bool,
    /// The content box of the last fullscreen or video draw. The gallery draw leaves it alone.
    pub(crate) image_rect: Rect,
    /// The box that a fullscreen image gets in the current terminal. The draw of each frame sets
    /// it, also in the gallery, so that the pre-decode of the hovered cell has the exact size.
    pub(crate) fullscreen_box: Rect,
    pub(crate) help_visible: bool,
    /// The areas of the last draw that a click lands on.
    pub(crate) layout: ScreenLayout,
    pub(crate) mouse: MouseState,
    loaded_for_rect: Rect,
    /// The decode of the fullscreen image, while it runs.
    fullscreen_decode: Option<fullscreen::FullscreenDecode>,

    /// The zoom factor of the fullscreen view, 1.0 for the fitted image.
    pub(crate) zoom: f64,
    /// The pan and the full-resolution source of the zoomed view.
    zoom_state: zoom::ZoomState,

    #[cfg(feature = "video")]
    pub(crate) video: Option<crate::player::Player>,
    #[cfg(feature = "video")]
    pub(crate) video_return: VideoReturn,
    /// `p` asked for the mpv window. The run loop takes the request.
    #[cfg(feature = "video")]
    video_window_request: bool,
    /// The mpv window that plays the current video, until it exits.
    #[cfg(feature = "video")]
    video_window: Option<VideoWindow>,
    /// The first frame of a new video removes the other pictures first, so that the photo of
    /// the view before does not show around the frame.
    #[cfg(feature = "video")]
    pub(crate) clear_before_video: bool,
    /// The terminal player that starts while the gallery cursor rests on a video.
    #[cfg(feature = "video")]
    pub(crate) video_prestart: crate::video_handoff::VideoPrestart,

    // Scanner state
    shared_list: SharedImageList,
    known_len: usize,
    pub(crate) scan_complete: bool,

    /// The header read for the cell shape and its start, while it runs. The thumbnail decodes
    /// wait for it, and the run loop waits less during its first milliseconds.
    shape_rx: Option<(mpsc::Receiver<f64>, std::time::Instant)>,
    /// True after the header read started for this scan. The shape stays until the next scan.
    shape_requested: bool,

    // Async filter
    filter_rx: Option<mpsc::Receiver<Vec<usize>>>,
    /// The long-lived thread behind `filter_rx`.
    filter_worker: crate::search::FilterWorker,
    /// The cursor position for the next filter result. A delete sets it, a new query clears it.
    filter_cursor: Option<usize>,

    /// The thumbnail jobs of the grid.
    thumb_jobs: thumbnails::ThumbnailJobs,
    /// The Kitty ids of previews that a sharp thumbnail replaced, with their cell. The next
    /// write of the grid deletes them in the terminal.
    pub(crate) stale_image_ids: Vec<(usize, u32)>,
    /// The animation flag of each file that a thumbnail job read: true for an animated GIF,
    /// WebP, APNG or AVIS. The grid marker reads it, so the draw reads no file header.
    pub(crate) animated: HashMap<PathBuf, bool>,

    /// The images that the terminal stores, so a cell can place one by its id instead of a
    /// transmit. The grid keeps their bytes below a budget.
    pub(crate) terminal_images: TerminalImages,
    /// The loops of the visible video and animated cells.
    pub(crate) grid_loops: grid_loops::GridLoops,
    /// True when the LRU thumb cache was invalidated but backend storage has not been cleared yet.
    /// Consumed by `render_gallery_images` to issue a full `d=A` at the next redraw.
    pub(crate) graphics_storage_dirty: bool,
}

impl App {
    /// A test app. The state file and the disk cache sit below `/dev/null`, so every write
    /// fails quietly and no test can touch the real files. A test that needs them sets a temp path.
    #[cfg(test)]
    pub(crate) fn new(cell_px: (u32, u32), shared_list: SharedImageList) -> Self {
        Self::new_with_config(cell_px, shared_list, &Config::default())
    }

    /// A test app with `config`. The Kitty sequences go to a buffer, not to the terminal.
    #[cfg(test)]
    pub(crate) fn new_with_config(
        cell_px: (u32, u32),
        shared_list: SharedImageList,
        config: &Config,
    ) -> Self {
        let mut app = Self::with_config(
            cell_px,
            shared_list,
            config,
            PathBuf::from("/dev/null/viewr-test/state.toml"),
        );
        app.state_file = PathBuf::from("/dev/null/viewr/state.toml");
        app.disk_cache_dir = PathBuf::from("/dev/null/viewr/cache");
        app.graphics_out = GraphicsOutput::buffer();
        app
    }

    pub(crate) fn with_config(
        cell_px: (u32, u32),
        shared_list: SharedImageList,
        config: &Config,
        state_file: PathBuf,
    ) -> Self {
        let mut gallery = GalleryState::with_zoom_rows(0, usize::from(config.visible_rows));
        gallery.default_zoom_rows = usize::from(config.configured_visible_rows).clamp(
            usize::from(config::MIN_VISIBLE_ROWS),
            usize::from(config::MAX_VISIBLE_ROWS),
        );
        gallery.set_cell_shape(crate::gallery::DEFAULT_IMAGE_ASPECT, cell_px);
        Self {
            images: Vec::new(),
            filenames: Vec::new(),
            folder_count: 0,
            mode: ViewMode::Gallery,
            cell_px,
            graphics: KittyBackend::default(),
            graphics_out: GraphicsOutput::Stdout,
            info_visible: config.info_panel,
            info_cache: LruCache::new(INFO_CACHE_CAPACITY),
            state_file,
            disk_cache_dir: default_disk_cache_dir(),
            slideshow: None,
            slideshow_interval: Duration::from_secs_f64(config.slideshow_interval),
            slideshow_loop: config.slideshow_loop,
            slideshow_autostart: false,
            gallery,
            thumb_cache: ThumbnailCache::new(),
            selection: HashSet::new(),
            pending_delete: None,
            delete_error: None,
            current_dir: None,
            initial_dir: None,
            return_to: None,
            meta: meta::MetaState::default(),
            current: 0,
            prefetcher: Prefetcher::new(),
            loaded: None,
            error: None,
            scan_error: None,
            needs_render: true,
            image_rect: Rect::default(),
            fullscreen_box: Rect::default(),
            help_visible: false,
            layout: ScreenLayout::default(),
            mouse: MouseState::default(),
            loaded_for_rect: Rect::default(),
            fullscreen_decode: None,
            zoom: 1.0,
            zoom_state: zoom::ZoomState::default(),
            #[cfg(feature = "video")]
            video: None,
            #[cfg(feature = "video")]
            video_return: VideoReturn::Gallery,
            #[cfg(feature = "video")]
            video_window_request: false,
            #[cfg(feature = "video")]
            video_window: None,
            #[cfg(feature = "video")]
            clear_before_video: false,
            #[cfg(feature = "video")]
            video_prestart: crate::video_handoff::VideoPrestart::default(),
            shared_list,
            known_len: 0,
            scan_complete: false,
            shape_rx: None,
            shape_requested: false,
            folder_prefetch: folder_prefetch::FolderPrefetch::default(),
            filter_rx: None,
            filter_worker: crate::search::FilterWorker::default(),
            filter_cursor: None,
            thumb_jobs: thumbnails::ThumbnailJobs::new(),
            stale_image_ids: Vec::new(),
            animated: HashMap::new(),
            terminal_images: TerminalImages::new(),
            grid_loops: grid_loops::GridLoops::new(config.animate),
            graphics_storage_dirty: false,
        }
    }

    /// Run `write` with a handle on the graphics output, so that the output and the app can
    /// both change inside. The `_to` twin of each write takes any `Write`, for the tests.
    pub(crate) fn with_graphics_out(
        &mut self,
        write: impl FnOnce(&mut Self, &mut GraphicsOutput) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut out = self.graphics_out.clone();
        write(self, &mut out)
    }

    /// Wipe backend storage and drop transmitted-image tracking.
    pub(crate) fn graphics_delete_all(&mut self) -> io::Result<()> {
        self.with_graphics_out(|app, out| {
            app.graphics_delete_all_to(out)?;
            out.flush()
        })
    }

    pub(crate) fn graphics_delete_all_to<W: Write>(&mut self, out: &mut W) -> io::Result<()> {
        crate::encoder::kitty::delete_all_to(out)?;
        self.terminal_images.clear();
        self.grid_loops.forget_placements();
        Ok(())
    }

    pub(crate) fn current_path(&self) -> &Path {
        &self.images[self.current]
    }

    /// True while a background job runs or waits: a decode for the view, a prefetch, a
    /// thumbnail, a grid loop, a filter, or the scan. The loop then looks for results every few milliseconds.
    /// The prefetch counts, because only `Prefetcher::poll` starts its waiting jobs.
    pub(crate) fn has_pending_work(&self) -> bool {
        self.has_pending_work_but_loops() || self.grid_loops_pending()
    }

    /// True while any background job except a grid loop job runs. A loop result needs no short
    /// poll, so the run loop waits the idle time for it.
    pub(crate) fn has_pending_work_but_loops(&self) -> bool {
        self.fullscreen_decode.is_some()
            || self.zoom_state.source_rx.is_some()
            || self.thumbnails_pending()
            || self.prefetcher.has_jobs()
            || self.filter_rx.is_some()
            || self.shape_rx.is_some()
            || !self.scan_complete
            || self.meta_pending()
    }

    /// Drop the shown image, the thumbnails and the prefetch, because the list or the cell size
    /// changed.
    pub(crate) fn mark_dirty(&mut self) {
        self.loaded = None;
        self.forget_thumbnails();
        self.prefetcher.invalidate();
        self.graphics_storage_dirty = true;
        self.needs_render = true;
    }
}

#[cfg(test)]
mod tests {

    use crate::test_support::{app, app_with};

    #[test]
    fn apps_from_new_never_write_the_real_state_or_cache() {
        let mut app = app_with(&["a.png"]);
        assert!(app.state_file.starts_with("/dev/null"));
        assert!(app.disk_cache_dir.starts_with("/dev/null"));
        app.thumb_zoom_in();
        assert!(!app.state_file.exists());
        app.thumb_zoom_reset();
    }

    #[test]
    fn graphics_delete_all_forgets_the_transmitted_ids() {
        let mut app = app();
        app.terminal_images.insert(7, 16);
        let mut out = Vec::new();
        app.graphics_delete_all_to(&mut out).unwrap();
        assert!(!out.is_empty());
        assert!(app.terminal_images.is_empty());

        app.terminal_images.insert(8, 16);
        app.graphics_delete_all().unwrap();
        assert_eq!(
            app.graphics_out.take_bytes(),
            out,
            "the test buffer, not stdout"
        );
        assert!(app.terminal_images.is_empty());
        assert_eq!(app.terminal_images.bytes(), 0);
    }
}
