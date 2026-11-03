//! The help popup: the keys of the gallery and of the fullscreen view, and the Kitty medium.

use crate::gallery::centered;
use crate::styles::STYLES;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph},
};

/// A key and its description. An empty key makes a heading, and both empty make a blank line.
type HelpRow = (&'static str, &'static str);

const HELP_TWO_COLUMN_WIDTH: u16 = 100;

/// The width of the longest key, `Enter, double-click`.
const HELP_KEY_WIDTH: usize = 19;

const HELP_SINGLE_WIDTH: u16 = 56;

/// The left column: the gallery and its search line.
const HELP_GALLERY_ROWS: &[HelpRow] = &[
    ("", "Gallery"),
    ("h/j/k/l, click", "Move in the grid"),
    ("g / G, Home / End", "First / last"),
    ("Enter, double-click", "Open image or folder"),
    ("Backspace", "Parent folder"),
    ("+ / -, Shift+wheel", "Larger / smaller grid"),
    ("0", "Reset thumbnail size"),
    ("Wheel", "Scroll the grid"),
    ("PgDn/PgUp, ^f/^b", "Page down / up"),
    ("n", "Toggle filename labels"),
    ("i / right click", "Toggle info panel"),
    ("s", "Slideshow from cursor"),
    #[cfg(feature = "video")]
    ("p", "Open in the mpv window"),
    ("Space", "Toggle selection"),
    ("a / A", "Select all / clear all"),
    ("d / D", "Trash / delete for good"),
    ("/", "Search"),
    ("?", "Toggle help"),
    ("q", "Quit"),
    ("", ""),
    ("", "Search"),
    ("<type>, Backspace", "Filter by filename"),
    ("Enter / Esc", "Confirm / cancel"),
];

/// The right column: the fullscreen view, the video player and the mouse.
const HELP_FULLSCREEN_ROWS: &[HelpRow] = &[
    ("", "Fullscreen"),
    ("\u{2190} / \u{2192}, h j / k l", "Previous / next image"),
    ("Edge click", "Previous / next image"),
    ("Home / End", "First / last"),
    ("+ / -, wheel", "Zoom in / out"),
    ("0", "Reset zoom to fit"),
    #[cfg(feature = "video")]
    ("Enter", "Play a video or an animation"),
    ("h/j/k/l, drag", "Pan when zoomed"),
    ("i / right click", "Toggle info panel"),
    ("s", "Start / stop slideshow"),
    #[cfg(feature = "video")]
    ("p", "Open in the mpv window"),
    ("Space", "Pause / resume slideshow"),
    ("[ / ]", "Shorter / longer interval"),
    ("d / D", "Trash / delete for good"),
    ("q / Esc", "Back to gallery"),
    ("?", "Toggle help"),
    #[cfg(feature = "video")]
    ("", ""),
    #[cfg(feature = "video")]
    ("", "Video player (no sound)"),
    #[cfg(feature = "video")]
    ("\u{2190}/\u{2192}, h j / k l", "Previous / next file"),
    #[cfg(feature = "video")]
    ("Home / End", "First / last file"),
    #[cfg(feature = "video")]
    ("n / N", "Next / previous file"),
    #[cfg(feature = "video")]
    ("p", "Open in mpv, with sound"),
    #[cfg(feature = "video")]
    ("q / Esc", "Back"),
    #[cfg(feature = "video")]
    ("Right click", "Back"),
    ("", ""),
    ("", "Mouse"),
    ("Shift+drag", "Select terminal text"),
];

fn help_lines<'a>(rows: impl IntoIterator<Item = &'a HelpRow>) -> Vec<Line<'static>> {
    rows.into_iter()
        .map(|&(key, description)| {
            if key.is_empty() {
                Line::from(Span::styled(format!(" {description}"), STYLES.title))
            } else {
                Line::from(vec![
                    Span::styled(format!(" {key:>HELP_KEY_WIDTH$}  "), STYLES.key),
                    Span::styled(description, STYLES.text),
                ])
            }
        })
        .collect()
}

/// The help popup. From 100 columns on it has two columns and fits into 40 rows. A narrower
/// terminal gets one column that the popup clips. The last row names the Kitty medium.
pub(super) fn draw_help_popup(
    frame: &mut Frame<'_>,
    transfer: crate::encoder::Transfer,
    loops: crate::encoder::Loops,
) {
    let area = frame.area();
    let two_columns = area.width >= HELP_TWO_COLUMN_WIDTH;
    let (left, right) = (HELP_GALLERY_ROWS, HELP_FULLSCREEN_ROWS);
    let content_rows = if two_columns {
        left.len().max(right.len())
    } else {
        left.len() + 1 + right.len()
    };

    let height = u16::try_from(content_rows + 4)
        .unwrap_or(u16::MAX)
        .min(area.height.saturating_sub(2));
    let width = if two_columns {
        area.width.saturating_sub(4).min(110)
    } else {
        HELP_SINGLE_WIDTH.min(area.width.saturating_sub(4))
    };
    let popup_area = centered(area, width, height);

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(STYLES.popup_border)
        .title(Line::from(Span::styled(" Help ", STYLES.title)));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);
    if two_columns {
        let columns = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(chunks[0]);
        frame.render_widget(Paragraph::new(help_lines(left)), columns[0]);
        frame.render_widget(Paragraph::new(help_lines(right)), columns[1]);
    } else {
        let single = left.iter().chain(&[("", "")]).chain(right);
        frame.render_widget(Paragraph::new(help_lines(single)), chunks[0]);
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(
                " press any key to close \u{b7} Kitty transfer: {} \u{b7} loops: {}",
                crate::encoder::kitty::describe(transfer),
                crate::encoder::kitty::describe_loops(loops)
            ),
            STYLES.dim,
        ))),
        chunks[1],
    );
}

