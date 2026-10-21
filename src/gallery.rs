use crate::config::MAX_VISIBLE_ROWS;
use image::RgbaImage;
use lru::LruCache;
use ratatui::layout::Rect;
use std::num::NonZeroUsize;

pub(crate) const LABEL_ROWS: u16 = 1;

/// Provisional cell size before the first `update_grid` computes the real one.
const DEFAULT_THUMB_COLS: u16 = 20;
const DEFAULT_THUMB_ROWS: u16 = 8;
const MIN_THUMB_COLS: u16 = 8;
const MAX_THUMB_COLS: u16 = 120;
const MIN_THUMB_ROWS: u16 = 3;
const MAX_THUMB_ROWS: u16 = 60;

/// Cell aspect (columns per row) for the column count: about a 4:3 photo in cells
/// that are twice as tall as wide.
const DEFAULT_CELL_ASPECT: f64 = 2.5;
/// The grid always shows at least this many columns.
const MIN_GRID_COLS: usize = 2;

pub(crate) struct GalleryState {
    pub filtered_indices: Vec<usize>,
    pub cursor: usize,
    pub scroll_offset: usize,
    pub grid_cols: usize,
    /// The rows that fit after the zoom level is clamped to the terminal height.
    pub visible_rows: usize,
    pub grid_rect: Rect,
    pub search_active: bool,
    pub search_query: String,
    /// Show the filename label below each thumbnail.
    pub show_filenames: bool,
    /// The zoom level: how many thumbnail rows the grid shows.
    pub zoom_rows: usize,
    /// Thumbnail cell size in terminal cells, border included. `update_grid` computes it.
    pub thumb_cols: u16,
    pub thumb_rows: u16,
    /// Zoom level the grid returns to on reset (from config).
    default_zoom_rows: usize,
}

impl GalleryState {
    #[cfg(test)]
    pub(crate) fn new(image_count: usize) -> Self {
        use crate::config::DEFAULT_VISIBLE_ROWS;
        Self::with_zoom_rows(image_count, usize::from(DEFAULT_VISIBLE_ROWS))
    }

    pub(crate) fn with_zoom_rows(image_count: usize, zoom_rows: usize) -> Self {
        let zoom_rows = zoom_rows.clamp(1, usize::from(MAX_VISIBLE_ROWS));
        Self {
            filtered_indices: (0..image_count).collect(),
            cursor: 0,
            scroll_offset: 0,
            grid_cols: 1,
            visible_rows: 1,
            grid_rect: Rect::default(),
            search_active: false,
            search_query: String::new(),
            show_filenames: false,
            zoom_rows,
            thumb_cols: DEFAULT_THUMB_COLS,
            thumb_rows: DEFAULT_THUMB_ROWS,
            default_zoom_rows: zoom_rows,
        }
    }

    /// Height of one grid row: the thumbnail and the optional label, no spacer.
    pub(crate) fn cell_height_total(&self) -> u16 {
        self.thumb_rows + self.label_rows()
    }

    /// Show or hide the filename labels and reflow the visible rows.
    pub(crate) fn toggle_filenames(&mut self) {
        self.show_filenames = !self.show_filenames;
        self.update_grid(self.grid_rect);
    }

    fn label_rows(&self) -> u16 {
        u16::from(self.show_filenames)
    }

    /// Back to the configured zoom level. Returns `true` if it changed.
    pub(crate) fn reset_zoom(&mut self) -> bool {
        self.set_zoom_rows(self.default_zoom_rows)
    }

    /// One row fewer: larger thumbnails. Returns `true` if the level changed.
    pub(crate) fn thumb_zoom_in(&mut self) -> bool {
        self.set_zoom_rows(self.zoom_rows.saturating_sub(1))
    }

    /// One row more: smaller thumbnails. Returns `true` if the level changed.
    pub(crate) fn thumb_zoom_out(&mut self) -> bool {
        self.set_zoom_rows(self.zoom_rows + 1)
    }

    fn set_zoom_rows(&mut self, rows: usize) -> bool {
        let rows = rows.clamp(1, self.max_visible_rows());
        if rows == self.zoom_rows {
            return false;
        }
        self.zoom_rows = rows;
        self.update_grid(self.grid_rect);
        true
    }

