use crate::config::{MAX_VISIBLE_ROWS, MIN_VISIBLE_ROWS};
use image::RgbaImage;
use lru::LruCache;
use ratatui::layout::Rect;
use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::ops::Range;

pub(crate) const LABEL_ROWS: u16 = 1;
/// The gap in terminal cells between two cells and at the edges of the grid. The border of the
/// selected and the marked cells sits in it.
pub(crate) const GAP: u16 = 1;

/// Provisional cell size before the first `update_grid` computes the real one.
const DEFAULT_THUMB_COLS: u16 = 20;
const DEFAULT_THUMB_ROWS: u16 = 8;
const MIN_THUMB_COLS: u16 = 8;
const MAX_THUMB_COLS: u16 = 120;
const MIN_THUMB_ROWS: u16 = 3;
const MAX_THUMB_ROWS: u16 = 60;

/// The aspect ratio (width over height) of a photo whose size nobody knows: 3:2.
pub(crate) const DEFAULT_IMAGE_ASPECT: f64 = 1.5;
/// The height of a terminal cell over its width, before the terminal reports its cell size.
const DEFAULT_CELL_RATIO: f64 = 2.0;
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
    /// The base cell size in terminal cells. `update_grid` computes it. Every thumbnail is
    /// decoded for this size.
    pub thumb_cols: u16,
    pub thumb_rows: u16,
    /// The first `extra_cols` columns are one cell wider, and the first `extra_rows` rows one
    /// cell taller, so that the grid reaches the right and the bottom edge, minus the edge gap.
    pub extra_cols: u16,
    pub extra_rows: u16,
    /// The width of a cell over its height, in terminal cells, for the column count. It comes
    /// from the photos of the folder and the pixel size of a cell.
    cols_per_row: f64,
    /// The width over the height of the photos of the folder, behind `cols_per_row`.
    image_aspect: f64,
    /// Zoom level the grid returns to on reset (from config).
    pub(crate) default_zoom_rows: usize,
}

impl GalleryState {
    #[cfg(test)]
    pub(crate) fn new(image_count: usize) -> Self {
        use crate::config::DEFAULT_VISIBLE_ROWS;
        Self::with_zoom_rows(image_count, usize::from(DEFAULT_VISIBLE_ROWS))
    }

