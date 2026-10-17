use crate::config::{MAX_THUMB_COLS, MAX_THUMB_ROWS, MIN_THUMB_COLS, MIN_THUMB_ROWS};
use image::RgbaImage;
use lru::LruCache;
use ratatui::layout::Rect;
use std::num::NonZeroUsize;

pub(crate) const LABEL_ROWS: u16 = 1;
/// Multiplier applied to the thumbnail width on each grid zoom step.
const THUMB_ZOOM_STEP: f64 = 1.25;

pub(crate) struct GalleryState {
    pub filtered_indices: Vec<usize>,
    pub cursor: usize,
    pub scroll_offset: usize,
    pub grid_cols: usize,
    pub visible_rows: usize,
    pub grid_rect: Rect,
    pub search_active: bool,
    pub search_query: String,
    /// Thumbnail cell size in terminal cells, border included.
    pub thumb_cols: u16,
    pub thumb_rows: u16,
    /// Size the grid returns to on reset (from config).
    default_thumb: (u16, u16),
}

impl GalleryState {
    #[cfg(test)]
    pub(crate) fn new(image_count: usize) -> Self {
        use crate::config::{DEFAULT_THUMB_COLS, DEFAULT_THUMB_ROWS};
        Self::with_thumb_size(image_count, DEFAULT_THUMB_COLS, DEFAULT_THUMB_ROWS)
    }

    pub(crate) fn with_thumb_size(image_count: usize, thumb_cols: u16, thumb_rows: u16) -> Self {
        let thumb_cols = thumb_cols.clamp(MIN_THUMB_COLS, MAX_THUMB_COLS);
        let thumb_rows = thumb_rows.clamp(MIN_THUMB_ROWS, MAX_THUMB_ROWS);
        Self {
            filtered_indices: (0..image_count).collect(),
            cursor: 0,
            scroll_offset: 0,
            grid_cols: 1,
            visible_rows: 1,
            grid_rect: Rect::default(),
            search_active: false,
            search_query: String::new(),
            thumb_cols,
            thumb_rows,
            default_thumb: (thumb_cols, thumb_rows),
        }
    }

    /// Height of one grid row: thumbnail, label and a spacer line.
    pub(crate) fn cell_height_total(&self) -> u16 {
        self.thumb_rows + LABEL_ROWS + 1
    }

    /// Make the configured default the current size again. Returns `true` if it changed.
    pub(crate) fn reset_thumb_size(&mut self) -> bool {
        let (cols, rows) = self.default_thumb;
        self.set_thumb_size(cols, rows)
    }

    /// Change the thumbnail size, keeping the hovered image on screen.
    /// Returns `true` if the size actually changed.
    pub(crate) fn set_thumb_size(&mut self, cols: u16, rows: u16) -> bool {
        let cols = cols.clamp(MIN_THUMB_COLS, MAX_THUMB_COLS);
        let rows = rows.clamp(MIN_THUMB_ROWS, MAX_THUMB_ROWS);
        if (cols, rows) == (self.thumb_cols, self.thumb_rows) {
            return false;
        }
        self.thumb_cols = cols;
        self.thumb_rows = rows;
        self.update_grid(self.grid_rect);
        true
    }

    /// Larger thumbnails. The width grows by a fixed factor and the height follows
    /// the configured aspect ratio, so the step feels the same at every size.
    pub(crate) fn thumb_zoom_in(&mut self) -> bool {
        let cols = (f64::from(self.thumb_cols) * THUMB_ZOOM_STEP).round() as u16;
        let cols = cols.max(self.thumb_cols + 1);
        let rows = self.rows_for_cols(cols);
        self.set_thumb_size(cols, rows)
    }

    pub(crate) fn thumb_zoom_out(&mut self) -> bool {
        let cols = (f64::from(self.thumb_cols) / THUMB_ZOOM_STEP).round() as u16;
        let cols = cols.min(self.thumb_cols.saturating_sub(1));
        let rows = self.rows_for_cols(cols);
        self.set_thumb_size(cols, rows)
    }

