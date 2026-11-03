//! The tests of the grid geometry, the cursor and the cell under the mouse.
use super::{GAP, GalleryState, LABEL_ROWS};
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
fn empty_grid_selects_nothing() {
    let mut gallery = GalleryState::new(0);
    gallery.move_right();
    gallery.move_down();
    gallery.move_to_last();
    assert_eq!(gallery.selected_index(), None);
}

#[test]
fn filename_toggle_gives_row_to_labels() {
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
fn two_rows_and_one_fill_height() {
    let mut gallery = GalleryState::with_zoom_rows(50, 2);
    gallery.update_grid(Rect::new(0, 0, 100, 40));
    assert_eq!(gallery.visible_rows, 2);
    let cols = gallery.grid_cols;
    assert_eq!(
        gallery.thumb_rect(cols).bottom() + GAP,
        40,
        "the second row ends at the bottom edge gap"
    );
    assert_eq!(gallery.visible_range(), 0..2 * cols);

    // A terminal with room for one row shows one row and scrolls one row at a time.
    gallery.update_grid(Rect::new(0, 0, 100, 6));
    assert_eq!(gallery.visible_rows, 1);
    let cols = gallery.grid_cols;
    assert_eq!(
        (gallery.thumb_rect(0).y, gallery.thumb_rect(0).height),
        (1, 4)
    );
    assert_eq!(gallery.visible_range(), 0..cols);
    gallery.move_down();
    assert_eq!(gallery.scroll_offset, 1);
    assert_eq!(gallery.visible_range(), cols..2 * cols);
}

#[test]
fn zero_rect_keeps_smallest_cells() {
    let mut gallery = GalleryState::with_zoom_rows(50, 4);
    gallery.update_grid(Rect::default());
    assert_eq!(gallery.grid_cols, 2);
    assert_eq!(
        gallery.thumb_box(),
        Rect::new(0, 0, 8, 3),
        "the smallest thumbnail area"
    );
    assert_eq!(gallery.visible_rows, 4);
    gallery.move_down();
    assert_eq!(gallery.cursor, 2);
    assert_eq!(gallery.cell_at(0, 0), None, "the edge gap");
}

#[test]
fn zoom_changes_and_clamps_rows() {
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
fn directional_moves_stay_in_bounds() {
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
fn visible_items_match_thumb_rects() {
    let mut gallery = GalleryState::with_zoom_rows(12, 2);
    gallery.update_grid(Rect::new(0, 0, 101, 22));
    gallery.scroll_offset = 1;

    let items: Vec<_> = gallery.visible_items().collect();
    assert_eq!(items.len(), gallery.visible_rows * gallery.grid_cols);

    let first = gallery.thumb_rect(0);
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
    let last = gallery.thumb_rect(last_col);
    assert_eq!(last.width, gallery.thumb_cols);
    assert_eq!(last.right() + GAP, gallery.grid_rect.right());
}

#[test]
fn gaps_sit_between_cells_and_edges() {
    for (width, height, labels) in [(101, 22, false), (157, 41, true), (80, 30, false)] {
        let mut gallery = GalleryState::with_zoom_rows(100, 3);
        gallery.show_filenames = labels;
        let grid = Rect::new(2, 3, width, height);
        gallery.update_grid(grid);
        let cols = gallery.grid_cols;
        let rows = gallery.visible_rows;

        let row: Vec<Rect> = (0..cols).map(|col| gallery.thumb_rect(col)).collect();
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
fn border_covers_gaps_around_cell() {
    let mut gallery = GalleryState::with_zoom_rows(12, 2);
    gallery.update_grid(Rect::new(0, 0, 101, 22));
    assert_eq!(gallery.thumb_rect(0), Rect::new(1, 1, 33, 10));
    assert_eq!(gallery.border_rect(0), Rect::new(0, 0, 35, 12));
    assert_eq!(
        gallery.border_rect(1).x,
        gallery.border_rect(0).right() - 1,
        "two neighbors share the gap column"
    );

    gallery.show_filenames = true;
    gallery.update_grid(Rect::new(0, 0, 101, 22));
    let cell = gallery.thumb_rect(0);
    let border = gallery.border_rect(0);
    assert_eq!(
        border.bottom(),
        cell.bottom() + LABEL_ROWS + 1,
        "the label is inside"
    );
}

#[test]
fn remainders_go_to_first_cells() {
    // 97 cell columns: 3 columns of 32 leave 1. 19 cell rows: 2 rows of 9 leave 1.
    let mut gallery = GalleryState::with_zoom_rows(12, 2);
    gallery.update_grid(Rect::new(5, 7, 101, 22));
    assert_eq!((gallery.grid_cols, gallery.thumb_cols), (3, 32));
    assert_eq!((gallery.extra_cols, gallery.extra_rows), (1, 1));

    let cells: Vec<Rect> = (0..6).map(|index| gallery.thumb_rect(index)).collect();
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
fn labels_sit_below_tall_rows() {
    let mut gallery = GalleryState::with_zoom_rows(12, 2);
    gallery.show_filenames = true;
    gallery.update_grid(Rect::new(0, 0, 100, 26));
    assert_eq!((gallery.thumb_rows, gallery.cell_height_total()), (10, 11));
    assert_eq!(gallery.extra_rows, 1);

    let first = gallery.thumb_rect(0);
    let second_row = gallery.thumb_rect(3);
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
fn cursor_crosses_uneven_cells() {
    let mut gallery = GalleryState::with_zoom_rows(12, 2);
    gallery.update_grid(Rect::new(0, 0, 101, 22));
    assert_eq!(gallery.extra_cols, 1);

    let widths: Vec<u16> = (0..3).map(|col| gallery.thumb_rect(col).width).collect();
    assert_eq!(widths, [33, 32, 32]);
    gallery.move_right();
    gallery.move_right();
    gallery.move_down();
    assert_eq!(gallery.selected_index(), Some(5));
    assert_eq!(gallery.thumb_rect(5), Rect::new(68, 12, 32, 9));
    gallery.move_left();
    gallery.move_left();
    assert_eq!(gallery.selected_index(), Some(3));
    assert_eq!(gallery.thumb_rect(3).width, 33);
}

#[test]
fn cell_shape_follows_photos_and_pixels() {
    let mut gallery = GalleryState::with_zoom_rows(50, 2);
    // 124 columns after the left edge gap. Each column takes its width plus 1 gap.
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
fn cell_at_hits_thumbnail_and_label() {
    let mut gallery = GalleryState::with_zoom_rows(12, 2);
    gallery.show_filenames = true;
    gallery.update_grid(Rect::new(0, 0, 100, 20));
    let cols = gallery.grid_cols;
    assert!((3..=4).contains(&cols), "{cols} columns, 3 rows");
    let cell = gallery.thumb_rect(cols + 1);

    assert_eq!(gallery.cell_at(cell.x, cell.y), Some(cols + 1));
    assert_eq!(
        gallery.cell_at(cell.x + cell.width - 1, cell.y + cell.height),
        Some(cols + 1),
        "the label row"
    );
    assert_eq!(gallery.cell_at(cell.x - 1, cell.y), None, "the gap");
    assert_eq!(gallery.cell_at(0, 0), None);

    gallery.scroll_offset = 1;
    assert_eq!(gallery.cell_at(cell.x, cell.y), Some(2 * cols + 1));
    gallery.scroll_offset = 2;
    assert_eq!(gallery.cell_at(cell.x, cell.y), None, "no 4th row");
    let first = gallery.thumb_rect(0);
    assert_eq!(gallery.cell_at(first.x, first.y), Some(2 * cols));
}

#[test]
fn cell_at_skips_empty_cells() {
    let mut gallery = GalleryState::with_zoom_rows(4, 2);
    gallery.update_grid(Rect::new(0, 0, 100, 20));
    let empty = gallery.thumb_rect(5);
    assert_eq!(gallery.cell_at(empty.x, empty.y), None);
    let last = gallery.thumb_rect(3);
    assert_eq!(gallery.cell_at(last.x, last.y), Some(3));
}

#[test]
fn scroll_rows_keeps_cursor_on_screen() {
    let mut gallery = GalleryState::with_zoom_rows(12, 2);
    gallery.update_grid(Rect::new(0, 0, 100, 20));
    gallery.cursor = 1;

    gallery.scroll_rows(1);
    assert_eq!(gallery.scroll_offset, 1);
    assert_eq!(gallery.cursor, 4, "one row down, same column");
    gallery.scroll_rows(5);
    assert_eq!(gallery.scroll_offset, 2, "4 rows, 2 visible");
    assert_eq!(gallery.cursor, 7);
    gallery.cursor = 11;
    gallery.scroll_rows(-2);
    assert_eq!(gallery.scroll_offset, 0);
    assert_eq!(gallery.cursor, 5, "the last visible row, column 2");
    gallery.scroll_rows(-1);
    assert_eq!(gallery.scroll_offset, 0);
}

#[test]
fn scroll_rows_ignores_short_list() {
    let mut gallery = GalleryState::with_zoom_rows(3, 2);
    gallery.update_grid(Rect::new(0, 0, 100, 20));
    gallery.scroll_rows(1);
    assert_eq!(gallery.scroll_offset, 0);
    assert_eq!(gallery.cursor, 0);
    let mut empty = GalleryState::new(0);
    empty.update_grid(Rect::new(0, 0, 100, 20));
    empty.scroll_rows(1);
    assert_eq!(empty.cell_at(5, 5), None);
}
