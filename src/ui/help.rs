//! The help popup: the keys of the gallery and of the fullscreen view, and the Kitty medium. The
//! box and the columns come from the tuilibr frame.

use ratatui::Frame;
use ratatui::layout::Rect;
use tuilibr::frame::{HelpView, Section, draw_help};
use tuilibr::pictures::kitty::{
    Loops, Transfer,
    protocol::{describe, describe_loops},
};

/// The cells of the key column: the longest key, `Enter, double-click`, and one space.
const KEY_WIDTH: u16 = 20;

/// The cells of the description column: the longest ones, `Mark, yazi selects on quit` and
/// `Slideshow on / off (image)`. Both columns fit 102 screen columns side by side.
const TEXT_WIDTH: u16 = 26;

/// The left column: the gallery and its search line.
const GALLERY_COLUMN: &[Section<'static>] = &[
    (
        "Gallery",
        &[
            ("h/j/k/l, click", "Move in the grid"),
            ("g / G, Home / End", "First / last"),
            ("Enter, double-click", "Open image or folder"),
            ("Backspace", "Parent folder"),
            ("+ / -, Shift+wheel", "Grid size (Shift if sent)"),
            ("0", "Reset thumbnail size"),
            ("Wheel", "Scroll the grid"),
            ("PgDn/PgUp, ^f/^b", "Page down / up"),
            ("Tab / right click", "Toggle info pane"),
            ("B", "Minimal / maximal layout"),
            ("s", "Slideshow from cursor"),
            #[cfg(feature = "video")]
            ("p", "Open in the mpv window"),
            ("Space", "Mark, yazi selects on quit"),
            ("a / A", "Mark all / clear marks"),
            ("/", "Search"),
            ("?", "Toggle help"),
            ("q", "Quit"),
        ],
    ),
    (
        "Search",
        &[
            ("<type>, Enter / Esc", "Filter, confirm / cancel"),
            ("\u{2190}/\u{2192}, ^w/^u/^k", "Cursor, readline edits"),
        ],
    ),
];

/// The right column without the Kitty section: the fullscreen view with the video player, and
/// the mouse. The player has no sound, so the `p` row names the mpv window "with sound".
const FULLSCREEN_SECTIONS: [Section<'static>; 2] = [
    (
        "Fullscreen / video player",
        &[
            ("\u{2190} / \u{2192}, h j / k l", "Previous / next file"),
            ("Edge click", "Previous / next (image)"),
            ("Home / End", "First / last file"),
            ("+ / -, wheel", "Zoom in / out (image)"),
            ("0", "Reset zoom to fit (image)"),
            ("h/j/k/l, drag", "Pan when zoomed (image)"),
            ("Tab / right click", "Toggle info pane (image)"),
            ("B", "Minimal / maximal (image)"),
            ("s", "Slideshow on / off (image)"),
            #[cfg(feature = "video")]
            ("p", "Open in mpv, with sound"),
            ("Space", "Pause / resume slideshow"),
            ("[ / ]", "Interval -/+ (image)"),
            ("q / Esc", "Back to gallery"),
            ("?", "Toggle help"),
            #[cfg(feature = "video")]
            ("Right click", "Back (video)"),
        ],
    ),
    ("Mouse", &[("Shift+drag", "Select terminal text")]),
];

/// The help popup: two columns on a wide screen, else one column when it fits the height. The
/// last section names the Kitty medium and the loop path. The box sits in `area`, above the
/// omnibox.
pub(super) fn draw_help_popup(
    frame: &mut Frame<'_>,
    area: Rect,
    transfer: Transfer,
    loops: Option<Loops>,
) {
    let kitty = kitty_rows(transfer, loops);
    let mut right = FULLSCREEN_SECTIONS.to_vec();
    right.push(("Kitty graphics", &kitty));
    let view = HelpView {
        title: "phewr keys",
        columns: [GALLERY_COLUMN, &right],
        key_width: KEY_WIDTH,
        text_width: TEXT_WIDTH,
    };
    draw_help(frame, area, &view);
}

/// The rows of the Kitty section: the medium and the loop path.
fn kitty_rows(transfer: Transfer, loops: Option<Loops>) -> [(&'static str, &'static str); 2] {
    [
        ("Transfer", describe(transfer)),
        ("Grid loops", describe_loops(loops)),
    ]
}

#[cfg(test)]
mod tests {
    use super::{FULLSCREEN_SECTIONS, GALLERY_COLUMN, KEY_WIDTH, TEXT_WIDTH, kitty_rows};
    use crate::ui::tests::{
        app_with, assert_ansi, assert_default_fg, assert_hides, assert_shows,
        assert_terminal_background, cell_of, render, render_buffer, screen_rows,
    };
    use ratatui::style::{Color, Modifier};
    use tuilibr::frame::{Section, column_rows};
    use tuilibr::pictures::kitty::{Loops, Transfer};
    use unicode_width::UnicodeWidthStr as _;

    /// Every Kitty section that a run can show.
    fn kitty_rows_all() -> Vec<[(&'static str, &'static str); 2]> {
        let transfers = [Transfer::Direct, Transfer::Png, Transfer::TempFile];
        let loops = [None, Some(Loops::Animation), Some(Loops::Ticks)];
        transfers
            .iter()
            .flat_map(|transfer| loops.iter().map(|loops| kitty_rows(*transfer, *loops)))
            .collect()
    }

