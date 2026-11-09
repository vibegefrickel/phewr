//! The grid geometry and the cursor. The in-memory thumbnail cache lives in
//! `crate::pictures::jobs::thumbnail_cache`.
//!
//! A line of n cells takes n + 1 gaps of `GAP` cells, one between two neighbors and one at each
//! edge. `update_grid` gives the remainder to the first columns and rows, and the grid reaches
//! the edges. Every thumbnail is decoded for the base cell size. The decode pads each picture to
//! whole cells (`pad_for_grid`). The grid centers it in its cell with `c=` and `r=`, never with a
//! pixel offset (`X=`, `Y=`): herdr relays an offset as a stretched picture. The borders sit in
//! the gaps (`GalleryState::border_rect`): plain for a marked cell, thick for the selected cell,
//! drawn last. A border that the grid edge cuts stays open at that edge.
use crate::config::{MAX_VISIBLE_ROWS, MIN_VISIBLE_ROWS};
use crate::pictures::shape::DEFAULT_IMAGE_ASPECT;
use ratatui::layout::Rect;
use std::ops::Range;

pub(crate) const LABEL_ROWS: u16 = 1;
/// The gap in terminal cells between two cells and at the grid edges. The borders sit in it.
pub(crate) const GAP: u16 = 1;

/// The cell size before the first `update_grid`.
const DEFAULT_THUMB_COLS: u16 = 20;
const DEFAULT_THUMB_ROWS: u16 = 8;
const MIN_THUMB_COLS: u16 = 8;
const MAX_THUMB_COLS: u16 = 120;
const MIN_THUMB_ROWS: u16 = 3;
const MAX_THUMB_ROWS: u16 = 60;

const DEFAULT_CELL_RATIO: f64 = 2.0;
const MIN_GRID_COLS: usize = 2;

/// The grid: its geometry, the cursor and the filter.
///
/// A terminal cell is one character cell (`cell_px`). The thumbnail area is the box that shows
/// the picture (`thumb_rect`). A grid cell is the thumbnail area plus its label (`cell_at`).
#[derive(Clone)]
pub(crate) struct GalleryState {
    pub filtered_indices: Vec<usize>,
    pub cursor: usize,
    pub scroll_offset: usize,
    /// The column count. `thumb_cols` is the width of one column.
    pub grid_cols: usize,
    /// The rows that fit after the clamp to the terminal height.
    pub visible_rows: usize,
    pub grid_rect: Rect,
    pub search_active: bool,
    pub search_query: String,
    pub show_filenames: bool,
    /// The zoom level: the thumbnail rows that the grid shows.
    pub zoom_rows: usize,
    /// The base thumbnail area in terminal cells. Every thumbnail is decoded for this size.
    pub thumb_cols: u16,
    pub thumb_rows: u16,
    /// The first `extra_cols` columns and `extra_rows` rows are one cell larger. The grid then
    /// reaches the right and the bottom edge.
    pub extra_cols: u16,
    pub extra_rows: u16,
    /// The cell width over its height, in terminal cells. It picks the column count.
    cols_per_row: f64,
    image_aspect: f64,
    /// The zoom level of the reset key.
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

    /// Shape the cells like a photo of `image_aspect` on cells of `cell_px`. Returns `true` when
    /// the grid changed and the thumbnails need a new decode.
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

    /// The grid that a switch to another folder starts from, with `entries` cells at `zoom_rows`.
    pub(crate) fn for_folder(&self, entries: usize, zoom_rows: usize) -> Self {
        let mut grid = self.clone();
        grid.search_active = false;
        grid.search_query.clear();
        grid.filtered_indices = (0..entries).collect();
        grid.reset_cursor();
        grid.set_zoom_rows(zoom_rows);
        grid
    }

    pub(crate) fn image_aspect(&self) -> f64 {
        self.image_aspect
    }

    /// The box that every thumbnail is decoded for.
    pub(crate) fn thumb_box(&self) -> Rect {
        Rect::new(0, 0, self.thumb_cols, self.thumb_rows)
    }

    fn cell_height_total(&self) -> u16 {
        self.thumb_rows + self.label_rows()
    }

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

    /// One row fewer, larger thumbnails. Returns `true` if the level changed.
    pub(crate) fn thumb_zoom_in(&mut self) -> bool {
        self.set_zoom_rows(self.zoom_rows.saturating_sub(1))
    }

    /// One row more, smaller thumbnails. Returns `true` if the level changed.
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

    /// The rows that fit the terminal at the smallest thumbnail height.
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

    fn min_visible_rows(&self) -> usize {
        usize::from(MIN_VISIBLE_ROWS).min(self.max_visible_rows())
    }