    pub(crate) fn with_zoom_rows(image_count: usize, zoom_rows: usize) -> Self {
        let zoom_rows =
            zoom_rows.clamp(usize::from(MIN_VISIBLE_ROWS), usize::from(MAX_VISIBLE_ROWS));
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
            extra_cols: 0,
            extra_rows: 0,
            cols_per_row: DEFAULT_IMAGE_ASPECT * DEFAULT_CELL_RATIO,
            image_aspect: DEFAULT_IMAGE_ASPECT,
            default_zoom_rows: zoom_rows,
        }
    }

    /// Shape the cells like a photo with `image_aspect` (width over height) on cells of
    /// `cell_px`. Returns `true` if the grid changed, so the thumbnails need a new decode.
    pub(crate) fn set_cell_shape(&mut self, image_aspect: f64, cell_px: (u32, u32)) -> bool {
        let cell_ratio = if cell_px.0 == 0 || cell_px.1 == 0 {
            DEFAULT_CELL_RATIO
        } else {
            f64::from(cell_px.1) / f64::from(cell_px.0)
        };
        let aspect = if image_aspect.is_finite() && image_aspect > 0.0 {
            image_aspect
        } else {
            DEFAULT_IMAGE_ASPECT
        };
        let before = (self.grid_cols, self.thumb_cols, self.extra_cols);
        self.image_aspect = aspect;
        self.cols_per_row = aspect * cell_ratio;
        self.update_grid(self.grid_rect);
        before != (self.grid_cols, self.thumb_cols, self.extra_cols)
    }

    /// The photo shape of the folder that the last `set_cell_shape` took.
    pub(crate) fn image_aspect(&self) -> f64 {
        self.image_aspect
    }

    /// The box that every thumbnail is decoded for: the base cell size.
    pub(crate) fn thumb_box(&self) -> Rect {
        Rect::new(0, 0, self.thumb_cols, self.thumb_rows)
    }

    /// Height of one grid row: the thumbnail and the optional label, without the gap.
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

    pub(crate) fn set_zoom_rows(&mut self, rows: usize) -> bool {
        let rows = rows.clamp(self.min_visible_rows(), self.max_visible_rows());
        if rows == self.zoom_rows {
            return false;
        }
        self.zoom_rows = rows;
        self.update_grid(self.grid_rect);
        true
    }

    /// The rows that fit the terminal at the smallest thumbnail height, at most
    /// `MAX_VISIBLE_ROWS`.
    fn max_visible_rows(&self) -> usize {
        let most = usize::from(MAX_VISIBLE_ROWS);
        if self.grid_rect.is_empty() {
            return most;
        }
        let min_cell = usize::from(MIN_THUMB_ROWS + self.label_rows() + GAP);
        (usize::from(self.grid_rect.height.saturating_sub(GAP)) / min_cell)
            .max(1)
            .min(most)
    }

    /// `MIN_VISIBLE_ROWS`, or fewer rows in a terminal that is too small for them.
    fn min_visible_rows(&self) -> usize {
        usize::from(MIN_VISIBLE_ROWS).min(self.max_visible_rows())
    }

    pub(crate) fn reset_cursor(&mut self) {
        self.cursor = 0;
        self.scroll_offset = 0;
    }

    /// Fit `zoom_rows` rows into the terminal: the rows share the height, and the columns
    /// share the width. `n` cells take `n + 1` gaps, one between two cells and one at each
    /// edge. The cell shape only picks the column count, then the width fills. The remainders
    /// of the division go to the first columns and the first rows, one cell each.
    pub(crate) fn update_grid(&mut self, grid_rect: Rect) {
        self.grid_rect = grid_rect;
        self.visible_rows = self
            .zoom_rows
            .clamp(self.min_visible_rows(), self.max_visible_rows());
        let rows = self.visible_rows as u16;
        let cell_area_height = grid_rect.height.saturating_sub(gaps(rows));
        let cell_height = (cell_area_height / rows).max(MIN_THUMB_ROWS + self.label_rows());
        self.thumb_rows = cell_height
            .saturating_sub(self.label_rows())
            .clamp(MIN_THUMB_ROWS, MAX_THUMB_ROWS);
        self.extra_rows = spread(cell_area_height, self.cell_height_total(), rows);
        let candidate_cols = (f64::from(self.thumb_rows) * self.cols_per_row).round() as u16;
        self.grid_cols = (usize::from(grid_rect.width.saturating_sub(GAP))
            / usize::from(candidate_cols.max(1) + GAP))
        .max(MIN_GRID_COLS);
        let cols = self.grid_cols as u16;
        let cell_area_width = grid_rect.width.saturating_sub(gaps(cols));
        self.thumb_cols = (cell_area_width / cols).clamp(MIN_THUMB_COLS, MAX_THUMB_COLS);
        self.extra_cols = spread(cell_area_width, self.thumb_cols, cols);
        self.ensure_cursor_visible();
    }

    /// The left edge of the column `col`: the gaps and the columns before it, plus one cell for
    /// each wide one.
    fn col_x(&self, col: u16) -> u16 {
        self.grid_rect.x + GAP + col * (self.thumb_cols + GAP) + col.min(self.extra_cols)
    }

    /// The top edge of the row `row`, with one cell for each tall row before it.
    fn row_y(&self, row: u16) -> u16 {
        self.grid_rect.y + GAP + row * (self.cell_height_total() + GAP) + row.min(self.extra_rows)
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

    /// The cells of one screen, filled or empty.
    pub(crate) fn visible_cells(&self) -> usize {
        self.visible_rows * self.grid_cols
    }

    /// The positions in `filtered_indices` that the screen shows. A range borrows nothing, so
    /// a caller can change the app while it walks the cells.
    pub(crate) fn visible_range(&self) -> Range<usize> {
        let start = self.scroll_offset * self.grid_cols;
        let count = self.visible_rows * self.grid_cols;
        start..self.filtered_indices.len().min(start + count)
    }

    /// The visible cells as (cell position, image index).
    pub(crate) fn visible_items(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.visible_range()
            .enumerate()
            .map(|(vis, position)| (vis, self.filtered_indices[position]))
    }

    /// The thumbnail area of the visible cell `vis_index`, without the label row.
    pub(crate) fn cell_rect(&self, vis_index: usize) -> Rect {
        let col = (vis_index % self.grid_cols) as u16;
        let row = (vis_index / self.grid_cols) as u16;
        Rect {
            x: self.col_x(col),
            y: self.row_y(row),
            width: self.thumb_cols + u16::from(col < self.extra_cols),
            height: self.thumb_rows + u16::from(row < self.extra_rows),
        }
    }

    /// The border of the visible cell `vis_index`: the thumbnail and the label, plus the gap on
    /// each side. Two neighbor cells share the gap between them.
    pub(crate) fn border_rect(&self, vis_index: usize) -> Rect {
        let cell = self.cell_rect(vis_index);
        Rect {
            x: cell.x - GAP,
            y: cell.y - GAP,
            width: cell.width + 2 * GAP,
            height: cell.height + self.label_rows() + 2 * GAP,
        }
    }
}

