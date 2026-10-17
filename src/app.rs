use crate::config::{self, Config};
use crate::encoder::{GraphicsBackend, GraphicsOutput, KittyBackend};
use crate::gallery::{GalleryState, ThumbnailCache};
use crate::image_list::SharedImageList;
use crate::info::{self, ImageInfo};
use crate::picker::{self, PickerState};
use crate::prefetch::Prefetcher;
use crate::scanner;
use crate::search;
use crate::theme::{NamedTheme, Theme};
use directories::ProjectDirs;
use fast_image_resize as fir;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader, RgbaImage};
use lru::LruCache;
use ratatui::layout::Rect;
use std::collections::HashSet;
use std::io::{self, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, mpsc};
use std::time::{Duration, Instant};

/// Multiplier applied on each zoom step.
const ZOOM_STEP: f64 = 1.25;
/// Maximum zoom factor (relative to fit-to-window).
const MAX_ZOOM: f64 = 16.0;
/// Step for `[` / `]` while a slideshow runs.
const SLIDESHOW_INTERVAL_STEP: f64 = 0.5;
const MIN_SLIDESHOW_INTERVAL: f64 = 0.5;
const MAX_SLIDESHOW_INTERVAL: f64 = 600.0;
const INFO_CACHE_CAPACITY: NonZeroUsize = NonZeroUsize::new(64).unwrap();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewMode {
    Gallery,
    Fullscreen,
    Picker,
    #[cfg(feature = "video")]
    Video,
}

/// The view that opened a video, and so the view that comes back after mpv.
#[cfg(feature = "video")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VideoReturn {
    Gallery,
    Fullscreen,
}

pub(crate) struct PendingDelete {
    pub paths: Vec<PathBuf>,
    pub from_fullscreen: bool,
    pub permanent: bool,
}

pub(crate) struct ThemePickerState {
    pub selected: usize,
    pub original_index: usize,
    pub scroll: usize,
    pub visible_height: usize,
}

/// A running slideshow in fullscreen mode.
pub(crate) struct Slideshow {
    pub interval: Duration,
    pub paused: bool,
    /// When the next advance is due. Pushed back while an image is still decoding.
    next_at: Instant,
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "App is the whole view state; each flag is an independent toggle"
)]
pub(crate) struct App {
    pub images: Vec<PathBuf>,
    pub filenames: Vec<String>,
    pub mode: ViewMode,
    pub theme: Theme,
    pub themes: Vec<NamedTheme>,
    pub theme_index: usize,
    pub theme_picker: Option<ThemePickerState>,
    pub cell_px: (u32, u32),
    pub graphics: KittyBackend,
    /// Where `graphics_delete_all` and the render functions write. A test app writes to a buffer.
    pub graphics_out: GraphicsOutput,

    // Info panel (gallery + fullscreen)
    pub info_visible: bool,
    info_cache: LruCache<PathBuf, ImageInfo>,

    /// The state file that remembers the thumbnail size. Tests point it into a temp directory.
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
    /// The slideshow position of a video in an mpv window, and the exit code of the window.
    #[cfg(feature = "video")]
    video_window: Option<(usize, mpsc::Receiver<Option<i32>>)>,

    // Scanner state
    shared_list: SharedImageList,
    known_len: usize,
    pub scan_complete: bool,

    // Async filter
    filter_rx: Option<mpsc::Receiver<Vec<usize>>>,
    /// The cursor position for the next filter result. A delete sets it, a new query clears it.
    filter_cursor: Option<usize>,

    // Async thumbnails
    thumb_tx: mpsc::Sender<(u32, usize, RgbaImage)>,
    thumb_rx: mpsc::Receiver<(u32, usize, RgbaImage)>,
    thumb_loading: HashSet<usize>,
    thumb_generation: u32,

    // Kitty IDs currently transmitted to the terminal — safe to `place_by_id` instead of retransmit.
    pub transmitted_image_ids: HashSet<u32>,
    /// True when the LRU thumb cache was invalidated but backend storage has not been cleared yet.
    /// Consumed by `render_gallery_images` to issue a full `d=A` at the next redraw.
    pub graphics_storage_dirty: bool,
}

impl App {
    /// A test app. The state file and the disk cache sit below `/dev/null`, so every write
    /// fails quietly and no test can touch the real files. A test that needs them sets a temp path.
    #[cfg(test)]
    pub(crate) fn new(theme: Theme, cell_px: (u32, u32), shared_list: SharedImageList) -> Self {
        Self::new_with_config(theme, cell_px, shared_list, &Config::default())
    }

    /// A test app with `config`. The Kitty sequences go to a buffer, not to the terminal.
    #[cfg(test)]
    pub(crate) fn new_with_config(
        theme: Theme,
        cell_px: (u32, u32),
        shared_list: SharedImageList,
        config: &Config,
    ) -> Self {
        let mut app = Self::with_config(theme, cell_px, shared_list, config);
        app.state_file = PathBuf::from("/dev/null/viewr/state.toml");
        app.disk_cache_dir = PathBuf::from("/dev/null/viewr/cache");
        app.graphics_out = GraphicsOutput::buffer();
        app
    }

    pub(crate) fn with_config(
        theme: Theme,
        cell_px: (u32, u32),
        shared_list: SharedImageList,
        config: &Config,
    ) -> Self {
        let (thumb_tx, thumb_rx) = mpsc::channel();
        Self {
            images: Vec::new(),
            filenames: Vec::new(),
            mode: ViewMode::Gallery,
            themes: vec![NamedTheme {
                name: "fallback".to_owned(),
                path: None,
                theme: theme.clone(),
            }],
            theme,
            theme_index: 0,
            theme_picker: None,
            cell_px,
            graphics: KittyBackend,
            graphics_out: GraphicsOutput::Stdout,
            info_visible: config.info_panel,
            info_cache: LruCache::new(INFO_CACHE_CAPACITY),
            state_file: config::default_state_path(),
            disk_cache_dir: default_disk_cache_dir(),
            slideshow: None,
            slideshow_interval: Duration::from_secs_f64(config.slideshow_interval),
            slideshow_loop: config.slideshow_loop,
            slideshow_autostart: false,
            gallery: GalleryState::with_thumb_size(0, config.thumb_cols, config.thumb_rows),
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
            filter_rx: None,
            filter_cursor: None,
            thumb_tx,
            thumb_rx,
            thumb_loading: HashSet::new(),
            thumb_generation: 0,
            transmitted_image_ids: HashSet::new(),
            graphics_storage_dirty: false,
        }
    }

    pub(crate) fn install_themes(&mut self, themes: Vec<NamedTheme>, selected: usize) {
        if themes.is_empty() {
            return;
        }
        self.theme_index = selected.min(themes.len() - 1);
        self.theme = themes[self.theme_index].theme.clone();
        self.themes = themes;
        self.needs_render = true;
    }

    pub(crate) fn open_theme_picker(&mut self) {
        self.theme_picker = Some(ThemePickerState {
            selected: self.theme_index,
            original_index: self.theme_index,
            scroll: 0,
            visible_height: 1,
        });
        self.needs_render = true;
    }

    pub(crate) fn theme_picker_select(&mut self, selected: usize) {
        if selected >= self.themes.len() {
            return;
        }
        if let Some(picker) = self.theme_picker.as_mut() {
            picker.selected = selected;
            self.theme_index = selected;
            self.theme = self.themes[selected].theme.clone();
            self.needs_render = true;
        }
    }

    pub(crate) fn theme_picker_confirm(&mut self) {
        self.theme_picker = None;
        self.needs_render = true;
    }

    pub(crate) fn theme_picker_cancel(&mut self) {
        let Some(picker) = self.theme_picker.take() else {
            return;
        };
        self.theme_index = picker.original_index;
        self.theme = self.themes[self.theme_index].theme.clone();
        self.needs_render = true;
    }

    // ----- Info panel -------------------------------------------------------

    pub(crate) fn toggle_info(&mut self) {
        self.info_visible = !self.info_visible;
        // The panel changes the image/grid area, so placements must be redone.
        self.needs_render = true;
    }

