//! The ratatui drawing of each view. Kitty draws the pictures above the frame afterwards.

mod grid;
mod help;
mod status;

use crate::app::{App, ViewMode};
use crate::styles::STYLES;
use grid::draw_thumbnail_cells;
use help::draw_help_popup;
use ratatui::{
    prelude::*,
    widgets::{Paragraph, Wrap},
};
#[cfg(feature = "video")]
use status::video_bar;
use status::{
    draw_slideshow_badge, draw_slideshow_mode, draw_status_row, fullscreen_bar, gallery_bar,
};
use std::path::PathBuf;
use tuilibr::frame;

const STATUS_ROWS: u16 = 1;

/// The title of the info pane in the maximal layout.
const INFO_TITLE: &str = "Info";

/// The areas of one draw. In the maximal layout, the panes are the areas inside the boxes, so the
/// text, the pictures and the click areas stay inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Panes {
    /// The info pane, the list pane of the frame. `None` while hidden and in a slideshow.
    pub(crate) info: Option<Rect>,
    /// The grid, the picture or the video frame.
    pub(crate) main: Rect,
    /// The status row under both panes. A slideshow has none.
    pub(crate) status: Rect,
}

/// Draw the whole frame. `status` is a note of the run loop in place of the file facts.
pub(crate) fn draw(frame: &mut Frame<'_>, app: &mut App, status: Option<&str>) {
    let area = frame.area();
    app.layout.screen = area;
    // Only the grid asks for icons.
    app.grid_icons.wanted.clear();
    let panes = panes(area, app);
    draw_pane_lines(frame.buffer_mut(), app, area);
    match app.mode {
        ViewMode::Gallery => draw_gallery(frame, app, panes, status),
        ViewMode::Fullscreen => draw_fullscreen(frame, app, panes, status),
        #[cfg(feature = "video")]
        ViewMode::Video => draw_video(frame, app, panes, status),
    }
    if let Some(info) = panes.info {
        draw_info_panel(frame, app, info);
    }
    // The main pane has the focus. The info pane holds no picture, so the cells dim all of it.
    frame::dim_pane(frame.buffer_mut(), panes.info);
    if app.help_visible {
        let loops = app.grid_loops.enabled.then(|| app.graphics.loops());
        draw_help_popup(frame, app.graphics.transfer(), loops);
    }
}

/// The info pane and the main pane above the status row, in the minimal or the maximal layout of
/// the frame. Below `NARROW_BELOW` columns, the info pane sits above the main pane. A slideshow
/// gives the whole screen to the picture.
pub(crate) fn panes(area: Rect, app: &App) -> Panes {
    let (middle, status) = middle_and_status(area, app);
    if app.slideshow_fills_screen() {
        return Panes {
            info: None,
            main: middle,
            status,
        };
    }
    let shown = app.info_visible;
    let (info, main) = match app.pane_layout {
        frame::Layout::Minimal => {
            let (info, _, main) = frame::split(middle, shown);
            (info, main)
        }
        frame::Layout::Maximal => {
            let (info, main) = frame::split_boxed(middle, shown);
            (frame::pane_inner(info), frame::pane_inner(main))
        }
    };
    Panes {
        info: shown.then_some(info),
        main,
        status,
    }
}

/// The divider of the minimal layout, or the two boxes of the maximal layout. The main pane has
/// the focus, so its box takes the accent.
fn draw_pane_lines(buffer: &mut Buffer, app: &App, area: Rect) {
    if app.slideshow_fills_screen() {
        return;
    }
    let (middle, _) = middle_and_status(area, app);
    let shown = app.info_visible;
    if app.pane_layout == frame::Layout::Minimal {
        let (_, split, _) = frame::split(middle, shown);
        frame::divider(buffer, split);
        return;
    }
    let (info, main) = frame::split_boxed(middle, shown);
    frame::pane_box(buffer, main, &main_title(app), true);
    if shown {
        frame::pane_box(buffer, info, INFO_TITLE, false);
    }
}

/// The title of the main box: the folder name in the grid, the file name in the other views. A
/// file list has no folder.
fn main_title(app: &App) -> String {
    let path = if app.mode == ViewMode::Gallery {
        app.current_dir.as_deref().filter(|_| !app.file_list)
    } else {
        app.images.get(app.current).map(PathBuf::as_path)
    };
    path.map_or_else(
        || "Files".to_owned(),
        |path| {
            path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            )
        },
    )
}

