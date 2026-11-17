//! The tests of the grid cells: the borders, the labels and the icons.
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
fn assert_border(buffer: &Buffer, border: Rect, symbols: [&str; 6], fg: Color, modifier: Modifier) {
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
fn file_cells_draw_no_name() {
    let mut app = app_with(&["a.jpg", "b.jpg"]);
    let screen = render(&mut app, None, 100, 30);
    assert_hides(&screen, "b.jpg");
    assert_shows(&screen, " a.jpg [1/2] ");
}

#[test]
fn selected_cell_gets_thick_border() {
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
fn marked_cell_gets_yellow_border() {
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
fn borders_reach_edges() {
    let names: Vec<String> = (0..40).map(|index| format!("{index:02}.jpg")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut app = app_with(&names);
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
    assert_eq!(cell.bottom(), border.bottom() - 1, "the bottom gap row");
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
        (marked.x, first.bottom() - 1),
        "\u{2502}",
        Color::Yellow,
        Modifier::empty(),
    );
    assert_symbol(
        &buffer,
        (marked.x, first.bottom()),
        "\u{2514}",
        Color::Yellow,
        Modifier::empty(),
    );

    // The status row below the grid stays text.
    let screen = screen_rows(&buffer).join("\n");
    assert!(
        status_row(&screen).starts_with(&format!(" {last:02}.jpg [{}/40]  1 selected", last + 1)),
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
        shown.symbol() == wanted.symbol() && (wanted.symbol() == " " || shown.fg == Color::DarkGray)
    })
}

#[test]
fn empty_image_cell_shows_icon() {
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
fn folder_cell_shows_icon_and_name() {
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
    // The names sit inside the cells, in the row below the icon.
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

    // The cursor style wins for the name of the selected folder.
    app.gallery.cursor = 1;
    let buffer = render_buffer(&mut app, None, 100, 30);
    let name = cell_of(&buffer, "summer");
    assert_eq!((name.fg, name.modifier), (Color::Blue, Modifier::BOLD));
}

#[cfg(feature = "video")]
#[test]
fn empty_video_cell_shows_play_icon() {
    let mut app = app_with(&["a.jpg", "clip.mp4"]);
    let buffer = render_buffer(&mut app, None, 100, 30);
    assert!(shows_icon(&app, &buffer, 1, Some(Icon::Video)));
    assert!(shows_icon(&app, &buffer, 0, Some(Icon::Loading)));
}

#[test]
fn help_popup_hides_icons() {
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
fn small_cell_gets_symbol() {
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
fn cut_cell_stays_open_at_edge() {
    // 12 columns: the cell width clamps to 5, so the second cell overflows the grid.
    let mut app = app_with(&["a.jpg", "b.jpg"]);
    app.gallery.cursor = 1;
    app.selection.insert(app.images[0].clone());
    let buffer = render_buffer(&mut app, None, 12, 8);
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
fn uses_terminal_defaults_and_ansi_roles() {
    let mut app = app_with(&["a.jpg", "b.jpg", "c.jpg"]);
    app.selection.insert(app.images[1].clone());
    let buffer = render_buffer(&mut app, None, 100, 30);
    assert_terminal_background(&buffer);

    assert_default_fg(cell_of(&buffer, "a.jpg ["), Modifier::empty());
    assert_ansi(cell_of(&buffer, "1 selected"), Color::Yellow);

    app.error = Some("scan failed".to_owned());
    let buffer = render_buffer(&mut app, None, 100, 30);
    assert_ansi(cell_of(&buffer, "Error: scan failed"), Color::Red);
}

#[test]
#[cfg(feature = "video")]
fn animated_still_carries_play_stamp() {
    use crate::test_support::fixture;
    let anim = fixture("anim.gif");
    let still = fixture("still.gif");
    let mut app = app_with(&[anim.to_str().unwrap(), still.to_str().unwrap()]);
    app.filenames = vec!["anim.gif".to_owned(), "still.gif".to_owned()];
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
        let (picture, _) = app.thumb_cache.peek(&index).unwrap();
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
fn wide_characters_fit_folder_name() {
    let name = format!("{}.jpg", "\u{5199}\u{771f}".repeat(10));
    let app = app_with(&[name.as_str()]);
    let label = super::folder_name_text(&app, 0, 12);
    assert!(label.ends_with('\u{2026}'), "{label}");
    assert!(Line::from(&*label).width() <= 10, "{label}");

    // An emoji with VS16 takes two columns, its base character alone one.
    let name = format!("{}.jpg", "\u{263a}\u{fe0f}".repeat(10));
    let app = app_with(&[name.as_str()]);
    let label = super::folder_name_text(&app, 0, 12);
    assert!(label.ends_with('\u{2026}'), "{label}");
    assert!(Line::from(&*label).width() <= 10, "{label}");
}

#[test]
fn head_matches_whole_width() {
    for text in [
        "ab\u{5199}\u{771f}\u{263a}\u{fe0f}cd\u{263a}\u{fe0f}\u{5199}e",
        "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}family.jpg",
        "\u{231a}\u{fe0e}watch",
        "\u{1f3f3}\u{fe0f}\u{200d}\u{1f308}x",
    ] {
        check_head(text);
    }
}

fn check_head(text: &str) {
    use unicode_width::UnicodeWidthStr;

    // VS15 lowers the width of its base, so such a text has no unique scan result.
    let widths: Vec<usize> = text
        .char_indices()
        .map(|(index, ch)| {
            text.get(..index + ch.len_utf8())
                .map_or(0, UnicodeWidthStr::width)
        })
        .collect();
    let monotone = widths.windows(2).all(|pair| pair[0] <= pair[1]);
    let scan = |room: usize| {
        let mut head = String::new();
        for ch in text.chars() {
            head.push(ch);
            if head.width() > room {
                head.pop();
                break;
            }
        }
        head
    };
    for room in 0..=text.width() + 2 {
        let head = super::head_within(text, room);
        if monotone {
            assert_eq!(head, scan(room), "{room}: {text}");
        }
        assert!(head.width() <= room, "{room}: {head}");
        if let Some(next) = text.strip_prefix(head).and_then(|rest| rest.chars().next()) {
            let longer = format!("{head}{next}");
            assert!(longer.width() > room, "{room}: {longer}");
        }
    }
}

#[test]
fn narrow_grid_cuts_first_cell() {
    // 6 columns: the cell width clamps to 5, so the first border needs 7 columns.
    let mut app = app_with(&["a.jpg", "b.jpg", "c.jpg"]);
    let buffer = render_buffer(&mut app, None, 6, 12);
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

/// `IMG_0001.JPG` is a Live Photo still with its motion `IMG_0001.MOV`.
fn live_app() -> App {
    let mut app = app_with(&["a.jpg", "IMG_0001.JPG", "c.jpg"]);
    app.set_live_photos(crate::scanner::LivePhotos::from_pairs([(
        "/photos/IMG_0001.JPG".into(),
        "/photos/IMG_0001.MOV".into(),
    )]));
    app
}

#[test]
fn live_still_carries_mark_above_cell() {
    let mut app = live_app();
    let buffer = render_buffer(&mut app, None, 100, 30);
    let border = app.gallery.border_rect(1);
    let rows = screen_rows(&buffer);
    let top = &rows[usize::from(border.y)];
    assert!(top.contains("\u{25ce} Live"), "{top}");
    let mark = cell_of(&buffer, "\u{25ce} Live");
    assert_default_fg(mark, Modifier::empty());
    assert_eq!(
        rows.iter().filter(|row| row.contains("Live")).count(),
        1,
        "only the Live cell carries the mark"
    );

    app.gallery.cursor = 1;
    let buffer = render_buffer(&mut app, None, 100, 30);
    assert_ansi_bold(cell_of(&buffer, "\u{25ce} Live"), Color::Blue);
    assert_symbol(
        &buffer,
        (border.x, border.y),
        "\u{250f}",
        Color::Blue,
        Modifier::BOLD,
    );
}

/// A 5x3 box: the Live mark is `◎` alone and stays inside its border, and the cell icon is the
/// one-cell symbol.
#[test]
fn narrow_live_cell_gets_symbol_mark() {
    let mut app = app_with(&["IMG_0001.JPG", "b.jpg", "c.jpg"]);
    app.set_live_photos(crate::scanner::LivePhotos::from_pairs([(
        "/photos/IMG_0001.JPG".into(),
        "/photos/IMG_0001.MOV".into(),
    )]));
    // The cursor sits in the second row, so no border covers the top row.
    app.gallery.cursor = 2;
    let buffer = render_buffer(&mut app, None, 13, 10);
    assert_eq!(app.gallery.thumb_box(), Rect::new(0, 0, 5, 3));
    let border = app.gallery.border_rect(0);
    assert_eq!(border.width, 7);

    let top = &screen_rows(&buffer)[usize::from(border.y)];
    assert_eq!(top.trim_end(), "  \u{25ce}", "nothing in the next cell");

    assert_eq!(icon_rect(app.gallery.thumb_rect(0), app.cell_px), None);
    assert_symbol(
        &buffer,
        cell_center(&app, 0),
        Icon::Loading.symbol(),
        Color::DarkGray,
        Modifier::empty(),
    );
}