    /// Path the info panel describes: the fullscreen image, or the hovered thumbnail.
    #[cfg_attr(
        not(feature = "video"),
        expect(
            clippy::match_wildcard_for_single_variants,
            reason = "the wildcard also covers ViewMode::Video when the feature is on"
        )
    )]
    pub(crate) fn info_path(&self) -> Option<PathBuf> {
        match self.mode {
            ViewMode::Gallery => self
                .gallery
                .selected_index()
                .and_then(|idx| self.images.get(idx))
                .cloned(),
            ViewMode::Picker => None,
            _ => self.images.get(self.current).cloned(),
        }
    }

    /// Metadata for `path`, read on first use and cached for the session.
    pub(crate) fn info_for(&mut self, path: &Path) -> ImageInfo {
        if let Some(cached) = self.info_cache.get(path) {
            return cached.clone();
        }
        let loaded = info::load(path);
        self.info_cache.put(path.to_path_buf(), loaded.clone());
        loaded
    }

    #[expect(
        clippy::iter_over_hash_type,
        reason = "each entry goes on its own; order does not matter"
    )]
    fn forget_info(&mut self, paths: &HashSet<PathBuf>) {
        for path in paths {
            self.info_cache.pop(path);
        }
    }

    // ----- Thumbnail size ---------------------------------------------------

    pub(crate) fn thumb_zoom_in(&mut self) {
        if self.gallery.thumb_zoom_in() {
            self.after_thumb_size_change(true);
        }
    }

    pub(crate) fn thumb_zoom_out(&mut self) {
        if self.gallery.thumb_zoom_out() {
            self.after_thumb_size_change(true);
        }
    }

    /// Back to the configured size; also forgets the remembered size.
    pub(crate) fn thumb_zoom_reset(&mut self) {
        config::clear_thumb_size(&self.state_file);
        if self.gallery.reset_thumb_size() {
            self.after_thumb_size_change(false);
        }
    }

    fn after_thumb_size_change(&mut self, remember: bool) {
        if remember {
            config::save_thumb_size(
                &self.state_file,
                self.gallery.thumb_cols,
                self.gallery.thumb_rows,
            );
        }
        // Thumbnails are decoded for a pixel size, so every cached one is stale.
        self.mark_dirty();
    }

    // ----- Slideshow --------------------------------------------------------

    pub(crate) fn slideshow_running(&self) -> bool {
        self.slideshow.is_some()
    }

    pub(crate) fn toggle_slideshow(&mut self) {
        if self.slideshow.is_some() {
            self.stop_slideshow();
        } else {
            self.start_slideshow();
        }
    }

    pub(crate) fn start_slideshow(&mut self) {
        if self.images.is_empty() {
            return;
        }
        let interval = self.slideshow_interval;
        self.slideshow = Some(Slideshow {
            interval,
            paused: false,
            next_at: Instant::now() + interval,
        });
        // A slideshow lives in the fullscreen view, also when it starts on a video.
        #[cfg(feature = "video")]
        if self.mode == ViewMode::Video {
            self.video_return = VideoReturn::Fullscreen;
        }
        self.needs_render = true;
    }

    /// A running slideshow that is not paused. It plays a video one time and then moves on.
    /// Outside of it, a video plays in a loop until the user stops it.
    pub(crate) fn slideshow_advancing(&self) -> bool {
        self.slideshow.as_ref().is_some_and(|show| !show.paused)
    }

    pub(crate) fn stop_slideshow(&mut self) {
        if self.slideshow.take().is_some() {
            self.needs_render = true;
        }
    }

    pub(crate) fn slideshow_toggle_pause(&mut self) {
        if let Some(show) = self.slideshow.as_mut() {
            show.paused = !show.paused;
            if !show.paused {
                show.next_at = Instant::now() + show.interval;
            }
            self.needs_render = true;
        }
    }

    /// Lengthen or shorten the interval by one step; also becomes the default for
    /// the next slideshow this session.
    pub(crate) fn slideshow_adjust_interval(&mut self, steps: f64) {
        let current = self
            .slideshow
            .as_ref()
            .map_or(self.slideshow_interval, |show| show.interval)
            .as_secs_f64();
        let next = (current + steps * SLIDESHOW_INTERVAL_STEP)
            .clamp(MIN_SLIDESHOW_INTERVAL, MAX_SLIDESHOW_INTERVAL);
        let interval = Duration::from_secs_f64(next);
        self.slideshow_interval = interval;
        if let Some(show) = self.slideshow.as_mut() {
            show.interval = interval;
            show.next_at = Instant::now() + interval;
        }
        self.needs_render = true;
    }

    /// Restart the countdown, e.g. after the user navigated manually.
    pub(crate) fn slideshow_touch(&mut self) {
        if let Some(show) = self.slideshow.as_mut() {
            show.next_at = Instant::now() + show.interval;
        }
    }

    /// How long the event loop may sleep before the slideshow needs attention.
    pub(crate) fn slideshow_timeout(&self) -> Option<Duration> {
        let show = self.slideshow.as_ref()?;
        // In the video view, the end of the clip moves the slideshow on, not the clock.
        if show.paused || self.in_video_view() {
            return None;
        }
        Some(show.next_at.saturating_duration_since(Instant::now()))
    }

    /// Advance when the interval elapsed. Waits for the current image to finish
    /// decoding so every slide is actually shown for the full interval.
    pub(crate) fn slideshow_tick(&mut self) {
        if !self.slideshow_advancing() {
            return;
        }
        #[cfg(feature = "video")]
        if self.slideshow_video_holds() {
            return;
        }
        let Some(show) = self.slideshow.as_ref() else {
            return;
        };
        if self.mode != ViewMode::Fullscreen || Instant::now() < show.next_at {
            return;
        }
        if self.loaded.is_none() && !self.fullscreen_on_video() {
            // Still decoding: check again shortly instead of skipping the slide.
            self.slideshow_check_again_soon();
            return;
        }
        self.slideshow_step();
    }

    /// Push the next check of the slideshow 100 ms out, while a decode or a video holds the slide.
    fn slideshow_check_again_soon(&mut self) {
        if let Some(show) = self.slideshow.as_mut() {
            show.next_at = Instant::now() + Duration::from_millis(100);
        }
    }

    /// Go to the next file, or wrap to the first with the loop setting, or stop at the end.
    /// The next file gets the full interval.
    fn slideshow_step(&mut self) {
        let last = self.images.len().saturating_sub(1);
        if self.current < last {
            self.next();
        } else if self.slideshow_loop && !self.images.is_empty() {
            self.jump_to(0);
        } else {
            self.stop_slideshow();
            return;
        }
        if let Some(show) = self.slideshow.as_mut() {
            show.next_at = Instant::now() + show.interval;
        }
    }

    /// Wipe backend storage and drop transmitted-image tracking.
    pub(crate) fn graphics_delete_all(&mut self) -> io::Result<()> {
        let mut out = self.graphics_out.clone();
        self.graphics_delete_all_to(&mut out)?;
        out.flush()
    }

    pub(crate) fn graphics_delete_all_to<W: Write>(&mut self, out: &mut W) -> io::Result<()> {
        self.graphics.delete_all_to(out)?;
        self.transmitted_image_ids.clear();
        Ok(())
    }

    pub(crate) fn refresh_from_scanner(&mut self) {
        let scan_errors = self.shared_list.drain_errors();
        if !scan_errors.is_empty() {
            self.error = Some(scan_errors.join("; "));
            self.needs_render = true;
        }

        let new_len = self.shared_list.len();
        if new_len > self.known_len {
            let (new_paths, new_filenames) = self.shared_list.drain_since(self.known_len);
            let old_len = self.images.len();
            self.images.extend(new_paths);
            self.filenames.extend(new_filenames);

            if self.gallery.search_query.is_empty() {
                self.gallery
                    .filtered_indices
                    .extend(old_len..self.images.len());
            } else {
                self.update_filter();
            }

            self.known_len = new_len;
            if old_len == 0 {
                self.needs_render = true;
            }
        }

        if !self.scan_complete && self.shared_list.is_complete() {
            self.scan_complete = true;
            self.finalize_scan_order();

            if self.images.len() == 1 && self.mode == ViewMode::Gallery {
                self.mode = ViewMode::Fullscreen;
                self.needs_render = true;
            }
        }
    }

    fn finalize_scan_order(&mut self) {
        if self.images.len() < 2 {
            return;
        }

        let selected_path = self
            .gallery
            .selected_index()
            .and_then(|index| self.images.get(index))
            .cloned();
        let current_path = self.images.get(self.current).cloned();
        let mut media: Vec<(PathBuf, String)> = self
            .images
            .drain(..)
            .zip(self.filenames.drain(..))
            .collect();
        media.sort_by_cached_key(|(path, filename)| (filename.to_lowercase(), path.clone()));
        (self.images, self.filenames) = media.into_iter().unzip();

        self.current = current_path
            .as_ref()
            .and_then(|path| self.images.iter().position(|candidate| candidate == path))
            .unwrap_or(0);
        if self.gallery.search_query.is_empty() {
            self.gallery.filtered_indices = (0..self.images.len()).collect();
            self.gallery.cursor = selected_path
                .as_ref()
                .and_then(|path| self.images.iter().position(|candidate| candidate == path))
                .unwrap_or(0);
            self.gallery.ensure_cursor_visible();
        } else {
            self.update_filter();
        }

        self.shared_list
            .replace_all(self.images.clone(), self.filenames.clone());
        self.known_len = self.images.len();
        self.thumb_cache.clear();
        self.thumb_loading.clear();
        self.thumb_generation += 1;
        self.prefetcher.invalidate();
        self.graphics_storage_dirty = true;
        self.loaded = None;
        self.needs_render = true;
    }

    pub(crate) fn is_scanning(&self) -> bool {
        !self.scan_complete
    }

    pub(crate) fn update_filter(&mut self) {
        self.filter_cursor = None;
        if self.gallery.search_query.is_empty() {
            self.gallery.filtered_indices = (0..self.images.len()).collect();
            self.gallery.reset_cursor();
            self.needs_render = true;
            self.filter_rx = None;
            return;
        }

        let query = self.gallery.search_query.clone();
        let filenames = self.filenames.clone();
        let (tx, rx) = mpsc::channel();
        self.filter_rx = Some(rx);

        std::thread::spawn(move || {
            let results = search::filter(&query, &filenames);
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a closed receiver means that nobody waits for the result"
            )]
            let _ = tx.send(results);
        });
    }

    pub(crate) fn poll_filter(&mut self) {
        let results = self.filter_rx.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(results) = results {
            self.gallery.filtered_indices = results;
            match self.filter_cursor.take() {
                Some(position) => self.place_cursor(position),
                None => self.gallery.reset_cursor(),
            }
            self.needs_render = true;
            self.filter_rx = None;
        }
    }

    pub(crate) fn current_path(&self) -> &Path {
        &self.images[self.current]
    }

    pub(crate) fn enter_fullscreen_selected(&mut self) {
        if let Some(idx) = self.gallery.selected_index() {
            self.enter_fullscreen_at(idx);
        }
    }

    pub(crate) fn enter_fullscreen_at(&mut self, idx: usize) {
        if idx >= self.images.len() {
            return;
        }
        #[cfg(feature = "video")]
        if crate::video::is_video(&self.images[idx]) {
            self.enter_video(idx);
            return;
        }
        self.current = idx;
        self.mode = ViewMode::Fullscreen;
        self.error = None;
        self.needs_render = true;
        self.fullscreen_rx = None;
        self.fullscreen_target = None;
        self.loaded = None;
        self.reset_zoom_state();
    }

    /// Honour `--slideshow` once the directory scan is done, so the show starts
    /// at the first image in sorted order.
    pub(crate) fn maybe_autostart_slideshow(&mut self) {
        if !self.slideshow_autostart || !self.scan_complete {
            return;
        }
        self.slideshow_autostart = false;
        if self.images.is_empty() {
            return;
        }
        if self.mode == ViewMode::Gallery {
            self.enter_fullscreen_at(0);
        }
        if self.in_slideshow_view() {
            self.start_slideshow();
        }
    }

    #[cfg(feature = "video")]
    pub(crate) fn enter_video(&mut self, idx: usize) {
        self.video_return = if self.mode == ViewMode::Fullscreen {
            VideoReturn::Fullscreen
        } else {
            VideoReturn::Gallery
        };
        self.current = idx;
        self.mode = ViewMode::Video;
        self.error = None;
        self.needs_render = true;
        self.loaded = None;
        self.video = None;
    }

    #[cfg(feature = "video")]
    pub(crate) fn open_video_if_needed(&mut self) {
        if self.video.is_some() {
            return;
        }
        let path = &self.images[self.current];
        let repeat = !self.slideshow_advancing();
        match crate::video::VideoPlayback::open(path, self.image_rect, self.cell_px, repeat) {
            Ok(v) => self.video = Some(v),
            Err(e) => {
                self.error = Some(e.to_string());
                self.mode = ViewMode::Fullscreen;
            }
        }
    }

    #[cfg(feature = "video")]
    pub(crate) fn exit_video(&mut self) {
        if let Some(v) = self.video.take() {
            v.stop();
        }
        self.mode = ViewMode::Gallery;
        self.needs_render = true;
    }

    /// Leave the video for the view that opened it.
    ///
    /// The fullscreen view stays on the video and shows a play hint, because the image
    /// decode cannot read a video. The arrow keys then step on from the video.
    #[cfg(feature = "video")]
    pub(crate) fn return_from_video(&mut self) {
        if self.video_return == VideoReturn::Gallery {
            self.exit_video();
            return;
        }
        if let Some(v) = self.video.take() {
            v.stop();
        }
        self.mode = ViewMode::Fullscreen;
        self.loaded = None;
        self.fullscreen_rx = None;
        self.fullscreen_target = None;
        self.reset_zoom_state();
        self.needs_render = true;
    }

    /// A slideshow video holds the slide until its end. Returns true while it holds.
    ///
    /// The built-in player ends the clip and the slideshow moves on. An mpv window holds the
    /// placeholder until the window closes: exit code 0 moves on, a quit stops the slideshow.
    /// If the user leaves the placeholder, the slideshow forgets the window.
    #[cfg(feature = "video")]
    fn slideshow_video_holds(&mut self) -> bool {
        if self.mode == ViewMode::Video {
            if self.video.as_ref().is_some_and(|video| video.finished) {
                self.return_from_video();
                self.slideshow_step();
            }
            return true;
        }
        let Some((index, exit)) = self.video_window.as_ref() else {
            return false;
        };
        if *index != self.current || self.mode != ViewMode::Fullscreen {
            self.video_window = None;
            return false;
        }
        let code = match exit.try_recv() {
            Ok(code) => code,
            Err(mpsc::TryRecvError::Empty) => {
                self.slideshow_check_again_soon();
                return true;
            }
            Err(mpsc::TryRecvError::Disconnected) => None,
        };
        self.video_window = None;
        self.after_slideshow_video(crate::mpv::reached_the_end(code));
        true
    }

    /// Hold the slideshow on the placeholder of the current video until the mpv window closes.
    #[cfg(feature = "video")]
    pub(crate) fn hold_slideshow_for_window(&mut self, exit: mpsc::Receiver<Option<i32>>) {
        self.video_window = Some((self.current, exit));
    }

    /// After a video that a slideshow played one time: the end of the clip moves the slideshow
    /// on. A quit by the user stops the slideshow and keeps the placeholder.
    #[cfg(feature = "video")]
    pub(crate) fn after_slideshow_video(&mut self, reached_end: bool) {
        if reached_end {
            self.slideshow_step();
        } else {
            self.stop_slideshow();
        }
    }

    /// True in the fullscreen view or in a video player: the views of a slideshow.
    pub(crate) fn in_slideshow_view(&self) -> bool {
        self.mode == ViewMode::Fullscreen || self.in_video_view()
    }

    /// True in the view of a video player, where the clip and not the clock drives the slideshow.
    #[cfg_attr(
        not(feature = "video"),
        expect(
            clippy::unused_self,
            reason = "without the video feature, no view plays a video"
        )
    )]
    fn in_video_view(&self) -> bool {
        #[cfg(feature = "video")]
        let in_video = self.mode == ViewMode::Video;
        #[cfg(not(feature = "video"))]
        let in_video = false;
        in_video
    }

    /// The image decode cannot read a video, so the fullscreen view must not try it.
    fn fullscreen_on_video(&self) -> bool {
        #[cfg(feature = "video")]
        let is_video = self
            .images
            .get(self.current)
            .is_some_and(|path| crate::video::is_video(path));
        #[cfg(not(feature = "video"))]
        let is_video = false;
        is_video && self.mode == ViewMode::Fullscreen
    }

    /// The play hint for a video in fullscreen. A failed open shows its error instead.
    pub(crate) fn shows_video_placeholder(&self) -> bool {
        self.fullscreen_on_video() && self.error.is_none()
    }

    fn start_fullscreen_decode(&mut self, idx: usize, rect: Rect) {
        let (tx, rx) = mpsc::channel();
        self.fullscreen_rx = Some(rx);
        self.fullscreen_target = Some((idx, rect));
        let path = self.images[idx].clone();
        let cell_px = self.cell_px;
        rayon::spawn(move || {
            let result = load_and_resize(&path, rect, cell_px, EmbeddedThumbnail::Deny);
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a closed receiver means that nobody waits for the result"
            )]
            let _ = tx.send(result);
        });
    }

    pub(crate) fn poll_fullscreen(&mut self) -> bool {
        let result = self
            .fullscreen_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok());
        if let Some(result) = result {
            if let Some((idx, rect)) = self.fullscreen_target
                && idx == self.current
                && rect == self.image_rect
                && self.mode == ViewMode::Fullscreen
            {
                match result {
                    Ok(img) => {
                        self.loaded = Some(img);
                        self.error = None;
                        self.loaded_for_rect = rect;
                        self.needs_render = true;
                    }
                    Err(e) => {
                        self.error = Some(e.to_string());
                    }
                }
            }
            self.fullscreen_rx = None;
            self.fullscreen_target = None;
            return true;
        }
        false
    }

    /// Reset zoom/pan and drop the cached full-resolution source. Called when the
    /// current fullscreen image changes.
    fn reset_zoom_state(&mut self) {
        self.zoom = 1.0;
        self.pan_x = 0.5;
        self.pan_y = 0.5;
        self.zoom_dirty = false;
        self.source = None;
        self.source_for = None;
        self.source_rx = None;
        self.source_target = None;
    }

    /// Return to fit-to-window, keeping the cached source for fast re-zoom.
    pub(crate) fn reset_zoom(&mut self) {
        if self.zoom <= 1.0 {
            return;
        }
        self.zoom = 1.0;
        self.pan_x = 0.5;
        self.pan_y = 0.5;
        self.zoom_dirty = true;
        self.needs_render = true;
    }

    pub(crate) fn zoom_in(&mut self) {
        if self.fullscreen_on_video() {
            return;
        }
        let new = (self.zoom * ZOOM_STEP).min(MAX_ZOOM);
        if new > self.zoom {
            self.zoom = new;
            self.clamp_pan();
            self.zoom_dirty = true;
            self.needs_render = true;
        }
    }

    pub(crate) fn zoom_out(&mut self) {
        let new = (self.zoom / ZOOM_STEP).max(1.0);
        if new < self.zoom {
            self.zoom = new;
            self.clamp_pan();
            self.zoom_dirty = true;
            self.needs_render = true;
        }
    }

    /// Pan the zoomed view. `dx`/`dy` are direction multipliers (usually -1, 0, or 1).
    pub(crate) fn pan(&mut self, dx: f64, dy: f64) {
        if self.zoom <= 1.0 {
            return;
        }
        let step = 0.15 / self.zoom;
        self.pan_x += dx * step;
        self.pan_y += dy * step;
        self.clamp_pan();
        self.zoom_dirty = true;
        self.needs_render = true;
    }

    fn clamp_pan(&mut self) {
        self.pan_x = self.pan_x.clamp(0.0, 1.0);
        self.pan_y = self.pan_y.clamp(0.0, 1.0);
    }

    fn start_source_decode(&mut self, idx: usize) {
        let (tx, rx) = mpsc::channel();
        self.source_rx = Some(rx);
        self.source_target = Some(idx);
        let path = self.images[idx].clone();
        rayon::spawn(move || {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a closed receiver means that nobody waits for the result"
            )]
            let _ = tx.send(decode_image_with_hint(&path, None, EmbeddedThumbnail::Deny));
        });
    }

    /// Receive an async full-resolution source decode for zooming.
    pub(crate) fn poll_source(&mut self) -> bool {
        let result = self.source_rx.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(result) = result {
            let idx = self.source_target.take();
            self.source_rx = None;
            if let (Some(idx), Ok(img)) = (idx, result)
                && idx == self.current
            {
                self.source = Some(img);
                self.source_for = Some(idx);
                self.zoom_dirty = true;
                self.needs_render = true;
            }
            return true;
        }
        false
    }

    pub(crate) fn enter_gallery(&mut self) {
        self.slideshow = None;
        self.mode = ViewMode::Gallery;
        self.loaded = None;
        self.needs_render = true;
    }

    pub(crate) fn toggle_selection_at_cursor(&mut self) {
        let Some(idx) = self.gallery.selected_index() else {
            return;
        };
        let path = self.images[idx].clone();
        if !self.selection.remove(&path) {
            self.selection.insert(path);
        }
    }

    pub(crate) fn select_all_filtered(&mut self) {
        for &idx in &self.gallery.filtered_indices {
            self.selection.insert(self.images[idx].clone());
        }
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selection.clear();
    }

    pub(crate) fn is_marked(&self, img_idx: usize) -> bool {
        self.images
            .get(img_idx)
            .is_some_and(|p| self.selection.contains(p))
    }

    pub(crate) fn begin_delete(&mut self, permanent: bool) {
        let from_fullscreen = matches!(self.mode, ViewMode::Fullscreen);
        #[cfg(feature = "video")]
        let from_fullscreen = from_fullscreen || matches!(self.mode, ViewMode::Video);

        let paths: Vec<PathBuf> = if from_fullscreen {
            if self.current >= self.images.len() {
                return;
            }
            vec![self.images[self.current].clone()]
        } else if !self.selection.is_empty() {
            let mut v: Vec<PathBuf> = self
                .gallery
                .filtered_indices
                .iter()
                .filter_map(|&i| {
                    let p = self.images.get(i)?;
                    if self.selection.contains(p) {
                        Some(p.clone())
                    } else {
                        None
                    }
                })
                .collect();
            if v.is_empty() {
                v = self.selection.iter().cloned().collect();
            }
            v
        } else if let Some(idx) = self.gallery.selected_index() {
            vec![self.images[idx].clone()]
        } else {
            return;
        };

        if paths.is_empty() {
            return;
        }

        self.pending_delete = Some(PendingDelete {
            paths,
            from_fullscreen,
            permanent,
        });
        self.delete_error = None;
    }

    pub(crate) fn cancel_delete(&mut self) {
        self.pending_delete.take();
    }

    pub(crate) fn confirm_delete(&mut self) {
        let Some(pending) = self.pending_delete.take() else {
            return;
        };

        let mut removed: HashSet<PathBuf> = HashSet::new();
        let mut errors: Vec<String> = Vec::new();
        for p in &pending.paths {
            let result = if pending.permanent {
                std::fs::remove_file(p).map_err(|error| error.to_string())
            } else {
                trash::delete(p).map_err(|error| error.to_string())
            };
            match result {
                Ok(()) => {
                    removed.insert(p.clone());
                }
                Err(error) => {
                    errors.push(format!("{}: {error}", p.display()));
                }
            }
        }

        if removed.is_empty() {
            self.delete_error = Some(errors.join("; "));
            self.needs_render = true;
            return;
        }

        let current_path = self.images.get(self.current).cloned();
        let cursor_target = self.cursor_after_delete(&removed);

        let mut new_images = Vec::with_capacity(self.images.len() - removed.len());
        let mut new_filenames = Vec::with_capacity(self.filenames.len() - removed.len());
        for (i, p) in self.images.iter().enumerate() {
            if !removed.contains(p) {
                new_images.push(p.clone());
                new_filenames.push(self.filenames[i].clone());
            }
        }

        self.images = new_images;
        self.filenames = new_filenames;
        self.selection.retain(|p| !removed.contains(p));
        self.forget_info(&removed);
        self.known_len = self.images.len();
        self.shared_list
            .replace_all(self.images.clone(), self.filenames.clone());

        if self.gallery.search_query.is_empty() {
            self.filter_rx = None;
            self.gallery.filtered_indices = (0..self.images.len()).collect();
            self.place_cursor(cursor_target);
        } else {
            self.update_filter();
            self.filter_cursor = Some(cursor_target);
        }

        self.thumb_cache.clear();
        self.thumb_loading.clear();
        self.thumb_generation += 1;
        self.prefetcher.invalidate();
        self.graphics_storage_dirty = true;
        self.fullscreen_rx = None;
        self.fullscreen_target = None;
        self.loaded = None;

        self.delete_error = if errors.is_empty() {
            None
        } else {
            Some(errors.join("; "))
        };

        if pending.from_fullscreen {
            if self.images.is_empty() {
                self.mode = ViewMode::Gallery;
                self.slideshow = None;
            } else {
                let old_current = current_path
                    .as_ref()
                    .and_then(|p| self.images.iter().position(|q| q == p));
                let target = old_current.unwrap_or_else(|| self.current.min(self.images.len() - 1));
                self.jump_to(target);
            }
        } else if self.images.is_empty() {
            self.current = 0;
        } else if self.current >= self.images.len() {
            self.current = self.images.len() - 1;
        }

        self.needs_render = true;
    }

    /// The grid position for the cursor after a delete of `removed`: the image that moves up
    /// into the place of the cursor. The count of the kept images before the cursor is that place.
    fn cursor_after_delete(&self, removed: &HashSet<PathBuf>) -> usize {
        self.gallery
            .filtered_indices
            .iter()
            .take(self.gallery.cursor)
            .filter(|&&index| {
                self.images
                    .get(index)
                    .is_some_and(|path| !removed.contains(path))
            })
            .count()
    }

    /// Put the grid cursor on `position`, or on the last image of a shorter list.
    fn place_cursor(&mut self, position: usize) {
        let last = self.gallery.filtered_indices.len().saturating_sub(1);
        self.gallery.cursor = position.min(last);
        self.gallery.ensure_cursor_visible();
    }

    pub(crate) fn open_picker(&mut self) {
        let start = self
            .current_dir
            .clone()
            .or_else(|| {
                self.initial_dir
                    .clone()
                    .map(|p| picker::canonicalize_or_self(&p))
            })
            .unwrap_or_else(|| picker::initial_picker_dir(&self.images));
        self.current_dir = Some(start.clone());
        self.picker = Some(PickerState::new(start));
        self.mode = ViewMode::Picker;
        self.needs_render = true;
    }

    pub(crate) fn close_picker(&mut self) {
        self.picker = None;
        self.mode = ViewMode::Gallery;
        self.needs_render = true;
    }

    /// Replace image list with the contents of `dir` and reset all view state.
    /// Spawns a fresh scanner thread; the previous scanner (if any) is orphaned
    /// against an unused `SharedImageList` clone and will be dropped as it completes.
    pub(crate) fn switch_to_dir(&mut self, dir: &Path) {
        let canonical = picker::canonicalize_or_self(dir);
        self.current_dir = Some(canonical.clone());

        let new_list = SharedImageList::new();
        scanner::spawn(vec![canonical.clone()], new_list.clone());
        self.shared_list = new_list;

        self.images.clear();
        self.filenames.clear();
        self.known_len = 0;
        self.scan_complete = false;
        self.selection.clear();
        self.pending_delete = None;
        self.delete_error = None;
        self.slideshow = None;

        self.gallery.search_active = false;
        self.gallery.search_query.clear();
        self.gallery.filtered_indices.clear();
        self.gallery.reset_cursor();
        self.filter_rx = None;

        self.current = 0;
        self.thumb_cache.clear();
        self.thumb_loading.clear();
        self.thumb_generation += 1;
        self.prefetcher.invalidate();
        self.graphics_storage_dirty = true;
        self.fullscreen_rx = None;
        self.fullscreen_target = None;
        self.loaded = None;
        self.error = None;

        if let Some(ref mut p) = self.picker {
            p.current_dir = canonical;
            p.load_dir();
            p.adjust_scroll(p.filtered_indices.len().max(1));
        }

        self.needs_render = true;
    }

    pub(crate) fn next(&mut self) {
        if self.current + 1 < self.images.len() {
            self.jump_to(self.current + 1);
        }
    }

    pub(crate) fn prev(&mut self) {
        if self.current > 0 {
            self.jump_to(self.current - 1);
        }
    }

    pub(crate) fn first(&mut self) {
        if !self.images.is_empty() && self.current != 0 {
            self.jump_to(0);
        }
    }

    pub(crate) fn last(&mut self) {
        let last = self.images.len().saturating_sub(1);
        if !self.images.is_empty() && self.current != last {
            self.jump_to(last);
        }
    }

    fn jump_to(&mut self, target: usize) {
        #[cfg(feature = "video")]
        if let Some(v) = self.video.take() {
            v.stop();
        }
        // A step away from the placeholder forgets the mpv window of the slideshow.
        #[cfg(feature = "video")]
        {
            self.video_window = None;
        }

        self.current = target;
        self.error = None;
        self.needs_render = true;
        self.fullscreen_rx = None;
        self.fullscreen_target = None;
        self.reset_zoom_state();

        #[cfg(feature = "video")]
        if crate::video::is_video(&self.images[self.current]) {
            self.mode = ViewMode::Video;
            self.video_return = VideoReturn::Fullscreen;
            self.loaded = None;
            return;
        }

        self.mode = ViewMode::Fullscreen;
        if let Some(img) = self
            .prefetcher
            .take_resized(self.current, self.image_rect, self.cell_px)
        {
            self.loaded = Some(img);
            self.loaded_for_rect = self.image_rect;
        } else {
            self.loaded = None;
            self.start_fullscreen_decode(self.current, self.image_rect);
        }
    }

    pub(crate) fn poll_thumbnails(&mut self) -> Vec<usize> {
        let mut new_indices = Vec::new();
        while let Ok((generation, img_idx, img)) = self.thumb_rx.try_recv() {
            self.thumb_loading.remove(&img_idx);
            if generation == self.thumb_generation {
                self.thumb_cache.insert(img_idx, img);
                new_indices.push(img_idx);
            }
        }
        new_indices
    }

    pub(crate) fn spawn_thumb_decode(&mut self, img_idx: usize, path: PathBuf, rect: Rect) {
        if self.thumb_cache.contains(img_idx) || self.thumb_loading.contains(&img_idx) {
            return;
        }
        self.thumb_loading.insert(img_idx);
        let tx = self.thumb_tx.clone();
        let cell_px = self.cell_px;
        let generation = self.thumb_generation;
        let cache_dir = self.disk_cache_dir.clone();
        rayon::spawn(move || {
            let target_w = u32::from(rect.width) * cell_px.0;
            let target_h = u32::from(rect.height) * cell_px.1;

            if let Some(img) = try_load_from_disk_cache(&cache_dir, &path, target_w, target_h) {
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "a closed receiver means that nobody waits for the result"
                )]
                let _ = tx.send((generation, img_idx, img));
                return;
            }

            #[cfg(feature = "video")]
            let res = if crate::video::is_video(&path) {
                crate::video::decode_first_frame(&path, rect, cell_px)
            } else {
                load_and_resize(&path, rect, cell_px, EmbeddedThumbnail::Allow)
            };

            #[cfg(not(feature = "video"))]
            let res = load_and_resize(&path, rect, cell_px, EmbeddedThumbnail::Allow);

            if let Ok(img) = res {
                try_save_to_disk_cache(&cache_dir, &path, target_w, target_h, &img);
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "a closed receiver means that nobody waits for the result"
                )]
                let _ = tx.send((generation, img_idx, img));
            }
        });
    }

    pub(crate) fn pre_decode_hovered(&mut self) {
        if let Some(idx) = self.gallery.selected_index() {
            self.prefetcher.kick_gallery(idx, &self.images);
        }
    }

    pub(crate) fn mark_dirty(&mut self) {
        self.loaded = None;
        self.thumb_cache.clear();
        self.thumb_loading.clear();
        self.thumb_generation += 1;
        self.prefetcher.invalidate();
        self.graphics_storage_dirty = true;
        self.needs_render = true;
    }

    pub(crate) fn load_if_needed(&mut self) {
        if self.fullscreen_on_video() {
            return;
        }
        // Zoomed view: crop the full-resolution source around the pan point and
        // resize that crop to fill the viewport.
        if self.zoom > 1.0 {
            if self.source_for == Some(self.current) {
                if (self.zoom_dirty
                    || self.loaded.is_none()
                    || self.loaded_for_rect != self.image_rect)
                    && let Some(view) = self.build_zoom_view()
                {
                    self.loaded = Some(view);
                    self.error = None;
                    self.loaded_for_rect = self.image_rect;
                    self.zoom_dirty = false;
                }
                return;
            }
            // Source not decoded yet: request it and keep showing the fit image
            // until it arrives (poll_source will trigger the rebuild).
            if self.source_target != Some(self.current) {
                self.start_source_decode(self.current);
            }
            self.zoom_dirty = false;
            if self.loaded.is_some() {
                return;
            }
        }

        if self.loaded.is_some() && self.loaded_for_rect == self.image_rect && !self.zoom_dirty {
            return;
        }
        self.zoom_dirty = false;

        if let Some(img) = self
            .prefetcher
            .take_resized(self.current, self.image_rect, self.cell_px)
        {
            self.loaded = Some(img);
            self.error = None;
            self.loaded_for_rect = self.image_rect;
            return;
        }

        if let Some((_, rect)) = self.fullscreen_target
            && rect == self.image_rect
        {
            return;
        }

        self.start_fullscreen_decode(self.current, self.image_rect);
    }

    /// Build the zoomed, panned crop of the current full-resolution source, sized
    /// to fill the viewport. Returns `None` when no source is available.
    fn build_zoom_view(&self) -> Option<RgbaImage> {
        let src = self.source.as_ref()?;
        let (ow, oh) = (src.width(), src.height());
        if ow == 0 || oh == 0 {
            return None;
        }

        let vw = (u32::from(self.image_rect.width) * self.cell_px.0).max(1);
        let vh = (u32::from(self.image_rect.height) * self.cell_px.1).max(1);

        // Fit-to-window scale, never upscaling the base image (matches zoom == 1.0).
        let fit = f64::min(f64::from(vw) / f64::from(ow), f64::from(vh) / f64::from(oh));
        let base = fit.min(1.0);
        let eff = base * self.zoom;

        // Source-pixel crop window that maps onto the viewport at the effective scale.
        let crop_w = ((f64::from(vw) / eff).round() as u32).clamp(1, ow);
        let crop_h = ((f64::from(vh) / eff).round() as u32).clamp(1, oh);
        let max_x = ow.saturating_sub(crop_w);
        let max_y = oh.saturating_sub(crop_h);
        let x = (self.pan_x * f64::from(max_x)).round() as u32;
        let y = (self.pan_y * f64::from(max_y)).round() as u32;

        let cropped = src.crop_imm(x, y, crop_w, crop_h);
        let dst_w = ((f64::from(crop_w) * eff).round() as u32).max(1).min(vw);
        let dst_h = ((f64::from(crop_h) * eff).round() as u32).max(1).min(vh);
        Some(resize_to_exact(&cropped, dst_w, dst_h))
    }
}

