//! The status row of each view: a question, an error or the position at the left, and the file
//! facts at the right.

use super::STYLES;
use crate::app::App;
use ratatui::{prelude::*, widgets::Paragraph};
use unicode_width::UnicodeWidthStr;

/// `line` at the left, the file facts at the right. A note, a question, an error or a long line
/// hides the facts.
pub(super) fn draw_status_row(
    frame: &mut Frame<'_>,
    app: &App,
    area: Rect,
    line: Line<'static>,
    status: Option<&str>,
) {
    // A slideshow has no status row.
    if area.height == 0 {
        return;
    }
    let busy = status.is_some()
        || app.error.is_some()
        || app.scan_error.is_some()
        || app.gallery.search_active;
    let facts = (!busy).then(|| app.meta_line()).flatten();
    let left_width = line.width();
    frame.render_widget(Paragraph::new(line), area);
    let Some(facts) = facts else {
        return;
    };
    let facts = format!(" {facts} ");
    if !facts_fit(left_width, &facts, area.width) {
        return;
    }
    frame.render_widget(
        Paragraph::new(Span::styled(facts, STYLES.dim)).alignment(Alignment::Right),
        area,
    );
}

/// Both sides count display columns, so a wide character in the facts cannot overlap the left part.
fn facts_fit(left_width: usize, facts: &str, area_width: u16) -> bool {
    left_width + facts.width() <= usize::from(area_width)
}

/// An error, or the name, the position, the zoom and the note. A slideshow has no status row.
pub(super) fn fullscreen_status_line(app: &App, status: Option<&str>) -> Line<'static> {
    if let Some(line) = error_line(app) {
        return line;
    }
    let filename = app
        .current_path()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let zoom = if app.zoom > 1.0 {
        format!(" {:.0}%", app.zoom * 100.0)
    } else {
        String::new()
    };
    let mut spans = vec![Span::styled(
        format!(
            " {} [{}/{}]{zoom} ",
            filename,
            app.current + 1,
            app.images.len(),
        ),
        STYLES.text,
    )];
    spans.extend(status.map(|note| Span::styled(format!(" {note} "), STYLES.text)));
    Line::from(spans)
}

#[cfg(feature = "video")]
const PLAYING_GLYPH: &str = "\u{25b6}";
/// The glyph at the end of a clip that does not loop. The player has no pause.
#[cfg(feature = "video")]
const FINISHED_GLYPH: &str = "\u{23f8}";

/// An error, or the state, the name, the time, the position and the note.
#[cfg(feature = "video")]
pub(super) fn video_status_line(app: &App, status: Option<&str>) -> Line<'static> {
    let error = app
        .error
        .as_deref()
        .or_else(|| app.video.as_ref().and_then(|v| v.error.as_deref()));
    if let Some(err) = error {
        return Line::from(Span::styled(format!(" Error: {err} "), STYLES.error));
    }
    let filename = app
        .current_path()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (glyph, position, duration) = match &app.video {
        Some(v) => {
            let glyph = if v.playing() {
                PLAYING_GLYPH
            } else {
                FINISHED_GLYPH
            };
            (glyph, v.position(), v.duration())
        }
        None => (PLAYING_GLYPH, 0.0, 0.0),
    };
    // A debug build shows the dropped frames of a slow terminal.
    let dropped = app.video.as_ref().map_or(0, |v| v.dropped);
    let drops = if cfg!(debug_assertions) && dropped > 0 {
        format!("[dropped {dropped}] ")
    } else {
        String::new()
    };
    Line::from(vec![
        Span::styled(
            format!(
                " {} {} {}/{} [{}/{}] {}",
                glyph,
                filename,
                crate::file_meta::format_time(position),
                crate::file_meta::format_time(duration),
                app.current + 1,
                app.images.len(),
                drops,
            ),
            STYLES.text,
        ),
        status.map_or_else(Span::default, |note| {
            Span::styled(format!(" {note} "), STYLES.text)
        }),
    ])
}

/// An error, the search query while typing, or the name, the position, the
/// counts, the selection, the scan and the note.
pub(super) fn gallery_status_line(
    app: &App,
    filtered_count: usize,
    status: Option<&str>,
) -> Line<'static> {
    if let Some(line) = error_line(app) {
        return line;
    }

    let mut spans = if app.gallery.search_active {
        vec![
            Span::styled(
                format!(" /{}\u{2588}", app.gallery.search_query),
                STYLES.text,
            ),
            Span::styled(
                format!("  {filtered_count}/{} matches ", app.images.len()),
                STYLES.dim,
            ),
        ]
    } else {
        selected_position(app, filtered_count)
    };
    if app.folder_count > 0 && !app.gallery.search_active {
        spans.push(Span::styled(entry_counts(app), STYLES.dim));
    }
    if !app.selection.is_empty() {
        spans.push(Span::styled(
            format!(" {} selected ", app.selection.len()),
            STYLES.marked,
        ));
    }
    if app.is_scanning() {
        spans.push(Span::styled(" scanning\u{2026} ", STYLES.dim));
    }
    if app.gallery.search_active {
        return Line::from(spans);
    }
    if let Some(status) = status {
        spans.push(Span::styled(format!(" {status} "), STYLES.text));
    } else if is_empty_folder(app) {
        spans.push(Span::styled(" No images or folders here ", STYLES.text));
    }
    Line::from(spans)
}

/// The error or the scan error, in this order.
fn error_line(app: &App) -> Option<Line<'static>> {
    let err = app.error.as_ref().or(app.scan_error.as_ref())?;
    Some(Line::from(Span::styled(
        format!(" Error: {err} "),
        STYLES.error,
    )))
}

/// Empty when the filter matches nothing.
fn selected_position(app: &App, filtered_count: usize) -> Vec<Span<'static>> {
    let Some(index) = app.gallery.selected_index() else {
        return Vec::new();
    };
    let name = &app.filenames[index];
    vec![Span::styled(
        format!(" {name} [{}/{filtered_count}] ", app.gallery.cursor + 1),
        STYLES.text,
    )]
}

/// The folder cells without `..`.
fn subfolder_count(app: &App) -> usize {
    app.folder_count.saturating_sub(usize::from(
        app.filenames.first().is_some_and(|name| name == ".."),
    ))
}

/// A finished scan without a file and without a subfolder.
fn is_empty_folder(app: &App) -> bool {
    !app.is_scanning() && app.file_count() == 0 && subfolder_count(app) == 0
}

/// The folder and the image count. `..` does not count.
fn entry_counts(app: &App) -> String {
    let folders = subfolder_count(app);
    let folder_word = if folders == 1 { "folder" } else { "folders" };
    let images = app.file_count();
    let image_word = if images == 1 { "image" } else { "images" };
    format!(" {folders} {folder_word}, {images} {image_word} ")
}

#[cfg(test)]
mod tests;