/// The gap cells that `count` cells in a line take: one between two cells and one at each edge.
fn gaps(count: u16) -> u16 {
    (count + 1) * GAP
}

/// How many of the `count` cells get one cell more, so that `count` cells of `base` fill
/// `total`. Zero when the base size came from a clamp and the cells cannot fill `total`.
fn spread(total: u16, base: u16, count: u16) -> u16 {
    let rest = total.saturating_sub(base.saturating_mul(count));
    if rest < count { rest } else { 0 }
}

/// The least number of thumbnails that the in-memory cache keeps. The capacity grows to two
/// times the visible cell count, so the inserts of one screen never evict a visible cell.
const THUMB_CACHE_MIN_CAPACITY: usize = 200;

/// The cache capacity for `visible_cells`: two screens, at least the minimum.
fn thumb_cache_capacity(visible_cells: usize) -> NonZeroUsize {
    let capacity = visible_cells
        .saturating_mul(2)
        .max(THUMB_CACHE_MIN_CAPACITY);
    NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::MIN)
}

/// The bytes of padded pixels that the in-memory cache holds, at most. Above it, the oldest
/// thumbnails that no visible cell shows go, like a count eviction.
pub(crate) const THUMB_CACHE_BYTE_LIMIT: usize = 512 * 1024 * 1024;

pub(crate) struct ThumbnailCache {
    cache: LruCache<usize, CachedThumb>,
    next_id: u32,
    /// The Kitty ids of the thumbnails that the cache dropped. The next write of the grid
    /// deletes them in the terminal, so its storage does not grow without end.
    evicted_ids: Vec<u32>,
    /// The padded pixel bytes of every entry.
    bytes: usize,
    /// The bytes that the entries may take. A test sets a small one.
    pub(crate) byte_limit: usize,
    /// The cells on the screen. The byte limit never evicts them.
    visible: HashSet<usize>,
}

struct CachedThumb {
    image: RgbaImage,
    image_id: u32,
    /// False for a preview from an embedded thumbnail, which the sharp decode replaces.
    sharp: bool,
}

impl ThumbnailCache {
    pub(crate) fn new() -> Self {
        Self {
            cache: LruCache::new(thumb_cache_capacity(0)),
            next_id: 1,
            evicted_ids: Vec::new(),
            bytes: 0,
            byte_limit: THUMB_CACHE_BYTE_LIMIT,
            visible: HashSet::new(),
        }
    }