    /// The rows that fit the terminal at the smallest thumbnail height.
    fn max_visible_rows(&self) -> usize {
        if self.grid_rect.is_empty() {
            return usize::from(MAX_VISIBLE_ROWS);
        }
        let min_cell = usize::from(MIN_THUMB_ROWS + self.label_rows());
        (self.grid_rect.height as usize / min_cell).max(1)
    }

    pub(crate) fn reset_cursor(&mut self) {
        self.cursor = 0;
        self.scroll_offset = 0;
    }

    /// Fit `zoom_rows` rows into the terminal: the rows share the height, and the columns
    /// share the width. The cell aspect only picks the column count, then the width fills.
    pub(crate) fn update_grid(&mut self, grid_rect: Rect) {
        self.grid_rect = grid_rect;
        self.visible_rows = self.zoom_rows.clamp(1, self.max_visible_rows());
        let cell_height =
            (grid_rect.height / self.visible_rows as u16).max(MIN_THUMB_ROWS + self.label_rows());
        self.thumb_rows = cell_height
            .saturating_sub(self.label_rows())
            .clamp(MIN_THUMB_ROWS, MAX_THUMB_ROWS);
        let candidate_cols = (f64::from(self.thumb_rows) * DEFAULT_CELL_ASPECT).round() as u16;
        self.grid_cols =
            (grid_rect.width as usize / usize::from(candidate_cols.max(1))).max(MIN_GRID_COLS);
        self.thumb_cols =
            (grid_rect.width / self.grid_cols as u16).clamp(MIN_THUMB_COLS, MAX_THUMB_COLS);
        self.ensure_cursor_visible();
    }

    pub(crate) fn selected_index(&self) -> Option<usize> {
        self.filtered_indices.get(self.cursor).copied()
    }

    pub(crate) fn cursor_row(&self) -> usize {
        self.cursor / self.grid_cols
    }

    pub(crate) fn ensure_cursor_visible(&mut self) {
        let row = self.cursor_row();
        if row < self.scroll_offset {
            self.scroll_offset = row;
        }
        if row >= self.scroll_offset + self.visible_rows {
            self.scroll_offset = row - self.visible_rows + 1;
        }
    }

    fn move_cursor_to(&mut self, new: usize) {
        let max = self.filtered_indices.len().saturating_sub(1);
        self.cursor = new.min(max);
        self.ensure_cursor_visible();
    }

    pub(crate) fn move_left(&mut self) {
        if self.cursor > 0 {
            self.move_cursor_to(self.cursor - 1);
        }
    }

    pub(crate) fn move_right(&mut self) {
        self.move_cursor_to(self.cursor + 1);
    }

    pub(crate) fn move_up(&mut self) {
        if self.cursor >= self.grid_cols {
            self.move_cursor_to(self.cursor - self.grid_cols);
        }
    }

    pub(crate) fn move_down(&mut self) {
        let target = self.cursor + self.grid_cols;
        if target < self.filtered_indices.len() {
            self.move_cursor_to(target);
        }
    }

    pub(crate) fn move_page_up(&mut self) {
        let jump = self.visible_rows * self.grid_cols;
        self.move_cursor_to(self.cursor.saturating_sub(jump));
    }

    pub(crate) fn move_page_down(&mut self) {
        let jump = self.visible_rows * self.grid_cols;
        self.move_cursor_to(self.cursor + jump);
    }

    pub(crate) fn move_to_first(&mut self) {
        self.move_cursor_to(0);
    }

    pub(crate) fn move_to_last(&mut self) {
        let last = self.filtered_indices.len().saturating_sub(1);
        self.move_cursor_to(last);
    }

    pub(crate) fn visible_items(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        let start = self.scroll_offset * self.grid_cols;
        let count = self.visible_rows * self.grid_cols;
        self.filtered_indices[start..self.filtered_indices.len().min(start + count)]
            .iter()
            .enumerate()
            .map(move |(vis, &img_idx)| (vis, img_idx))
    }

