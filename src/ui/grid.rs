//! The cells of the grid: the borders in the gaps, the labels and the icons of the cells that
//! show no thumbnail. Kitty draws the thumbnails above them.

use crate::app::App;
use crate::gallery::LABEL_ROWS;
use crate::icons::{self, Icon};
use crate::styles::STYLES;
use ratatui::{
    prelude::*,
    widgets::{Block, BorderType, Borders, Paragraph},
};
use std::borrow::Cow;
use unicode_width::UnicodeWidthStr;

/// The style of a cell label and its mark, read once per cell per frame.
#[derive(Clone, Copy)]
struct CellLook {
    style: Style,
    marked: bool,
}

/// One cell per visible thumbnail, with an optional filename label. Kitty draws the pictures.
/// The marked cells get a plain border and the selected cell a thick border, in the gaps around
/// the cell. The selected border comes last, so it wins on a gap that two cells share.
/// `draw_gallery` runs `update_grid` before, so the grid fits `grid_rect` one time per frame.
pub(super) fn draw_thumbnail_cells(frame: &mut Frame<'_>, app: &App, grid_rect: Rect) {
    let first_visible = app.gallery.scroll_offset * app.gallery.grid_cols;
    let mut selected_border = None;

    for (vis_idx, img_idx) in app.gallery.visible_items() {
        let is_selected = vis_idx + first_visible == app.gallery.cursor;
        let is_marked = app.is_marked(img_idx);
        let border = app.gallery.border_rect(vis_idx);
        if is_selected {
            selected_border = Some(border);
        } else if is_marked {
            draw_cell_border(frame, BorderType::Plain, STYLES.marked, border, grid_rect);
        }
        let look = CellLook {
            style: if is_selected {
                STYLES.cursor_cell
            } else if is_marked {
                STYLES.marked
            } else {
                STYLES.text
            },
            marked: is_marked,
        };
        if app.is_folder(img_idx) {
            draw_folder_cell(frame, app, vis_idx, img_idx, look, grid_rect);
        } else if !app.thumb_cache.contains(img_idx) {
            let icon = if app.is_video(img_idx) {
                Icon::Video
            } else {
                Icon::Loading
            };
            draw_cell_icon(frame, app, app.gallery.thumb_rect(vis_idx), icon, grid_rect);
        }
        if app.gallery.show_filenames && !is_parent(app, img_idx) {
            draw_label(frame, app, vis_idx, img_idx, look, grid_rect);
        }
    }

    if let Some(border) = selected_border {
        draw_cell_border(
            frame,
            BorderType::Thick,
            STYLES.cursor_cell,
            border,
            grid_rect,
        );
    }
}

/// A large dim icon in the center of the thumbnail area `thumb`, or the one-cell symbol when the
/// area is small or the grid edge cuts the icon. Kitty draws a thumbnail above the text, so the
/// icon of a cell that loads needs no redraw when the thumbnail arrives. The help popup hides the
/// pictures, so it hides the icons too. Returns the row below the icon.
fn draw_cell_icon(
    frame: &mut Frame<'_>,
    app: &App,
    thumb: Rect,
    icon: Icon,
    grid_rect: Rect,
) -> Option<u16> {
    let shape =
        icons::icon_rect(thumb, app.cell_px).filter(|area| grid_rect.intersection(*area) == *area);
    if let Some(area) = shape {
        if !app.help_visible {
            icons::draw_icon(frame.buffer_mut(), area, icon, STYLES.dim);
        }
        return Some(area.bottom());
    }
    let center = Position::new(thumb.x + thumb.width / 2, thumb.y + thumb.height / 2);
    if !grid_rect.contains(center) {
        return None;
    }
    if !app.help_visible {
        frame
            .buffer_mut()
            .set_string(center.x, center.y, icon.symbol(), STYLES.dim);
    }
    Some(center.y + 1)
}

/// True for the `..` entry, which shows two dots and no name.
fn is_parent(app: &App, img_idx: usize) -> bool {
    app.is_parent_entry(img_idx)
}

