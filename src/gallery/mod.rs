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
use crate::icons;
use crate::pictures::shape::DEFAULT_IMAGE_ASPECT;
use ratatui::layout::Rect;
use std::ops::Range;

/// The gap in terminal cells between two cells and at the grid edges. The borders sit in it.
pub(crate) const GAP: u16 = 1;

/// The cell size before the first `update_grid`.
const DEFAULT_THUMB_COLS: u16 = 20;
const DEFAULT_THUMB_ROWS: u16 = 8;
const MIN_THUMB_COLS: u16 = 5;
const MAX_THUMB_COLS: u16 = 120;
const MIN_THUMB_ROWS: u16 = 3;
const MAX_THUMB_ROWS: u16 = 60;

const DEFAULT_CELL_RATIO: f64 = 2.0;
const MIN_GRID_COLS: usize = 2;

/// The grid: its geometry, the cursor and the filter.
///
/// A terminal cell is one character cell (`cell_px`). The thumbnail area is the box that shows
/// the picture (`thumb_rect`). A grid cell is the thumbnail area (`cell_at`).
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
    /// The cell size in pixels that `set_cell_shape` got. `(0, 0)` is unknown.
    cell_px: (u32, u32),
    image_aspect: f64,
    /// The zoom level of the reset key.
    pub(crate) default_zoom_rows: usize,
    /// The cursor place before the first of several layout changes. It holds while the cursor and
    /// the scroll stay, so a zoom out and back in returns to the same screen after a clamp at the
    /// list edge.
    kept_place: Option<KeptPlace>,
}

/// The layout of one row count in the grid rectangle (`GalleryState::row_layout`).
struct RowLayout {
    /// The rows of the cells after the gap rows.
    cell_area_height: u16,
    thumb_rows: u16,
    /// The width in terminal cells that the folder shape gives the box, before any clamp.
    shaped_width: u16,
    grid_cols: usize,
    /// The columns of the cells after the gap columns.
    cell_area_width: u16,
    thumb_cols: u16,
    /// The box is at least as high as the picture at the full box width. The picture is then
    /// limited by the width, and it keeps its size with fewer box rows.
    holds_picture_width: bool,
}

/// A cursor place (`GalleryState::cursor_place`) and the cursor and the scroll after its relayout.
#[derive(Clone, Copy)]
struct KeptPlace {
    place: (usize, usize),
    cursor: usize,
    scroll_offset: usize,
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
            zoom_rows,
            thumb_cols: DEFAULT_THUMB_COLS,
            thumb_rows: DEFAULT_THUMB_ROWS,
            extra_cols: 0,
            extra_rows: 0,
            cols_per_row: DEFAULT_IMAGE_ASPECT * DEFAULT_CELL_RATIO,
            cell_px: (0, 0),
            image_aspect: DEFAULT_IMAGE_ASPECT,
            default_zoom_rows: zoom_rows,
            kept_place: None,
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
        self.cell_px = cell_px;
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

    /// Back to the configured zoom level. Returns `true` if it changed.
    pub(crate) fn reset_zoom(&mut self) -> bool {
        self.set_zoom_rows(self.default_zoom_rows)
    }

    /// The next level with fewer rows, larger thumbnails. Returns `true` if the level changed.
    pub(crate) fn thumb_zoom_in(&mut self) -> bool {
        let levels = self.zoom_levels();
        let shown = shown_level(&levels, self.zoom_rows);
        let Some(&rows) = levels.iter().rev().find(|&&rows| rows < shown) else {
            return false;
        };
        self.set_zoom_rows(rows)
    }

