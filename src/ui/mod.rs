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
    widgets::{Block, Borders, Paragraph, Wrap},
};
#[cfg(feature = "video")]
use status::video_status_line;
use status::{draw_status_row, fullscreen_status_line, gallery_status_line};

/// Preferred width of the info panel, including its border.
const INFO_PANEL_WIDTH: u16 = 42;

/// The status row at the bottom of the gallery, the fullscreen view and the video view.
const STATUS_ROWS: u16 = 1;

/// Draw the whole frame. `status` is a one-line note of the run loop, in place of the file facts.
pub(crate) fn draw(frame: &mut Frame<'_>, app: &mut App, status: Option<&str>) {
    let area = frame.area();
    app.layout.screen = area;
    let (main, info) = split_info(area, app);
    match app.mode {
        ViewMode::Gallery => draw_gallery(frame, app, main, status),
        ViewMode::Fullscreen => draw_fullscreen(frame, app, main, status),
        #[cfg(feature = "video")]
        ViewMode::Video => draw_video(frame, app, main, status),
    }
    if let Some(info) = info {
        draw_info_panel(frame, app, info);
    }
    if app.help_visible {
        draw_help_popup(frame, app.graphics.transfer(), app.graphics.loops());
    }
}

/// Reserve the right-hand column for the info panel when it is open.
/// Very narrow terminals keep the full width.
pub(crate) fn split_info(area: Rect, app: &App) -> (Rect, Option<Rect>) {
    if !app.info_visible {
        return (area, None);
    }
    let width = INFO_PANEL_WIDTH.min(area.width / 3);
    if width < 20 {
        return (area, None);
    }
    let chunks = Layout::horizontal([Constraint::Min(1), Constraint::Length(width)]).split(area);
    (chunks[0], Some(chunks[1]))
}

fn draw_info_panel(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(STYLES.border)
        .title(Line::from(Span::styled(" Info ", STYLES.title)))
        .title_bottom(Line::from(Span::styled(" i: close ", STYLES.key)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

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

fn draw_fullscreen(frame: &mut Frame<'_>, app: &mut App, area: Rect, status: Option<&str>) {
    let placeholder = app.shows_video_placeholder();

    let bottom_line = fullscreen_status_line(app, status);
    let inner = content_area(area);
    draw_status_row(frame, app, status_area(area), bottom_line, status);
    if placeholder {
        let middle = Rect {
            y: inner.y + inner.height / 2,
            height: inner.height.min(1),
            ..inner
        };
        frame.render_widget(
            Paragraph::new(Span::styled(
                "video: Enter plays, p opens the mpv window",
                STYLES.dim,
            ))
            .alignment(Alignment::Center),
            middle,
        );
    }

    app.image_rect = inner;
}

#[cfg(feature = "video")]
fn draw_video(frame: &mut Frame<'_>, app: &mut App, area: Rect, status: Option<&str>) {
    let bottom_line = video_status_line(app, status);
    draw_status_row(frame, app, status_area(area), bottom_line, status);
    app.image_rect = content_area(area);
}

fn draw_gallery(frame: &mut Frame<'_>, app: &mut App, area: Rect, status: Option<&str>) {
    let grid_rect = content_area(area);
    app.gallery.update_grid(grid_rect);
    let filtered_count = app.gallery.filtered_indices.len();
    let line = gallery_status_line(app, filtered_count, status);
    draw_status_row(frame, app, status_area(area), line, status);
    draw_thumbnail_cells(frame, app, grid_rect);
}

/// The rows above the status row. The grid and the image use all of them, without a frame.
pub(crate) fn content_area(area: Rect) -> Rect {
    Rect {
        height: area.height.saturating_sub(STATUS_ROWS),
        ..area
    }
}

/// The status row at the bottom of `area`.
pub(crate) fn status_area(area: Rect) -> Rect {
    Rect {
        y: area.y + content_area(area).height,
        height: area.height.min(STATUS_ROWS),
        ..area
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
    use ratatui::style::{Color, Modifier};
    use std::path::PathBuf;

    /// An app with `names` below `/photos`, as if a finished scan had listed them.
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

    /// Draw one frame and return the screen buffer with the symbols and the styles.
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

    /// The rows of `buffer` as text.
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

    /// Draw one frame and return the screen as text, one line per row.
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

    /// Fail when a cell sets a background, so the terminal background shows everywhere.
    pub(super) fn assert_terminal_background(buffer: &Buffer) {
        let area = buffer.area;
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                let bg = buffer[(x, y)].bg;
                assert_eq!(bg, Color::Reset, "cell ({x}, {y}) sets the background");
            }
        }
    }

    /// Fail when the cell at `(x, y)` does not show `symbol` in the ANSI color `fg` with
    /// `modifier`.
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

    /// The last row of the screen: the status row.
    pub(super) fn status_row(screen: &str) -> &str {
        screen.lines().last().unwrap_or_default()
    }

    /// Fail when the cell does not carry the terminal foreground and `modifier`.
    pub(super) fn assert_default_fg(cell: &Cell, modifier: Modifier) {
        assert_eq!((cell.fg, cell.modifier), (Color::Reset, modifier));
    }

    /// Fail when the cell does not carry the ANSI color `fg` without a modifier.
    pub(super) fn assert_ansi(cell: &Cell, fg: Color) {
        assert_eq!((cell.fg, cell.modifier), (fg, Modifier::empty()));
    }

    /// Fail with the whole screen when `screen` does not contain `needle`.
    pub(super) fn assert_shows(screen: &str, needle: &str) {
        assert!(screen.contains(needle), "missing {needle:?} in:\n{screen}");
    }

    /// Fail with the whole screen when `screen` contains `needle`.
    pub(super) fn assert_hides(screen: &str, needle: &str) {
        assert!(
            !screen.contains(needle),
            "unexpected {needle:?} in:\n{screen}"
        );
    }

    #[test]
    fn info_panel_lists_the_file_rows_of_the_hovered_image() {
        let mut app = app_with(&["a.jpg"]);
        app.info_visible = true;
        let screen = render(&mut app, None, 120, 30);
        assert_shows(&screen, " Info ");
        assert_shows(&screen, " i: close ");
        assert_shows(&screen, "File");
        assert_shows(&screen, "Name    a.jpg");
        assert_shows(&screen, "Format  JPG");

        let buffer = render_buffer(&mut app, None, 120, 30);
        assert_terminal_background(&buffer);
        assert_default_fg(cell_of(&buffer, "Info "), Modifier::BOLD);
        assert_ansi(cell_of(&buffer, "i: close"), Color::Cyan);
        assert_default_fg(cell_of(&buffer, "File"), Modifier::BOLD);
        assert_ansi(cell_of(&buffer, "Name "), Color::DarkGray);
        assert_default_fg(cell_of(&buffer, "a.jpg"), Modifier::empty());
    }

    #[test]
    fn info_panel_without_an_image_says_so() {
        let mut app = app_with(&[]);
        app.info_visible = true;
        let screen = render(&mut app, None, 120, 30);
        assert_shows(&screen, "no image selected");
    }

    #[test]
    fn info_panel_stays_closed_in_a_narrow_terminal() {
        let mut app = app_with(&["a.jpg"]);
        app.info_visible = true;
        let screen = render(&mut app, None, 50, 30);
        assert_hides(&screen, " Info ");
    }
}