/// A folder cell: a folder icon and the name below it, or two dots for `..`. The name goes inside
/// the cell when the filename labels are off, because the label row then does not exist.
fn draw_folder_cell(
    frame: &mut Frame<'_>,
    app: &App,
    vis_idx: usize,
    img_idx: usize,
    look: CellLook,
    grid_rect: Rect,
) {
    let thumb = app.gallery.thumb_rect(vis_idx);
    let icon = if app.filenames[img_idx] == ".." {
        Icon::Parent
    } else {
        Icon::Folder
    };
    let Some(below) = draw_cell_icon(frame, app, thumb, icon, grid_rect) else {
        return;
    };
    if app.gallery.show_filenames || icon == Icon::Parent {
        return;
    }
    let name_rect = Rect {
        y: below,
        height: LABEL_ROWS,
        ..thumb
    }
    .intersection(grid_rect);
    draw_centered_text(
        frame,
        label_text(app, img_idx, look.marked, name_rect.width),
        look.style,
        name_rect,
    );
}

/// A line border without a fill, so the label and the cells inside keep their own styles. A
/// side outside `grid_rect` is left out, so a cut cell does not look closed at the clip edge.
fn draw_cell_border(
    frame: &mut Frame<'_>,
    border_type: BorderType,
    style: Style,
    border: Rect,
    grid_rect: Rect,
) {
    let visible = border.intersection(grid_rect);
    let mut sides = Borders::ALL;
    if visible.x > border.x {
        sides.remove(Borders::LEFT);
    }
    if visible.y > border.y {
        sides.remove(Borders::TOP);
    }
    if visible.right() < border.right() {
        sides.remove(Borders::RIGHT);
    }
    if visible.bottom() < border.bottom() {
        sides.remove(Borders::BOTTOM);
    }
    frame.render_widget(
        Block::default()
            .borders(sides)
            .border_type(border_type)
            .border_style(style),
        visible,
    );
}

/// The filename below the thumbnail of the visible cell `vis_idx`, cut to the cell width.
fn draw_label(
    frame: &mut Frame<'_>,
    app: &App,
    vis_idx: usize,
    img_idx: usize,
    look: CellLook,
    grid_rect: Rect,
) {
    let thumb = app.gallery.thumb_rect(vis_idx).intersection(grid_rect);
    let label_y = thumb.y + thumb.height;
    if label_y >= grid_rect.bottom() {
        return;
    }
    let label_rect = Rect {
        y: label_y,
        height: LABEL_ROWS,
        ..thumb
    };
    draw_centered_text(
        frame,
        label_text(app, img_idx, look.marked, thumb.width),
        look.style,
        label_rect,
    );
}

/// One line of `text` in the middle of `area`, with `style` on the whole area, as the label row
/// and the folder name take it.
fn draw_centered_text(frame: &mut Frame<'_>, text: Cow<'_, str>, style: Style, area: Rect) {
    frame.render_widget(
        Paragraph::new(text)
            .style(style)
            .alignment(Alignment::Center),
        area,
    );
}

