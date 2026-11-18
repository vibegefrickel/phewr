//! The status row of each view: a question, an error or the position at the left, and the file
//! facts at the right.

use super::STYLES;
use crate::app::{App, ViewMode};
use ratatui::{prelude::*, widgets::Paragraph};
use std::path::Path;
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
pub(super) fn fullscreen_status_line(app: &App, status: Option<&str>, width: u16) -> Line<'static> {
    if let Some(line) = error_line(app) {
        return with_mode(app, line, width);
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
    with_folder(app, Line::from(spans), width)
}

/// The badge at the bottom right of a slideshow: the picture stays visible around it, because the
/// picture sits below the cell backgrounds.
pub(super) fn draw_slideshow_badge(
    frame: &mut Frame<'_>,
    app: &App,
    area: Rect,
    status: Option<&str>,
) {
    let Some(line) = slideshow_badge(app, status) else {
        return;
    };
    let width = u16::try_from(line.width())
        .unwrap_or(u16::MAX)
        .min(area.width);
    if width == 0 || area.height == 0 {
        return;
    }
    let badge = Rect {
        x: area.right() - width,
        y: area.bottom() - 1,
        width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(line), badge);
}

/// An error, or the note and the state of the slideshow: the play or pause glyph and the interval.
fn slideshow_badge(app: &App, status: Option<&str>) -> Option<Line<'static>> {
    let show = app.slideshow.as_ref()?;
    if let Some(err) = app
        .error
        .as_deref()
        .or(app.scan_error.as_deref())
        .or_else(|| video_error(app))
    {
        return Some(Line::from(Span::styled(
            format!(" Error: {err} "),
            STYLES.badge_error,
        )));
    }
    let glyph = if show.paused { "\u{23f8}" } else { "\u{25b6}" };
    let note = status.map(|note| format!(" {note} ")).unwrap_or_default();
    Some(Line::from(Span::styled(
        format!("{note} {glyph} {:.1}s ", show.interval.as_secs_f64()),
        STYLES.badge,
    )))
}

#[cfg(feature = "video")]
fn video_error(app: &App) -> Option<&str> {
    app.video.as_ref().and_then(|v| v.error.as_deref())
}

#[cfg(not(feature = "video"))]
fn video_error(_app: &App) -> Option<&str> {
    None
}

#[cfg(feature = "video")]
const PLAYING_GLYPH: &str = "\u{25b6}";
/// The glyph at the end of a clip that does not loop. The player has no pause.
#[cfg(feature = "video")]
const FINISHED_GLYPH: &str = "\u{23f8}";

/// An error, or the state, the name, the time, the position and the note.
#[cfg(feature = "video")]
pub(super) fn video_status_line(app: &App, status: Option<&str>, width: u16) -> Line<'static> {
    let error = app
        .error
        .as_deref()
        .or_else(|| app.video.as_ref().and_then(|v| v.error.as_deref()));
    if let Some(err) = error {
        let line = Line::from(Span::styled(format!(" Error: {err} "), STYLES.error));
        return with_mode(app, line, width);
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
    let line = Line::from(vec![
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
    ]);
    with_folder(app, line, width)
}

/// An error, the search query while typing, or the name, the position, the
/// counts, the selection, the scan and the note.
pub(super) fn gallery_status_line(
    app: &App,
    filtered_count: usize,
    status: Option<&str>,
    width: u16,
) -> Line<'static> {
    if let Some(line) = error_line(app) {
        return with_mode(app, line, width);
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
        return with_folder(app, Line::from(spans), width);
    }
    if let Some(status) = status {
        spans.push(Span::styled(format!(" {status} "), STYLES.text));
    } else if is_empty_folder(app) {
        spans.push(Span::styled(" No images or folders here ", STYLES.text));
    }
    with_folder(app, Line::from(spans), width)
}