#[cfg(test)]
mod tests {
    use super::HELP_TWO_COLUMN_WIDTH;
    use crate::ui::tests::{
        app_with, assert_ansi, assert_default_fg, assert_shows, assert_terminal_background,
        cell_of, render, render_buffer, screen_rows,
    };
    use ratatui::style::{Color, Modifier};

    #[test]
    fn lists_keys_in_two_columns() {
        let mut app = app_with(&["a.jpg"]);
        app.help_visible = true;
        let screen = render(&mut app, None, 120, 52);
        assert_shows(&screen, " Help ");
        assert_shows(&screen, "h/j/k/l, click  Move in the grid");
        assert_shows(&screen, "Open image or folder");
        assert_shows(&screen, "Backspace  Parent folder");
        assert_shows(&screen, "Trash / delete for good");
        assert_shows(&screen, " Fullscreen");
        assert_shows(&screen, "Pan when zoomed");
        assert_shows(&screen, "Shorter / longer interval");
        assert_shows(&screen, " Mouse");
        assert_shows(&screen, "Shift+drag  Select terminal text");
        assert_shows(
            &screen,
            " press any key to close \u{b7} Kitty transfer: direct (t=d) \u{b7} loops: ticks (a=p)",
        );
        #[cfg(feature = "video")]
        {
            assert_shows(&screen, " Video player (no sound)");
            assert_shows(&screen, "Previous / next file");
        }

        let rows = screen_rows(&render_buffer(&mut app, None, 120, 52));
        let top = rows.iter().position(|row| row.contains(" Help ")).unwrap();
        let bottom = rows
            .iter()
            .position(|row| row.contains("press any key to close"))
            .unwrap();
        let height = bottom - top + 2;
        assert!(height <= 40, "the popup takes {height} rows");
        let gallery_row = rows
            .iter()
            .position(|row| row.contains(" Gallery"))
            .unwrap();
        assert_eq!(
            rows.iter().position(|row| row.contains(" Fullscreen")),
            Some(gallery_row),
            "the two columns start in the same row"
        );

        let buffer = render_buffer(&mut app, None, 120, 52);
        assert_terminal_background(&buffer);
        assert_default_fg(cell_of(&buffer, "Help "), Modifier::BOLD);
        assert_default_fg(cell_of(&buffer, "Gallery   "), Modifier::BOLD);
        assert_ansi(cell_of(&buffer, "h/j/k/l"), Color::Cyan);
        assert_default_fg(cell_of(&buffer, "Move in the grid"), Modifier::empty());
        assert_ansi(cell_of(&buffer, "press any key"), Color::DarkGray);
    }

    #[test]
    fn narrow_terminal_gets_one_column() {
        let mut app = app_with(&["a.jpg"]);
        app.help_visible = true;
        let rows = screen_rows(&render_buffer(&mut app, None, 80, 120));
        let gallery_row = rows
            .iter()
            .position(|row| row.contains(" Gallery"))
            .unwrap();
        let fullscreen_row = rows
            .iter()
            .position(|row| row.contains(" Fullscreen"))
            .unwrap();
        assert!(
            fullscreen_row > gallery_row + 20,
            "one column, one below the other"
        );
        let screen = rows.join("\n");
        assert_shows(&screen, "Move in the grid");
        assert_shows(&screen, "Pan when zoomed");
        assert_shows(&screen, " Mouse");
    }

    #[test]
    fn clips_in_small_terminal() {
        let mut app = app_with(&["a.jpg"]);
        app.help_visible = true;
        let rows = screen_rows(&render_buffer(&mut app, None, 80, 30));
        let top = rows.iter().position(|row| row.contains(" Help ")).unwrap();
        let bottom = rows
            .iter()
            .position(|row| row.contains("press any key to close"))
            .unwrap();
        assert!(top < bottom && bottom < 30);
        assert_shows(&rows.join("\n"), "Move in the grid");
        assert!(
            !rows.iter().any(|row| row.contains(" Mouse")),
            "the end of the one column is clipped"
        );
    }

    #[test]
    fn two_columns_from_100_columns() {
        let mut app = app_with(&["a.jpg"]);
        app.help_visible = true;
        let mut two_columns = |width| {
            let rows = screen_rows(&render_buffer(&mut app, None, width, 120));
            let gallery = rows.iter().position(|row| row.contains(" Gallery"));
            let fullscreen = rows.iter().position(|row| row.contains(" Fullscreen"));
            assert!(gallery.is_some() && fullscreen.is_some(), "{width} columns");
            (gallery == fullscreen, rows.join("\n"))
        };
        let (two, screen) = two_columns(HELP_TWO_COLUMN_WIDTH);
        assert!(two, "{HELP_TWO_COLUMN_WIDTH} columns");
        assert_shows(&screen, "Pause / resume slideshow");
        assert!(
            !two_columns(HELP_TWO_COLUMN_WIDTH - 1).0,
            "one column below"
        );
    }
}