/// The label of the entry `img_idx`, cut to `width` with a margin, with a dot for a mark.
fn label_text(app: &App, img_idx: usize, is_marked: bool, width: u16) -> Cow<'_, str> {
    let raw_filename = app.filenames[img_idx].as_str();
    let prefix_len = 2 * usize::from(is_marked);
    let max_label = usize::from(width).saturating_sub(2 + prefix_len);
    let trimmed = if raw_filename.width() > max_label {
        // The ellipsis takes one column. The head is measured as a whole string, like the full
        // name, so a wide character or an emoji sequence counts the same in both.
        let room = max_label.saturating_sub(1);
        let mut head = String::new();
        for ch in raw_filename.chars() {
            head.push(ch);
            if head.width() > room {
                head.pop();
                break;
            }
        }
        Cow::Owned(format!("{head}\u{2026}"))
    } else {
        Cow::Borrowed(raw_filename)
    };
    if is_marked {
        Cow::Owned(format!("\u{25cf} {trimmed}"))
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::icons::{Icon, draw_icon, icon_rect};
    use crate::styles::STYLES;
    use crate::test_support::folder_app;
    use crate::ui::tests::{
        app_with, assert_ansi, assert_default_fg, assert_hides, assert_shows, assert_symbol,
        assert_terminal_background, cell_of, render, render_buffer, screen_rows, status_row,
    };
    use image::RgbaImage;
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier};
    use ratatui::text::Line;

    /// Fail when `border` is not a line border of `symbols` (top left, top right, bottom left,
    /// bottom right, horizontal, vertical) in `fg` with `modifier`.
    fn assert_border(
        buffer: &Buffer,
        border: Rect,
        symbols: [&str; 6],
        fg: Color,
        modifier: Modifier,
    ) {
        let [
            top_left,
            top_right,
            bottom_left,
            bottom_right,
            horizontal,
            vertical,
        ] = symbols;
        let (left, top) = (border.x, border.y);
        let (right, bottom) = (border.right() - 1, border.bottom() - 1);
        assert_symbol(buffer, (left, top), top_left, fg, modifier);
        assert_symbol(buffer, (right, top), top_right, fg, modifier);
        assert_symbol(buffer, (left, bottom), bottom_left, fg, modifier);
        assert_symbol(buffer, (right, bottom), bottom_right, fg, modifier);
        assert_symbol(buffer, (left + 1, top), horizontal, fg, modifier);
        assert_symbol(buffer, (left + 1, bottom), horizontal, fg, modifier);
        assert_symbol(buffer, (left, top + 1), vertical, fg, modifier);
        assert_symbol(buffer, (right, top + 1), vertical, fg, modifier);
    }

    const THICK: [&str; 6] = [
        "\u{250f}", "\u{2513}", "\u{2517}", "\u{251b}", "\u{2501}", "\u{2503}",
    ];

    const PLAIN: [&str; 6] = [
        "\u{250c}", "\u{2510}", "\u{2514}", "\u{2518}", "\u{2500}", "\u{2502}",
    ];

    /// Fail when the cell does not carry the ANSI colors `fg` on `bg` and `modifier`.
    fn assert_fill(cell: &Cell, fg: Color, bg: Color, modifier: Modifier) {
        assert_eq!((cell.fg, cell.bg, cell.modifier), (fg, bg, modifier));
    }

    /// Fail when the cell does not carry the ANSI color `fg` in bold.
    fn assert_ansi_bold(cell: &Cell, fg: Color) {
        assert_eq!((cell.fg, cell.modifier), (fg, Modifier::BOLD));
    }

    #[test]
    fn gallery_hides_thumbnail_labels_by_default() {
        let mut app = app_with(&["a.jpg", "b.jpg"]);
        let screen = render(&mut app, None, 100, 30);
        assert_hides(&screen, "b.jpg");
        assert_shows(&screen, " a.jpg [1/2] ");

        app.gallery.toggle_filenames();
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, "b.jpg");
    }

    #[test]
    fn gallery_draws_a_thick_border_around_the_selected_cell() {
        let mut app = app_with(&["a.jpg", "b.jpg", "c.jpg"]);
        let buffer = render_buffer(&mut app, None, 100, 30);
        let screen = screen_rows(&buffer).join("\n");
        let border = app.gallery.border_rect(0);
        assert_eq!(
            (border.x, border.y),
            (0, 0),
            "the edge gaps hold the border"
        );
        assert_border(&buffer, border, THICK, Color::Blue, Modifier::BOLD);
        assert_eq!(screen.matches('\u{250f}').count(), 1, "one thick border");
        assert_eq!(
            screen.matches('\u{250c}').count(),
            0,
            "no frame, no plain border"
        );

        // The thumbnail area stays empty for Kitty, and the other cells get no border.
        let cell = app.gallery.thumb_rect(0);
        assert_symbol(
            &buffer,
            (cell.x, cell.y),
            " ",
            Color::Reset,
            Modifier::empty(),
        );
        let second = app.gallery.border_rect(1);
        assert_symbol(
            &buffer,
            (second.right() - 1, second.y),
            " ",
            Color::Reset,
            Modifier::empty(),
        );
        assert_terminal_background(&buffer);
    }

    #[test]
    fn gallery_draws_a_yellow_plain_border_around_each_marked_cell() {
        let mut app = app_with(&["a.jpg", "b.jpg", "c.jpg"]);
        app.selection.insert(app.images[2].clone());
        let buffer = render_buffer(&mut app, None, 100, 30);
        let marked = app.gallery.border_rect(2);
        assert_border(&buffer, marked, PLAIN, Color::Yellow, Modifier::empty());
        assert_terminal_background(&buffer);

        // A marked neighbor shares the gap column with the cursor. The cursor border wins.
        app.selection.clear();
        app.selection.insert(app.images[1].clone());
        let buffer = render_buffer(&mut app, None, 100, 30);
        let cursor = app.gallery.border_rect(0);
        let neighbor = app.gallery.border_rect(1);
        assert_eq!(neighbor.x, cursor.right() - 1, "one shared gap column");
        assert_border(&buffer, cursor, THICK, Color::Blue, Modifier::BOLD);
        assert_symbol(
            &buffer,
            (neighbor.right() - 1, neighbor.y),
            "\u{2510}",
            Color::Yellow,
            Modifier::empty(),
        );
        assert_symbol(
            &buffer,
            (neighbor.x + 1, neighbor.y),
            "\u{2500}",
            Color::Yellow,
            Modifier::empty(),
        );

        // On a marked cell under the cursor, the cursor border wins.
        app.selection.insert(app.images[0].clone());
        let buffer = render_buffer(&mut app, None, 100, 30);
        assert_border(&buffer, cursor, THICK, Color::Blue, Modifier::BOLD);
    }

    #[test]
    fn gallery_borders_reach_the_grid_edges_and_hold_the_label() {
        let names: Vec<String> = (0..40).map(|index| format!("{index:02}.jpg")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut app = app_with(&names);
        app.gallery.show_filenames = true;
        render_buffer(&mut app, None, 100, 30);
        let last = app.gallery.visible_rows * app.gallery.grid_cols - 1;
        app.gallery.cursor = last;
        app.selection.insert(app.images[0].clone());
        let buffer = render_buffer(&mut app, None, 100, 30);
        let grid = app.gallery.grid_rect;

        let border = app.gallery.border_rect(last);
        assert_eq!(border.right(), grid.right(), "the right edge gap");
        assert_eq!(border.bottom(), grid.bottom(), "the bottom edge gap");
        assert_border(&buffer, border, THICK, Color::Blue, Modifier::BOLD);
        let cell = app.gallery.thumb_rect(last);
        assert_eq!(
            cell.bottom() + 1,
            border.bottom() - 1,
            "the label row sits inside"
        );
        assert_fill(
            &buffer[(cell.x + 1, cell.bottom())],
            Color::Blue,
            Color::Reset,
            Modifier::BOLD,
        );

        let marked = app.gallery.border_rect(0);
        assert_eq!(
            (marked.x, marked.y),
            (grid.x, grid.y),
            "the top left edge gaps"
        );
        assert_border(&buffer, marked, PLAIN, Color::Yellow, Modifier::empty());
        let first = app.gallery.thumb_rect(0);
        assert_symbol(
            &buffer,
            (marked.x, first.bottom()),
            "\u{2502}",
            Color::Yellow,
            Modifier::empty(),
        );
        assert_symbol(
            &buffer,
            (marked.x, first.bottom() + 1),
            "\u{2514}",
            Color::Yellow,
            Modifier::empty(),
        );

        // The status row below the grid stays text.
        let screen = screen_rows(&buffer).join("\n");
        assert!(
            status_row(&screen)
                .starts_with(&format!(" {last:02}.jpg [{}/40]  1 selected", last + 1)),
            "{screen}"
        );
        assert_terminal_background(&buffer);
    }

    /// The center cell of the thumbnail area of the visible cell `vis_idx`.
    fn cell_center(app: &App, vis_idx: usize) -> (u16, u16) {
        let cell = app.gallery.thumb_rect(vis_idx);
        (cell.x + cell.width / 2, cell.y + cell.height / 2)
    }

    /// The icon box of the grid cell `vis_idx`. The test cells fit a large icon.
    fn icon_area(app: &App, vis_idx: usize) -> Rect {
        icon_rect(app.gallery.thumb_rect(vis_idx), app.cell_px).unwrap()
    }

    /// True when the icon box of the cell shows `icon` in the dim color, or nothing for `None`.
    fn shows_icon(app: &App, buffer: &Buffer, vis_idx: usize, icon: Option<Icon>) -> bool {
        let area = icon_area(app, vis_idx);
        let mut expected = Buffer::empty(area);
        if let Some(icon) = icon {
            draw_icon(&mut expected, area, icon, STYLES.dim);
        }
        area.positions().all(|position| {
            let (shown, wanted) = (&buffer[position], &expected[position]);
            shown.symbol() == wanted.symbol()
                && (wanted.symbol() == " " || shown.fg == Color::DarkGray)
        })
    }

    #[test]
    fn an_empty_image_cell_shows_a_large_dim_loading_icon() {
        let mut app = app_with(&["a.jpg", "b.jpg"]);
        app.thumb_cache.insert(1, RgbaImage::new(4, 4));
        let buffer = render_buffer(&mut app, None, 100, 30);
        assert!(icon_area(&app, 0).height >= 2, "a large icon, not one cell");
        assert!(shows_icon(&app, &buffer, 0, Some(Icon::Loading)));
        assert!(
            shows_icon(&app, &buffer, 1, None),
            "the cached cell has no icon"
        );
    }

    #[test]
    fn a_folder_cell_shows_a_large_folder_icon_and_its_name() {
        let mut app = folder_app(&["..", "summer"], &["a.jpg"]);
        app.thumb_cache.insert(2, RgbaImage::new(4, 4));
        let buffer = render_buffer(&mut app, None, 100, 30);
        let screen = screen_rows(&buffer).join("\n");
        assert!(shows_icon(&app, &buffer, 0, Some(Icon::Parent)));
        assert!(shows_icon(&app, &buffer, 1, Some(Icon::Folder)));
        assert!(
            shows_icon(&app, &buffer, 2, None),
            "the file cell has no icon"
        );
        // The names sit inside the cells, in the row below the icon, also without labels.
        let (x, _) = cell_center(&app, 1);
        let below = icon_area(&app, 1).bottom();
        let row = &screen_rows(&buffer)[usize::from(below)];
        assert!(row.contains("summer"), "{screen}");
        assert!(!row.contains(".."), "the two dots stand for `..`: {screen}");
        assert_eq!(
            buffer[(x, below)].symbol(),
            "m",
            "the name is centered: sum|mer"
        );
        assert_hides(&screen, "a.jpg");
        assert_eq!(
            status_row(&screen).trim_end(),
            " .. [1/3]  1 folder, 1 image",
            "{screen}"
        );

        // With labels, the names move to the label row, and the selected border wins.
        app.gallery.toggle_filenames();
        app.gallery.cursor = 1;
        let buffer = render_buffer(&mut app, None, 100, 30);
        let screen = screen_rows(&buffer).join("\n");
        let row = &screen_rows(&buffer)[usize::from(icon_area(&app, 1).bottom())];
        assert!(!row.contains("summer"), "{screen}");
        assert_shows(&screen, "summer");
        assert_shows(&screen, "a.jpg");
        let label = cell_of(&buffer, "summer");
        assert_eq!((label.fg, label.modifier), (Color::Blue, Modifier::BOLD));
        assert_border(
            &buffer,
            app.gallery.border_rect(1),
            THICK,
            Color::Blue,
            Modifier::BOLD,
        );
        assert!(
            status_row(&screen).starts_with(" summer [2/3]  1 folder, 1 image "),
            "{screen}"
        );
    }

    #[cfg(feature = "video")]
    #[test]
    fn an_empty_video_cell_shows_a_large_dim_play_icon() {
        let mut app = app_with(&["a.jpg", "clip.mp4"]);
        let buffer = render_buffer(&mut app, None, 100, 30);
        assert!(shows_icon(&app, &buffer, 1, Some(Icon::Video)));
        assert!(shows_icon(&app, &buffer, 0, Some(Icon::Loading)));
    }

    #[test]
    fn the_help_popup_hides_the_icons_with_the_pictures() {
        let mut app = folder_app(&["..", "summer"], &["a.jpg", "clip.mp4"]);
        app.help_visible = true;
        let buffer = render_buffer(&mut app, None, 100, 30);
        for vis_idx in 0..4 {
            let area = icon_area(&app, vis_idx);
            let popup_free = area
                .positions()
                .filter(|position| buffer[*position].symbol() != " ")
                .all(|position| buffer[position].fg != Color::DarkGray);
            assert!(
                popup_free,
                "cell {vis_idx} shows no dim icon under the help"
            );
        }
        app.help_visible = false;
        let buffer = render_buffer(&mut app, None, 100, 30);
        assert!(shows_icon(&app, &buffer, 2, Some(Icon::Loading)));
    }

    #[test]
    fn a_small_cell_gets_the_one_cell_symbol() {
        let mut app = app_with(&["a.jpg", "clip.mp4"]);
        let buffer = render_buffer(&mut app, None, 100, 5);
        assert_eq!(icon_rect(app.gallery.thumb_rect(0), app.cell_px), None);
        assert_symbol(
            &buffer,
            cell_center(&app, 0),
            Icon::Loading.symbol(),
            Color::DarkGray,
            Modifier::empty(),
        );
    }

    #[test]
    fn a_cut_cell_gets_no_side_at_the_clip_edge() {
        // 15 columns: the cell width clamps to 8, so the second cell overflows the grid.
        let mut app = app_with(&["a.jpg", "b.jpg"]);
        app.gallery.cursor = 1;
        app.selection.insert(app.images[0].clone());
        let buffer = render_buffer(&mut app, None, 15, 8);
        let grid = app.gallery.grid_rect;
        let border = app.gallery.border_rect(1);
        assert!(border.right() > grid.right(), "the cell overflows");

        let (left, top, bottom) = (border.x, border.y, border.bottom() - 1);
        let edge = grid.right() - 1;
        assert_symbol(
            &buffer,
            (left, top),
            "\u{250f}",
            Color::Blue,
            Modifier::BOLD,
        );
        assert_symbol(
            &buffer,
            (left, bottom),
            "\u{2517}",
            Color::Blue,
            Modifier::BOLD,
        );
        assert_symbol(
            &buffer,
            (edge, top),
            "\u{2501}",
            Color::Blue,
            Modifier::BOLD,
        );
        assert_symbol(
            &buffer,
            (edge, bottom),
            "\u{2501}",
            Color::Blue,
            Modifier::BOLD,
        );
        assert_symbol(
            &buffer,
            (edge, top + 1),
            " ",
            Color::Reset,
            Modifier::empty(),
        );

        // The marked cell fits, so its border stays closed.
        let marked = app.gallery.border_rect(0);
        assert_symbol(
            &buffer,
            (marked.x, marked.y),
            "\u{250c}",
            Color::Yellow,
            Modifier::empty(),
        );
    }

    #[test]
    fn gallery_uses_the_terminal_defaults_and_the_ansi_roles() {
        let mut app = app_with(&["a.jpg", "b.jpg", "c.jpg"]);
        app.gallery.show_filenames = true;
        app.selection.insert(app.images[1].clone());
        let buffer = render_buffer(&mut app, None, 100, 30);
        assert_terminal_background(&buffer);

        assert_ansi_bold(cell_of(&buffer, "a.jpg"), Color::Blue);
        assert_ansi(cell_of(&buffer, "\u{25cf} b.jpg"), Color::Yellow);
        assert_default_fg(cell_of(&buffer, "c.jpg"), Modifier::empty());
        assert_default_fg(cell_of(&buffer, "a.jpg ["), Modifier::empty());
        assert_ansi(cell_of(&buffer, "1 selected"), Color::Yellow);

        // On a marked cell under the cursor, the cursor style wins for the label.
        app.selection.insert(app.images[0].clone());
        let buffer = render_buffer(&mut app, None, 100, 30);
        assert_ansi_bold(cell_of(&buffer, "\u{25cf} a.jpg"), Color::Blue);

        app.error = Some("scan failed".to_owned());
        let buffer = render_buffer(&mut app, None, 100, 30);
        assert_ansi(cell_of(&buffer, "Error: scan failed"), Color::Red);
    }

    #[test]
    #[cfg(feature = "video")]
    fn the_still_of_an_animated_image_carries_the_play_stamp() {
        use crate::test_support::fixture;
        let anim = fixture("anim.gif");
        let still = fixture("still.gif");
        let mut app = app_with(&[anim.to_str().unwrap(), still.to_str().unwrap()]);
        app.filenames = vec!["anim.gif".to_owned(), "still.gif".to_owned()];
        app.gallery.show_filenames = true;
        let screen = render(&mut app, None, 100, 30);
        assert!(
            !screen.contains('\u{25b6}'),
            "no small marker anywhere: {screen}"
        );

        let rect = app.gallery.thumb_box();
        app.spawn_thumb_decode(0, rect);
        app.spawn_thumb_decode(1, rect);
        crate::test_support::wait_until(|| {
            app.poll_thumbnails();
            app.animated.len() == 2
        });
        // Both fixtures are 16x12 and yellow. The stamp turns the middle of the animation white.
        let middle = |index: usize| {
            let (picture, _) = app.thumb_cache.peek(index).unwrap();
            let image = &picture.image;
            image.get_pixel(image.width() / 2, image.height() / 2).0
        };
        assert!(
            middle(0)[2] > 200,
            "the animation has the stamp: {:?}",
            middle(0)
        );
        assert_eq!(middle(1)[2], 0, "the still has no stamp: {:?}", middle(1));
        app.mark_dirty();
        let screen = render(&mut app, None, 100, 30);
        assert!(!screen.contains('\u{25b6}'), "{screen}");
        assert_shows(&screen, "anim.gif");
    }

    #[test]
    fn gallery_shortens_a_long_label() {
        let long = format!("{}.jpg", "x".repeat(80));
        let mut app = app_with(&[long.as_str()]);
        app.gallery.show_filenames = true;
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, "xx\u{2026}");
    }

    #[test]
    fn a_label_of_wide_characters_fits_the_cell_width_with_its_margin() {
        let name = format!("{}.jpg", "\u{5199}\u{771f}".repeat(10));
        let app = app_with(&[name.as_str()]);
        let label = super::label_text(&app, 0, false, 12);
        assert!(label.ends_with('\u{2026}'), "{label}");
        assert!(Line::from(&*label).width() <= 10, "{label}");

        // An emoji with VS16 takes two columns, its base character alone one.
        let name = format!("{}.jpg", "\u{263a}\u{fe0f}".repeat(10));
        let app = app_with(&[name.as_str()]);
        let label = super::label_text(&app, 0, false, 12);
        assert!(label.ends_with('\u{2026}'), "{label}");
        assert!(Line::from(&*label).width() <= 10, "{label}");
    }

    #[test]
    fn a_grid_narrower_than_one_cell_cuts_the_first_cell_and_hides_the_second() {
        // 9 columns: the cell width clamps to 8, so the first border needs 10 columns.
        let mut app = app_with(&["a.jpg", "b.jpg", "c.jpg"]);
        app.gallery.show_filenames = true;
        let buffer = render_buffer(&mut app, None, 9, 12);
        let grid = app.gallery.grid_rect;
        assert_eq!(app.gallery.grid_cols, 2);
        assert!(
            app.gallery.border_rect(0).right() > grid.right(),
            "the first cell overflows"
        );
        assert!(
            app.gallery.border_rect(1).x >= grid.right(),
            "the second cell starts outside"
        );
        assert_symbol(&buffer, (0, 0), "\u{250f}", Color::Blue, Modifier::BOLD);
        let top_row = &screen_rows(&buffer)[0];
        assert!(
            !top_row.contains('\u{2513}'),
            "no right side at the clip edge: {top_row}"
        );
        assert_eq!(app.gallery.cell_at(1, 1), Some(0));
    }
}