    pub(crate) fn cell_rect(&self, vis_index: usize) -> Rect {
        let col = vis_index % self.grid_cols;
        let row = vis_index / self.grid_cols;
        Rect {
            x: self.grid_rect.x + (col as u16) * self.thumb_cols,
            y: self.grid_rect.y + (row as u16) * self.cell_height_total(),
            width: self.thumb_cols,
            height: self.thumb_rows,
        }
    }
}

const THUMB_CACHE_CAPACITY: NonZeroUsize = NonZeroUsize::new(200).unwrap();

pub(crate) struct ThumbnailCache {
    cache: LruCache<usize, CachedThumb>,
    next_id: u32,
}

struct CachedThumb {
    image: RgbaImage,
    image_id: u32,
}

impl ThumbnailCache {
    pub(crate) fn new() -> Self {
        Self {
            cache: LruCache::new(THUMB_CACHE_CAPACITY),
            next_id: 1,
        }
    }

    pub(crate) fn contains(&self, index: usize) -> bool {
        self.cache.contains(&index)
    }

    pub(crate) fn peek(&self, index: usize) -> Option<(&RgbaImage, u32)> {
        self.cache.peek(&index).map(|t| (&t.image, t.image_id))
    }

    pub(crate) fn insert(&mut self, index: usize, image: RgbaImage) {
        let id = self.next_id;
        self.next_id += 1;
        self.cache.push(
            index,
            CachedThumb {
                image,
                image_id: id,
            },
        );
    }

