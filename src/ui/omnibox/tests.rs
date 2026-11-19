use super::{Shown, shown};
use crate::test_support::{char_key, draw_frame, fullscreen_app, gallery_app, press};
use crate::ui::tests::{assert_hides, cell_of, omnibox_row, render, render_buffer, status_row};
use crossterm::event::KeyCode;
use ratatui::layout::{Position, Rect};
use std::borrow::Cow;
use tuilibr::frame::{self, Tone};

/// A status text of the omnibox.
fn status(text: &str, tone: Tone) -> Shown<'_> {
    Shown::Status {
        text: Cow::Borrowed(text),
        tone,
    }
}

#[test]
fn shows_one_thing_in_order() {
    let mut app = gallery_app(3);
    assert_eq!(shown(&app, None), None, "nothing to show");
    assert_eq!(
        shown(&app, Some("a note")),
        Some(status("a note", Tone::Note))
    );

    app.scan_error = Some("denied".to_owned());
    assert_eq!(
        shown(&app, Some("a note")),
        Some(status("Error: denied", Tone::Alert)),
        "an error beats the note"
    );
    app.error = Some("broken".to_owned());
    assert_eq!(
        shown(&app, None),
        Some(status("Error: broken", Tone::Alert)),
        "the error of the app beats the scan error"
    );

    app.gallery.search_active = true;
    assert_eq!(
        shown(&app, Some("a note")),
        Some(Shown::Search {
            matches: "3/3 matches".to_owned()
        }),
        "the search line beats the error"
    );
}

#[test]
fn search_line_shows_query_and_matches() {
    let mut app = gallery_app(3);
    char_key(&mut app, '/');
    char_key(&mut app, 'b');
    app.gallery.filtered_indices = vec![1];
    let screen = render(&mut app, Some("a note"), 80, 12);
    let row = omnibox_row(&screen);
    assert!(row.starts_with("/b "), "{row:?}");
    assert!(row.trim_end().ends_with("1/3 matches"), "{row:?}");
    assert_hides(&screen, "a note");
    assert!(
        !status_row(&screen).contains("/b"),
        "the bar holds no search line"
    );
}

#[test]
fn search_cursor_follows_the_field() {
    let mut app = gallery_app(3);
    char_key(&mut app, '/');
    char_key(&mut app, 'a');
    char_key(&mut app, 'b');
    assert_eq!(draw_frame(&mut app, 80, 12), Some(Position::new(3, 11)));
    press(&mut app, KeyCode::Left);
    assert_eq!(draw_frame(&mut app, 80, 12), Some(Position::new(2, 11)));
    press(&mut app, KeyCode::Home);
    assert_eq!(
        draw_frame(&mut app, 80, 12),
        Some(Position::new(1, 11)),
        "the cursor starts after the prompt"
    );
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        draw_frame(&mut app, 80, 12),
        None,
        "an applied search shows no cursor"
    );
}

#[test]
fn note_and_error_take_their_tones() {
    let mut app = gallery_app(3);
    let buffer = render_buffer(&mut app, Some("a note"), 80, 12);
    assert_eq!(cell_of(&buffer, "a note").fg, frame::note().fg.unwrap());
    assert_eq!(buffer[(0, 11)].symbol(), "a");

    app.error = Some("broken".to_owned());
    let buffer = render_buffer(&mut app, None, 80, 12);
    assert_eq!(
        cell_of(&buffer, "Error: broken").fg,
        frame::alert().fg.unwrap()
    );
    assert_eq!(buffer[(0, 11)].symbol(), "E");
}

#[test]
fn empty_omnibox_keeps_its_row() {
    let mut app = fullscreen_app(3, 0);
    let screen = render(&mut app, None, 80, 12);
    assert_eq!(omnibox_row(&screen).trim(), "");
    let note = render(&mut app, Some("a note"), 80, 12);
    assert_eq!(omnibox_row(&note).trim(), "a note");
    assert_eq!(
        app.image_rect,
        Rect::new(0, 0, 80, 10),
        "a note moves no picture"
    );
}

#[test]
fn slideshow_has_no_omnibox() {
    let mut app = fullscreen_app(3, 0);
    app.start_slideshow();
    let panes = crate::ui::panes(Rect::new(0, 0, 80, 12), &app);
    assert_eq!(panes.omnibox.height, 0);
    assert_eq!(panes.status.height, 0);
    let screen = render(&mut app, Some("a note"), 80, 12);
    assert_eq!(app.image_rect, Rect::new(0, 0, 80, 12));
    assert!(
        omnibox_row(&screen).contains(" a note  \u{25b6} "),
        "the badge keeps the note: {screen}"
    );
}
