//! The status row of each view, as the bar of the tuilibr frame: the mode block, the folder path,
//! the name and the position in the middle, and the file facts at the right.

use super::STYLES;
use crate::app::{App, ViewMode};
use ratatui::{prelude::*, widgets::Paragraph};
use std::path::Path;
use tuilibr::frame::{BarText, BarView, ModeView, SEPARATOR, draw_bar, split::NARROW_BELOW};
use unicode_width::UnicodeWidthStr;

/// The middle texts and the right groups of one view.
pub(super) struct BarParts {
    /// The texts in the middle, the most important first.
    pub middle: Vec<BarText>,
    /// The groups at the right. The first group goes first when the row is too narrow.
    pub right: Vec<String>,
}

/// The bar of a view: the mode block, the folder path and `parts`. phewr shows no clock and no
/// icons. A slideshow has no status row.
pub(super) fn draw_status_row(frame: &mut Frame<'_>, app: &App, area: Rect, parts: BarParts) {
    if area.height == 0 {
        return;
    }
    let mode = mode_view(app);
    let left = folder_span(app, &mode, &parts.middle, area.width);
    let view = BarView {
        mode,
        left: left.into_iter().collect(),
        middle: parts.middle,
        right: parts.right,
        time: String::new(),
        icons: Vec::new(),
    };
    // The folder path takes no click, so the places of the left group do not matter.
    draw_bar(frame, view, area, 0);
}

/// One text of the middle in one style.
fn text(part: impl Into<String>, style: Style) -> BarText {
    BarText {
        parts: vec![part.into()],
        style,
        keep_end: false,
    }
}

/// The file facts as the right group. The note, the error and the search line sit in the
/// omnibox, so only a narrow row hides the facts.
fn facts(app: &App) -> Vec<String> {
    app.meta_line().into_iter().collect()
}

/// The name of the current file.
fn file_name(app: &App) -> String {
    app.current_path()
        .file_name()
        .map_or_default(|n| n.to_string_lossy().into_owned())
}

/// The name, the position and the zoom. A slideshow has no status row.
pub(super) fn fullscreen_bar(app: &App) -> BarParts {
    let mut parts = vec![format!(
        "{} [{}/{}]",
        file_name(app),
        app.current + 1,
        app.images.len()
    )];
    // The zoom goes before the name when the row is too narrow.
    if app.zoom > 1.0 {
        parts.push(format!("{:.0}%", app.zoom * 100.0));
    }
    BarParts {
        middle: vec![BarText {
            parts,
            style: STYLES.text,
            keep_end: false,
        }],
        right: facts(app),
    }
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
    let note = status.map_or_default(|note| format!(" {note} "));
    Some(Line::from(Span::styled(
        format!("{note} {glyph} {:.1}s ", show.interval.as_secs_f64()),
        STYLES.badge,
    )))
}

#[cfg(feature = "video")]
pub(super) fn video_error(app: &App) -> Option<&str> {
    app.video.as_ref().and_then(|v| v.error.as_deref())
}

#[cfg(not(feature = "video"))]
pub(super) const fn video_error(_app: &App) -> Option<&str> {
    None
}

#[cfg(feature = "video")]
const PLAYING_GLYPH: &str = "\u{25b6}";
/// The glyph at the end of a clip that does not loop. The player has no pause.
#[cfg(feature = "video")]
const FINISHED_GLYPH: &str = "\u{23f8}";

/// The state, the name, the time and the position.
#[cfg(feature = "video")]
pub(super) fn video_bar(app: &App) -> BarParts {
    use crate::file_meta::whole_seconds;
    use tuilibr::time::{LengthStyle, media_length};
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
    let mut parts = vec![format!(
        "{glyph} {} {}/{} [{}/{}]",
        file_name(app),
        media_length(whole_seconds(position), LengthStyle::Padded),
        media_length(whole_seconds(duration), LengthStyle::Padded),
        app.current + 1,
        app.images.len(),
    )];
    // A debug build shows the dropped frames of a slow terminal.
    let dropped = app.video.as_ref().map_or(0, |v| v.dropped);
    if cfg!(debug_assertions) && dropped > 0 {
        parts.push(format!("[dropped {dropped}]"));
    }
    BarParts {
        middle: vec![BarText {
            parts,
            style: STYLES.text,
            keep_end: false,
        }],
        right: facts(app),
    }
}