    /// The padded pixel bytes of every entry.
    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    /// Name the cells of the screen. The grid calls this before its inserts, so that the byte
    /// limit evicts only the cells of other screens.
    pub(crate) fn set_visible(&mut self, indices: impl IntoIterator<Item = usize>) {
        self.visible = indices.into_iter().collect();
    }

    /// Make room for two screens of `visible_cells`, at least `THUMB_CACHE_MIN_CAPACITY`. A
    /// smaller grid drops the oldest thumbnails and names their ids for the delete.
    pub(crate) fn fit_capacity(&mut self, visible_cells: usize) {
        let capacity = thumb_cache_capacity(visible_cells);
        if capacity == self.cache.cap() {
            return;
        }
        while self.cache.len() > capacity.get() {
            if let Some((_, old)) = self.cache.pop_lru() {
                self.forget(&old);
            }
        }
        self.cache.resize(capacity);
    }

    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.cache.cap().get()
    }

    /// True when the cell has an image, a preview or a sharp one.
    pub(crate) fn contains(&self, index: usize) -> bool {
        self.cache.contains(&index)
    }

    /// True when the cell has its final thumbnail, so no decode is necessary.
    pub(crate) fn is_sharp(&self, index: usize) -> bool {
        self.cache.peek(&index).is_some_and(|t| t.sharp)
    }

    /// The image and the Kitty id of a cell, without a change of the eviction order.
    pub(crate) fn peek(&self, index: usize) -> Option<(&RgbaImage, u32)> {
        self.cache.peek(&index).map(|t| (&t.image, t.image_id))
    }

    /// Make the cell the newest entry. The grid touches each visible cell before its inserts,
    /// so the inserts of one screen evict only the cells of older screens.
    pub(crate) fn touch(&mut self, index: usize) {
        self.cache.promote(&index);
    }

    /// Store the sharp thumbnail. Returns the Kitty id of the preview that it replaces, so
    /// that the terminal can delete it.
    pub(crate) fn insert(&mut self, index: usize, image: RgbaImage) -> Option<u32> {
        self.push(index, image, true)
    }

    /// Store a preview. A later `insert` replaces it.
    pub(crate) fn insert_preview(&mut self, index: usize, image: RgbaImage) {
        // A cell never has two previews, so there is no id to delete.
        self.push(index, image, false);
    }

    fn push(&mut self, index: usize, image: RgbaImage, sharp: bool) -> Option<u32> {
        let id = self.next_id;
        self.next_id += 1;
        self.bytes += image.as_raw().len();
        let thumb = CachedThumb {
            image,
            image_id: id,
            sharp,
        };
        let replaced = self.cache.push(index, thumb).and_then(|(old_index, old)| {
            self.bytes -= old.image.as_raw().len();
            if old_index == index {
                return Some(old.image_id);
            }
            self.evicted_ids.push(old.image_id);
            None
        });
        self.trim_to_byte_limit();
        replaced
    }

    /// Drop the oldest entries that no visible cell shows, until the bytes fit the limit.
    fn trim_to_byte_limit(&mut self) {
        while self.bytes > self.byte_limit {
            let Some(&index) = self
                .cache
                .iter()
                .rev()
                .map(|(index, _)| index)
                .find(|index| !self.visible.contains(index))
            else {
                return;
            };
            if let Some(old) = self.cache.pop(&index) {
                self.forget(&old);
            }
        }
    }

    /// Take the bytes and the id of an entry that left the cache. The terminal deletes the id.
    fn forget(&mut self, old: &CachedThumb) {
        self.bytes -= old.image.as_raw().len();
        self.evicted_ids.push(old.image_id);
    }

    /// The ids that left the cache since the last take. The caller deletes them in the terminal.
    pub(crate) fn take_evicted_ids(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.evicted_ids)
    }

    /// Drop every thumbnail. The caller deletes all images in the terminal, so the evicted ids
    /// need no single delete.
    pub(crate) fn clear(&mut self) {
        self.cache.clear();
        self.evicted_ids.clear();
        self.bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::{GAP, GalleryState, LABEL_ROWS, ThumbnailCache};
    use ratatui::layout::Rect;

    #[test]
    fn grid_navigation_tracks_rows_and_scroll() {
        let mut gallery = GalleryState::with_zoom_rows(12, 2);
        gallery.update_grid(Rect::new(0, 0, 100, 20));
        assert_eq!(gallery.grid_cols, 3);

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
        // 20 rows for the cells, after the 5 gap rows of 4 rows.
        gallery.update_grid(Rect::new(0, 0, 75, 25));
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
        assert_eq!(gallery.grid_cols, 3);
        assert_eq!((gallery.thumb_cols, gallery.thumb_rows), (32, 8));

        assert!(gallery.thumb_zoom_in());
        assert_eq!(gallery.zoom_rows, 3);
        assert!(gallery.thumb_rows > 8);

        assert!(gallery.thumb_zoom_out());
        assert_eq!((gallery.thumb_cols, gallery.thumb_rows), (32, 8));

        for _ in 0..40 {
            gallery.thumb_zoom_in();
        }
        assert_eq!(gallery.zoom_rows, 2, "a 2x2 grid is the largest size");
        assert!(!gallery.thumb_zoom_in());
        assert_eq!(gallery.visible_rows, 2);

        for _ in 0..40 {
            gallery.thumb_zoom_out();
        }
        assert_eq!(
            gallery.zoom_rows, 9,
            "40 rows: 1 edge gap, then 3 rows and 1 gap each"
        );
        assert!(!gallery.thumb_zoom_out());

        // A tall terminal stops at 12 rows.
        gallery.update_grid(Rect::new(0, 0, 100, 200));
        for _ in 0..40 {
            gallery.thumb_zoom_out();
        }
        assert_eq!(gallery.zoom_rows, 12);
        assert_eq!(gallery.visible_rows, 12);

        // A terminal with room for one row keeps that row.
        gallery.update_grid(Rect::new(0, 0, 100, 6));
        assert_eq!(gallery.visible_rows, 1);
        assert!(gallery.thumb_zoom_in(), "from 12 to the 1 row that fits");
        assert_eq!(gallery.zoom_rows, 1);
        assert!(!gallery.thumb_zoom_in());

        gallery.update_grid(Rect::new(0, 0, 100, 40));
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
        gallery.update_grid(Rect::new(0, 0, 100, 20));

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
        gallery.update_grid(Rect::new(0, 0, 100, 20));

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
        gallery.update_grid(Rect::new(0, 0, 150, 24));
        assert_eq!(gallery.grid_cols, 4);

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
        gallery.update_grid(Rect::new(0, 0, 150, 24));

        gallery.move_page_down();
        gallery.move_right();
        gallery.move_right();
        assert_eq!(gallery.selected_index(), Some(10));
        assert_eq!(gallery.scroll_offset, 1);
    }

    #[test]
    fn visible_items_and_cell_rect_match_geometry() {
        let mut gallery = GalleryState::with_zoom_rows(12, 2);
        gallery.update_grid(Rect::new(0, 0, 101, 22));
        gallery.scroll_offset = 1;

        let items: Vec<_> = gallery.visible_items().collect();
        assert_eq!(items.len(), gallery.visible_rows * gallery.grid_cols);

        let first = gallery.cell_rect(0);
        assert_eq!(first.x, gallery.grid_rect.x + GAP);
        assert_eq!(first.y, gallery.grid_rect.y + GAP);
        assert_eq!(
            first.width,
            gallery.thumb_cols + 1,
            "the first column is wide"
        );
        assert_eq!(
            first.height,
            gallery.thumb_rows + 1,
            "the first row is tall"
        );

        let last_col = gallery.grid_cols - 1;
        let last = gallery.cell_rect(last_col);
        assert_eq!(last.width, gallery.thumb_cols);
        assert_eq!(last.right() + GAP, gallery.grid_rect.right());
    }

    #[test]
    fn the_gaps_sit_between_the_cells_and_at_the_edges() {
        for (width, height, labels) in [(101, 22, false), (157, 41, true), (80, 30, false)] {
            let mut gallery = GalleryState::with_zoom_rows(100, 3);
            gallery.show_filenames = labels;
            let grid = Rect::new(2, 3, width, height);
            gallery.update_grid(grid);
            let cols = gallery.grid_cols;
            let rows = gallery.visible_rows;

            let row: Vec<Rect> = (0..cols).map(|col| gallery.cell_rect(col)).collect();
            assert_eq!(row[0].x, grid.x + 1, "the left edge gap");
            for pair in row.windows(2) {
                assert_eq!(pair[1].x, pair[0].right() + 1, "1 gap column between cells");
            }
            assert_eq!(
                row[cols - 1].right() + 1,
                grid.right(),
                "the right edge gap"
            );
            let cell_columns: u16 = row.iter().map(|cell| cell.width).sum();
            assert_eq!(
                usize::from(width - cell_columns),
                cols + 1,
                "n + 1 gap columns"
            );

            let column: Vec<Rect> = (0..rows).map(|r| gallery.border_rect(r * cols)).collect();
            assert_eq!(column[0].y, grid.y, "the top edge gap holds the border");
            for pair in column.windows(2) {
                assert_eq!(pair[1].y + 1, pair[0].bottom(), "two rows share 1 gap row");
            }
            assert_eq!(
                column[rows - 1].bottom(),
                grid.bottom(),
                "the bottom edge gap"
            );
            let cell_rows: u16 = column.iter().map(|border| border.height - 2).sum();
            assert_eq!(usize::from(height - cell_rows), rows + 1, "n + 1 gap rows");
        }
    }

    #[test]
    fn the_border_takes_the_gaps_around_the_cell_and_the_label() {
        let mut gallery = GalleryState::with_zoom_rows(12, 2);
        gallery.update_grid(Rect::new(0, 0, 101, 22));
        assert_eq!(gallery.cell_rect(0), Rect::new(1, 1, 33, 10));
        assert_eq!(gallery.border_rect(0), Rect::new(0, 0, 35, 12));
        assert_eq!(
            gallery.border_rect(1).x,
            gallery.border_rect(0).right() - 1,
            "two neighbors share the gap column"
        );

        gallery.show_filenames = true;
        gallery.update_grid(Rect::new(0, 0, 101, 22));
        let cell = gallery.cell_rect(0);
        let border = gallery.border_rect(0);
        assert_eq!(
            border.bottom(),
            cell.bottom() + LABEL_ROWS + 1,
            "the label is inside"
        );
    }

    #[test]
    fn the_remainders_go_to_the_first_columns_and_rows() {
        // 97 cell columns: 3 columns of 32 leave 1. 19 cell rows: 2 rows of 9 leave 1.
        let mut gallery = GalleryState::with_zoom_rows(12, 2);
        gallery.update_grid(Rect::new(5, 7, 101, 22));
        assert_eq!((gallery.grid_cols, gallery.thumb_cols), (3, 32));
        assert_eq!((gallery.extra_cols, gallery.extra_rows), (1, 1));

        let cells: Vec<Rect> = (0..6).map(|index| gallery.cell_rect(index)).collect();
        assert_eq!(cells[0], Rect::new(6, 8, 33, 10));
        assert_eq!(cells[1], Rect::new(40, 8, 32, 10));
        assert_eq!(cells[2], Rect::new(73, 8, 32, 10));
        assert_eq!(cells[3], Rect::new(6, 19, 33, 9));
        assert_eq!(cells[5], Rect::new(73, 19, 32, 9));
        assert_eq!(cells[2].right() + 1, gallery.grid_rect.right());
        assert_eq!(cells[5].bottom() + 1, gallery.grid_rect.bottom());

        // The thumbnails of a wide column are decoded for the base size.
        assert_eq!(gallery.thumb_box(), Rect::new(0, 0, 32, 9));

        // A clamped cell size cannot fill the width, so nothing spreads.
        gallery.update_grid(Rect::new(0, 0, 12, 21));
        assert_eq!((gallery.grid_cols, gallery.thumb_cols), (2, 8));
        assert_eq!(gallery.extra_cols, 0);
    }

    #[test]
    fn the_labels_sit_below_the_tall_rows() {
        let mut gallery = GalleryState::with_zoom_rows(12, 2);
        gallery.show_filenames = true;
        gallery.update_grid(Rect::new(0, 0, 100, 26));
        assert_eq!((gallery.thumb_rows, gallery.cell_height_total()), (10, 11));
        assert_eq!(gallery.extra_rows, 1);

        let first = gallery.cell_rect(0);
        let second_row = gallery.cell_rect(3);
        assert_eq!(first.height, 11);
        assert_eq!(
            second_row.y, 14,
            "1 gap, 11 rows of thumbnail, 1 row of label and 1 gap"
        );
        assert_eq!(second_row.height, 10);
        assert_eq!(
            second_row.bottom() + LABEL_ROWS + 1,
            gallery.grid_rect.bottom()
        );
    }

    #[test]
    fn the_cursor_moves_over_cells_of_different_widths() {
        let mut gallery = GalleryState::with_zoom_rows(12, 2);
        gallery.update_grid(Rect::new(0, 0, 101, 22));
        assert_eq!(gallery.extra_cols, 1);

        let widths: Vec<u16> = (0..3).map(|col| gallery.cell_rect(col).width).collect();
        assert_eq!(widths, [33, 32, 32]);
        gallery.move_right();
        gallery.move_right();
        gallery.move_down();
        assert_eq!(gallery.selected_index(), Some(5));
        assert_eq!(gallery.cell_rect(5), Rect::new(68, 12, 32, 9));
        gallery.move_left();
        gallery.move_left();
        assert_eq!(gallery.selected_index(), Some(3));
        assert_eq!(gallery.cell_rect(3).width, 33);
    }

    #[test]
    fn the_cell_shape_follows_the_photos_and_the_cell_pixels() {
        let mut gallery = GalleryState::with_zoom_rows(50, 2);
        // 124 columns after the left edge gap; each column takes its width plus 1 gap.
        let rect = Rect::new(0, 0, 125, 23);
        gallery.update_grid(rect);
        assert_eq!(
            gallery.grid_cols, 4,
            "3:2 on 1:2 cells: 30 columns per 10 rows"
        );

        // A portrait folder on 8x16 px cells: 3:4 photos are 15 columns wide.
        assert!(gallery.set_cell_shape(0.75, (8, 16)));
        assert_eq!(gallery.grid_cols, 7);
        assert!(
            !gallery.set_cell_shape(0.75, (8, 16)),
            "the same shape changes nothing"
        );

        // A landscape folder: 16:9 photos are 36 columns wide.
        assert!(gallery.set_cell_shape(16.0 / 9.0, (8, 16)));
        assert_eq!(gallery.grid_cols, 3);

        // Wider cells (10x16 px) make the same photo fewer columns wide: 28, so 4 columns.
        assert!(gallery.set_cell_shape(16.0 / 9.0, (10, 16)));
        assert_eq!(gallery.grid_cols, 4);

        // A bad aspect or an unknown cell size falls back to 3:2 on 1:2 cells.
        assert!(gallery.set_cell_shape(0.75, (8, 16)));
        assert!(gallery.set_cell_shape(f64::NAN, (0, 0)));
        assert_eq!(gallery.grid_cols, 4);
    }

    #[test]
    fn the_cache_names_the_ids_it_drops_and_grows_with_the_grid() {
        let mut cache = ThumbnailCache::new();
        assert_eq!(cache.capacity(), 200);
        for index in 0..201 {
            cache.insert(index, image::RgbaImage::new(1, 1));
        }
        assert!(!cache.contains(0), "the oldest cell went");
        assert_eq!(
            cache.take_evicted_ids(),
            vec![1],
            "its id goes to the terminal"
        );
        assert!(cache.take_evicted_ids().is_empty(), "one take");

        // A touched cell is the newest one, so the next insert drops another.
        cache.touch(1);
        cache.insert(300, image::RgbaImage::new(1, 1));
        assert!(cache.contains(1));
        assert!(!cache.contains(2));

        // A grid of 150 cells needs 300 entries. A small grid shrinks the cache again and
        // drops the oldest entries.
        cache.fit_capacity(150);
        assert_eq!(cache.capacity(), 300);
        cache.fit_capacity(10);
        assert_eq!(cache.capacity(), 200);
        cache.fit_capacity(10);
        assert_eq!(cache.take_evicted_ids().len(), 1, "200 entries stay of 201");
        assert_eq!(cache.cache.len(), 200);

        // A replaced preview is no eviction. A clear forgets the ids, the delete-all covers them.
        cache.insert_preview(400, image::RgbaImage::new(1, 1));
        cache.take_evicted_ids();
        assert!(cache.insert(400, image::RgbaImage::new(1, 1)).is_some());
        assert!(cache.take_evicted_ids().is_empty());
        cache.insert(401, image::RgbaImage::new(1, 1));
        cache.clear();
        assert!(cache.take_evicted_ids().is_empty());
    }

    #[test]
    fn the_byte_limit_drops_the_oldest_entries_that_no_visible_cell_shows() {
        let mut cache = ThumbnailCache::new();
        assert_eq!(cache.byte_limit, 512 * 1024 * 1024);
        // 2x2 RGBA is 16 bytes. Four entries fit.
        cache.byte_limit = 64;
        for index in 0..4 {
            cache.insert(index, image::RgbaImage::new(2, 2));
        }
        assert_eq!(cache.bytes(), 64);
        assert!(cache.take_evicted_ids().is_empty());

        cache.set_visible([0, 1]);
        cache.insert(4, image::RgbaImage::new(2, 2));
        assert!(
            !cache.contains(2),
            "the oldest cell that is not on the screen"
        );
        assert!(cache.contains(0) && cache.contains(1));
        assert_eq!(cache.take_evicted_ids(), vec![3]);
        assert_eq!(cache.bytes(), 64);

        // A replaced preview counts only once. A larger image drops two entries.
        cache.insert_preview(5, image::RgbaImage::new(1, 1));
        cache.take_evicted_ids();
        cache.insert(5, image::RgbaImage::new(2, 2));
        assert_eq!(cache.bytes(), 64);
        cache.insert(6, image::RgbaImage::new(4, 2));
        assert_eq!(cache.bytes(), 64);
        assert_eq!(cache.take_evicted_ids().len(), 2);

        // When only visible cells are left, the cache keeps them above the limit.
        cache.set_visible(0..10);
        cache.insert(7, image::RgbaImage::new(2, 2));
        assert!(cache.bytes() > 64);
        assert!(cache.take_evicted_ids().is_empty());
        cache.clear();
        assert_eq!(cache.bytes(), 0);
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

    #[test]
    fn a_sharp_thumbnail_replaces_the_preview_and_names_its_old_id() {
        let mut cache = ThumbnailCache::new();
        let img = image::RgbaImage::new(2, 2);
        cache.insert_preview(0, img.clone());
        assert!(cache.contains(0));
        assert!(!cache.is_sharp(0));
        let (_, preview_id) = cache.peek(0).unwrap();

        assert_eq!(cache.insert(0, img.clone()), Some(preview_id));
        assert!(cache.is_sharp(0));
        let (_, sharp_id) = cache.peek(0).unwrap();
        assert_ne!(
            sharp_id, preview_id,
            "the terminal needs a new id for the new pixels"
        );

        assert_eq!(
            cache.insert(1, img),
            None,
            "a first sharp image replaces nothing"
        );
        assert!(cache.is_sharp(1));
    }
}