/// The rows of both panes, and the status row under them.
fn middle_and_status(area: Rect, app: &App) -> (Rect, Rect) {
    let rows = status_rows(app);
    let middle = Rect {
        height: area.height.saturating_sub(rows),
        ..area
    };
    let status = Rect {
        y: area.y + middle.height,
        height: area.height.min(rows),
        ..area
    };
    (middle, status)
}

fn draw_info_panel(frame: &mut Frame<'_>, app: &mut App, inner: Rect) {
    let Some(path) = app.view_file() else {
        frame.render_widget(
            Paragraph::new(Span::styled("no image selected", STYLES.dim)),
            inner,
        );
        return;
    };
    let info = app.info_for(&path);

    let label_width = info
        .rows
        .iter()
        .filter(|(label, _)| !label.is_empty())
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    let mut lines: Vec<Line<'_>> = Vec::new();
    for (label, value) in &info.rows {
        if label.is_empty() {
            if !lines.is_empty() {
                lines.push(Line::default());
            }
            lines.push(Line::from(Span::styled(value.clone(), STYLES.title)));
        } else {
            lines.push(Line::from(vec![
                Span::styled(format!("{label:<label_width$}  "), STYLES.dim),
                Span::styled(value.clone(), STYLES.text),
            ]));
        }
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn draw_fullscreen(frame: &mut Frame<'_>, app: &mut App, panes: Panes, status: Option<&str>) {
    draw_status_row(frame, app, panes.status, fullscreen_bar(app, status));
    if app.slideshow_fills_screen() {
        draw_slideshow_mode(frame, app, panes.main);
        draw_slideshow_badge(frame, app, panes.main, status);
    }
    app.image_rect = panes.main;
}

#[cfg(feature = "video")]
fn draw_video(frame: &mut Frame<'_>, app: &mut App, panes: Panes, status: Option<&str>) {
    draw_status_row(frame, app, panes.status, video_bar(app, status));
    if app.slideshow_fills_screen() {
        draw_slideshow_mode(frame, app, panes.main);
        draw_slideshow_badge(frame, app, panes.main, status);
    }
    app.image_rect = panes.main;
}

fn draw_gallery(frame: &mut Frame<'_>, app: &mut App, panes: Panes, status: Option<&str>) {
    app.gallery.update_grid(panes.main);
    let filtered_count = app.gallery.filtered_indices.len();
    let parts = gallery_bar(app, filtered_count, status);
    draw_status_row(frame, app, panes.status, parts);
    let mut wanted = std::mem::take(&mut app.grid_icons.wanted);
    draw_thumbnail_cells(frame, app, panes.main, &mut wanted);
    app.grid_icons.wanted = wanted;
}

fn status_rows(app: &App) -> u16 {
    if app.slideshow_fills_screen() {
        0
    } else {
        STATUS_ROWS
    }
}

#[cfg(test)]
mod tests {
    use super::draw;
    use crate::app::App;
    use crate::image_list::SharedImageList;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier};
    use std::path::PathBuf;
    use tuilibr::frame::Layout;

    /// An app with `names` below `/photos`, as after a finished scan.
    pub(super) fn app_with(names: &[&str]) -> App {
        let mut app = App::new((8, 16), SharedImageList::new());
        app.images = names
            .iter()
            .map(|name| PathBuf::from("/photos").join(name))
            .collect();
        app.filenames = names.iter().map(|name| (*name).to_owned()).collect();
        app.gallery.filtered_indices = (0..names.len()).collect();
        app.scan_complete = true;
        app
    }

    /// A slideshow gives the whole screen to the picture, without the panes, and its end brings
    /// the main box and the status row back.
    #[test]
    fn slideshow_hides_status_row() {
        let mut app = crate::test_support::fullscreen_app(3, 0);
        app.info_visible = true;
        render_buffer(&mut app, None, 80, 12);
        assert_eq!(app.image_rect, Rect::new(37, 1, 42, 9));

        app.start_slideshow();
        let screen = render(&mut app, Some("a note"), 80, 12);
        assert_eq!(app.image_rect, Rect::new(0, 0, 80, 12));
        assert_hides(&screen, "Info");
        assert_hides(&screen, "\u{256d}");

        app.stop_slideshow();
        app.info_visible = false;
        render_buffer(&mut app, None, 80, 12);
        assert_eq!(app.image_rect, Rect::new(1, 1, 78, 9));
    }

    /// The badge sits at the bottom right of a slideshow: the state, a note or an error.
    #[test]
    fn slideshow_badge_shows_state() {
        let mut app = crate::test_support::fullscreen_app(3, 0);
        app.slideshow_interval = std::time::Duration::from_secs(5);
        app.start_slideshow();
        let buffer = render_buffer(&mut app, None, 40, 12);
        let rows = screen_rows(&buffer);
        assert!(rows[11].ends_with(" \u{25b6} 5.0s "), "{rows:?}");
        assert_eq!(buffer[(39, 11)].style().bg, Some(Color::Gray));
        assert_ne!(
            buffer[(10, 11)].style().bg,
            Some(Color::Gray),
            "the picture shows left of the badge"
        );

        app.slideshow_toggle_pause();
        let rows = screen_rows(&render_buffer(&mut app, Some("a note"), 40, 12));
        assert!(rows[11].ends_with(" a note  \u{23f8} 5.0s "), "{rows:?}");

        app.error = Some("broken".to_owned());
        let rows = screen_rows(&render_buffer(&mut app, None, 40, 12));
        assert!(rows[11].ends_with(" Error: broken "), "{rows:?}");

        app.error = None;
        app.stop_slideshow();
        let rows = screen_rows(&render_buffer(&mut app, None, 40, 12));
        assert!(
            !rows[11].contains("5.0s"),
            "no badge after the slideshow: {rows:?}"
        );
    }

    pub(super) fn render_buffer(
        app: &mut App,
        status: Option<&str>,
        width: u16,
        height: u16,
    ) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app, status)).unwrap();
        terminal.backend().buffer().clone()
    }

    pub(super) fn screen_rows(buffer: &Buffer) -> Vec<String> {
        let area = buffer.area;
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    pub(super) fn render(app: &mut App, status: Option<&str>, width: u16, height: u16) -> String {
        screen_rows(&render_buffer(app, status, width, height)).join("\n")
    }

    /// The cell of the first character of `needle`. The test screens hold one character per cell.
    pub(super) fn cell_of<'a>(buffer: &'a Buffer, needle: &str) -> &'a Cell {
        let (x, y) = screen_rows(buffer)
            .iter()
            .enumerate()
            .find_map(|(y, row)| {
                row.find(needle)
                    .and_then(|byte| row.get(..byte))
                    .map(|head| (head.chars().count(), y))
            })
            .unwrap_or_else(|| panic!("missing {needle:?}"));
        &buffer[(u16::try_from(x).unwrap(), u16::try_from(y).unwrap())]
    }

    /// Fail when a cell sets a background. The status row in the last row has the black of the
    /// palette as the ground of the bar.
    pub(super) fn assert_terminal_background(buffer: &Buffer) {
        let area = buffer.area;
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                let bg = buffer[(x, y)].bg;
                let bar = y + 1 == area.bottom() && bg == Color::Black;
                assert!(
                    bg == Color::Reset || bar,
                    "cell ({x}, {y}) sets the background {bg:?}"
                );
            }
        }
    }

    pub(super) fn assert_symbol(
        buffer: &Buffer,
        (x, y): (u16, u16),
        symbol: &str,
        fg: Color,
        modifier: Modifier,
    ) {
        let cell = &buffer[(x, y)];
        assert_eq!(
            (cell.symbol(), cell.fg, cell.modifier),
            (symbol, fg, modifier),
            "cell ({x}, {y})"
        );
    }

    pub(super) fn status_row(screen: &str) -> &str {
        screen.lines().last().unwrap_or_default()
    }

    /// The groups of the status row: the texts between runs of two or more spaces. The bar sets
    /// its parts apart with two spaces and centers the middle.
    pub(super) fn status_parts(screen: &str) -> Vec<&str> {
        status_row(screen)
            .split("  ")
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect()
    }

    pub(super) fn assert_default_fg(cell: &Cell, modifier: Modifier) {
        assert_eq!((cell.fg, cell.modifier), (Color::Reset, modifier));
    }

    pub(super) fn assert_ansi(cell: &Cell, fg: Color) {
        assert_eq!((cell.fg, cell.modifier), (fg, Modifier::empty()));
    }

    pub(super) fn assert_shows(screen: &str, needle: &str) {
        assert!(screen.contains(needle), "missing {needle:?} in:\n{screen}");
    }

    pub(super) fn assert_hides(screen: &str, needle: &str) {
        assert!(
            !screen.contains(needle),
            "unexpected {needle:?} in:\n{screen}"
        );
    }

    /// The info pane is the dim left pane in its box, the grid has the focus.
    #[test]
    fn info_panel_lists_file_rows() {
        let mut app = app_with(&["a.jpg"]);
        app.info_visible = true;
        let screen = render(&mut app, None, 120, 30);
        assert_shows(&screen, "\u{256d}\u{2500} Info ");
        assert_hides(&screen, "i: close");
        assert_shows(&screen, "File");
        assert_shows(&screen, "Name    a.jpg");
        assert_shows(&screen, "Format  JPG");
        let rows = screen_rows(&render_buffer(&mut app, None, 120, 30));
        assert!(rows[0].starts_with("\u{256d}\u{2500} Info "), "{rows:?}");
        assert!(rows[1].starts_with("\u{2502}File"), "{rows:?}");

        let buffer = render_buffer(&mut app, None, 120, 30);
        assert_terminal_background(&buffer);
        assert_ansi(cell_of(&buffer, "Info "), Color::DarkGray);
        let dim = Modifier::DIM;
        assert_default_fg(cell_of(&buffer, "File "), Modifier::BOLD | dim);
        let name = cell_of(&buffer, "Name ");
        assert_eq!((name.fg, name.modifier), (Color::DarkGray, dim));
        assert_default_fg(cell_of(&buffer, "a.jpg"), dim);
    }

    #[test]
    fn info_panel_without_image_says_so() {
        let mut app = app_with(&[]);
        app.info_visible = true;
        let screen = render(&mut app, None, 120, 30);
        assert_shows(&screen, "no image selected");
    }

    /// Below 60 columns, the info pane sits above the grid, a third of the height.
    #[test]
    fn info_panel_above_when_narrow() {
        let mut app = app_with(&["a.jpg"]);
        app.info_visible = true;
        let rows = screen_rows(&render_buffer(&mut app, None, 50, 30));
        assert!(rows[0].starts_with("\u{256d}\u{2500} Info "), "{rows:?}");
        assert!(rows[9].starts_with("\u{256d}\u{2500} Files "), "{rows:?}");
        assert_eq!(app.gallery.grid_rect, Rect::new(1, 10, 48, 18));
    }

    /// The maximal layout boxes both panes, the main box in the accent with the folder name. The
    /// minimal layout has one divider and no titles. Hidden, the info pane leaves the whole area to
    /// the main pane.
    #[test]
    fn both_layouts_with_and_without_info() {
        let mut app = app_with(&["a.jpg"]);
        app.current_dir = Some(PathBuf::from("/photos/summer"));
        let screen = render(&mut app, None, 100, 20);
        assert!(screen.starts_with("\u{256d}\u{2500} summer "), "{screen}");
        assert_hides(&screen, "Info");
        assert_eq!(app.gallery.grid_rect, Rect::new(1, 1, 98, 17));
        let buffer = render_buffer(&mut app, None, 100, 20);
        assert_eq!(cell_of(&buffer, "summer").fg, tuilibr::frame::accent());

        app.info_visible = true;
        let screen = render(&mut app, None, 100, 20);
        assert!(screen.starts_with("\u{256d}\u{2500} Info "), "{screen}");
        assert_shows(&screen, "\u{256d}\u{2500} summer ");
        assert_eq!(app.gallery.grid_rect, Rect::new(37, 1, 62, 17));

        app.pane_layout = Layout::Minimal;
        let rows = screen_rows(&render_buffer(&mut app, None, 100, 20));
        assert_eq!(rows[0].chars().nth(36), Some('\u{2502}'), "{rows:?}");
        assert_eq!(rows[18].chars().nth(36), Some('\u{2502}'), "{rows:?}");
        assert!(rows[0].starts_with("File "), "no title: {rows:?}");
        assert_hides(&rows.join("\n"), "\u{256d}");
        assert_eq!(app.gallery.grid_rect, Rect::new(37, 0, 63, 19));

        app.info_visible = false;
        let screen = render(&mut app, None, 100, 20);
        assert_hides(&screen, "\u{2502}");
        assert_eq!(app.gallery.grid_rect, Rect::new(0, 0, 100, 19));
    }

    /// The fullscreen box carries the file name, and the picture goes inside it.
    #[test]
    fn fullscreen_box_names_the_file() {
        let mut app = crate::test_support::fullscreen_app(3, 1);
        app.info_visible = true;
        let screen = render(&mut app, None, 100, 20);
        let name = app.images[1]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_shows(&screen, &format!("\u{256d}\u{2500} {name} "));
        assert_eq!(app.image_rect, Rect::new(37, 1, 62, 17));
    }

    #[test]
    fn info_panel_separates_sections() {
        let mut app = app_with(&["orientation-6.png"]);
        app.images = vec![crate::test_support::module_fixture("png/orientation-6.png")];
        app.info_visible = true;
        let screen = render(&mut app, None, 120, 30);
        assert_shows(&screen, "File");
        assert_shows(&screen, "EXIF");
    }
}