/// The name, the position, the selection and the scan in the middle. The facts and the entry
/// counts sit at the right. The search line sits in the omnibox.
pub(super) fn gallery_bar(app: &App, filtered_count: usize) -> BarParts {
    let mut right = facts(app);
    if app.folder_count > 0 {
        right.push(entry_counts(app));
    }
    let mut middle: Vec<_> = selected_position(app, filtered_count).into_iter().collect();
    if !app.selection.is_empty() {
        middle.push(text(
            format!("{} selected", app.selection.len()),
            STYLES.marked,
        ));
    }
    if app.is_scanning() {
        middle.push(text("scanning\u{2026}", STYLES.dim));
    }
    if is_empty_folder(app) {
        middle.push(text("No images or folders here", STYLES.text));
    }
    BarParts { middle, right }
}

/// The folder path as the left group, with the home folder as `~`. The frame leaves out a left
/// part that does not fit, so `shorten_path` cuts the path to the room that the mode block and
/// the whole middle leave, from its start. A file list shows no path.
fn folder_span(
    app: &App,
    mode: &ModeView,
    middle: &[BarText],
    width: u16,
) -> Option<Span<'static>> {
    // The title of the main box shows the path while the boxes show.
    if super::boxed(app) {
        return None;
    }
    let dir = app.current_dir.as_deref().filter(|_| !app.file_list)?;
    let name = if width < NARROW_BELOW {
        &mode.short_name
    } else {
        &mode.name
    };
    let middle_width: usize = middle
        .iter()
        .map(|text| text.parts.join(SEPARATOR).width())
        .filter(|width| *width > 0)
        .map(|width| width + 2)
        .sum();
    // The mode block and the path add one space at each side.
    let budget = usize::from(width).saturating_sub(name.width() + 2 + middle_width + 2);
    let path = shorten_path(&display_path(dir, app.home_dir.as_deref()), budget)?;
    Some(Span::styled(format!(" {path} "), STYLES.dim))
}

/// The mode block of the view: the help overlay, the fullscreen view and the player, the search
/// line or the grid.
fn mode_view(app: &App) -> ModeView {
    let (name, short_name, color) = if app.help_visible {
        ("HELP", "?", STYLES.mode_help)
    } else if app.mode != ViewMode::Gallery {
        ("FULLSCREEN", "F", STYLES.mode_fullscreen)
    } else if app.gallery.search_active {
        ("SEARCH", "/", STYLES.mode_search)
    } else {
        ("NORMAL", "N", STYLES.mode_normal)
    };
    ModeView {
        name: name.to_owned(),
        short_name: short_name.to_owned(),
        color,
    }
}

/// The box SLIDESHOW at the bottom left of a slideshow, on top of the picture.
pub(super) fn draw_slideshow_mode(frame: &mut Frame<'_>, app: &App, area: Rect) {
    if app.slideshow.is_none() || area.height == 0 {
        return;
    }
    // The look of the badge at the bottom right, and one letter where the mode block has one.
    let name = if area.width < NARROW_BELOW {
        "S"
    } else {
        "SLIDESHOW"
    };
    let block = Span::styled(
        format!(" {name} "),
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
pub(super) fn display_path(path: &Path, home: Option<&Path>) -> String {
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
pub(super) fn shorten_path(text: &str, budget: usize) -> Option<String> {
    if text.width() <= budget {
        return Some(text.to_owned());
    }
    let parts: Vec<&str> = text.split('/').collect();
    (1..parts.len())
        .map(|start| format!("\u{2026}/{}", parts[start..].join("/")))
        .find(|candidate| candidate.width() <= budget)
}

/// The name and the position of the cursor. `None` when the filter matches nothing.
fn selected_position(app: &App, filtered_count: usize) -> Option<BarText> {
    let index = app.gallery.selected_index()?;
    let name = &app.filenames[index];
    Some(text(
        format!("{name} [{}/{filtered_count}]", app.gallery.cursor + 1),
        STYLES.text,
    ))
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
    format!("{folders} {folder_word}, {images} {image_word}")
}

#[cfg(test)]
mod tests;