    fn rows_for_cols(&self, cols: u16) -> u16 {
        let (default_cols, default_rows) = self.default_thumb;
        let ratio = f64::from(default_rows) / f64::from(default_cols.max(1));
        (f64::from(cols) * ratio)
            .round()
            .max(f64::from(MIN_THUMB_ROWS)) as u16
    }

    pub(crate) fn reset_cursor(&mut self) {
        self.cursor = 0;
        self.scroll_offset = 0;
    }

    pub(crate) fn update_grid(&mut self, grid_rect: Rect) {
        self.grid_rect = grid_rect;
        self.grid_cols = (grid_rect.width as usize / self.thumb_cols as usize).max(1);
        self.visible_rows = (grid_rect.height as usize / self.cell_height_total() as usize).max(1);
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
    use crate::config::{DEFAULT_THUMB_COLS, MAX_THUMB_COLS, MIN_THUMB_COLS};
    use ratatui::layout::Rect;

    #[test]
    fn grid_navigation_tracks_rows_and_scroll() {
        let mut gallery = GalleryState::new(12);
        gallery.update_grid(Rect::new(0, 0, DEFAULT_THUMB_COLS * 3, 20));

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
    fn thumb_zoom_steps_keep_aspect_and_clamp() {
        let mut gallery = GalleryState::with_thumb_size(50, 20, 8);
        gallery.update_grid(Rect::new(0, 0, 100, 40));
        assert_eq!(gallery.grid_cols, 5);

        assert!(gallery.thumb_zoom_in());
        assert_eq!((gallery.thumb_cols, gallery.thumb_rows), (25, 10));
        assert_eq!(gallery.grid_cols, 4);

        assert!(gallery.thumb_zoom_out());
        assert_eq!((gallery.thumb_cols, gallery.thumb_rows), (20, 8));

        for _ in 0..40 {
            gallery.thumb_zoom_in();
        }
        assert_eq!(gallery.thumb_cols, MAX_THUMB_COLS);
        assert!(!gallery.thumb_zoom_in());

        for _ in 0..40 {
            gallery.thumb_zoom_out();
        }
        assert_eq!(gallery.thumb_cols, MIN_THUMB_COLS);
        assert!(!gallery.thumb_zoom_out());

        assert!(gallery.reset_thumb_size());
        assert_eq!((gallery.thumb_cols, gallery.thumb_rows), (20, 8));
        assert!(!gallery.reset_thumb_size());
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
        let mut gallery = GalleryState::new(15);
        gallery.update_grid(Rect::new(0, 0, DEFAULT_THUMB_COLS * 3, 20));

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
        let mut gallery = GalleryState::new(12);
        gallery.update_grid(Rect::new(0, 0, DEFAULT_THUMB_COLS * 3, 20));

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
        let mut gallery = GalleryState::new(20);
        gallery.update_grid(Rect::new(0, 0, DEFAULT_THUMB_COLS * 4, 24));

        gallery.move_page_down();
        assert_eq!(gallery.selected_index(), Some(8));
        gallery.move_page_down();
        assert_eq!(gallery.selected_index(), Some(16));
        gallery.move_page_up();
        assert_eq!(gallery.selected_index(), Some(8));
    }

    #[test]
    fn scroll_follows_cursor() {
        let mut gallery = GalleryState::new(20);
        gallery.update_grid(Rect::new(0, 0, DEFAULT_THUMB_COLS * 4, 24));

        gallery.move_page_down();
        gallery.move_right();
        gallery.move_right();
        assert_eq!(gallery.selected_index(), Some(10));
        assert_eq!(gallery.scroll_offset, 1);
    }

    #[test]
    fn visible_items_and_cell_rect_match_geometry() {
        let mut gallery = GalleryState::new(12);
        gallery.update_grid(Rect::new(0, 0, DEFAULT_THUMB_COLS * 3, 20));
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