    /// The next level with more rows, smaller thumbnails. Returns `true` if the level changed.
    pub(crate) fn thumb_zoom_out(&mut self) -> bool {
        let levels = self.zoom_levels();
        let shown = shown_level(&levels, self.zoom_rows);
        let Some(&rows) = levels.iter().find(|&&rows| rows > shown) else {
            return false;
        };
        self.set_zoom_rows(rows)
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

    /// The row counts that a zoom step shows, fewest rows first. Each level keeps the cell shape
    /// (`MIN_THUMB_COLS`), and each step changes the column count, so every step is visible. Of
    /// the row counts with one column count, the level is the count with the largest picture
    /// and then the most rows. Each level holds the large grid icon (`icons::icon_size`). Before
    /// the first `update_grid`, every row count is a level.
    fn zoom_levels(&self) -> Vec<usize> {
        let (fewest, most) = (self.min_visible_rows(), self.max_visible_rows());
        if self.grid_rect.is_empty() {
            return (fewest..=most).collect();
        }
        let mut levels = Vec::new();
        let mut last_cols = None;
        for rows in fewest..=most {
            let layout = self.row_layout(rows);
            // The shaped width does not grow with the rows, so no later count keeps the shape.
            // The icon height falls with the rows, so no later count holds the icon.
            if rows > fewest && (layout.shaped_width < MIN_THUMB_COLS || !self.holds_icon(&layout))
            {
                break;
            }
            // The column count does not fall with the rows, so equal counts are neighbors. The
            // box height falls with the rows: the fewest rows give the largest picture. A picture
            // that the width limits keeps its size while the box holds the full width, so the
            // most of those rows win.
            if last_cols != Some(layout.grid_cols) {
                last_cols = Some(layout.grid_cols);
                levels.push(rows);
            } else if layout.holds_picture_width
                && let Some(level) = levels.last_mut()
            {
                *level = rows;
            }
        }
        levels
    }

    /// The box of `layout` holds the large grid icon. An unknown cell size uses the default
    /// cell ratio.
    fn holds_icon(&self, layout: &RowLayout) -> bool {
        let cell_px = if self.cell_px.0 == 0 || self.cell_px.1 == 0 {
            (1, DEFAULT_CELL_RATIO as u32)
        } else {
            self.cell_px
        };
        icons::icon_size((layout.thumb_cols, layout.thumb_rows), cell_px).is_some()
    }

    /// The box size, the shaped width and the column count of `rows` rows in `grid_rect`.
    fn row_layout(&self, rows: usize) -> RowLayout {
        let grid_rect = self.grid_rect;
        let rows = rows as u16;
        let cell_area_height = grid_rect.height.saturating_sub(gaps(rows));
        let cell_height = (cell_area_height / rows).max(MIN_THUMB_ROWS);
        let thumb_rows = cell_height.clamp(MIN_THUMB_ROWS, MAX_THUMB_ROWS);
        let shaped_width = (f64::from(thumb_rows) * self.cols_per_row).round() as u16;
        // The count uses the clamped width, else the clamp of `thumb_cols` pushes the last
        // columns past the right edge of a far zoomed-out portrait grid.
        let grid_cols = (usize::from(grid_rect.width.saturating_sub(GAP))
            / usize::from(shaped_width.max(MIN_THUMB_COLS) + GAP))
        .max(MIN_GRID_COLS);
        let cols = grid_cols as u16;
        let cell_area_width = grid_rect.width.saturating_sub(gaps(cols));
        let thumb_cols = (cell_area_width / cols).clamp(MIN_THUMB_COLS, MAX_THUMB_COLS);
        RowLayout {
            cell_area_height,
            thumb_rows,
            shaped_width,
            grid_cols,
            cell_area_width,
            thumb_cols,
            holds_picture_width: f64::from(thumb_rows) * self.cols_per_row >= f64::from(thumb_cols),
        }
    }

    /// The rows that fit the terminal at the smallest thumbnail height.
    fn max_visible_rows(&self) -> usize {
        let most = usize::from(MAX_VISIBLE_ROWS);
        if self.grid_rect.is_empty() {
            return most;
        }
        let min_cell = usize::from(MIN_THUMB_ROWS + GAP);
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
        self.kept_place = None;
    }

    /// Fit the largest zoom level of at most `zoom_rows` rows, else the first level, into
    /// `grid_rect`. The cell shape picks the column count, then the columns fill the width.
    pub(crate) fn update_grid(&mut self, grid_rect: Rect) {
        let before = (self.grid_cols, self.visible_rows);
        let place = self
            .kept_place
            .filter(|kept| kept.cursor == self.cursor && kept.scroll_offset == self.scroll_offset)
            .map_or_else(|| self.cursor_place(), |kept| kept.place);
        self.grid_rect = grid_rect;
        self.visible_rows = shown_level(&self.zoom_levels(), self.zoom_rows);
        let layout = self.row_layout(self.visible_rows);
        self.thumb_rows = layout.thumb_rows;
        self.extra_rows = spread(
            layout.cell_area_height,
            self.thumb_rows,
            self.visible_rows as u16,
        );
        self.grid_cols = layout.grid_cols;
        self.thumb_cols = layout.thumb_cols;
        self.extra_cols = spread(
            layout.cell_area_width,
            self.thumb_cols,
            self.grid_cols as u16,
        );
        if before != (self.grid_cols, self.visible_rows) {
            self.keep_cursor_place(place);
            self.ensure_cursor_visible();
            self.kept_place = Some(KeptPlace {
                place,
                cursor: self.cursor,
                scroll_offset: self.scroll_offset,
            });
        }
        self.ensure_cursor_visible();
    }

    /// The screen row of the cursor and the screen rows.
    fn cursor_place(&self) -> (usize, usize) {
        let rows = self.visible_rows.max(1);
        let row = self
            .cursor_row()
            .saturating_sub(self.scroll_offset)
            .min(rows - 1);
        (row, rows)
    }

    /// Scroll so that the cursor keeps its relative place on a screen with new rows or columns:
    /// a zoom or the info panel then moves nothing under the eye. The scroll stays inside the
    /// list, so at its start or its end the grid stays flush with the edge and fills the screen.
    /// The top and the bottom row stay at their edge. A row between maps its middle to the new
    /// screen, so a zoom out and back in returns to the same row.
    fn keep_cursor_place(&mut self, (row, rows): (usize, usize)) {
        let last = self.visible_rows.saturating_sub(1);
        let on_screen = match row {
            _ if rows == 1 => last / 2,
            0 => 0,
            _ if row == rows - 1 => last,
            _ => (2 * row + 1) * self.visible_rows / (2 * rows),
        };
        self.scroll_offset = self
            .cursor_row()
            .saturating_sub(on_screen)
            .min(self.max_scroll_offset());
    }

    /// The scroll offset that shows the last row at the bottom of the screen.
    fn max_scroll_offset(&self) -> usize {
        let total_rows = self.filtered_indices.len().div_ceil(self.grid_cols.max(1));
        total_rows.saturating_sub(self.visible_rows)
    }

    fn col_x(&self, col: u16) -> u16 {
        self.grid_rect.x + GAP + col * (self.thumb_cols + GAP) + col.min(self.extra_cols)
    }

    fn row_y(&self, row: u16) -> u16 {
        self.grid_rect.y + GAP + row * (self.thumb_rows + GAP) + row.min(self.extra_rows)
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
        let offset = self
            .scroll_offset
            .saturating_add_signed(delta)
            .min(self.max_scroll_offset());
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
            self.thumb_rect(vis_idx)
                .contains(ratatui::layout::Position::new(column, row))
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

    /// The thumbnail area of the visible cell `vis_idx`.
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

    /// The border of the visible cell `vis_idx`, in the gaps.
    pub(crate) fn border_rect(&self, vis_idx: usize) -> Rect {
        let thumb = self.thumb_rect(vis_idx);
        Rect {
            x: thumb.x - GAP,
            y: thumb.y - GAP,
            width: thumb.width + 2 * GAP,
            height: thumb.height + 2 * GAP,
        }
    }
}

/// The largest level of at most `zoom_rows` rows, else the first level.
fn shown_level(levels: &[usize], zoom_rows: usize) -> usize {
    levels
        .iter()
        .rev()
        .find(|&&rows| rows <= zoom_rows)
        .or(levels.first())
        .copied()
        .unwrap_or(1)
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