    pub(crate) fn reset_cursor(&mut self) {
        self.cursor = 0;
        self.scroll_offset = 0;
    }

    /// Fit `zoom_rows` rows into `grid_rect`. The cell shape picks the column count, then the
    /// columns fill the width.
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
        let shaped_width = (f64::from(self.thumb_rows) * self.cols_per_row).round() as u16;
        self.grid_cols = (usize::from(grid_rect.width.saturating_sub(GAP))
            / usize::from(shaped_width.max(1) + GAP))
        .max(MIN_GRID_COLS);
        let cols = self.grid_cols as u16;
        let cell_area_width = grid_rect.width.saturating_sub(gaps(cols));
        self.thumb_cols = (cell_area_width / cols).clamp(MIN_THUMB_COLS, MAX_THUMB_COLS);
        self.extra_cols = spread(cell_area_width, self.thumb_cols, cols);
        self.ensure_cursor_visible();
    }

    fn col_x(&self, col: u16) -> u16 {
        self.grid_rect.x + GAP + col * (self.thumb_cols + GAP) + col.min(self.extra_cols)
    }

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

    pub(crate) fn move_cursor_to(&mut self, new: usize) {
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

    /// Scroll the grid by `delta` rows. The cursor keeps its column and stays on the screen.
    pub(crate) fn scroll_rows(&mut self, delta: isize) {
        let total_rows = self.filtered_indices.len().div_ceil(self.grid_cols.max(1));
        let max_offset = total_rows.saturating_sub(self.visible_rows);
        let offset = self
            .scroll_offset
            .saturating_add_signed(delta)
            .min(max_offset);
        if offset == self.scroll_offset {
            return;
        }
        self.scroll_offset = offset;
        let row = self
            .cursor_row()
            .clamp(offset, offset + self.visible_rows.saturating_sub(1));
        let max = self.filtered_indices.len().saturating_sub(1);
        self.cursor = (row * self.grid_cols + self.cursor % self.grid_cols.max(1)).min(max);
    }

    /// The position in `filtered_indices` of the cell under `(column, row)`. A gap gives `None`.
    pub(crate) fn cell_at(&self, column: u16, row: u16) -> Option<usize> {
        let range = self.visible_range();
        (0..range.len()).find_map(|vis_idx| {
            let thumb = self.thumb_rect(vis_idx);
            let hit = Rect {
                height: thumb.height + self.label_rows(),
                ..thumb
            };
            hit.contains(ratatui::layout::Position::new(column, row))
                .then_some(range.start + vis_idx)
        })
    }

    pub(crate) fn move_to_last(&mut self) {
        let last = self.filtered_indices.len().saturating_sub(1);
        self.move_cursor_to(last);
    }

    /// The cells of one screen, filled or empty.
    pub(crate) fn visible_cells(&self) -> usize {
        self.visible_rows * self.grid_cols
    }

    /// The positions in `filtered_indices` that the screen shows. A range borrows nothing.
    pub(crate) fn visible_range(&self) -> Range<usize> {
        let start = self.scroll_offset * self.grid_cols;
        let count = self.visible_rows * self.grid_cols;
        start..self.filtered_indices.len().min(start + count)
    }

    /// The visible cells as (cell position, image index).
    pub(crate) fn visible_items(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.visible_range()
            .enumerate()
            .map(|(vis_idx, position)| (vis_idx, self.filtered_indices[position]))
    }

    /// The thumbnail area of the visible cell `vis_idx`, without the label row.
    pub(crate) fn thumb_rect(&self, vis_idx: usize) -> Rect {
        let col = (vis_idx % self.grid_cols) as u16;
        let row = (vis_idx / self.grid_cols) as u16;
        Rect {
            x: self.col_x(col),
            y: self.row_y(row),
            width: self.thumb_cols + u16::from(col < self.extra_cols),
            height: self.thumb_rows + u16::from(row < self.extra_rows),
        }
    }

    /// The border of the visible cell `vis_idx`, in the gaps around the thumbnail and the label.
    pub(crate) fn border_rect(&self, vis_idx: usize) -> Rect {
        let thumb = self.thumb_rect(vis_idx);
        Rect {
            x: thumb.x - GAP,
            y: thumb.y - GAP,
            width: thumb.width + 2 * GAP,
            height: thumb.height + self.label_rows() + 2 * GAP,
        }
    }
}

fn gaps(count: u16) -> u16 {
    (count + 1) * GAP
}

/// How many of the `count` cells get one cell more. Zero when clamped cells cannot fill
/// `total`.
fn spread(total: u16, base: u16, count: u16) -> u16 {
    let rest = total.saturating_sub(base.saturating_mul(count));
    if rest < count { rest } else { 0 }
}

#[cfg(test)]
mod tests;
