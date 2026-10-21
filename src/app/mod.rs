//! `App`, the whole view state, and its construction.

mod cell_shape;
mod delete;
mod directory;
mod disk_cache;
mod fullscreen;
mod grid_zoom;
mod info_panel;
mod scan;
mod slideshow;
pub(crate) mod thumbnails;
mod video;
mod zoom;

pub(crate) use delete::{DeleteMode, PendingDelete};
pub(crate) use slideshow::Slideshow;
#[cfg(feature = "video")]
pub(crate) use video::VideoReturn;

use crate::config::{self, Config};
use crate::encoder::{GraphicsBackend, GraphicsOutput, KittyBackend};
use crate::gallery::{GalleryState, ThumbnailCache};
use crate::image_list::SharedImageList;
use crate::info::ImageInfo;
use crate::kitty_output::TerminalImages;
use crate::picker::PickerState;
use crate::prefetch::Prefetcher;
use disk_cache::default_disk_cache_dir;
use image::{DynamicImage, RgbaImage};
use lru::LruCache;
use ratatui::layout::Rect;
use std::collections::HashSet;
use std::io::{self, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;
use thumbnails::{JobSlots, ThumbResult};
#[cfg(feature = "video")]
use video::VideoWindow;

const INFO_CACHE_CAPACITY: NonZeroUsize = NonZeroUsize::new(64).unwrap();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewMode {
    Gallery,
    Fullscreen,
    Picker,
    #[cfg(feature = "video")]
    Video,
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "App is the whole view state; each flag is an independent toggle"
)]
pub(crate) struct App {
    pub images: Vec<PathBuf>,
    pub filenames: Vec<String>,
    pub mode: ViewMode,
    pub cell_px: (u32, u32),
    pub graphics: KittyBackend,
    /// Where `graphics_delete_all` and the render functions write. A test app writes to a buffer.
    pub graphics_out: GraphicsOutput,

    // Info panel (gallery + fullscreen)
    pub info_visible: bool,
    info_cache: LruCache<PathBuf, ImageInfo>,

    /// The state file that remembers the grid zoom. Tests point it into a temp directory.
    pub state_file: PathBuf,
    /// The directory of the thumbnail disk cache. Tests point it into a temp directory.
    pub disk_cache_dir: PathBuf,

    // Slideshow (fullscreen)
    pub slideshow: Option<Slideshow>,
    pub slideshow_interval: Duration,
    pub slideshow_loop: bool,
    /// `--slideshow`: start once the scan has produced the image list.
    pub slideshow_autostart: bool,

    // Gallery state
    pub gallery: GalleryState,
    pub thumb_cache: ThumbnailCache,
    pub selection: HashSet<PathBuf>,
    pub pending_delete: Option<PendingDelete>,
    pub delete_error: Option<String>,

    // Directory picker state
    pub picker: Option<PickerState>,
    pub current_dir: Option<PathBuf>,
    pub initial_dir: Option<PathBuf>,

    // Fullscreen state
    pub current: usize,
    pub prefetcher: Prefetcher,
    pub loaded: Option<RgbaImage>,
    pub error: Option<String>,
    pub needs_render: bool,
    pub image_rect: Rect,
    /// The box that a fullscreen image gets in the current terminal. The draw of each frame sets
    /// it, also in the gallery, so that the pre-decode of the hovered cell has the exact size.
    pub fullscreen_box: Rect,
    pub help_visible: bool,
    loaded_for_rect: Rect,

    // Async fullscreen decode
    fullscreen_rx: Option<mpsc::Receiver<io::Result<RgbaImage>>>,
    fullscreen_target: Option<(usize, Rect)>,

    // Fullscreen zoom / pan
    pub zoom: f64,
    pan_x: f64,
    pan_y: f64,
    zoom_dirty: bool,
    /// Full-resolution decoded source for the current fullscreen image, used for zoomed crops.
    source: Option<DynamicImage>,
    source_for: Option<usize>,
    source_rx: Option<mpsc::Receiver<io::Result<DynamicImage>>>,
    source_target: Option<usize>,

    #[cfg(feature = "video")]
    pub video: Option<crate::video::VideoPlayback>,
    #[cfg(feature = "video")]
    pub video_return: VideoReturn,
    /// The mpv window that plays the current video, until it exits.
    #[cfg(feature = "video")]
    video_window: Option<VideoWindow>,

    // Scanner state
    shared_list: SharedImageList,
    known_len: usize,
    pub scan_complete: bool,

    /// The header read for the cell shape, while it runs. The thumbnail decodes wait for it.
    shape_rx: Option<mpsc::Receiver<f64>>,
    /// True after the header read started for this scan. The shape stays until the next scan.
    shape_requested: bool,