    /// The widths are fixed, so the frame cuts a longer key or description without a sign.
    #[test]
    fn every_entry_fits_its_column() {
        let rows = kitty_rows_all();
        let kitty: Vec<Section<'_>> = rows
            .iter()
            .map(|rows| ("Kitty graphics", rows.as_slice()))
            .collect();
        let sections = GALLERY_COLUMN
            .iter()
            .chain(&FULLSCREEN_SECTIONS)
            .chain(&kitty);
        for (title, keys) in sections {
            assert!(
                title.width() <= usize::from(KEY_WIDTH + TEXT_WIDTH),
                "{title}"
            );
            for (key, text) in *keys {
                assert!(key.width() < usize::from(KEY_WIDTH), "{key}");
                assert!(text.width() <= usize::from(TEXT_WIDTH), "{text}");
            }
        }
        // The border and the padding take 2 rows each, and the margin 2. The box sits above the
        // omnibox, so a screen of 30 rows gives it 29. The margin keeps the bar free.
        let right = [FULLSCREEN_SECTIONS.as_slice(), &kitty[..1]].concat();
        for column in [GALLERY_COLUMN, &right] {
            assert!(column_rows(column) + 6 <= 29, "the box fits 30 rows");
        }
    }

    #[test]
    fn kitty_rows_name_the_medium_and_the_loops() {
        let rows = kitty_rows(Transfer::TempFile, None);
        assert_eq!(
            rows,
            [("Transfer", "temp file (t=t)"), ("Grid loops", "off")]
        );
    }

    #[test]
    fn lists_keys_in_two_columns() {
        let mut app = app_with(&["a.jpg"]);
        app.help_visible = true;
        let screen = render(&mut app, None, 120, 52);
        assert_shows(&screen, "\u{2500} phewr keys ");
        assert_shows(&screen, "any key closes");
        let key =
            |key: &str, text: &str| format!("{key:<width$}{text}", width = usize::from(KEY_WIDTH));
        assert_shows(&screen, &key("h/j/k/l, click", "Move in the grid"));
        assert_shows(&screen, &key("Backspace", "Parent folder"));
        assert_shows(&screen, &key("B", "Minimal / maximal layout"));
        assert_shows(&screen, &key("Shift+drag", "Select terminal text"));
        assert_shows(&screen, &key("Transfer", "direct (t=d)"));
        assert_shows(&screen, &key("Grid loops", "ticks (a=p)"));
        assert_shows(&screen, &key("[ / ]", "Interval -/+ (image)"));
        assert_shows(
            &screen,
            &key("<type>, Enter / Esc", "Filter, confirm / cancel"),
        );
        assert_shows(
            &screen,
            &key("\u{2190}/\u{2192}, ^w/^u/^k", "Cursor, readline edits"),
        );
        assert_shows(&screen, " Fullscreen / video player");
        assert_shows(&screen, " Mouse");
        assert_hides(&screen, "Play animation");
        assert_hides(&screen, "Play the motion once");
        #[cfg(feature = "video")]
        assert_shows(&screen, "Open in mpv, with sound");

        let rows = screen_rows(&render_buffer(&mut app, None, 120, 52));
        let gallery_row = rows.iter().position(|row| row.contains(" Gallery"));
        assert!(gallery_row.is_some());
        assert_eq!(
            rows.iter().position(|row| row.contains(" Fullscreen")),
            gallery_row,
            "the two columns start in the same row"
        );

        let buffer = render_buffer(&mut app, None, 120, 52);
        assert_terminal_background(&buffer);
        let heading = cell_of(&buffer, "Gallery");
        assert_eq!(
            (heading.fg, heading.modifier),
            (Color::Yellow, Modifier::BOLD)
        );
        assert_default_fg(cell_of(&buffer, "h/j/k/l"), Modifier::BOLD);
        assert_default_fg(cell_of(&buffer, "Move in the grid"), Modifier::empty());
        assert_ansi(cell_of(&buffer, "any key closes"), Color::DarkGray);
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
        assert_shows(&screen, "\u{2500} phewr keys ");
        assert_shows(&screen, "Move in the grid");
        assert_shows(&screen, "Pan when zoomed");
        assert_shows(&screen, " Mouse");
        assert_shows(&screen, " Kitty graphics");
    }

    /// One column does not fit 30 rows, so the box keeps two columns and cuts text, not keys.
    #[test]
    fn low_narrow_terminal_keeps_two_columns() {
        let mut app = app_with(&["a.jpg"]);
        app.help_visible = true;
        let rows = screen_rows(&render_buffer(&mut app, None, 80, 30));
        let gallery = rows.iter().position(|row| row.contains(" Gallery"));
        assert!(gallery.is_some());
        assert_eq!(
            rows.iter().position(|row| row.contains(" Fullscreen")),
            gallery
        );
        let screen = rows.join("\n");
        assert_shows(&screen, "h/j/k/l, click");
        assert_shows(&screen, " Mouse");
    }

    #[test]
    fn two_columns_from_102_columns() {
        let mut app = app_with(&["a.jpg"]);
        app.help_visible = true;
        let mut two_columns = |width| {
            let rows = screen_rows(&render_buffer(&mut app, None, width, 120));
            let gallery = rows.iter().position(|row| row.contains(" Gallery"));
            let fullscreen = rows.iter().position(|row| row.contains(" Fullscreen"));
            assert!(gallery.is_some() && fullscreen.is_some(), "{width} columns");
            (gallery == fullscreen, rows.join("\n"))
        };
        let (two, screen) = two_columns(102);
        assert!(two, "102 columns");
        assert_shows(&screen, "Pause / resume slideshow");
        assert_shows(&screen, "Grid size (Shift if sent)");
        assert!(!two_columns(101).0, "one column below");
    }
}