pub(crate) fn load_and_resize(
    path: &Path,
    rect: Rect,
    cell_px: (u32, u32),
    embedded: EmbeddedThumbnail,
) -> io::Result<RgbaImage> {
    let target_w = u32::from(rect.width) * cell_px.0;
    let target_h = u32::from(rect.height) * cell_px.1;
    let hint = if target_w > 0 && target_h > 0 {
        Some((target_w, target_h))
    } else {
        None
    };
    let img = decode_image_with_hint(path, hint, embedded)?;
    Ok(resize_decoded(&img, rect, cell_px))
}

pub(crate) fn resize_decoded_to_dims(img: &DynamicImage, max_w: u32, max_h: u32) -> RgbaImage {
    fit_rgba_within(img.to_rgba8(), max_w, max_h)
}

/// Scale `rgba` down into `max_w` x `max_h` with its aspect ratio, and never up. An owned image
/// needs no second copy.
fn fit_rgba_within(rgba: RgbaImage, max_w: u32, max_h: u32) -> RgbaImage {
    let size = rgba.dimensions();
    scaled_down_size(size, (max_w, max_h))
        .and_then(|dst| resize_rgba(rgba.as_raw(), size, dst))
        .unwrap_or(rgba)
}

/// The size of `size` scaled down into `max` with its aspect ratio, rounded down. `None` when
/// no scale is necessary: an empty size or box, or an image that fits already.
fn scaled_down_size(size: (u32, u32), max: (u32, u32)) -> Option<(u32, u32)> {
    let ((orig_w, orig_h), (max_w, max_h)) = (size, max);
    if max_w == 0 || max_h == 0 || orig_w == 0 || orig_h == 0 {
        return None;
    }
    let scale = f64::min(
        f64::from(max_w) / f64::from(orig_w),
        f64::from(max_h) / f64::from(orig_h),
    );
    if scale >= 1.0 {
        return None;
    }
    let dst_w = ((f64::from(orig_w) * scale) as u32).max(1);
    let dst_h = ((f64::from(orig_h) * scale) as u32).max(1);
    Some((dst_w, dst_h))
}

/// Resize the packed RGBA `pixels` of `size` to `dst` with `fast_image_resize`. The pixels stay
/// borrowed, so a caller can pass a decoder buffer without a copy. `None` when the resize fails.
fn resize_rgba(pixels: &[u8], size: (u32, u32), dst: (u32, u32)) -> Option<RgbaImage> {
    let src_image =
        fir::images::ImageRef::new(size.0, size.1, pixels, fir::PixelType::U8x4).ok()?;
    let mut dst_image = fir::images::Image::new(dst.0, dst.1, fir::PixelType::U8x4);
    fir::Resizer::new()
        .resize(&src_image, &mut dst_image, None)
        .ok()?;
    RgbaImage::from_raw(dst.0, dst.1, dst_image.into_vec())
}

pub(crate) fn resize_decoded(img: &DynamicImage, rect: Rect, cell_px: (u32, u32)) -> RgbaImage {
    let max_w = u32::from(rect.width) * cell_px.0;
    let max_h = u32::from(rect.height) * cell_px.1;
    resize_decoded_to_dims(img, max_w, max_h)
}

/// Resize `img` to exactly `dst_w` x `dst_h` (used for zoomed crops, which may upscale).
pub(crate) fn resize_to_exact(img: &DynamicImage, dst_w: u32, dst_h: u32) -> RgbaImage {
    let (ow, oh) = (img.width(), img.height());
    if dst_w == 0 || dst_h == 0 || ow == 0 || oh == 0 {
        return img.to_rgba8();
    }
    if dst_w == ow && dst_h == oh {
        return img.to_rgba8();
    }
    let src_rgba = img.to_rgba8();
    let Ok(src_image) =
        fir::images::Image::from_vec_u8(ow, oh, src_rgba.into_raw(), fir::PixelType::U8x4)
    else {
        return img.to_rgba8();
    };
    let mut dst_image = fir::images::Image::new(dst_w, dst_h, fir::PixelType::U8x4);
    let mut resizer = fir::Resizer::new();
    if resizer.resize(&src_image, &mut dst_image, None).is_err() {
        return img.to_rgba8();
    }
    RgbaImage::from_raw(dst_w, dst_h, dst_image.into_vec()).unwrap_or_else(|| img.to_rgba8())
}

/// Only grid thumbnails can use an embedded HEIF thumbnail. A camera preview can differ
/// in color or crop from the primary image, so the fullscreen view does not use it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmbeddedThumbnail {
    Allow,
    Deny,
}

pub(crate) fn decode_image_with_hint(
    path: &Path,
    target: Option<(u32, u32)>,
    embedded: EmbeddedThumbnail,
) -> io::Result<DynamicImage> {
    #[cfg(feature = "heic")]
    if decodes_with_libheif(path) {
        // libheif applies the irot, imir and clap properties itself, so the EXIF
        // orientation must not apply a second time.
        return decode_with_libheif(path, target, embedded);
    }
    #[cfg(not(feature = "heic"))]
    let _ = embedded;

    decode_raster(path, target)
}