    // Async filter
    filter_rx: Option<mpsc::Receiver<Vec<usize>>>,
    /// The cursor position for the next filter result. A delete sets it, a new query clears it.
    filter_cursor: Option<usize>,

    // Async thumbnails
    thumb_tx: mpsc::Sender<ThumbResult>,
    thumb_rx: mpsc::Receiver<ThumbResult>,
    /// The sharp thumbnail decodes, at most `MAX_THUMB_JOBS` at once.
    thumb_jobs: JobSlots,
    /// The HEIC preview decodes, at most `MAX_PREVIEW_JOBS` at once.
    preview_jobs: JobSlots,
    thumb_generation: u32,
    /// The Kitty ids of previews that a sharp thumbnail replaced, with their cell. The next
    /// write of the grid deletes them in the terminal.
    pub stale_image_ids: Vec<(usize, u32)>,

    /// The images that the terminal stores, so a cell can place one by its id instead of a
    /// transmit. The grid keeps their bytes below a budget.
    pub terminal_images: TerminalImages,
    /// True when the LRU thumb cache was invalidated but backend storage has not been cleared yet.
    /// Consumed by `render_gallery_images` to issue a full `d=A` at the next redraw.
    pub graphics_storage_dirty: bool,
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
        let mut app = Self::with_config(cell_px, shared_list, config);
        app.state_file = PathBuf::from("/dev/null/viewr/state.toml");
        app.disk_cache_dir = PathBuf::from("/dev/null/viewr/cache");
        app.graphics_out = GraphicsOutput::buffer();
        app
    }

    pub(crate) fn with_config(
        cell_px: (u32, u32),
        shared_list: SharedImageList,
        config: &Config,
    ) -> Self {
        let (thumb_tx, thumb_rx) = mpsc::channel();
        let mut gallery = GalleryState::with_zoom_rows(0, usize::from(config.visible_rows));
        gallery.default_zoom_rows = usize::from(config.configured_visible_rows).clamp(
            usize::from(config::MIN_VISIBLE_ROWS),
            usize::from(config::MAX_VISIBLE_ROWS),
        );
        gallery.set_cell_shape(crate::gallery::DEFAULT_IMAGE_ASPECT, cell_px);
        Self {
            images: Vec::new(),
            filenames: Vec::new(),
            mode: ViewMode::Gallery,
            cell_px,
            graphics: KittyBackend::default(),
            graphics_out: GraphicsOutput::Stdout,
            info_visible: config.info_panel,
            info_cache: LruCache::new(INFO_CACHE_CAPACITY),
            state_file: config::default_state_path(),
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
            picker: None,
            current_dir: None,
            initial_dir: None,
            current: 0,
            prefetcher: Prefetcher::new(),
            loaded: None,
            error: None,
            needs_render: true,
            image_rect: Rect::default(),
            fullscreen_box: Rect::default(),
            help_visible: false,
            loaded_for_rect: Rect::default(),
            fullscreen_rx: None,
            fullscreen_target: None,
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            zoom_dirty: false,
            source: None,
            source_for: None,
            source_rx: None,
            source_target: None,
            #[cfg(feature = "video")]
            video: None,
            #[cfg(feature = "video")]
            video_return: VideoReturn::Gallery,
            #[cfg(feature = "video")]
            video_window: None,
            shared_list,
            known_len: 0,
            scan_complete: false,
            shape_rx: None,
            shape_requested: false,
            filter_rx: None,
            filter_cursor: None,
            thumb_tx,
            thumb_rx,
            thumb_jobs: Self::thumb_slots(),
            preview_jobs: Self::preview_slots(),
            thumb_generation: 0,
            stale_image_ids: Vec::new(),
            terminal_images: TerminalImages::new(),
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
        self.graphics.delete_all_to(out)?;
        self.terminal_images.clear();
        Ok(())
    }

    pub(crate) fn current_path(&self) -> &Path {
        &self.images[self.current]
    }

    /// True while a background job runs or waits: a decode for the view, a prefetch, a
    /// thumbnail, a filter, or the scan. The loop then looks for results every few milliseconds.
    /// The prefetch counts, because only `Prefetcher::poll` starts its waiting jobs.
    pub(crate) fn has_pending_work(&self) -> bool {
        self.fullscreen_rx.is_some()
            || self.source_rx.is_some()
            || self.thumbnails_pending()
            || self.prefetcher.has_jobs()
            || self.filter_rx.is_some()
            || self.shape_rx.is_some()
            || !self.scan_complete
    }

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