/// `rest` behind the folder path, so the path is the first thing on the row.
fn with_folder(app: &App, rest: Line<'static>, width: u16) -> Line<'static> {
    let Some(dir) = app.current_dir.as_deref().filter(|_| !app.file_list) else {
        return with_mode(app, rest, width);
    };
    let block = mode_span(app, width);
    let text = display_path(dir, app.home_dir.as_deref());
    // The path span adds one space at each side.
    let budget = usize::from(width).saturating_sub(block.width() + rest.width() + 2);
    let Some(text) = shorten_path(&text, budget) else {
        return with_mode(app, rest, width);
    };
    let mut spans = vec![block, Span::styled(format!(" {text} "), STYLES.dim)];
    spans.extend(rest.spans);
    Line::from(spans)
}

/// `rest` behind the mode block.
fn with_mode(app: &App, rest: Line<'static>, width: u16) -> Line<'static> {
    let mut spans = vec![mode_span(app, width)];
    spans.extend(rest.spans);
    Line::from(spans)
}

/// Below this many columns the mode block shows one letter.
const MODE_SHORT_WIDTH: u16 = 60;

/// The block of the mode that the status row shows: the help overlay, the search line, the grid
/// or the fullscreen view and the player.
fn mode_span(app: &App, width: u16) -> Span<'static> {
    let (name, short_name, color) = if app.help_visible {
        ("HELP", "?", STYLES.mode_help)
    } else if app.mode != ViewMode::Gallery {
        ("FULLSCREEN", "F", STYLES.mode_fullscreen)
    } else if app.gallery.search_active {
        ("SEARCH", "/", STYLES.mode_search)
    } else {
        ("NORMAL", "N", STYLES.mode_normal)
    };
    mode_block(name, short_name, color, width)
}

/// `short_name` on a screen narrower than `MODE_SHORT_WIDTH`, else `name`.
fn mode_text<'a>(name: &'a str, short_name: &'a str, width: u16) -> &'a str {
    if width < MODE_SHORT_WIDTH {
        short_name
    } else {
        name
    }
}

/// A block with the mode name in reversed `color`. A screen narrower than `MODE_SHORT_WIDTH`
/// gets `short_name`.
fn mode_block(name: &str, short_name: &str, color: Color, width: u16) -> Span<'static> {
    Span::styled(
        format!(" {} ", mode_text(name, short_name, width)),
        Style::default()
            .fg(color)
            .bg(Color::Reset)
            .add_modifier(Modifier::REVERSED | Modifier::BOLD),
    )
}

/// The box SLIDESHOW at the bottom left of a slideshow, on top of the picture.
pub(super) fn draw_slideshow_mode(frame: &mut Frame<'_>, app: &App, area: Rect) {
    if app.slideshow.is_none() || area.height == 0 {
        return;
    }
    // The look of the badge at the bottom right.
    let block = Span::styled(
        format!(" {} ", mode_text("SLIDESHOW", "S", area.width)),
        STYLES.badge.add_modifier(Modifier::BOLD),
    );
    let width = u16::try_from(block.width())
        .unwrap_or(u16::MAX)
        .min(area.width);
    let rect = Rect {
        x: area.x,
        y: area.bottom() - 1,
        width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(block), rect);
}

/// `path` with the home folder as `~`. A path outside the home folder stays as it is.
fn display_path(path: &Path, home: Option<&Path>) -> String {
    let relative = home
        .filter(|home| home.parent().is_some())
        .and_then(|home| path.strip_prefix(home).ok());
    match relative {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// `text` when it fits in `budget` columns, else its tail behind `…/`. The last folder stays
/// whole. `None` when not even that fits.
fn shorten_path(text: &str, budget: usize) -> Option<String> {
    if text.width() <= budget {
        return Some(text.to_owned());
    }
    let parts: Vec<&str> = text.split('/').collect();
    (1..parts.len())
        .map(|start| format!("\u{2026}/{}", parts[start..].join("/")))
        .find(|candidate| candidate.width() <= budget)
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