    pub(crate) fn clear(&mut self) {
        self.cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{GalleryState, ThumbnailCache};
    use ratatui::layout::Rect;

    #[test]
    fn grid_navigation_tracks_rows_and_scroll() {
        let mut gallery = GalleryState::with_zoom_rows(12, 2);
        gallery.update_grid(Rect::new(0, 0, 75, 20));

        gallery.move_down();
        assert_eq!(gallery.selected_index(), Some(3));
        gallery.move_page_down();
        assert_eq!(gallery.selected_index(), Some(9));
        assert_eq!(gallery.scroll_offset, 2);
        gallery.move_to_last();
        assert_eq!(gallery.selected_index(), Some(11));
    }

    #[test]
    fn empty_gallery_navigation_remains_unselected() {
        let mut gallery = GalleryState::new(0);
        gallery.move_right();
        gallery.move_down();
        gallery.move_to_last();
        assert_eq!(gallery.selected_index(), None);
    }

    #[test]
    fn filename_toggle_moves_a_row_from_the_thumbnails_to_the_labels() {
        let mut gallery = GalleryState::with_zoom_rows(12, 4);
        gallery.update_grid(Rect::new(0, 0, 75, 20));
        assert!(!gallery.show_filenames);
        assert_eq!(gallery.cell_height_total(), 5);
        assert_eq!(gallery.thumb_rows, 5);

        gallery.toggle_filenames();
        assert!(gallery.show_filenames);
        assert_eq!(gallery.thumb_rows, 4);
        assert_eq!(gallery.cell_height_total(), 5);
        assert_eq!(gallery.visible_rows, 4);
    }

    #[test]
    fn zoom_changes_the_row_count_and_clamps() {
        let mut gallery = GalleryState::with_zoom_rows(50, 4);
        gallery.update_grid(Rect::new(0, 0, 100, 40));
        assert_eq!(gallery.grid_cols, 4);
        assert_eq!((gallery.thumb_cols, gallery.thumb_rows), (25, 10));

        assert!(gallery.thumb_zoom_in());
        assert_eq!(gallery.zoom_rows, 3);
        assert!(gallery.thumb_rows > 10);

        assert!(gallery.thumb_zoom_out());
        assert_eq!((gallery.thumb_cols, gallery.thumb_rows), (25, 10));

        for _ in 0..40 {
            gallery.thumb_zoom_in();
        }
        assert_eq!(gallery.zoom_rows, 1);
        assert!(!gallery.thumb_zoom_in());

        for _ in 0..40 {
            gallery.thumb_zoom_out();
        }
        assert_eq!(gallery.zoom_rows, 13, "40 rows at 3 rows minimum each");
        assert!(!gallery.thumb_zoom_out());

        assert!(gallery.reset_zoom());
        assert_eq!(gallery.zoom_rows, 4);
        assert!(!gallery.reset_zoom());
    }

    #[test]
    fn selected_index_follows_filtered_indices() {
        let mut gallery = GalleryState::new(5);
        gallery.filtered_indices = vec![4, 2, 0];
        gallery.cursor = 1;

        assert_eq!(gallery.selected_index(), Some(2));
    }

    #[test]
    fn cursor_row_matches_grid_columns() {
        let mut gallery = GalleryState::with_zoom_rows(15, 2);
        gallery.update_grid(Rect::new(0, 0, 75, 20));

        gallery.move_to_first();
        for _ in 0..4 {
            gallery.move_right();
        }
        assert_eq!(gallery.cursor_row(), 1);
        gallery.move_down();
        gallery.move_down();
        assert_eq!(gallery.cursor_row(), 3);
    }

    #[test]
    fn directional_moves_within_grid_bounds() {
        let mut gallery = GalleryState::with_zoom_rows(12, 2);
        gallery.update_grid(Rect::new(0, 0, 75, 20));

        gallery.move_right();
        assert_eq!(gallery.selected_index(), Some(1));
        gallery.move_down();
        assert_eq!(gallery.selected_index(), Some(4));
        gallery.move_left();
        assert_eq!(gallery.selected_index(), Some(3));
        gallery.move_up();
        assert_eq!(gallery.selected_index(), Some(0));
        gallery.move_to_last();
        assert_eq!(gallery.selected_index(), Some(11));
        gallery.move_to_first();
        assert_eq!(gallery.selected_index(), Some(0));
    }

    #[test]
    fn page_moves_jump_by_visible_rows() {
        let mut gallery = GalleryState::with_zoom_rows(20, 2);
        gallery.update_grid(Rect::new(0, 0, 120, 24));

        gallery.move_page_down();
        assert_eq!(gallery.selected_index(), Some(8));
        gallery.move_page_down();
        assert_eq!(gallery.selected_index(), Some(16));
        gallery.move_page_up();
        assert_eq!(gallery.selected_index(), Some(8));
    }

    #[test]
    fn scroll_follows_cursor() {
        let mut gallery = GalleryState::with_zoom_rows(20, 2);
        gallery.update_grid(Rect::new(0, 0, 120, 24));

        gallery.move_page_down();
        gallery.move_right();
        gallery.move_right();
        assert_eq!(gallery.selected_index(), Some(10));
        assert_eq!(gallery.scroll_offset, 1);
    }

    #[test]
    fn visible_items_and_cell_rect_match_geometry() {
        let mut gallery = GalleryState::with_zoom_rows(12, 2);
        gallery.update_grid(Rect::new(0, 0, 75, 20));
        gallery.scroll_offset = 1;

        let items: Vec<_> = gallery.visible_items().collect();
        assert_eq!(items.len(), gallery.visible_rows * gallery.grid_cols);

        let first = gallery.cell_rect(0);
        assert_eq!(first.x, gallery.grid_rect.x);
        assert_eq!(first.y, gallery.grid_rect.y);
        assert_eq!(first.width, gallery.thumb_cols);
        assert_eq!(first.height, gallery.thumb_rows);

        let last_col = gallery.grid_cols - 1;
        let last = gallery.cell_rect(last_col);
        assert_eq!(
            last.x,
            gallery.grid_rect.x + (last_col as u16) * gallery.thumb_cols
        );
    }

    #[test]
    fn thumbnail_cache_insert_peek_and_clear() {
        let mut cache = ThumbnailCache::new();
        let img = image::RgbaImage::new(2, 2);
        cache.insert(0, img.clone());

        assert!(cache.contains(0));
        assert!(!cache.contains(1));

        let (peeked, id) = cache.peek(0).expect("peek should return the cached image");
        assert_eq!(peeked.dimensions(), (2, 2));
        assert_eq!(id, 1);

        cache.insert(1, img);
        let (_, id2) = cache.peek(1).expect("second image should be cached");
        assert_eq!(id2, 2);

        cache.clear();
        assert!(!cache.contains(0));
        assert!(!cache.contains(1));
    }
}