/// Send HEIC, HEIF and AVIF to libheif, because the `image` crate decodes none of them.
#[cfg(feature = "heic")]
pub(crate) fn decodes_with_libheif(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| scanner::LIBHEIF_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

#[cfg(feature = "heic")]
pub(crate) fn libheif_dimensions(path: &Path) -> Option<(u32, u32)> {
    let name = path.to_str()?;
    let ctx = libheif_rs::HeifContext::read_from_file(name).ok()?;
    let handle = ctx.primary_image_handle().ok()?;
    Some((handle.width(), handle.height()))
}

#[cfg(feature = "heic")]
fn decode_with_libheif(
    path: &Path,
    target: Option<(u32, u32)>,
    embedded: EmbeddedThumbnail,
) -> io::Result<DynamicImage> {
    use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};

    let invalid = |e: libheif_rs::HeifError| io::Error::new(io::ErrorKind::InvalidData, e);
    let name = path
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path is not valid UTF-8"))?;
    let ctx = HeifContext::read_from_file(name).map_err(invalid)?;
    let primary = ctx.primary_image_handle().map_err(invalid)?;
    // An embedded thumbnail (iPhone: 320x240) saves the decode of the full frame.
    let handle = target
        .filter(|_| embedded == EmbeddedThumbnail::Allow)
        .and_then(|target| usable_thumbnail(&primary, target))
        .unwrap_or(primary);
    let image = LibHeif::new()
        .decode(&handle, ColorSpace::Rgb(RgbChroma::Rgba), None)
        .map_err(invalid)?;
    heif_image_to_rgba(&image, target).map(DynamicImage::ImageRgba8)
}

/// Pick the smallest embedded thumbnail that is large enough for `target`, so that the
/// grid does not decode the full frame.
#[cfg(feature = "heic")]
fn usable_thumbnail(
    primary: &libheif_rs::ImageHandle,
    target: (u32, u32),
) -> Option<libheif_rs::ImageHandle> {
    let mut ids = vec![0; primary.number_of_thumbnails()];
    let count = primary.thumbnail_ids(&mut ids);
    ids.truncate(count);
    let primary_size = (primary.width(), primary.height());
    ids.into_iter()
        .filter_map(|id| primary.thumbnail(id).ok())
        .filter(|thumb| thumbnail_is_usable((thumb.width(), thumb.height()), primary_size, target))
        .min_by_key(|thumb| u64::from(thumb.width()) * u64::from(thumb.height()))
}

/// Accept a thumbnail only if it is at least as large as the primary image fitted into
/// `target`, so that the grid never upscales it. It must also have the portrait or
/// landscape shape of the primary image.
///
/// libheif applies the irot and imir properties of the thumbnail item. A thumbnail
/// without the 90 or 270 degree rotation of the primary image has the other shape and is
/// rejected. A missing 180 degree rotation or mirror has no effect on the size, so this
/// check cannot find it. libheif-rs has no safe API for the properties of an item.
#[cfg(feature = "heic")]
fn thumbnail_is_usable(thumbnail: (u32, u32), primary: (u32, u32), target: (u32, u32)) -> bool {
    let Some(fitted) = fit_within(primary, target) else {
        return false;
    };
    let covers = thumbnail.0 >= fitted.0 && thumbnail.1 >= fitted.1;
    let same_shape = (thumbnail.0 >= thumbnail.1) == (primary.0 >= primary.1);
    covers && same_shape
}

/// The size of `size` fitted into `target` without upscale, rounded down as
/// `resize_decoded_to_dims` does. `None` for an empty size or target.
#[cfg(feature = "heic")]
fn fit_within(size: (u32, u32), target: (u32, u32)) -> Option<(u32, u32)> {
    if size.0 == 0 || size.1 == 0 || target.0 == 0 || target.1 == 0 {
        return None;
    }
    let scale = f64::min(
        f64::from(target.0) / f64::from(size.0),
        f64::from(target.1) / f64::from(size.1),
    )
    .min(1.0);
    let fitted = |side: u32| ((f64::from(side) * scale) as u32).max(1);
    Some((fitted(size.0), fitted(size.1)))
}

/// Turn the interleaved RGBA plane of a libheif image into an image that fits `target`.
///
/// libheif decodes the selected image (primary image or thumbnail) at its full size. Its
/// `scale` is nearest neighbour, so `fast_image_resize` scales, as for the other formats. A plane
/// without row padding goes to the resize directly, so a full-size frame is not copied. A plane
/// with padding is copied row by row first.
#[cfg(feature = "heic")]
fn heif_image_to_rgba(
    image: &libheif_rs::Image,
    target: Option<(u32, u32)>,
) -> io::Result<RgbaImage> {
    let planes = image.planes();
    let plane = planes.interleaved.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "libheif decode produced no RGBA plane",
        )
    })?;
    let (w, h) = (plane.width, plane.height);
    let row_bytes = w as usize * 4;
    let fitted = target.and_then(|max| scaled_down_size((w, h), max));
    if let Some(dst) = fitted
        && plane.stride == row_bytes
        && let Some(resized) = plane
            .data
            .get(..row_bytes * h as usize)
            .and_then(|pixels| resize_rgba(pixels, (w, h), dst))
    {
        return Ok(resized);
    }

    let mut pixels = Vec::with_capacity(row_bytes * h as usize);
    for y in 0..h as usize {
        let start = y * plane.stride;
        pixels.extend_from_slice(&plane.data[start..start + row_bytes]);
    }
    let rgba = RgbaImage::from_raw(w, h, pixels).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid libheif image dimensions",
        )
    })?;
    Ok(match fitted {
        Some(dst) => resize_rgba(rgba.as_raw(), (w, h), dst).unwrap_or(rgba),
        None => rgba,
    })
}

fn decode_raster(path: &Path, target: Option<(u32, u32)>) -> io::Result<DynamicImage> {
    // libjpeg-turbo is the fast path, but it rejects some JPEG variants the
    // pure-Rust decoder accepts (and vice versa), so a turbo failure falls
    // through to `image` rather than aborting. Remember turbo's error so that if
    // the fallback also fails we can report *both* reasons instead of only the
    // fallback's — otherwise a genuine turbo diagnosis is lost.
    #[cfg(not(feature = "turbo"))]
    let _ = target;
    #[cfg(feature = "turbo")]
    let turbo_error = if is_jpeg(path) {
        match decode_jpeg_turbo(path, target) {
            Ok(img) => return Ok(img),
            Err(error) => Some(error),
        }
    } else {
        None
    };

    let reader = ImageReader::open(path)
        .map_err(|e| io::Error::new(io::ErrorKind::NotFound, e))?
        .with_guessed_format()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    match decode_upright(reader) {
        Ok(img) => Ok(img),
        Err(fallback_error) => {
            #[cfg(feature = "turbo")]
            if let Some(turbo_error) = turbo_error {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "failed to decode image (libjpeg-turbo: {turbo_error}; image: {fallback_error})"
                    ),
                ));
            }
            Err(io::Error::new(io::ErrorKind::InvalidData, fallback_error))
        }
    }
}

/// Decode with `image` and apply the EXIF orientation that its decoder reads (JPEG, PNG,
/// TIFF, WebP), so that phone photos show upright.
fn decode_upright<R: io::BufRead + io::Seek>(
    reader: ImageReader<R>,
) -> image::ImageResult<DynamicImage> {
    let mut decoder = reader.into_decoder()?;
    // A broken EXIF block must not stop the decode of the pixels.
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    // The same allocation limit as `ImageReader::decode`.
    image::Limits::default().reserve(decoder.total_bytes())?;
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok(img)
}

#[cfg(feature = "turbo")]
fn is_jpeg(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "jpg" | "jpeg"))
}

#[cfg(feature = "turbo")]
fn pick_scaling_factor(
    orig_w: usize,
    orig_h: usize,
    target_w: u32,
    target_h: u32,
) -> turbojpeg::ScalingFactor {
    let candidates = [
        turbojpeg::ScalingFactor::ONE_EIGHTH,
        turbojpeg::ScalingFactor::ONE_QUARTER,
        turbojpeg::ScalingFactor::ONE_HALF,
        turbojpeg::ScalingFactor::ONE,
    ];
    let tw = target_w as usize;
    let th = target_h as usize;
    for &sf in &candidates {
        let sw = sf.scale(orig_w);
        let sh = sf.scale(orig_h);
        if sw >= tw && sh >= th {
            return sf;
        }
    }
    turbojpeg::ScalingFactor::ONE
}

#[cfg(feature = "turbo")]
fn decode_jpeg_turbo(path: &Path, target: Option<(u32, u32)>) -> io::Result<DynamicImage> {
    let data = std::fs::read(path)?;
    let mut decompressor = turbojpeg::Decompressor::new()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let header = decompressor
        .read_header(&data)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let scaling = match target {
        Some((tw, th)) if tw > 0 && th > 0 && !header.is_lossless => {
            pick_scaling_factor(header.width, header.height, tw, th)
        }
        _ => turbojpeg::ScalingFactor::ONE,
    };

    if scaling != turbojpeg::ScalingFactor::ONE {
        decompressor
            .set_scaling_factor(scaling)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    }

    let scaled = header.scaled(scaling);
    let pitch = scaled.width * 4;
    let mut image = turbojpeg::Image {
        pixels: vec![0u8; scaled.height * pitch],
        width: scaled.width,
        pitch,
        height: scaled.height,
        format: turbojpeg::PixelFormat::RGBA,
    };

    decompressor
        .decompress(&data, image.as_deref_mut())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let w = image.width as u32;
    let h = image.height as u32;
    let mut img = RgbaImage::from_raw(w, h, image.pixels)
        .map(DynamicImage::ImageRgba8)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid image dimensions"))?;
    // libjpeg-turbo ignores the EXIF orientation, so phone photos need it here.
    if let Some(orientation) = info::orientation_from_bytes(&data).and_then(Orientation::from_exif)
    {
        img.apply_orientation(orientation);
    }
    Ok(img)
}

/// The thumbnail disk cache below the user's cache directory, or a temp directory without one.
fn default_disk_cache_dir() -> PathBuf {
    ProjectDirs::from("", "", "viewr").map_or_else(
        || std::env::temp_dir().join("viewr-cache"),
        |dirs| dirs.cache_dir().to_path_buf(),
    )
}

fn get_cache_file_path(
    cache_dir: &Path,
    path: &Path,
    target_w: u32,
    target_h: u32,
) -> Option<PathBuf> {
    use std::collections::hash_map::DefaultHasher;
    use std::fs;
    use std::hash::{Hash, Hasher};

    let metadata = fs::metadata(path).ok()?;
    let mtime = metadata
        .modified()
        .ok()?
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .ok()?;
    let canonical_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    let mut hasher = DefaultHasher::new();
    "viewr-thumbnail-v3".hash(&mut hasher);
    canonical_path.hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    mtime.as_secs().hash(&mut hasher);
    mtime.subsec_nanos().hash(&mut hasher);
    target_w.hash(&mut hasher);
    target_h.hash(&mut hasher);
    let filename = format!("{:016x}.png", hasher.finish());

    Some(cache_dir.join(filename))
}

fn try_load_from_disk_cache(
    cache_dir: &Path,
    path: &Path,
    target_w: u32,
    target_h: u32,
) -> Option<RgbaImage> {
    let cache_path = get_cache_file_path(cache_dir, path, target_w, target_h)?;
    if cache_path.exists() {
        if let Ok(image) = image::open(&cache_path) {
            Some(image.to_rgba8())
        } else {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a cache file that stays is only disk space, the cache prunes it"
            )]
            let _ = std::fs::remove_file(cache_path);
            None
        }
    } else {
        None
    }
}

fn try_save_to_disk_cache(
    cache_dir: &Path,
    path: &Path,
    target_w: u32,
    target_h: u32,
    img: &RgbaImage,
) -> Option<()> {
    use std::fs;
    static CACHE_PRUNED: OnceLock<()> = OnceLock::new();
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let cache_path = get_cache_file_path(cache_dir, path, target_w, target_h)?;
    let parent = cache_path.parent()?;
    fs::create_dir_all(parent).ok()?;
    CACHE_PRUNED.get_or_init(|| prune_disk_cache(parent, 512 * 1024 * 1024));
    if cache_path.exists() {
        return Some(());
    }

    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let cache_name = cache_path.file_name()?.to_string_lossy();
    let temp_path = parent.join(format!(
        ".{cache_name}.{}.{}.tmp",
        std::process::id(),
        sequence
    ));
    if img
        .save_with_format(&temp_path, image::ImageFormat::Png)
        .is_err()
    {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a cache file that stays is only disk space, the cache prunes it"
        )]
        let _ = fs::remove_file(temp_path);
        return None;
    }
    if fs::rename(&temp_path, &cache_path).is_err() {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a cache file that stays is only disk space, the cache prunes it"
        )]
        let _ = fs::remove_file(temp_path);
        if !cache_path.exists() {
            return None;
        }
    }
    if sequence.is_multiple_of(64) {
        prune_disk_cache(parent, 512 * 1024 * 1024);
    }
    Some(())
}

