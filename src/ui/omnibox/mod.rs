//! The omnibox: the row under the status bar, as in every app of the tuilibr frame. It shows one
//! thing at a time, in this order: the search line while it takes keys, an error, the status
//! note. Else the row stays empty, so a note never moves the panes and their pictures. A
//! slideshow has no omnibox.

use super::status::video_error;
use crate::app::{App, ViewMode};
use ratatui::{
    Frame,
    layout::{Position, Rect},
    text::Span,
};
use std::borrow::Cow;
use tuilibr::frame::{self, OmniboxView, Tone};

/// The rows of the omnibox under the bar.
pub(super) const OMNIBOX_ROWS: u16 = 1;

/// The prompt of the search line.
const SEARCH_PROMPT: &str = "/";

/// The one thing that the omnibox shows.
#[derive(Debug, PartialEq, Eq)]
enum Shown<'a> {
    /// The search line with the count of the matches at its right.
    Search { matches: String },
    /// An error in red or the status note in gray.
    Status { text: Cow<'a, str>, tone: Tone },
}

/// What the omnibox shows for `app` and the note of the run loop: the search line, the error,
/// the note, or nothing.
fn shown<'a>(app: &'a App, note: Option<&'a str>) -> Option<Shown<'a>> {
    if app.mode == ViewMode::Gallery && app.gallery.search_active {
        let matches = format!(
            "{}/{} matches",
            app.gallery.filtered_indices.len(),
            app.images.len()
        );
        return Some(Shown::Search { matches });
    }
    if let Some(error) = error(app) {
        return Some(Shown::Status {
            text: Cow::Owned(format!("Error: {error}")),
            tone: Tone::Alert,
        });
    }
    note.map(|note| Shown::Status {
        text: Cow::Borrowed(note),
        tone: Tone::Note,
    })
}

/// The error of the view: `App::error` first, then the video error in the player or the scan
/// error in the other views.
fn error(app: &App) -> Option<&str> {
    let of_view = if app.mode == ViewMode::Gallery || app.mode == ViewMode::Fullscreen {
        app.scan_error.as_deref()
    } else {
        video_error(app)
    };
    app.error.as_deref().or(of_view)
}

/// Draws the omnibox into `area`. Returns the cell of the terminal cursor while the search line
/// takes keys. No note ends with a path or a file name, so every status text keeps its start.
pub(super) fn draw_omnibox(
    frame: &mut Frame<'_>,
    app: &App,
    area: Rect,
    note: Option<&str>,
) -> Option<Position> {
    if area.height == 0 {
        return None;
    }
    match shown(app, note)? {
        Shown::Search { matches } => frame::draw_omnibox(
            frame,
            area,
            &OmniboxView::Typed {
                prompt: Span::raw(SEARCH_PROMPT),
                field: &app.gallery.search_query,
                right: Some(Span::raw(matches)),
                cursor: true,
            },
        ),
        Shown::Status { text, tone } => frame::draw_omnibox(
            frame,
            area,
            &OmniboxView::Status {
                text: &text,
                tone,
                keep_end: false,
            },
        ),
    }
}

#[cfg(test)]
mod tests;