fn prune_disk_cache(cache_dir: &Path, max_bytes: u64) {
    let Ok(entries) = std::fs::read_dir(cache_dir) else {
        return;
    };
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = entries
        .flatten()
        .filter_map(|entry| {
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("png") {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            metadata.is_file().then(|| {
                (
                    entry.path(),
                    metadata.len(),
                    metadata.modified().unwrap_or(std::time::UNIX_EPOCH),
                )
            })
        })
        .collect();
    let mut total_bytes: u64 = files.iter().map(|(_, bytes, _)| bytes).sum();
    if total_bytes <= max_bytes {
        return;
    }

    files.sort_by_key(|(_, _, modified)| *modified);
    for (path, bytes, _) in files {
        if total_bytes <= max_bytes {
            break;
        }
        if std::fs::remove_file(path).is_ok() {
            total_bytes = total_bytes.saturating_sub(bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EmbeddedThumbnail, decode_image_with_hint, get_cache_file_path, resize_decoded_to_dims,
        resize_to_exact,
    };
    use image::metadata::Orientation;
    use image::{DynamicImage, ImageDecoder, ImageEncoder, Rgb, RgbImage};
    use std::fs;
    use tempfile::TempDir;

    #[cfg(feature = "turbo")]
    #[test]
    fn corrupt_jpeg_reports_both_decoder_errors() {
        // A file with the JPEG SOI magic but no valid frame: both libjpeg-turbo
        // and the pure-Rust fallback reject it. The error must mention both so a
        // turbo-only diagnosis is not silently lost.
        let dir = test_dir();
        let path = dir.path().join("corrupt.jpg");
        fs::write(&path, [0xFF, 0xD8, 0xFF, 0xD9]).unwrap();
        let err = decode_image_with_hint(&path, None, EmbeddedThumbnail::Deny).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("libjpeg-turbo:"), "message: {message}");
        assert!(message.contains("image:"), "message: {message}");
    }

    const RED: [u8; 3] = [255, 0, 0];
    const BLUE: [u8; 3] = [0, 0, 255];

    /// A minimal little-endian TIFF block with one IFD entry: Orientation = 6 (rotate 90 CW).
    const EXIF_ORIENTATION_6: [u8; 26] = [
        b'I', b'I', 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x12, 0x01, 0x03, 0x00, 0x01,
        0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    /// Give each test its own temp directory; it goes away with the `TempDir`.
    fn test_dir() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    /// A 32x16 image: the left half is red, the right half is blue.
    fn red_left_blue_right() -> RgbImage {
        RgbImage::from_fn(32, 16, |x, _| Rgb(if x < 16 { RED } else { BLUE }))
    }

    fn assert_close(pixel: [u8; 4], expected: [u8; 3]) {
        let close = pixel
            .iter()
            .zip(expected)
            .all(|(&got, want)| got.abs_diff(want) < 60);
        assert!(close, "pixel {pixel:?}, expected {expected:?}");
    }

    /// Orientation 6 turns the stored 32x16 into 16x32 with red on top and blue below.
    fn assert_upright(decoded: &DynamicImage) {
        let rgba = decoded.to_rgba8();
        assert_eq!(rgba.dimensions(), (16, 32));
        assert_close(rgba.get_pixel(8, 4).0, RED);
        assert_close(rgba.get_pixel(8, 27).0, BLUE);
    }

    #[test]
    fn webp_with_exif_orientation_6_decodes_upright() {
        let dir = test_dir();
        let path = dir.path().join("rotated.webp");
        let file = fs::File::create(&path).unwrap();
        let mut encoder = image::codecs::webp::WebPEncoder::new_lossless(file);
        encoder
            .set_exif_metadata(EXIF_ORIENTATION_6.to_vec())
            .unwrap();
        let stored = red_left_blue_right();
        encoder
            .write_image(stored.as_raw(), 32, 16, image::ExtendedColorType::Rgb8)
            .unwrap();
        let decoded = decode_image_with_hint(&path, None, EmbeddedThumbnail::Deny).unwrap();
        assert_upright(&decoded);
    }

    #[test]
    fn jpeg_with_exif_orientation_6_decodes_upright() {
        let dir = test_dir();
        let path = dir.path().join("rotated.jpg");
        let file = fs::File::create(&path).unwrap();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(file, 95);
        encoder
            .set_exif_metadata(EXIF_ORIENTATION_6.to_vec())
            .unwrap();
        let stored = red_left_blue_right();
        encoder
            .write_image(stored.as_raw(), 32, 16, image::ExtendedColorType::Rgb8)
            .unwrap();
        let decoded = decode_image_with_hint(&path, None, EmbeddedThumbnail::Deny).unwrap();
        let scaled = decode_image_with_hint(&path, Some((8, 8)), EmbeddedThumbnail::Deny).unwrap();
        assert_upright(&decoded);
        assert!(
            scaled.height() > scaled.width(),
            "scaled decodes rotate too"
        );
    }

    #[cfg(feature = "heic")]
    mod libheif {
        use super::{
            BLUE, EmbeddedThumbnail, RED, assert_close, decode_image_with_hint,
            resize_decoded_to_dims, resize_to_exact, test_dir,
        };
        use crate::app::{
            decodes_with_libheif, fit_within, libheif_dimensions, thumbnail_is_usable,
        };
        use image::GenericImageView;
        use libheif_rs::{
            Channel, ColorSpace, CompressionFormat, EncoderQuality, EncodingOptions, HeifContext,
            Image, ImageOrientation, LibHeif, RgbChroma,
        };
        use std::path::Path;

        fn solid_image(width: u32, height: u32, rgb: [u8; 3]) -> Image {
            let mut image = Image::new(width, height, ColorSpace::Rgb(RgbChroma::Rgb)).unwrap();
            image
                .create_plane(Channel::Interleaved, width, height, 8)
                .unwrap();
            let planes = image.planes_mut();
            let plane = planes.interleaved.unwrap();
            for y in 0..height as usize {
                for x in 0..width as usize {
                    let start = y * plane.stride + x * 3;
                    plane.data[start..start + 3].copy_from_slice(&rgb);
                }
            }
            image
        }

        fn options(orientation: ImageOrientation) -> EncodingOptions {
            let mut options = EncodingOptions::new().unwrap();
            options.set_image_orientation(orientation);
            options
        }

        /// Write a red 256x128 primary image. A blue thumbnail of 64x32 is optional, with
        /// its own orientation.
        fn write_heif(
            path: &Path,
            format: CompressionFormat,
            orientation: ImageOrientation,
            thumbnail: Option<ImageOrientation>,
        ) {
            let lib_heif = LibHeif::new();
            let mut ctx = HeifContext::new().unwrap();
            let mut encoder = lib_heif.encoder_for_format(format).unwrap();
            encoder.set_quality(EncoderQuality::Lossy(90)).unwrap();
            let primary = ctx
                .encode_image(
                    &solid_image(256, 128, RED),
                    &mut encoder,
                    Some(options(orientation)),
                )
                .unwrap();
            if let Some(thumbnail_orientation) = thumbnail {
                ctx.encode_thumbnail(
                    &solid_image(256, 128, BLUE),
                    &primary,
                    64,
                    &mut encoder,
                    Some(options(thumbnail_orientation)),
                )
                .unwrap();
            }
            ctx.write_to_file(path.to_str().unwrap()).unwrap();
        }

        /// Write a red AVIF of `width` x `height` without a thumbnail.
        fn write_red_avif(path: &Path, width: u32, height: u32) {
            let lib_heif = LibHeif::new();
            let mut ctx = HeifContext::new().unwrap();
            let mut encoder = lib_heif.encoder_for_format(CompressionFormat::Av1).unwrap();
            encoder.set_quality(EncoderQuality::Lossy(90)).unwrap();
            ctx.encode_image(&solid_image(width, height, RED), &mut encoder, None)
                .unwrap();
            ctx.write_to_file(path.to_str().unwrap()).unwrap();
        }

        /// True when the decoded RGBA plane of `path` has padding at the end of each row.
        fn rows_have_padding(path: &Path) -> bool {
            let ctx = HeifContext::read_from_file(path.to_str().unwrap()).unwrap();
            let handle = ctx.primary_image_handle().unwrap();
            let image = LibHeif::new()
                .decode(&handle, ColorSpace::Rgb(RgbChroma::Rgba), None)
                .unwrap();
            let plane = image.planes().interleaved.unwrap();
            plane.stride != plane.width as usize * 4
        }

        fn center(image: &image::DynamicImage) -> [u8; 4] {
            image
                .to_rgba8()
                .get_pixel(image.width() / 2, image.height() / 2)
                .0
        }

        #[test]
        fn libheif_routing_covers_heic_and_avif() {
            assert!(decodes_with_libheif(Path::new("IMG_0001.HEIC")));
            assert!(decodes_with_libheif(Path::new("photo.hif")));
            assert!(decodes_with_libheif(Path::new("photo.AVIF")));
            assert!(!decodes_with_libheif(Path::new("photo.webp")));
            assert!(!decodes_with_libheif(Path::new("avif")));
        }

        #[test]
        fn thumbnail_must_cover_the_fitted_primary_and_keep_the_shape() {
            // iPhone: 320x240 thumbnail of a 4032x3024 photo. The default grid cell on a
            // Retina display is 18x6 cells of 17x37 px: the photo fits as 296x222.
            assert!(thumbnail_is_usable((320, 240), (4032, 3024), (306, 222)));
            // A wide box: the photo fits as 320x240, so the thumbnail is enough.
            assert!(thumbnail_is_usable((320, 240), (4032, 3024), (400, 240)));
            // The photo fits as 391x293: the thumbnail is too small.
            assert!(!thumbnail_is_usable((320, 240), (4032, 3024), (391, 296)));
            // Portrait photo, rotated thumbnail, tall box: the photo fits as 200x266.
            assert!(thumbnail_is_usable((240, 320), (3024, 4032), (200, 400)));
            // The thumbnail misses the rotation of the primary image.
            assert!(!thumbnail_is_usable((320, 240), (3024, 4032), (200, 200)));
            // A zero target never uses a thumbnail.
            assert!(!thumbnail_is_usable((320, 240), (4032, 3024), (0, 100)));
        }

        #[test]
        fn fit_within_does_not_upscale() {
            assert_eq!(fit_within((4032, 3024), (306, 222)), Some((296, 222)));
            assert_eq!(fit_within((100, 50), (400, 400)), Some((100, 50)));
            assert_eq!(fit_within((100, 50), (0, 400)), None);
            assert_eq!(fit_within((0, 50), (400, 400)), None);
        }

        #[test]
        fn avif_opens_through_libheif() {
            let dir = test_dir();
            let path = dir.path().join("red.avif");
            write_heif(
                &path,
                CompressionFormat::Av1,
                ImageOrientation::Normal,
                None,
            );
            let full = decode_image_with_hint(&path, None, EmbeddedThumbnail::Deny).unwrap();
            let thumb =
                decode_image_with_hint(&path, Some((64, 64)), EmbeddedThumbnail::Deny).unwrap();
            let dims = libheif_dimensions(&path);

            assert_eq!(full.dimensions(), (256, 128));
            assert_close(center(&full), RED);
            assert_eq!(thumb.dimensions(), (64, 32));
            assert_eq!(dims, Some((256, 128)));
        }

        #[test]
        fn libheif_output_has_the_fitted_size_of_the_other_decoders() {
            let dir = test_dir();
            let path = dir.path().join("red.avif");
            write_heif(
                &path,
                CompressionFormat::Av1,
                ImageOrientation::Normal,
                None,
            );
            for target in [(64, 64), (77, 77), (100, 30), (500, 500)] {
                let decoded =
                    decode_image_with_hint(&path, Some(target), EmbeddedThumbnail::Deny).unwrap();
                assert_eq!(
                    Some(decoded.dimensions()),
                    fit_within((256, 128), target),
                    "{target:?}"
                );
                // load_and_resize resizes again: the fitted decode must stay as it is.
                let resized = resize_decoded_to_dims(&decoded, target.0, target.1);
                assert_eq!(resized.dimensions(), decoded.dimensions(), "{target:?}");
                assert_close(center(&decoded), RED);
            }
            let sizes: Vec<_> = [(64, 64), (100, 30), (500, 500)]
                .into_iter()
                .map(|target| fit_within((256, 128), target).unwrap())
                .collect();
            assert_eq!(sizes, [(64, 32), (60, 30), (256, 128)]);
        }

        #[test]
        fn libheif_scales_planes_with_and_without_row_padding() {
            let dir = test_dir();
            let mut padding = Vec::new();
            for (width, height) in [(256, 128), (250, 126)] {
                let path = dir.path().join(format!("red-{width}.avif"));
                write_red_avif(&path, width, height);
                padding.push(rows_have_padding(&path));
                for target in [Some((50, 50)), None] {
                    let decoded =
                        decode_image_with_hint(&path, target, EmbeddedThumbnail::Deny).unwrap();
                    let expected = target.map_or(Some((width, height)), |target| {
                        fit_within((width, height), target)
                    });
                    assert_eq!(Some(decoded.dimensions()), expected, "{width} {target:?}");
                    assert_close(center(&decoded), RED);
                }
            }
            assert_eq!(padding, [false, true], "the test covers both plane layouts");
        }

        #[test]
        fn heic_grid_uses_a_large_enough_thumbnail_with_its_rotation() {
            let dir = test_dir();
            let path = dir.path().join("rotated.heic");
            let rotation = ImageOrientation::Rotate90Cw;
            write_heif(&path, CompressionFormat::Hevc, rotation, Some(rotation));
            let decode =
                |target, embedded| decode_image_with_hint(&path, target, embedded).unwrap();
            let full = decode(None, EmbeddedThumbnail::Allow);
            let small = decode(Some((32, 32)), EmbeddedThumbnail::Allow);
            let wide = decode(Some((64, 64)), EmbeddedThumbnail::Allow);
            let large = decode(Some((100, 100)), EmbeddedThumbnail::Allow);
            let fullscreen = decode(Some((32, 32)), EmbeddedThumbnail::Deny);

            assert_eq!(full.dimensions(), (128, 256));
            assert_close(center(&full), RED);
            // The 32x64 thumbnail covers the fitted 16x32: blue, and upright like the primary.
            assert_eq!(small.dimensions(), (16, 32));
            assert_close(center(&small), BLUE);
            // In a 64x64 box the primary image fits as 32x64: the thumbnail is just enough.
            assert_eq!(wide.dimensions(), (32, 64));
            assert_close(center(&wide), BLUE);
            // In a 100x100 box the primary image fits as 50x100: libheif decodes the primary.
            assert_eq!(large.dimensions(), (50, 100));
            assert_close(center(&large), RED);
            // The fullscreen view never uses the embedded thumbnail.
            assert_eq!(fullscreen.dimensions(), (16, 32));
            assert_close(center(&fullscreen), RED);
        }

        #[test]
        fn heic_thumbnail_without_the_rotation_is_not_used() {
            let dir = test_dir();
            let path = dir.path().join("rotated.heic");
            write_heif(
                &path,
                CompressionFormat::Hevc,
                ImageOrientation::Rotate90Cw,
                Some(ImageOrientation::Normal),
            );
            let small =
                decode_image_with_hint(&path, Some((32, 32)), EmbeddedThumbnail::Allow).unwrap();

            assert_eq!(small.dimensions(), (16, 32));
            assert_close(center(&small), RED);
        }

        /// Decode a real AVIF file: `VIEWR_TEST_AVIF=/path/to/image.avif cargo test -- --ignored`.
        #[test]
        #[ignore = "needs VIEWR_TEST_AVIF, a real AVIF file"]
        fn decodes_avif_from_env() {
            let path = std::env::var("VIEWR_TEST_AVIF").expect("VIEWR_TEST_AVIF not set");
            let path = Path::new(&path);
            let full =
                decode_image_with_hint(path, None, EmbeddedThumbnail::Deny).expect("full decode");
            assert_eq!(libheif_dimensions(path), Some(full.dimensions()));
            let thumb = decode_image_with_hint(path, Some((200, 200)), EmbeddedThumbnail::Deny)
                .expect("scaled decode");
            assert!(thumb.width() <= 200 && thumb.height() <= 200);
        }

        /// The grid decode of a real iPhone HEIC uses the embedded thumbnail and keeps the
        /// shape of the full image: `VIEWR_TEST_HEIC=/path/to/photo.heic`.
        #[test]
        #[ignore = "needs VIEWR_TEST_HEIC, a real photo"]
        fn heic_thumbnail_from_env() {
            let path = std::env::var("VIEWR_TEST_HEIC").expect("VIEWR_TEST_HEIC not set");
            let path = Path::new(&path);
            let ctx = HeifContext::read_from_file(path.to_str().unwrap()).unwrap();
            let primary = ctx.primary_image_handle().unwrap();
            let thumbnail = crate::app::usable_thumbnail(&primary, (160, 160))
                .expect("an embedded thumbnail of at least 160x160");
            let shape = |w: u32, h: u32| w >= h;
            assert_eq!(
                shape(thumbnail.width(), thumbnail.height()),
                shape(primary.width(), primary.height())
            );
            let thumb = decode_image_with_hint(path, Some((160, 160)), EmbeddedThumbnail::Allow)
                .expect("thumbnail decode");
            assert_eq!(
                shape(thumb.width(), thumb.height()),
                shape(primary.width(), primary.height())
            );
            // A shape check misses a 180 degree turn: compare the pixels with the primary image.
            let full =
                decode_image_with_hint(path, None, EmbeddedThumbnail::Deny).expect("full decode");
            let reference = resize_to_exact(&full, thumb.width(), thumb.height());
            let thumb = thumb.to_rgba8();
            let total: u64 = thumb
                .as_raw()
                .iter()
                .zip(reference.as_raw())
                .map(|(&a, &b)| u64::from(a.abs_diff(b)))
                .sum();
            let mean = total / thumb.as_raw().len() as u64;
            assert!(mean < 20, "mean pixel difference {mean}");
        }
    }

    #[test]
    fn resize_preserves_aspect_ratio_without_upscaling() {
        let image = DynamicImage::new_rgba8(100, 50);
        assert_eq!(
            resize_decoded_to_dims(&image, 20, 20).dimensions(),
            (20, 10)
        );
        assert_eq!(
            resize_decoded_to_dims(&image, 200, 200).dimensions(),
            (100, 50)
        );
    }

    #[test]
    fn resize_to_exact_upscales_to_requested_dimensions() {
        let image = DynamicImage::new_rgba8(10, 10);
        // Zoomed crops may upscale, unlike the fit-to-window path.
        assert_eq!(resize_to_exact(&image, 40, 30).dimensions(), (40, 30));
        assert_eq!(resize_to_exact(&image, 10, 10).dimensions(), (10, 10));
    }

    #[test]
    fn zoom_view_crops_and_fills_the_viewport() {
        use super::App;
        use crate::image_list::SharedImageList;
        use crate::theme::Theme;
        use ratatui::layout::Rect;

        let mut app = App::new(Theme::fallback(), (1, 1), SharedImageList::new());
        app.image_rect = Rect::new(0, 0, 100, 100);
        // 400x400 source, viewport 100x100 -> fit scale 0.25 (base image is 100x100).
        app.source = Some(DynamicImage::new_rgba8(400, 400));
        app.source_for = Some(0);
        app.current = 0;

        // At 2x zoom the effective scale is 0.5, so the crop is 200x200 source px
        // upscaled to exactly the 100x100 viewport.
        app.zoom = 2.0;
        let view = app.build_zoom_view().expect("zoom view");
        assert_eq!(view.dimensions(), (100, 100));
    }

    /// Decode a real JPEG whose EXIF orientation is 6 or 8 (portrait stored sideways):
    /// `VIEWR_TEST_JPEG=/path/to/photo.jpg cargo test -- --ignored`.
    #[test]
    #[ignore = "needs VIEWR_TEST_JPEG, a real photo"]
    fn decodes_jpeg_upright_from_env() {
        let path = std::env::var("VIEWR_TEST_JPEG").expect("VIEWR_TEST_JPEG not set");
        let path = std::path::Path::new(&path);
        let mut decoder = image::ImageReader::open(path)
            .unwrap()
            .into_decoder()
            .unwrap();
        let orientation = decoder.orientation().expect("orientation tag");
        let stored = decoder.dimensions();
        let decoded = decode_image_with_hint(path, None, EmbeddedThumbnail::Deny).expect("decode");
        let expected = if matches!(
            orientation,
            Orientation::Rotate90
                | Orientation::Rotate270
                | Orientation::Rotate90FlipH
                | Orientation::Rotate270FlipH
        ) {
            (stored.1, stored.0)
        } else {
            stored
        };
        assert_eq!((decoded.width(), decoded.height()), expected);
        // Scaled decodes must be rotated the same way.
        let small = decode_image_with_hint(path, Some((300, 300)), EmbeddedThumbnail::Deny)
            .expect("scaled decode");
        assert_eq!(small.width() < small.height(), expected.0 < expected.1);
    }

    /// Decode a real HEIC file: `VIEWR_TEST_HEIC=/path/to/photo.heic cargo test -- --ignored`.
    #[cfg(feature = "heic")]
    #[test]
    #[ignore = "needs VIEWR_TEST_HEIC, a real photo"]
    fn decodes_heic_from_env() {
        let path = std::env::var("VIEWR_TEST_HEIC").expect("VIEWR_TEST_HEIC not set");
        let path = std::path::Path::new(&path);
        let full =
            decode_image_with_hint(path, None, EmbeddedThumbnail::Deny).expect("full decode");
        assert!(full.width() > 0 && full.height() > 0);
        let thumb = decode_image_with_hint(path, Some((200, 200)), EmbeddedThumbnail::Deny)
            .expect("scaled decode");
        assert!(
            thumb.width() <= 200 && thumb.height() <= 200,
            "{}x{}",
            thumb.width(),
            thumb.height()
        );
        let dims = super::libheif_dimensions(path).expect("dimensions");
        assert_eq!(dims, (full.width(), full.height()));
        let info = crate::info::load(path);
        assert!(
            info.rows.iter().any(|(_, v)| v == "EXIF"),
            "rows: {:?}",
            info.rows
        );
    }

    #[test]
    fn slideshow_advances_wraps_and_stops_without_loop() {
        use super::{App, Slideshow};
        use crate::image_list::SharedImageList;
        use crate::theme::Theme;
        use std::path::PathBuf;
        use std::time::{Duration, Instant};

        let mut app = App::new(Theme::fallback(), (8, 16), SharedImageList::new());
        app.images = vec![PathBuf::from("a.png"), PathBuf::from("b.png")];
        app.filenames = vec!["a.png".into(), "b.png".into()];
        app.mode = super::ViewMode::Fullscreen;
        app.current = 0;
        app.loaded = Some(image::RgbaImage::new(1, 1));

        app.start_slideshow();
        app.slideshow = Some(Slideshow {
            interval: Duration::from_millis(10),
            paused: false,
            next_at: Instant::now()
                .checked_sub(Duration::from_millis(1))
                .unwrap(),
        });
        app.slideshow_tick();
        assert_eq!(app.current, 1);

        // Wrap to the first image when looping.
        app.loaded = Some(image::RgbaImage::new(1, 1));
        app.slideshow.as_mut().unwrap().next_at = Instant::now()
            .checked_sub(Duration::from_millis(1))
            .unwrap();
        app.slideshow_tick();
        assert_eq!(app.current, 0);
        assert!(app.slideshow_running());

        // Without looping the show ends on the last image.
        app.slideshow_loop = false;
        app.current = 1;
        app.loaded = Some(image::RgbaImage::new(1, 1));
        app.slideshow.as_mut().unwrap().next_at = Instant::now()
            .checked_sub(Duration::from_millis(1))
            .unwrap();
        app.slideshow_tick();
        assert_eq!(app.current, 1);
        assert!(!app.slideshow_running());
    }

    #[test]
    fn slideshow_waits_for_a_pending_decode() {
        use super::{App, Slideshow};
        use crate::image_list::SharedImageList;
        use crate::theme::Theme;
        use std::path::PathBuf;
        use std::time::{Duration, Instant};

        let mut app = App::new(Theme::fallback(), (8, 16), SharedImageList::new());
        app.images = vec![PathBuf::from("a.png"), PathBuf::from("b.png")];
        app.filenames = vec!["a.png".into(), "b.png".into()];
        app.mode = super::ViewMode::Fullscreen;
        app.loaded = None;
        app.slideshow = Some(Slideshow {
            interval: Duration::from_millis(10),
            paused: false,
            next_at: Instant::now()
                .checked_sub(Duration::from_millis(1))
                .unwrap(),
        });
        app.slideshow_tick();
        assert_eq!(app.current, 0, "must not advance while decoding");
        assert!(app.slideshow_timeout().is_some());
    }

    #[test]
    fn slideshow_interval_adjusts_in_steps_and_clamps() {
        use super::App;
        use crate::image_list::SharedImageList;
        use crate::theme::Theme;

        let mut app = App::new(Theme::fallback(), (8, 16), SharedImageList::new());
        assert_eq!(app.slideshow_interval.as_secs_f64(), 3.0);
        app.slideshow_adjust_interval(1.0);
        assert_eq!(app.slideshow_interval.as_secs_f64(), 3.5);
        app.slideshow_adjust_interval(-10.0);
        assert_eq!(app.slideshow_interval.as_secs_f64(), 0.5);
    }

    #[test]
    fn cache_key_changes_with_dimensions_and_source_size() {
        let dir = test_dir();
        let cache = dir.path().join("cache");
        let source = dir.path().join("source.png");
        fs::write(&source, b"synthetic-a").unwrap();
        let first = get_cache_file_path(&cache, &source, 100, 100).unwrap();
        let resized = get_cache_file_path(&cache, &source, 200, 100).unwrap();
        fs::write(&source, b"synthetic-content-b").unwrap();
        let changed = get_cache_file_path(&cache, &source, 100, 100).unwrap();

        assert_eq!(first.parent(), Some(cache.as_path()));
        assert_ne!(first, resized);
        assert_ne!(first, changed);
    }

    /// Tests of the view state in `App`: slideshow, zoom, selection, delete, scan and caches.
    mod state {
        use super::test_dir;
        use crate::app::{
            App, EmbeddedThumbnail, PendingDelete, Slideshow, ViewMode, get_cache_file_path,
            load_and_resize, prune_disk_cache, resize_decoded, try_load_from_disk_cache,
            try_save_to_disk_cache,
        };
        use crate::image_list::SharedImageList;
        use crate::theme::{NamedTheme, Theme};
        use image::{DynamicImage, RgbaImage};
        use ratatui::layout::Rect;
        use ratatui::style::Color;
        use std::fs;
        use std::path::{Path, PathBuf};
        use std::thread;
        use std::time::{Duration, Instant, SystemTime};

        fn app() -> App {
            App::new(Theme::fallback(), (8, 16), SharedImageList::new())
        }

        /// An app with the synthetic paths `names` in the list and the grid. The files do not exist.
        fn app_with(names: &[&str]) -> App {
            let mut app = app();
            app.images = names.iter().map(PathBuf::from).collect();
            app.filenames = names.iter().map(|name| (*name).to_owned()).collect();
            app.gallery.filtered_indices = (0..names.len()).collect();
            app
        }

        /// An app with real PNG files `names` in `dir`.
        fn app_with_files(dir: &Path, names: &[&str]) -> App {
            let mut app = app();
            for name in names {
                let path = dir.join(name);
                RgbaImage::new(8, 8).save(&path).unwrap();
                app.images.push(path);
                app.filenames.push((*name).to_owned());
            }
            app.gallery.filtered_indices = (0..names.len()).collect();
            app
        }

        /// Poll `done` every 10 ms for at most 2 s, so background work cannot hang a test.
        fn wait_until(mut done: impl FnMut() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !done() {
                assert!(Instant::now() < deadline, "background work did not finish");
                thread::sleep(Duration::from_millis(10));
            }
        }

        fn overdue() -> Instant {
            Instant::now()
                .checked_sub(Duration::from_millis(1))
                .unwrap()
        }

        fn named(name: &str, accent: Color) -> NamedTheme {
            let mut theme = Theme::fallback();
            theme.title = theme.title.fg(accent);
            NamedTheme {
                name: name.to_owned(),
                path: None,
                theme,
            }
        }

        #[test]
        fn theme_picker_previews_confirms_and_cancels() {
            let mut app = app();
            app.install_themes(Vec::new(), 3);
            assert_eq!(app.themes.len(), 1);

            let themes = vec![named("one", Color::Red), named("two", Color::Blue)];
            app.install_themes(themes, 7);
            assert_eq!(app.theme_index, 1);
            assert_eq!(app.theme.title.fg, Some(Color::Blue));

            // Without an open picker, a selection does nothing.
            app.theme_picker_select(0);
            assert_eq!(app.theme_index, 1);
            app.theme_picker_cancel();
            assert_eq!(app.theme_index, 1);

            app.open_theme_picker();
            app.theme_picker_select(5);
            assert_eq!(app.theme_index, 1);
            app.theme_picker_select(0);
            assert_eq!(app.theme.title.fg, Some(Color::Red));
            app.theme_picker_cancel();
            assert!(app.theme_picker.is_none());
            assert_eq!(app.theme_index, 1);
            assert_eq!(app.theme.title.fg, Some(Color::Blue));

            app.open_theme_picker();
            app.theme_picker_select(0);
            app.theme_picker_confirm();
            assert!(app.theme_picker.is_none());
            assert_eq!(app.theme_index, 0);
        }

        #[test]
        fn info_path_follows_the_view_and_info_is_cached() {
            let dir = test_dir();
            let mut app = app_with_files(dir.path(), &["a.png", "b.png"]);
            app.toggle_info();
            assert!(app.info_visible);
            app.toggle_info();
            assert!(!app.info_visible);

            app.gallery.cursor = 1;
            assert_eq!(app.info_path(), Some(app.images[1].clone()));
            app.mode = ViewMode::Fullscreen;
            app.current = 0;
            assert_eq!(app.info_path(), Some(app.images[0].clone()));
            app.mode = ViewMode::Picker;
            assert_eq!(app.info_path(), None);

            let path = app.images[0].clone();
            let first = app.info_for(&path);
            fs::remove_file(&path).unwrap();
            // The cache answers, so the removed file still has its first rows.
            assert_eq!(app.info_for(&path).rows, first.rows);
        }

        #[test]
        fn thumb_zoom_writes_and_reset_clears_the_state_file() {
            let dir = test_dir();
            let mut app = app_with(&["a.png"]);
            app.state_file = dir.path().join("state/state.toml");
            let start = (app.gallery.thumb_cols, app.gallery.thumb_rows);

            app.thumb_zoom_in();
            assert!(app.gallery.thumb_cols > start.0);
            assert!(app.graphics_storage_dirty);
            let saved = fs::read_to_string(&app.state_file).unwrap();
            assert!(
                saved.contains(&format!("thumb_cols = {}", app.gallery.thumb_cols)),
                "{saved}"
            );

            app.thumb_zoom_out();
            app.thumb_zoom_out();
            assert!(app.gallery.thumb_cols < start.0);

            app.thumb_zoom_reset();
            assert_eq!((app.gallery.thumb_cols, app.gallery.thumb_rows), start);
            assert!(!app.state_file.exists());

            // A reset at the default size only removes the state file.
            app.thumb_zoom_reset();
            assert!(!app.state_file.exists());
        }

        #[test]
        fn thumb_zoom_stops_at_the_limits() {
            let dir = test_dir();
            let mut app = app_with(&["a.png"]);
            app.state_file = dir.path().join("state.toml");
            for _ in 0..40 {
                app.thumb_zoom_out();
            }
            let smallest = app.gallery.thumb_cols;
            fs::remove_file(&app.state_file).unwrap();
            app.thumb_zoom_out();
            assert_eq!(app.gallery.thumb_cols, smallest);
            assert!(!app.state_file.exists(), "no change, no write");
        }

        #[test]
        fn slideshow_starts_pauses_and_stops() {
            let mut app = app();
            app.start_slideshow();
            assert!(!app.slideshow_running(), "no images, no slideshow");
            app.toggle_slideshow();
            assert!(!app.slideshow_running());

            let mut app = app_with(&["a.png", "b.png"]);
            app.toggle_slideshow();
            assert!(app.slideshow_running());
            assert!(app.slideshow_timeout().unwrap() <= app.slideshow_interval);

            app.slideshow_toggle_pause();
            assert!(app.slideshow.as_ref().unwrap().paused);
            assert_eq!(app.slideshow_timeout(), None);
            app.slideshow_toggle_pause();
            assert!(!app.slideshow.as_ref().unwrap().paused);
            assert!(app.slideshow_timeout().is_some());

            app.slideshow.as_mut().unwrap().next_at = overdue();
            app.slideshow_touch();
            assert!(app.slideshow_timeout().unwrap() > Duration::from_secs(1));

            app.slideshow_adjust_interval(2.0);
            assert_eq!(
                app.slideshow.as_ref().unwrap().interval,
                Duration::from_secs(4)
            );
            assert_eq!(app.slideshow_interval, Duration::from_secs(4));

            app.toggle_slideshow();
            assert!(!app.slideshow_running());
            app.needs_render = false;
            app.stop_slideshow();
            assert!(!app.needs_render, "a second stop changes nothing");

            // Without a slideshow, pause and touch do nothing.
            app.slideshow_toggle_pause();
            app.slideshow_touch();
            assert_eq!(app.slideshow_timeout(), None);
        }

        #[test]
        fn slideshow_tick_waits_for_its_turn() {
            let mut app = app_with(&["a.png", "b.png"]);
            app.slideshow_tick();
            assert_eq!(app.current, 0, "no slideshow");

            app.loaded = Some(RgbaImage::new(1, 1));
            app.start_slideshow();
            app.slideshow.as_mut().unwrap().next_at = overdue();
            app.slideshow_tick();
            assert_eq!(app.current, 0, "the gallery does not advance");

            app.mode = ViewMode::Fullscreen;
            app.slideshow_toggle_pause();
            app.slideshow.as_mut().unwrap().next_at = overdue();
            app.slideshow_tick();
            assert_eq!(app.current, 0, "a paused show does not advance");

            app.slideshow_toggle_pause();
            app.slideshow_tick();
            assert_eq!(app.current, 0, "the interval did not elapse");

            app.slideshow.as_mut().unwrap().next_at = overdue();
            app.slideshow_tick();
            assert_eq!(app.current, 1);
            assert!(app.slideshow_timeout().unwrap() > Duration::from_secs(1));
        }

        #[test]
        fn autostart_waits_for_the_scan_and_opens_the_first_image() {
            let mut app = app_with(&["a.png", "b.png"]);
            app.slideshow_autostart = true;
            app.maybe_autostart_slideshow();
            assert!(!app.slideshow_running(), "the scan is still running");

            app.scan_complete = true;
            app.gallery.cursor = 1;
            app.maybe_autostart_slideshow();
            assert!(!app.slideshow_autostart);
            assert_eq!(app.mode, ViewMode::Fullscreen);
            assert_eq!(app.current, 0);
            assert!(app.slideshow_running());

            let mut empty = app_with(&[]);
            empty.slideshow_autostart = true;
            empty.scan_complete = true;
            empty.maybe_autostart_slideshow();
            assert!(!empty.slideshow_autostart);
            assert!(!empty.slideshow_running());
        }

        #[test]
        fn zoom_and_pan_stay_inside_the_limits() {
            let mut app = app_with(&["a.png"]);
            app.mode = ViewMode::Fullscreen;
            app.reset_zoom();
            app.pan(1.0, 1.0);
            assert_eq!((app.pan_x, app.pan_y), (0.0, 0.0), "no pan at fit size");

            app.zoom_out();
            assert_eq!(app.zoom, 1.0);
            app.zoom_in();
            assert_eq!(app.zoom, 1.25);
            assert!(app.zoom_dirty);
            for _ in 0..30 {
                app.zoom_in();
            }
            assert_eq!(app.zoom, 16.0);
            app.needs_render = false;
            app.zoom_in();
            assert!(!app.needs_render, "the maximum stops the zoom");

            app.pan_x = 0.5;
            app.pan_y = 0.5;
            app.pan(-1.0, 1.0);
            assert!(app.pan_x < 0.5 && app.pan_y > 0.5);
            for _ in 0..200 {
                app.pan(-1.0, 1.0);
            }
            assert_eq!((app.pan_x, app.pan_y), (0.0, 1.0));

            app.zoom_out();
            assert_eq!(app.zoom, 12.8);
            app.reset_zoom();
            assert_eq!(app.zoom, 1.0);
            assert_eq!((app.pan_x, app.pan_y), (0.5, 0.5));
        }

        #[test]
        fn zoom_view_follows_the_pan_point() {
            let mut app = app();
            app.cell_px = (1, 1);
            app.image_rect = Rect::new(0, 0, 10, 10);
            // The left half is black, the right half is white.
            let source = RgbaImage::from_fn(40, 40, |x, _| {
                image::Rgba(if x < 20 { [0, 0, 0, 255] } else { [255; 4] })
            });
            app.source = Some(DynamicImage::ImageRgba8(source));
            app.zoom = 4.0;
            app.pan_x = 0.0;
            let left = app.build_zoom_view().unwrap();
            app.pan_x = 1.0;
            let right = app.build_zoom_view().unwrap();
            assert_eq!(left.dimensions(), (10, 10));
            assert_eq!(left.get_pixel(5, 5).0, [0, 0, 0, 255]);
            assert_eq!(right.get_pixel(5, 5).0, [255; 4]);

            app.source = Some(DynamicImage::new_rgba8(0, 0));
            assert!(app.build_zoom_view().is_none());
            app.source = None;
            assert!(app.build_zoom_view().is_none());
        }

        #[test]
        fn fullscreen_decode_arrives_and_stale_results_are_dropped() {
            let dir = test_dir();
            let mut app = app_with_files(dir.path(), &["a.png", "b.png"]);
            app.image_rect = Rect::new(0, 0, 4, 4);
            app.enter_fullscreen_selected();
            assert_eq!(app.mode, ViewMode::Fullscreen);
            assert!(!app.poll_fullscreen(), "nothing requested yet");

            app.load_if_needed();
            wait_until(|| app.poll_fullscreen());
            assert!(app.loaded.is_some());
            assert!(app.error.is_none());

            // Loaded for this rect: nothing to do.
            app.load_if_needed();
            assert!(app.fullscreen_target.is_none());

            // A result for an image that is no longer current is dropped.
            app.loaded = None;
            app.load_if_needed();
            // The same request is in flight, so a second call starts nothing new.
            app.load_if_needed();
            app.current = 1;
            wait_until(|| app.poll_fullscreen());
            assert!(app.loaded.is_none());
        }

        #[test]
        fn a_broken_file_reports_the_decode_error() {
            let dir = test_dir();
            let path = dir.path().join("broken.png");
            fs::write(&path, b"not a png").unwrap();
            let mut app = app_with(&[path.to_str().unwrap()]);
            app.image_rect = Rect::new(0, 0, 4, 4);
            app.enter_fullscreen_at(0);
            app.load_if_needed();
            wait_until(|| app.poll_fullscreen());
            assert!(app.loaded.is_none());
            assert!(app.error.is_some());

            app.enter_fullscreen_at(9);
            assert_eq!(app.current, 0, "an index past the end is ignored");
        }

        #[test]
        fn zoom_decodes_the_source_and_then_crops_it() {
            let dir = test_dir();
            let mut app = app_with_files(dir.path(), &["a.png", "b.png"]);
            app.image_rect = Rect::new(0, 0, 2, 2);
            app.enter_fullscreen_at(0);
            app.loaded = Some(RgbaImage::new(1, 1));
            app.zoom_in();
            assert!(!app.poll_source(), "nothing requested yet");

            app.load_if_needed();
            assert_eq!(app.source_target, Some(0));
            assert_eq!(
                app.loaded.as_ref().unwrap().dimensions(),
                (1, 1),
                "keeps the fit image"
            );
            wait_until(|| app.poll_source());
            assert_eq!(app.source_for, Some(0));
            assert!(app.zoom_dirty);

            app.load_if_needed();
            assert!(!app.zoom_dirty);
            // The 8x8 source at zoom 1.25 fills 10x10 of the 16x32 viewport.
            assert_eq!(app.loaded.as_ref().unwrap().dimensions(), (10, 10));

            // A source that arrives for an old image is dropped.
            app.source_for = None;
            app.source = None;
            app.load_if_needed();
            app.current = 1;
            wait_until(|| app.poll_source());
            assert!(app.source.is_none());

            // Zoom without a fit image: request the source and decode the fit image too.
            app.loaded = None;
            app.load_if_needed();
            assert_eq!(app.source_target, Some(1));
            assert!(app.fullscreen_target.is_some());
        }

        #[test]
        fn selection_marks_and_clears() {
            let mut app = app_with(&["a.png", "b.png", "c.png"]);
            app.toggle_selection_at_cursor();
            assert!(app.is_marked(0));
            app.toggle_selection_at_cursor();
            assert!(!app.is_marked(0));
            assert!(!app.is_marked(99));

            app.gallery.filtered_indices = vec![0, 2];
            app.select_all_filtered();
            assert!(app.is_marked(0) && !app.is_marked(1) && app.is_marked(2));
            app.clear_selection();
            assert!(app.selection.is_empty());

            app.gallery.filtered_indices.clear();
            app.toggle_selection_at_cursor();
            assert!(app.selection.is_empty());
        }

        #[test]
        fn begin_delete_picks_the_paths_of_the_view() {
            let mut app = app_with(&[]);
            app.begin_delete(false);
            assert!(
                app.pending_delete.is_none(),
                "an empty grid deletes nothing"
            );

            let mut app = app_with(&["a.png", "b.png", "c.png"]);
            app.gallery.cursor = 1;
            app.delete_error = Some("old".to_owned());
            app.begin_delete(false);
            let pending = app.pending_delete.take().unwrap();
            assert_eq!(pending.paths, vec![PathBuf::from("b.png")]);
            assert!(!pending.from_fullscreen && !pending.permanent);
            assert!(app.delete_error.is_none());

            // Marked files go in grid order.
            app.selection.insert(PathBuf::from("c.png"));
            app.selection.insert(PathBuf::from("a.png"));
            app.begin_delete(true);
            let pending = app.pending_delete.take().unwrap();
            assert_eq!(
                pending.paths,
                vec![PathBuf::from("a.png"), PathBuf::from("c.png")]
            );
            assert!(pending.permanent);

            // Marked files outside the filter still go.
            app.gallery.filtered_indices = vec![1];
            app.selection.remove(Path::new("a.png"));
            app.begin_delete(false);
            assert_eq!(
                app.pending_delete.take().unwrap().paths,
                vec![PathBuf::from("c.png")]
            );

            app.mode = ViewMode::Fullscreen;
            app.current = 2;
            app.begin_delete(false);
            let pending = app.pending_delete.take().unwrap();
            assert_eq!(pending.paths, vec![PathBuf::from("c.png")]);
            assert!(pending.from_fullscreen);

            app.current = 7;
            app.begin_delete(false);
            assert!(app.pending_delete.is_none());

            app.current = 0;
            app.begin_delete(false);
            app.cancel_delete();
            assert!(app.pending_delete.is_none());
        }

        #[test]
        fn confirm_delete_removes_files_from_the_grid() {
            let dir = test_dir();
            let mut app = app_with_files(dir.path(), &["a.png", "b.png", "c.png"]);
            app.confirm_delete();
            assert_eq!(app.images.len(), 3, "nothing pending");

            let removed = app.images[2].clone();
            app.info_for(&removed);
            app.gallery.cursor = 2;
            app.current = 2;
            app.selection.insert(removed.clone());
            app.begin_delete(true);
            app.confirm_delete();

            assert!(!removed.exists());
            assert_eq!(app.filenames, ["a.png", "b.png"]);
            assert_eq!(app.gallery.filtered_indices, vec![0, 1]);
            assert_eq!(app.gallery.cursor, 1, "the new last image");
            assert_eq!(app.current, 1);
            assert!(app.selection.is_empty());
            assert!(app.delete_error.is_none());
            assert!(app.graphics_storage_dirty);
            assert_eq!(app.shared_list.len(), 2);
            assert_eq!(app.known_len, 2);
        }

        #[test]
        fn confirm_delete_in_the_grid_keeps_the_cursor_in_place() {
            let dir = test_dir();
            let names = ["a.png", "b.png", "c.png", "d.png", "e.png"];
            let mut app = app_with_files(dir.path(), &names);
            app.gallery.visible_rows = 1;
            app.gallery.grid_cols = 2;

            // In the middle, the next image moves up into the place of the cursor.
            app.gallery.cursor = 1;
            app.begin_delete(true);
            app.confirm_delete();
            assert_eq!(app.filenames, ["a.png", "c.png", "d.png", "e.png"]);
            assert_eq!(app.gallery.cursor, 1);
            assert_eq!(app.gallery.selected_index(), Some(1), "c.png");

            // At the end, the cursor goes to the new last image and the grid scrolls to it.
            app.gallery.cursor = 3;
            app.gallery.scroll_offset = 1;
            app.begin_delete(true);
            app.confirm_delete();
            assert_eq!(app.filenames, ["a.png", "c.png", "d.png"]);
            assert_eq!(app.gallery.cursor, 2);
            assert_eq!(app.gallery.scroll_offset, 1);

            // A selection before the cursor: the cursor stays on its image.
            app.selection.insert(app.images[0].clone());
            app.selection.insert(app.images[1].clone());
            app.begin_delete(true);
            app.confirm_delete();
            assert_eq!(app.filenames, ["d.png"]);
            assert_eq!(app.gallery.cursor, 0);
            assert_eq!(app.gallery.scroll_offset, 0);
        }

        #[test]
        fn confirm_delete_reports_the_files_that_stay() {
            let dir = test_dir();
            let mut app = app_with_files(dir.path(), &["a.png", "b.png"]);
            let missing = dir.path().join("gone.png");
            app.pending_delete = Some(PendingDelete {
                paths: vec![missing.clone()],
                from_fullscreen: false,
                permanent: true,
            });
            app.confirm_delete();
            let error = app.delete_error.clone().unwrap();
            assert!(error.contains("gone.png"), "{error}");
            assert_eq!(app.images.len(), 2);

            // The trash route fails for a missing file before it asks the system.
            app.pending_delete = Some(PendingDelete {
                paths: vec![missing.clone()],
                from_fullscreen: false,
                permanent: false,
            });
            app.confirm_delete();
            assert!(app.delete_error.as_ref().unwrap().contains("gone.png"));

            // One file goes, the other one reports its error.
            let first = app.images[0].clone();
            app.pending_delete = Some(PendingDelete {
                paths: vec![first.clone(), missing],
                from_fullscreen: false,
                permanent: true,
            });
            app.confirm_delete();
            assert!(!first.exists());
            assert_eq!(app.filenames, ["b.png"]);
            assert!(app.delete_error.as_ref().unwrap().contains("gone.png"));
        }

        #[test]
        fn confirm_delete_in_fullscreen_moves_on_and_ends_in_the_gallery() {
            let dir = test_dir();
            let mut app = app_with_files(dir.path(), &["a.png", "b.png"]);
            app.enter_fullscreen_at(0);
            app.start_slideshow();
            app.begin_delete(true);
            app.confirm_delete();
            assert_eq!(app.mode, ViewMode::Fullscreen);
            assert_eq!(app.filenames, ["b.png"]);
            assert_eq!(app.current, 0);

            app.begin_delete(true);
            app.confirm_delete();
            assert!(app.images.is_empty());
            assert_eq!(app.mode, ViewMode::Gallery);
            assert!(!app.slideshow_running());
            assert_eq!(app.current, 0);
        }

        #[test]
        fn confirm_delete_in_the_grid_keeps_the_current_index_valid() {
            let dir = test_dir();
            let mut app = app_with_files(dir.path(), &["a.png"]);
            app.begin_delete(true);
            app.confirm_delete();
            assert!(app.images.is_empty());
            assert_eq!(app.current, 0);
            assert_eq!(app.mode, ViewMode::Gallery);
        }

        #[test]
        fn confirm_delete_keeps_an_active_filter() {
            let dir = test_dir();
            let mut app = app_with_files(dir.path(), &["apple.png", "berry.png", "apricot.png"]);
            app.gallery.search_query = "ap".to_owned();
            app.gallery.filtered_indices = vec![0, 2];
            app.gallery.cursor = 1;
            app.begin_delete(true);
            app.confirm_delete();
            assert_eq!(app.filenames, ["apple.png", "berry.png"]);
            wait_until(|| {
                app.poll_filter();
                app.filter_rx.is_none()
            });
            assert_eq!(app.gallery.filtered_indices, vec![0]);
            assert_eq!(app.gallery.cursor, 0, "the new last match");
        }

        #[test]
        fn confirm_delete_with_a_filter_keeps_the_cursor_place() {
            let dir = test_dir();
            let names = ["ap-1.png", "ap-2.png", "ap-3.png", "berry.png"];
            let mut app = app_with_files(dir.path(), &names);
            app.gallery.search_query = "ap".to_owned();
            app.gallery.filtered_indices = vec![0, 1, 2];
            app.gallery.cursor = 1;
            app.begin_delete(true);
            app.confirm_delete();
            wait_until(|| {
                app.poll_filter();
                app.filter_rx.is_none()
            });
            assert_eq!(app.gallery.filtered_indices.len(), 2);
            assert_eq!(app.gallery.cursor, 1);

            // A new query starts at the first match again.
            app.gallery.search_query = "ap-".to_owned();
            app.update_filter();
            wait_until(|| {
                app.poll_filter();
                app.filter_rx.is_none()
            });
            assert_eq!(app.gallery.cursor, 0);
        }

        #[test]
        fn scanner_results_extend_the_list_and_sort_at_the_end() {
            let list = SharedImageList::new();
            let mut app = App::new(Theme::fallback(), (8, 16), list.clone());
            assert!(app.is_scanning());

            list.push_error("synthetic: denied".to_owned());
            list.push_error("other: denied".to_owned());
            list.push_batch(vec![PathBuf::from("/x/b.png"), PathBuf::from("/x/C.png")]);
            app.refresh_from_scanner();
            assert_eq!(
                app.error.as_deref(),
                Some("synthetic: denied; other: denied")
            );
            assert_eq!(app.gallery.filtered_indices, vec![0, 1]);

            list.push_batch(vec![PathBuf::from("/x/a.png")]);
            app.needs_render = false;
            app.refresh_from_scanner();
            assert_eq!(app.images.len(), 3);
            assert!(!app.needs_render, "only the first batch forces a redraw");

            // Hover and show C.png, then let the scan end: the order sorts, both stay on C.png.
            app.gallery.cursor = 1;
            app.current = 1;
            list.mark_complete();
            app.refresh_from_scanner();
            assert!(!app.is_scanning());
            assert_eq!(app.filenames, ["a.png", "b.png", "C.png"]);
            assert_eq!(app.gallery.cursor, 2);
            assert_eq!(app.current, 2);
            assert_eq!(app.current_path(), Path::new("/x/C.png"));
            assert!(app.graphics_storage_dirty);
            let (paths, _) = list.drain_since(0);
            assert_eq!(paths, app.images);
        }

        #[test]
        fn a_single_scanned_image_opens_in_fullscreen() {
            let list = SharedImageList::new();
            let mut app = App::new(Theme::fallback(), (8, 16), list.clone());
            list.push_batch(vec![PathBuf::from("/x/only.png")]);
            list.mark_complete();
            app.refresh_from_scanner();
            assert_eq!(app.mode, ViewMode::Fullscreen);
            // A second refresh after the end changes nothing.
            app.refresh_from_scanner();
            assert_eq!(app.images.len(), 1);
        }

        #[test]
        fn scanner_results_respect_an_active_filter() {
            let list = SharedImageList::new();
            let mut app = App::new(Theme::fallback(), (8, 16), list.clone());
            app.gallery.search_query = "be".to_owned();
            list.push_batch(vec![
                PathBuf::from("/x/zebra.png"),
                PathBuf::from("/x/berry.png"),
            ]);
            app.refresh_from_scanner();
            wait_until(|| {
                app.poll_filter();
                app.filter_rx.is_none()
            });
            assert!(app.gallery.filtered_indices.contains(&1));
            assert!(!app.gallery.filtered_indices.contains(&0));

            list.mark_complete();
            app.refresh_from_scanner();
            assert_eq!(app.filenames, ["berry.png", "zebra.png"]);
            wait_until(|| {
                app.poll_filter();
                app.filter_rx.is_none()
            });
            assert_eq!(app.gallery.filtered_indices, vec![0]);
        }

        #[test]
        fn update_filter_runs_in_the_background_and_an_empty_query_resets() {
            let mut app = app_with(&["apple.png", "berry.png", "cherry.png"]);
            app.poll_filter();
            app.gallery.search_query = "rry".to_owned();
            app.gallery.cursor = 1;
            app.update_filter();
            wait_until(|| {
                app.poll_filter();
                app.filter_rx.is_none()
            });
            let mut found = app.gallery.filtered_indices.clone();
            found.sort_unstable();
            assert_eq!(found, vec![1, 2]);
            assert_eq!(app.gallery.cursor, 0);

            app.gallery.search_query.clear();
            app.update_filter();
            assert_eq!(app.gallery.filtered_indices, vec![0, 1, 2]);
            assert!(app.filter_rx.is_none());
        }

        #[test]
        fn picker_opens_at_the_known_directory_and_switches() {
            let dir = test_dir();
            let root = fs::canonicalize(dir.path()).unwrap();
            let photos = root.join("photos");
            fs::create_dir(&photos).unwrap();
            RgbaImage::new(4, 4).save(photos.join("b.png")).unwrap();
            RgbaImage::new(4, 4).save(photos.join("a.png")).unwrap();

            let mut app = app_with(&["/nowhere/x.png"]);
            app.initial_dir = Some(root.clone());
            app.open_picker();
            assert_eq!(app.mode, ViewMode::Picker);
            assert_eq!(app.current_dir.as_deref(), Some(root.as_path()));
            app.close_picker();
            assert_eq!(app.mode, ViewMode::Gallery);
            assert!(app.picker.is_none());

            app.open_picker();
            app.selection.insert(PathBuf::from("/nowhere/x.png"));
            app.gallery.search_query = "x".to_owned();
            app.switch_to_dir(&photos);
            assert_eq!(app.current_dir.as_deref(), Some(photos.as_path()));
            assert_eq!(app.picker.as_ref().unwrap().current_dir, photos);
            assert!(app.images.is_empty());
            assert!(app.selection.is_empty());
            assert!(app.gallery.search_query.is_empty());

            wait_until(|| {
                app.refresh_from_scanner();
                !app.is_scanning()
            });
            assert_eq!(app.filenames, ["a.png", "b.png"]);

            // Without a known directory, the picker starts at the folder of the first image.
            let mut app = app_with(&[photos.join("a.png").to_str().unwrap()]);
            app.open_picker();
            assert_eq!(app.current_dir.as_deref(), Some(photos.as_path()));
        }

        #[test]
        fn navigation_moves_inside_the_list() {
            let dir = test_dir();
            let mut app = app_with_files(dir.path(), &["a.png", "b.png", "c.png"]);
            app.enter_fullscreen_at(0);
            app.prev();
            assert_eq!(app.current, 0);
            app.first();
            assert_eq!(app.current, 0);
            app.next();
            assert_eq!(app.current, 1);
            assert!(app.fullscreen_target.is_some(), "the move starts a decode");
            app.last();
            assert_eq!(app.current, 2);
            app.next();
            assert_eq!(app.current, 2);
            app.last();
            app.prev();
            assert_eq!(app.current, 1);
            app.first();
            assert_eq!(app.current, 0);

            // A prefetched neighbor shows at once.
            app.prefetcher
                .set_target_hint(Rect::new(0, 0, 1, 1), (8, 16));
            app.prefetcher.kick(0, &app.images);
            wait_until(|| {
                app.prefetcher.poll();
                app.prefetcher.pending_count() == 0
            });
            app.next();
            assert!(app.loaded.is_some());
            assert!(app.fullscreen_target.is_none());

            app.start_slideshow();
            app.enter_gallery();
            assert_eq!(app.mode, ViewMode::Gallery);
            assert!(!app.slideshow_running());
            assert!(app.loaded.is_none());
        }

        #[test]
        fn load_if_needed_takes_a_prefetched_image() {
            let dir = test_dir();
            let mut app = app_with_files(dir.path(), &["a.png", "b.png"]);
            app.image_rect = Rect::new(0, 0, 1, 1);
            app.enter_fullscreen_at(0);
            app.gallery.cursor = 1;
            app.pre_decode_hovered();
            wait_until(|| {
                app.prefetcher.poll();
                app.prefetcher.pending_count() == 0
            });
            app.current = 1;
            app.load_if_needed();
            assert!(app.loaded.is_some());
            assert!(app.fullscreen_target.is_none());
        }

        #[test]
        fn thumbnails_decode_once_and_come_back_from_the_disk_cache() {
            let dir = test_dir();
            let cache = dir.path().join("cache");
            let mut app = app_with_files(dir.path(), &["a.png"]);
            app.disk_cache_dir = cache.clone();
            let rect = Rect::new(0, 0, 1, 1);
            let path = app.images[0].clone();

            app.spawn_thumb_decode(0, path.clone(), rect);
            // A second request while the first one runs starts nothing.
            app.spawn_thumb_decode(0, path.clone(), rect);
            let mut arrived = Vec::new();
            wait_until(|| {
                arrived.extend(app.poll_thumbnails());
                !arrived.is_empty()
            });
            assert_eq!(arrived, vec![0]);
            assert!(app.thumb_cache.contains(0));
            let cached = get_cache_file_path(&cache, &path, 8, 16).unwrap();
            assert!(cached.is_file());
            app.spawn_thumb_decode(0, path.clone(), rect);
            assert!(
                !app.thumb_loading.contains(&0),
                "a cached thumbnail needs no decode"
            );

            // A fresh app reads the disk cache.
            let mut fresh = app_with(&[path.to_str().unwrap()]);
            fresh.disk_cache_dir = cache;
            fresh.spawn_thumb_decode(0, path, rect);
            let mut arrived = Vec::new();
            wait_until(|| {
                arrived.extend(fresh.poll_thumbnails());
                !arrived.is_empty()
            });
            assert_eq!(fresh.thumb_cache.peek(0).unwrap().0.dimensions(), (8, 8));
        }

        #[test]
        fn thumbnails_of_an_old_generation_are_dropped() {
            let mut app = app_with(&["a.png", "b.png"]);
            app.mark_dirty();
            let current = app.thumb_generation;
            // A job of the old generation and a job of the current one, in this order.
            app.thumb_tx
                .send((current - 1, 0, RgbaImage::new(1, 1)))
                .unwrap();
            app.thumb_tx
                .send((current, 1, RgbaImage::new(1, 1)))
                .unwrap();
            app.thumb_loading.insert(0);

            assert_eq!(app.poll_thumbnails(), vec![1]);
            assert!(!app.thumb_cache.contains(0));
            assert!(app.thumb_cache.contains(1));
            assert!(
                !app.thumb_loading.contains(&0),
                "a stale result still ends the load"
            );
        }

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
            app.transmitted_image_ids.insert(7);
            let mut out = Vec::new();
            app.graphics_delete_all_to(&mut out).unwrap();
            assert!(!out.is_empty());
            assert!(app.transmitted_image_ids.is_empty());

            app.transmitted_image_ids.insert(8);
            app.graphics_delete_all().unwrap();
            assert_eq!(
                app.graphics_out.take_bytes(),
                out,
                "the test buffer, not stdout"
            );
            assert!(app.transmitted_image_ids.is_empty());
        }

        #[test]
        fn disk_cache_round_trip_and_broken_entries() {
            let dir = test_dir();
            let cache = dir.path().join("cache");
            let source = dir.path().join("a.png");
            RgbaImage::new(4, 4).save(&source).unwrap();
            let thumb = RgbaImage::new(2, 3);

            assert!(get_cache_file_path(&cache, &dir.path().join("missing.png"), 1, 1).is_none());
            assert!(try_load_from_disk_cache(&cache, &source, 2, 3).is_none());
            assert_eq!(
                try_save_to_disk_cache(&cache, &source, 2, 3, &thumb),
                Some(())
            );
            assert_eq!(
                try_save_to_disk_cache(&cache, &source, 2, 3, &thumb),
                Some(())
            );
            assert_eq!(
                try_load_from_disk_cache(&cache, &source, 2, 3)
                    .unwrap()
                    .dimensions(),
                (2, 3)
            );

            let entry = get_cache_file_path(&cache, &source, 2, 3).unwrap();
            fs::write(&entry, b"not a png").unwrap();
            assert!(try_load_from_disk_cache(&cache, &source, 2, 3).is_none());
            assert!(!entry.exists(), "a broken entry goes away");

            // A cache directory that is a file cannot hold entries.
            let blocked = dir.path().join("blocked");
            fs::write(&blocked, b"file").unwrap();
            assert!(try_save_to_disk_cache(&blocked, &source, 2, 3, &thumb).is_none());
        }

        #[test]
        fn prune_removes_the_oldest_entries_first() {
            let dir = test_dir();
            let cache = dir.path();
            let base = SystemTime::now() - Duration::from_secs(1000);
            for (index, name) in ["old.png", "middle.png", "new.png"].iter().enumerate() {
                let path = cache.join(name);
                fs::write(&path, [0_u8; 100]).unwrap();
                let file = fs::File::options().write(true).open(&path).unwrap();
                file.set_modified(base + Duration::from_secs(index as u64 * 100))
                    .unwrap();
            }
            fs::write(cache.join("notes.txt"), [0_u8; 500]).unwrap();
            fs::create_dir(cache.join("folder.png")).unwrap();

            prune_disk_cache(cache, 1000);
            assert!(
                cache.join("old.png").exists(),
                "under the limit, nothing goes"
            );

            prune_disk_cache(cache, 150);
            assert!(!cache.join("old.png").exists());
            assert!(!cache.join("middle.png").exists());
            assert!(cache.join("new.png").exists());
            assert!(cache.join("notes.txt").exists());

            prune_disk_cache(&cache.join("missing"), 0);
        }

        #[test]
        fn load_and_resize_fits_the_cells() {
            let dir = test_dir();
            let path = dir.path().join("wide.png");
            RgbaImage::new(64, 32).save(&path).unwrap();
            let fitted = load_and_resize(
                &path,
                Rect::new(0, 0, 2, 2),
                (8, 8),
                EmbeddedThumbnail::Deny,
            )
            .unwrap();
            assert_eq!(fitted.dimensions(), (16, 8));
            let full =
                load_and_resize(&path, Rect::default(), (8, 8), EmbeddedThumbnail::Deny).unwrap();
            assert_eq!(full.dimensions(), (64, 32));
            assert!(
                load_and_resize(
                    &dir.path().join("missing.png"),
                    Rect::new(0, 0, 1, 1),
                    (8, 8),
                    EmbeddedThumbnail::Deny
                )
                .is_err()
            );
            let image = DynamicImage::new_rgba8(10, 0);
            assert_eq!(
                resize_decoded(&image, Rect::new(0, 0, 1, 1), (4, 4)).dimensions(),
                (10, 0)
            );
        }

        #[test]
        fn slideshow_struct_reports_its_interval() {
            let show = Slideshow {
                interval: Duration::from_secs(2),
                paused: true,
                next_at: Instant::now(),
            };
            let mut app = app_with(&["a.png"]);
            app.slideshow = Some(show);
            assert_eq!(app.slideshow_timeout(), None);
            app.slideshow_adjust_interval(-1.0);
            assert_eq!(
                app.slideshow.as_ref().unwrap().interval,
                Duration::from_millis(1500)
            );
        }

        #[cfg(feature = "video")]
        mod video {
            use super::{app_with, overdue};
            use crate::app::{VideoReturn, ViewMode};

            #[test]
            fn a_video_from_the_grid_returns_to_the_grid() {
                let mut app = app_with(&["clip.mp4", "b.png"]);
                app.enter_fullscreen_selected();
                assert_eq!(app.mode, ViewMode::Video);
                assert_eq!(app.video_return, VideoReturn::Gallery);
                app.return_from_video();
                assert_eq!(app.mode, ViewMode::Gallery);
            }

            #[test]
            fn a_video_from_fullscreen_returns_to_its_placeholder() {
                let mut app = app_with(&["a.png", "clip.mp4"]);
                app.enter_fullscreen_at(0);
                app.next();
                assert_eq!(app.mode, ViewMode::Video);
                assert_eq!(app.video_return, VideoReturn::Fullscreen);

                app.return_from_video();
                assert_eq!(app.mode, ViewMode::Fullscreen);
                assert!(app.shows_video_placeholder());

                // The placeholder needs no decode and no zoom.
                app.load_if_needed();
                assert!(app.fullscreen_target.is_none());
                app.zoom_in();
                assert_eq!(app.zoom, 1.0);

                app.enter_video(1);
                assert_eq!(app.video_return, VideoReturn::Fullscreen);
                app.exit_video();
                assert_eq!(app.mode, ViewMode::Gallery);
            }

            #[test]
            fn a_video_that_does_not_open_shows_its_error() {
                let mut app = app_with(&["/nowhere/clip.mp4"]);
                app.enter_fullscreen_at(0);
                app.open_video_if_needed();
                assert_eq!(app.mode, ViewMode::Fullscreen);
                assert!(app.error.is_some());
                assert!(!app.shows_video_placeholder());
            }

            #[test]
            fn the_clock_does_not_move_the_slideshow_in_the_video_view() {
                let mut app = app_with(&["a.png", "clip.mp4", "b.png"]);
                app.enter_fullscreen_at(0);
                app.start_slideshow();
                app.next();
                assert_eq!(app.mode, ViewMode::Video);
                assert!(app.slideshow_advancing());
                assert_eq!(app.slideshow_timeout(), None, "no wake-up for the clock");

                app.slideshow.as_mut().unwrap().next_at = overdue();
                app.slideshow_tick();
                assert_eq!(app.current, 1, "the clip, not the clock, ends the slide");
                assert_eq!(app.mode, ViewMode::Video);
            }

            #[test]
            fn a_slideshow_can_start_on_a_video() {
                let mut app = app_with(&["clip.mp4", "b.png"]);
                app.slideshow_autostart = true;
                app.scan_complete = true;
                app.maybe_autostart_slideshow();
                assert_eq!(app.mode, ViewMode::Video);
                assert!(app.slideshow_running());
                assert_eq!(app.video_return, VideoReturn::Fullscreen);

                // A paused slideshow does not wait for the end of the clip.
                app.slideshow_toggle_pause();
                assert!(!app.slideshow_advancing());
                app.slideshow_tick();
                assert_eq!(app.current, 0, "a paused slideshow waits");
            }

            #[test]
            fn the_slideshow_moves_past_a_video_placeholder() {
                let mut app = app_with(&["clip.mp4", "b.png"]);
                app.mode = ViewMode::Fullscreen;
                app.start_slideshow();
                app.slideshow.as_mut().unwrap().next_at = overdue();
                app.slideshow_tick();
                assert_eq!(app.current, 1);
                assert_eq!(app.mode, ViewMode::Fullscreen);
                // b.png is not prefetched, so its decode starts.
                assert!(app.loaded.is_none());
                assert_eq!(app.fullscreen_target.map(|(index, _)| index), Some(1));
            }
        }
    }
}
