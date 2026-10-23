use crate::app::{App, DeleteMode, ViewMode};
use crate::gallery::LABEL_ROWS;
use crate::icons::{self, Icon};
use crate::styles::STYLES;
use ratatui::{
    prelude::*,
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};

/// Preferred width of the info panel, including its border.
const INFO_PANEL_WIDTH: u16 = 42;
/// The status row at the bottom of the gallery, the fullscreen view and the video view.
const STATUS_ROWS: u16 = 1;
/// The marker of a playable file in the grid, a video or an animated image: a play symbol
/// before the label, or in the gap below the cell when the labels are off.
const ANIMATION_MARKER: &str = "\u{25b6}";

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
        draw_help_popup(frame);
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

    let Some(path) = app.info_path() else {
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

fn draw_fullscreen(frame: &mut Frame<'_>, app: &mut App, area: Rect, status_note: Option<&str>) {
    let placeholder = app.shows_video_placeholder();

    let filename = app
        .current_path()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let bottom_line = if let Some(pending) = &app.pending_delete {
        Line::from(Span::styled(
            format!(
                " {} {}? [y/N] ",
                if pending.mode == DeleteMode::Permanent {
                    "Permanently delete"
                } else {
                    "Move to trash"
                },
                pending
                    .paths
                    .first()
                    .and_then(|p| p.file_name())
                    .map_or_else(
                        || format!("{} files", pending.paths.len()),
                        |n| n.to_string_lossy().into_owned()
                    ),
            ),
            STYLES.error,
        ))
    } else if let Some(err) = &app.delete_error {
        Line::from(Span::styled(format!(" Delete error: {err} "), STYLES.error))
    } else if let Some(err) = &app.error {
        Line::from(Span::styled(format!(" Error: {err} "), STYLES.error))
    } else {
        let zoom = if app.zoom > 1.0 {
            format!(" {:.0}%", app.zoom * 100.0)
        } else {
            String::new()
        };
        let slideshow = app
            .slideshow
            .as_ref()
            .map(|show| {
                format!(
                    " {} {:.1}s",
                    if show.paused { "\u{23f8}" } else { "\u{25b6}" },
                    show.interval.as_secs_f64()
                )
            })
            .unwrap_or_default();
        let mut spans = vec![Span::styled(
            format!(
                " {} [{}/{}]{zoom}{slideshow} ",
                filename,
                app.current + 1,
                app.images.len(),
            ),
            STYLES.text,
        )];
        spans.extend(status_note.map(|note| Span::styled(format!(" {note} "), STYLES.text)));
        Line::from(spans)
    };

    let inner = content_area(area);
    draw_status_row(frame, app, status_area(area), bottom_line, status_note);
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
fn draw_video(frame: &mut Frame<'_>, app: &mut App, area: Rect, status_note: Option<&str>) {
    let filename = app
        .current_path()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let (status, position, duration) = match &app.video {
        Some(v) => {
            let status = if v.playing() { "\u{25b6}" } else { "\u{23f8}" };
            (status, v.position(), v.duration())
        }
        None => ("\u{25b6}", 0.0, 0.0),
    };
    // The dropped frames show in a debug build only, so the maintainer sees a slow terminal.
    let dropped = app.video.as_ref().map_or(0, |v| v.dropped);
    let drops = if cfg!(debug_assertions) && dropped > 0 {
        format!("[dropped {dropped}] ")
    } else {
        String::new()
    };

    let error = app
        .error
        .as_deref()
        .or_else(|| app.video.as_ref().and_then(|v| v.error.as_deref()));
    let bottom_line = if let Some(err) = error {
        Line::from(Span::styled(format!(" Error: {err} "), STYLES.error))
    } else {
        Line::from(vec![
            Span::styled(
                format!(
                    " {} {} {}/{} [{}/{}] {}",
                    status,
                    filename,
                    crate::file_meta::format_time(position),
                    crate::file_meta::format_time(duration),
                    app.current + 1,
                    app.images.len(),
                    drops,
                ),
                STYLES.text,
            ),
            status_note.map_or_else(Span::default, |note| {
                Span::styled(format!(" {note} "), STYLES.text)
            }),
        ])
    };

    draw_status_row(frame, app, status_area(area), bottom_line, status_note);
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

/// The status row: `line` at the left, the facts of the file at the right. The facts give way
/// to a status note, a question, an error and a line too long for both.
fn draw_status_row(
    frame: &mut Frame<'_>,
    app: &App,
    area: Rect,
    line: Line<'static>,
    status: Option<&str>,
) {
    let busy = status.is_some()
        || app.error.is_some()
        || app.pending_delete.is_some()
        || app.delete_error.is_some()
        || app.gallery.search_active;
    let facts = (!busy).then(|| app.meta_line()).flatten();
    let left_width = u16::try_from(line.width()).unwrap_or(u16::MAX);
    frame.render_widget(Paragraph::new(line), area);
    let Some(facts) = facts else {
        return;
    };
    let facts = format!(" {facts} ");
    let facts_width = u16::try_from(facts.chars().count()).unwrap_or(u16::MAX);
    if left_width.saturating_add(facts_width) > area.width {
        return;
    }
    frame.render_widget(
        Paragraph::new(Span::styled(facts, STYLES.dim)).alignment(Alignment::Right),
        area,
    );
}

/// The status row of the gallery: a delete question, an error, the search query while typing,
/// or name, position, entry counts, selection count, scan note and the status note.
fn gallery_status_line(app: &App, filtered_count: usize, status: Option<&str>) -> Line<'static> {
    if let Some(pending) = &app.pending_delete {
        let action = if pending.mode == DeleteMode::Permanent {
            "Permanently delete"
        } else {
            "Move to trash"
        };
        let prompt = if pending.paths.len() == 1 {
            let name = pending.paths[0]
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            format!(" {action} {name}? [y/N] ")
        } else {
            format!(" {action} {} files? [y/N] ", pending.paths.len())
        };
        return Line::from(Span::styled(prompt, STYLES.error));
    }
    if let Some(err) = &app.delete_error {
        return Line::from(Span::styled(format!(" Delete error: {err} "), STYLES.error));
    }
    if let Some(err) = &app.error {
        return Line::from(Span::styled(format!(" Error: {err} "), STYLES.error));
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

/// The name and the position of the selected entry. Empty when the filter matches nothing.
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

/// The folder and the image count of a grid with folders. `..` does not count.
/// The folder cells without the `..` cell.
fn subfolder_count(app: &App) -> usize {
    app.folder_count.saturating_sub(usize::from(
        app.filenames.first().is_some_and(|name| name == ".."),
    ))
}

/// A finished scan that found no file and no subfolder. The `..` cell does not count.
fn is_empty_folder(app: &App) -> bool {
    !app.is_scanning() && app.file_count() == 0 && subfolder_count(app) == 0
}

fn entry_counts(app: &App) -> String {
    let folders = subfolder_count(app);
    let folder_word = if folders == 1 { "folder" } else { "folders" };
    let images = app.file_count();
    let image_word = if images == 1 { "image" } else { "images" };
    format!(" {folders} {folder_word}, {images} {image_word} ")
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

/// One cell per visible thumbnail, with an optional filename label. Kitty draws the pictures.
/// The marked cells get a plain border and the selected cell a thick border, in the gaps around
/// the cell. The selected border comes last, so it wins on a gap that two cells share.
fn draw_thumbnail_cells(frame: &mut Frame<'_>, app: &mut App, grid_rect: Rect) {
    app.gallery.update_grid(grid_rect);
    let first_visible = app.gallery.scroll_offset * app.gallery.grid_cols;
    let mut selected_border = None;

    for (vis_idx, img_idx) in app.gallery.visible_items() {
        let is_selected = vis_idx + first_visible == app.gallery.cursor;
        let is_marked = app.is_marked(img_idx);
        let border = app.gallery.border_rect(vis_idx);
        if is_selected {
            selected_border = Some(border);
        } else if is_marked {
            draw_cell_border(frame, BorderType::Plain, STYLES.marked, border, grid_rect);
        }
        let label_style = if is_selected {
            STYLES.cursor_cell
        } else if is_marked {
            STYLES.marked
        } else {
            STYLES.text
        };
        if app.is_folder(img_idx) {
            draw_folder_cell(frame, app, vis_idx, img_idx, label_style, grid_rect);
        } else if !app.thumb_cache.contains(img_idx) {
            let icon = if app.is_video_at(img_idx) {
                Icon::Video
            } else {
                Icon::Loading
            };
            draw_cell_icon(frame, app, app.gallery.cell_rect(vis_idx), icon, grid_rect);
        }
        if app.gallery.show_filenames {
            draw_label(frame, app, vis_idx, img_idx, label_style, grid_rect);
        }
    }

    if let Some(border) = selected_border {
        draw_cell_border(
            frame,
            BorderType::Thick,
            STYLES.cursor_cell,
            border,
            grid_rect,
        );
    }
    if !app.gallery.show_filenames {
        for (vis_idx, img_idx) in app.gallery.visible_items() {
            if shows_play_marker(app, img_idx) {
                draw_animation_marker(frame, app, vis_idx, grid_rect);
            }
        }
    }
}

/// True when the entry `img_idx` is playable and its thumbnail is shown. A video counts by its
/// extension and an animated image by the flag of its thumbnail job, so the draw reads no file.
fn shows_play_marker(app: &App, img_idx: usize) -> bool {
    app.is_animated_at(img_idx) || (app.is_video_at(img_idx) && app.thumb_cache.contains(img_idx))
}

/// The play symbol of a playable file, in the middle of the gap below its cell. It comes
/// after the borders, so the border of the selected cell does not cover it. Kitty draws the
/// picture only inside the cell, so the gap stays visible.
fn draw_animation_marker(frame: &mut Frame<'_>, app: &App, vis_idx: usize, grid_rect: Rect) {
    let cell = app.gallery.cell_rect(vis_idx).intersection(grid_rect);
    let marker = Position::new(cell.x + cell.width / 2, cell.y + cell.height);
    if cell.width == 0 || !grid_rect.contains(marker) {
        return;
    }
    frame
        .buffer_mut()
        .set_string(marker.x, marker.y, ANIMATION_MARKER, STYLES.dim);
}

/// A large dim icon in the center of `cell`, or the one-cell symbol when the cell is small or
/// the grid edge cuts the icon. Kitty draws a thumbnail above the text, so the icon of a cell
/// that loads needs no redraw when the thumbnail arrives. Returns the row below the icon.
fn draw_cell_icon(
    frame: &mut Frame<'_>,
    app: &App,
    cell: Rect,
    icon: Icon,
    grid_rect: Rect,
) -> Option<u16> {
    let shape =
        icons::icon_rect(cell, app.cell_px).filter(|area| grid_rect.intersection(*area) == *area);
    if let Some(area) = shape {
        icons::draw_icon(frame.buffer_mut(), area, icon, STYLES.dim);
        return Some(area.bottom());
    }
    let center = Position::new(cell.x + cell.width / 2, cell.y + cell.height / 2);
    if !grid_rect.contains(center) {
        return None;
    }
    frame
        .buffer_mut()
        .set_string(center.x, center.y, icon.symbol(), STYLES.dim);
    Some(center.y + 1)
}

/// A folder cell: a folder icon, or an arrow for `..`, and the name below it. The name goes inside the
/// cell when the filename labels are off, because the label row then does not exist.
fn draw_folder_cell(
    frame: &mut Frame<'_>,
    app: &App,
    vis_idx: usize,
    img_idx: usize,
    style: Style,
    grid_rect: Rect,
) {
    let cell = app.gallery.cell_rect(vis_idx);
    let icon = if app.filenames[img_idx] == ".." {
        Icon::Parent
    } else {
        Icon::Folder
    };
    let Some(below) = draw_cell_icon(frame, app, cell, icon, grid_rect) else {
        return;
    };
    if app.gallery.show_filenames {
        return;
    }
    let name_rect = Rect {
        y: below,
        height: LABEL_ROWS,
        ..cell
    }
    .intersection(grid_rect);
    let name = label_text(app, img_idx, name_rect.width);
    frame.render_widget(
        Paragraph::new(name)
            .style(style)
            .alignment(Alignment::Center),
        name_rect,
    );
}

/// A line border without a fill, so the label and the cells inside keep their own styles. A
/// side outside `grid_rect` is left out, so a cut cell does not look closed at the clip edge.
fn draw_cell_border(
    frame: &mut Frame<'_>,
    border_type: BorderType,
    style: Style,
    border: Rect,
    grid_rect: Rect,
) {
    let visible = border.intersection(grid_rect);
    let mut sides = Borders::ALL;
    if visible.x > border.x {
        sides.remove(Borders::LEFT);
    }
    if visible.y > border.y {
        sides.remove(Borders::TOP);
    }
    if visible.right() < border.right() {
        sides.remove(Borders::RIGHT);
    }
    if visible.bottom() < border.bottom() {
        sides.remove(Borders::BOTTOM);
    }
    frame.render_widget(
        Block::default()
            .borders(sides)
            .border_type(border_type)
            .border_style(style),
        visible,
    );
}

/// The filename below the thumbnail of the visible cell `vis_idx`, cut to the cell width.
fn draw_label(
    frame: &mut Frame<'_>,
    app: &App,
    vis_idx: usize,
    img_idx: usize,
    style: Style,
    grid_rect: Rect,
) {
    let cell = app.gallery.cell_rect(vis_idx).intersection(grid_rect);
    let label_y = cell.y + cell.height;
    if label_y >= grid_rect.bottom() {
        return;
    }
    let filename = label_text(app, img_idx, cell.width);
    let label_rect = Rect {
        y: label_y,
        height: LABEL_ROWS,
        ..cell
    };
    frame.render_widget(
        Paragraph::new(filename)
            .style(style)
            .alignment(Alignment::Center),
        label_rect,
    );
}

/// The label of the entry `img_idx`, cut to `width` with a margin, with a dot for a mark and a
/// play symbol for a playable file.
fn label_text(app: &App, img_idx: usize, width: u16) -> String {
    let is_marked = app.is_marked(img_idx);
    let is_playable = shows_play_marker(app, img_idx);
    let raw_filename = app.filenames[img_idx].clone();
    let prefix_len = 2 * (usize::from(is_marked) + usize::from(is_playable));
    let max_label = usize::from(width).saturating_sub(2 + prefix_len);
    let trimmed = if raw_filename.chars().count() > max_label {
        let head: String = raw_filename
            .chars()
            .take(max_label.saturating_sub(1))
            .collect();
        format!("{head}\u{2026}")
    } else {
        raw_filename
    };
    let mut label = String::new();
    if is_marked {
        label.push_str("\u{25cf} ");
    }
    if is_playable {
        label.push_str(ANIMATION_MARKER);
        label.push(' ');
    }
    label.push_str(&trimmed);
    label
}

/// One row of the help popup: a key and its description. An empty key makes a heading, and both
/// empty make a blank line.
type HelpRow = (&'static str, &'static str);

/// The smallest terminal width that shows the help popup in two columns.
const HELP_TWO_COLUMN_WIDTH: u16 = 100;
/// The width of the key part of a help row, the longest key being `Enter, double-click`.
const HELP_KEY_WIDTH: usize = 19;
/// The width of the popup in one column, for a narrow terminal.
const HELP_SINGLE_WIDTH: u16 = 56;

/// The left column of the help popup: the gallery and its search line.
fn help_gallery_rows() -> Vec<HelpRow> {
    vec![
        ("", "Gallery"),
        ("h/j/k/l, click", "Move in the grid"),
        ("g / G, Home / End", "First / last"),
        ("Enter, double-click", "Open image or folder"),
        ("Backspace", "Parent folder"),
        ("+ / -, Shift+wheel", "Larger / smaller grid"),
        ("0", "Reset thumbnail size"),
        ("Wheel", "Scroll the grid"),
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
        ("q / Esc", "Quit"),
        ("", ""),
        ("", "Search"),
        ("<type>, Backspace", "Filter by filename"),
        ("Enter / Esc", "Confirm / cancel"),
    ]
}

/// The right column of the help popup: the fullscreen view, the video player and the mouse.
fn help_viewer_rows() -> Vec<HelpRow> {
    vec![
        ("", "Fullscreen"),
        ("\u{2190} / \u{2192}, h / l", "Previous / next image"),
        ("Edge click", "Previous / next image"),
        ("Home / End", "First / last"),
        ("+ / -, wheel", "Zoom in / out"),
        ("0", "Reset zoom to fit"),
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
        ("Space / click", "Pause / resume"),
        #[cfg(feature = "video")]
        ("\u{2190}/\u{2192}, h/l, wheel", "Seek 5 s back / forward"),
        #[cfg(feature = "video")]
        ("n / N", "Next / previous file"),
        #[cfg(feature = "video")]
        ("Home / End", "First / last file"),
        #[cfg(feature = "video")]
        ("p", "Open in mpv, with sound"),
        #[cfg(feature = "video")]
        ("q / Esc", "Back"),
        #[cfg(feature = "video")]
        ("Right click", "Back"),
        ("", ""),
        ("", "Mouse"),
        ("Shift+drag", "Select terminal text"),
    ]
}

/// The help rows as styled lines: headings in the title style, keys in the key style.
fn help_lines(rows: &[HelpRow]) -> Vec<Line<'static>> {
    rows.iter()
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

/// The help popup. A terminal of 100 columns or more gets two columns, so that it fits into 40
/// rows. A narrower one gets one column that the popup clips.
fn draw_help_popup(frame: &mut Frame<'_>) {
    let area = frame.area();
    let two_columns = area.width >= HELP_TWO_COLUMN_WIDTH;
    let left = help_gallery_rows();
    let right = help_viewer_rows();
    let rows = left.len().max(right.len());
    let single: Vec<HelpRow> = left
        .iter()
        .chain(&[("", "")])
        .chain(&right)
        .copied()
        .collect();
    let content_rows = if two_columns { rows } else { single.len() };

    let height = u16::try_from(content_rows + 4)
        .unwrap_or(u16::MAX)
        .min(area.height.saturating_sub(2));
    let width = if two_columns {
        area.width.saturating_sub(4).min(110)
    } else {
        HELP_SINGLE_WIDTH.min(area.width.saturating_sub(4))
    };
    let x = area.width.saturating_sub(width) / 2;
    let y = area.height.saturating_sub(height) / 2;
    let popup_area = Rect::new(x, y, width, height);

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
        frame.render_widget(Paragraph::new(help_lines(&left)), columns[0]);
        frame.render_widget(Paragraph::new(help_lines(&right)), columns[1]);
    } else {
        frame.render_widget(Paragraph::new(help_lines(&single)), chunks[0]);
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " press any key to close",
            STYLES.dim,
        ))),
        chunks[1],
    );
}

#[cfg(test)]
mod tests {
    use super::draw;
    use crate::app::{App, DeleteMode, PendingDelete, ViewMode};
    use crate::icons::{Icon, draw_icon, icon_rect};
    use crate::image_list::SharedImageList;
    use crate::styles::STYLES;
    use crate::test_support::folder_app;
    use image::RgbaImage;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier};
    use std::path::PathBuf;

    /// An app with `names` below `/photos`, as if a finished scan had listed them.
    fn app_with(names: &[&str]) -> App {
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
    fn render_buffer(app: &mut App, status: Option<&str>, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app, status)).unwrap();
        terminal.backend().buffer().clone()
    }

    /// The rows of `buffer` as text.
    fn screen_rows(buffer: &Buffer) -> Vec<String> {
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
    fn render(app: &mut App, status: Option<&str>, width: u16, height: u16) -> String {
        screen_rows(&render_buffer(app, status, width, height)).join("\n")
    }

    /// The cell of the first character of `needle`. The test screens hold one character per cell.
    fn cell_of<'a>(buffer: &'a Buffer, needle: &str) -> &'a Cell {
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
    fn assert_terminal_background(buffer: &Buffer) {
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
    fn assert_symbol(
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

    /// Fail when `border` is not a line border of `symbols` (top left, top right, bottom left,
    /// bottom right, horizontal, vertical) in `fg` with `modifier`.
    fn assert_border(
        buffer: &Buffer,
        border: Rect,
        symbols: [&str; 6],
        fg: Color,
        modifier: Modifier,
    ) {
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

    /// The last row of the screen: the status row.
    fn status_row(screen: &str) -> &str {
        screen.lines().last().unwrap_or_default()
    }

    /// Fail when the cell does not carry the ANSI colors `fg` on `bg` and `modifier`.
    fn assert_fill(cell: &Cell, fg: Color, bg: Color, modifier: Modifier) {
        assert_eq!((cell.fg, cell.bg, cell.modifier), (fg, bg, modifier));
    }

    /// Fail when the cell does not carry the terminal foreground and `modifier`.
    fn assert_default_fg(cell: &Cell, modifier: Modifier) {
        assert_eq!((cell.fg, cell.modifier), (Color::Reset, modifier));
    }

    /// Fail when the cell does not carry the ANSI color `fg` in bold.
    fn assert_ansi_bold(cell: &Cell, fg: Color) {
        assert_eq!((cell.fg, cell.modifier), (fg, Modifier::BOLD));
    }

    /// Fail when the cell does not carry the ANSI color `fg` without a modifier.
    fn assert_ansi(cell: &Cell, fg: Color) {
        assert_eq!((cell.fg, cell.modifier), (fg, Modifier::empty()));
    }

    /// Fail with the whole screen when `screen` does not contain `needle`.
    fn assert_shows(screen: &str, needle: &str) {
        assert!(screen.contains(needle), "missing {needle:?} in:\n{screen}");
    }

    /// Fail with the whole screen when `screen` contains `needle`.
    fn assert_hides(screen: &str, needle: &str) {
        assert!(
            !screen.contains(needle),
            "unexpected {needle:?} in:\n{screen}"
        );
    }

    /// A delete question for `names` below `/photos`.
    fn pending(names: &[&str], mode: DeleteMode) -> PendingDelete {
        PendingDelete {
            paths: names
                .iter()
                .map(|name| PathBuf::from("/photos").join(name))
                .collect(),
            from_fullscreen: false,
            mode,
        }
    }

    #[test]
    fn gallery_status_row_shows_the_position_and_no_hints() {
        let mut app = app_with(&["a.jpg", "b.jpg"]);
        app.gallery.show_filenames = true;
        let screen = render(&mut app, None, 100, 30);
        assert_eq!(status_row(&screen).trim_end(), " a.jpg [1/2]", "{screen}");
        assert_hides(&screen, "?: help");
        assert_shows(&screen, "b.jpg");
        assert_hides(&screen, "Gallery");
        assert_hides(&screen, "Search");
        assert_eq!(app.gallery.grid_rect, Rect::new(0, 0, 100, 29), "no frame");
    }

    #[test]
    fn gallery_hides_thumbnail_labels_by_default() {
        let mut app = app_with(&["a.jpg", "b.jpg"]);
        let screen = render(&mut app, None, 100, 30);
        assert_hides(&screen, "b.jpg");
        assert_shows(&screen, " a.jpg [1/2] ");

        app.gallery.toggle_filenames();
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, "b.jpg");
    }

    #[test]
    fn an_empty_folder_shows_a_note_and_the_parent_cell_does_not_hide_it() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = app_with(&[]);
        app.scan_complete = true;
        let screen = render(&mut app, None, 100, 30);
        assert!(
            status_row(&screen).starts_with(" No images or folders here"),
            "{screen}"
        );
        assert_hides(&screen, "[1/");

        app.switch_to_dir(temp.path());
        crate::test_support::wait_until(|| {
            app.refresh_from_scanner();
            !app.is_scanning()
        });
        assert_eq!(app.filenames, [".."], "the scan lists only the parent");
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, "No images or folders here");
        assert_hides(&screen, "Backspace");

        std::fs::create_dir(temp.path().join("sub")).unwrap();
        app.switch_to_dir(temp.path());
        crate::test_support::wait_until(|| {
            app.refresh_from_scanner();
            !app.is_scanning()
        });
        let screen = render(&mut app, None, 100, 30);
        assert_hides(&screen, "No images or folders here");
    }

    #[test]
    fn gallery_search_shows_the_query_and_the_matches() {
        let mut app = app_with(&["a.jpg", "b.jpg"]);
        app.gallery.search_active = true;
        app.gallery.search_query = "b".to_owned();
        app.gallery.filtered_indices = vec![1];
        let screen = render(&mut app, None, 100, 30);
        assert!(
            status_row(&screen).trim_end() == " /b\u{2588}  1/2 matches",
            "{screen}"
        );

        // After Enter, the filter stays and the position counts the matches.
        app.gallery.search_active = false;
        let screen = render(&mut app, None, 100, 30);
        assert!(status_row(&screen).starts_with(" b.jpg [1/1] "), "{screen}");
    }

    #[test]
    fn gallery_shows_the_selection_count_and_marks_the_label() {
        let mut app = app_with(&["a.jpg", "b.jpg"]);
        app.gallery.show_filenames = true;
        app.selection.insert(app.images[1].clone());
        let screen = render(&mut app, None, 100, 30);
        assert!(
            status_row(&screen).trim_end() == " a.jpg [1/2]  1 selected",
            "{screen}"
        );
        assert_shows(&screen, "\u{25cf} b.jpg");
    }

    #[test]
    fn gallery_status_row_holds_the_search_the_selection_and_the_scan() {
        let mut app = app_with(&["a.jpg", "b.jpg"]);
        app.scan_complete = false;
        let screen = render(&mut app, None, 100, 30);
        assert!(
            status_row(&screen).trim_end() == " a.jpg [1/2]  scanning\u{2026}",
            "{screen}"
        );

        app.selection.insert(app.images[0].clone());
        app.gallery.search_active = true;
        app.gallery.search_query = "a".to_owned();
        app.gallery.filtered_indices = vec![0];
        let buffer = render_buffer(&mut app, None, 100, 30);
        let screen = screen_rows(&buffer).join("\n");
        assert!(
            status_row(&screen).trim_end()
                == " /a\u{2588}  1/2 matches  1 selected  scanning\u{2026}",
            "{screen}"
        );
        assert_default_fg(cell_of(&buffer, "/a\u{2588}"), Modifier::empty());
        assert_ansi(cell_of(&buffer, "1/2 matches"), Color::DarkGray);
        assert_ansi(cell_of(&buffer, "1 selected"), Color::Yellow);
        assert_ansi(cell_of(&buffer, "scanning"), Color::DarkGray);
    }

    #[test]
    fn the_status_row_shows_the_file_facts_at_the_right_until_a_note() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = crate::test_support::app_with_files(dir.path(), &["a.png"]);
        app.request_meta();
        crate::test_support::wait_until(|| {
            app.poll_meta();
            !app.meta_pending()
        });
        let buffer = render_buffer(&mut app, None, 100, 30);
        let screen = screen_rows(&buffer).join("\n");
        let row = status_row(&screen);
        assert!(row.starts_with(" a.png [1/1] "), "{screen}");
        assert!(row.contains("8\u{d7}8 \u{b7} 0.0 MP \u{b7} "), "{row}");
        assert!(row.contains(" \u{b7} PNG \u{b7} "), "{row}");
        assert_eq!(
            row.trim_end().chars().count(),
            99,
            "the facts end at the right edge"
        );
        assert_ansi(cell_of(&buffer, "8\u{d7}8"), Color::DarkGray);

        let screen = render(&mut app, Some("copied"), 100, 30);
        assert_hides(&screen, "PNG");
        assert_shows(&screen, "copied");
    }

    #[test]
    fn gallery_draws_a_thick_border_around_the_selected_cell() {
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
        let cell = app.gallery.cell_rect(0);
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
    fn gallery_draws_a_yellow_plain_border_around_each_marked_cell() {
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
    fn gallery_borders_reach_the_grid_edges_and_hold_the_label() {
        let names: Vec<String> = (0..40).map(|index| format!("{index:02}.jpg")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut app = app_with(&names);
        app.gallery.show_filenames = true;
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
        let cell = app.gallery.cell_rect(last);
        assert_eq!(
            cell.bottom() + 1,
            border.bottom() - 1,
            "the label row sits inside"
        );
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
        let first = app.gallery.cell_rect(0);
        assert_symbol(
            &buffer,
            (marked.x, first.bottom()),
            "\u{2502}",
            Color::Yellow,
            Modifier::empty(),
        );
        assert_symbol(
            &buffer,
            (marked.x, first.bottom() + 1),
            "\u{2514}",
            Color::Yellow,
            Modifier::empty(),
        );

        // The status row below the grid stays text.
        let screen = screen_rows(&buffer).join("\n");
        assert!(
            status_row(&screen)
                .starts_with(&format!(" {last:02}.jpg [{}/40]  1 selected", last + 1)),
            "{screen}"
        );
        assert_terminal_background(&buffer);
    }

    /// The center cell of the thumbnail area of the visible cell `vis_index`.
    fn cell_center(app: &App, vis_index: usize) -> (u16, u16) {
        let cell = app.gallery.cell_rect(vis_index);
        (cell.x + cell.width / 2, cell.y + cell.height / 2)
    }

    /// The icon box of the grid cell `vis_index`. The test cells fit a large icon.
    fn icon_area(app: &App, vis_index: usize) -> Rect {
        icon_rect(app.gallery.cell_rect(vis_index), app.cell_px).unwrap()
    }

    /// True when the icon box of the cell shows `icon` in the dim color, or nothing for `None`.
    fn shows_icon(app: &App, buffer: &Buffer, vis_index: usize, icon: Option<Icon>) -> bool {
        let area = icon_area(app, vis_index);
        let mut expected = Buffer::empty(area);
        if let Some(icon) = icon {
            draw_icon(&mut expected, area, icon, STYLES.dim);
        }
        area.positions().all(|position| {
            let (shown, wanted) = (&buffer[position], &expected[position]);
            shown.symbol() == wanted.symbol()
                && (wanted.symbol() == " " || shown.fg == Color::DarkGray)
        })
    }

    #[test]
    fn an_empty_image_cell_shows_a_large_dim_loading_icon() {
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
    fn a_folder_cell_shows_a_large_folder_icon_and_its_name() {
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
        // The names sit inside the cells, in the row below the icon, also without labels.
        let (x, _) = cell_center(&app, 1);
        let below = icon_area(&app, 1).bottom();
        let row = &screen_rows(&buffer)[usize::from(below)];
        assert!(row.contains("summer"), "{screen}");
        assert!(row.contains(".."), "{screen}");
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

        // With labels, the names move to the label row, and the selected border wins.
        app.gallery.toggle_filenames();
        app.gallery.cursor = 1;
        let buffer = render_buffer(&mut app, None, 100, 30);
        let screen = screen_rows(&buffer).join("\n");
        let row = &screen_rows(&buffer)[usize::from(icon_area(&app, 1).bottom())];
        assert!(!row.contains("summer"), "{screen}");
        assert_shows(&screen, "summer");
        assert_shows(&screen, "a.jpg");
        let label = cell_of(&buffer, "summer");
        assert_eq!((label.fg, label.modifier), (Color::Blue, Modifier::BOLD));
        assert_border(
            &buffer,
            app.gallery.border_rect(1),
            THICK,
            Color::Blue,
            Modifier::BOLD,
        );
        assert!(
            status_row(&screen).starts_with(" summer [2/3]  1 folder, 1 image "),
            "{screen}"
        );
    }

    #[cfg(feature = "video")]
    #[test]
    fn an_empty_video_cell_shows_a_large_dim_play_icon() {
        let mut app = app_with(&["a.jpg", "clip.mp4"]);
        let buffer = render_buffer(&mut app, None, 100, 30);
        assert!(shows_icon(&app, &buffer, 1, Some(Icon::Video)));
        assert!(shows_icon(&app, &buffer, 0, Some(Icon::Loading)));
    }

    #[test]
    fn a_small_cell_gets_the_one_cell_symbol() {
        let mut app = app_with(&["a.jpg", "clip.mp4"]);
        let buffer = render_buffer(&mut app, None, 100, 5);
        assert_eq!(icon_rect(app.gallery.cell_rect(0), app.cell_px), None);
        assert_symbol(
            &buffer,
            cell_center(&app, 0),
            Icon::Loading.symbol(),
            Color::DarkGray,
            Modifier::empty(),
        );
    }

    #[test]
    fn a_cut_cell_gets_no_side_at_the_clip_edge() {
        // 15 columns: the cell width clamps to 8, so the second cell overflows the grid.
        let mut app = app_with(&["a.jpg", "b.jpg"]);
        app.gallery.cursor = 1;
        app.selection.insert(app.images[0].clone());
        let buffer = render_buffer(&mut app, None, 15, 8);
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
    fn gallery_uses_the_terminal_defaults_and_the_ansi_roles() {
        let mut app = app_with(&["a.jpg", "b.jpg", "c.jpg"]);
        app.gallery.show_filenames = true;
        app.selection.insert(app.images[1].clone());
        let buffer = render_buffer(&mut app, None, 100, 30);
        assert_terminal_background(&buffer);

        assert_ansi_bold(cell_of(&buffer, "a.jpg"), Color::Blue);
        assert_ansi(cell_of(&buffer, "\u{25cf} b.jpg"), Color::Yellow);
        assert_default_fg(cell_of(&buffer, "c.jpg"), Modifier::empty());
        assert_default_fg(cell_of(&buffer, "a.jpg ["), Modifier::empty());
        assert_ansi(cell_of(&buffer, "1 selected"), Color::Yellow);

        // On a marked cell under the cursor, the cursor style wins for the label.
        app.selection.insert(app.images[0].clone());
        let buffer = render_buffer(&mut app, None, 100, 30);
        assert_ansi_bold(cell_of(&buffer, "\u{25cf} a.jpg"), Color::Blue);

        app.error = Some("scan failed".to_owned());
        let buffer = render_buffer(&mut app, None, 100, 30);
        assert_ansi(cell_of(&buffer, "Error: scan failed"), Color::Red);
    }

    #[test]
    #[cfg(feature = "video")]
    fn gallery_marks_an_animated_image_in_the_label_or_in_the_gap_below_the_cell() {
        use crate::test_support::fixture;
        let anim = fixture("anim.gif");
        let still = fixture("still.gif");
        let mut app = app_with(&[anim.to_str().unwrap(), still.to_str().unwrap()]);
        app.filenames = vec!["anim.gif".to_owned(), "still.gif".to_owned()];
        app.gallery.show_filenames = true;
        let screen = render(&mut app, None, 100, 30);
        assert!(
            !screen.contains('\u{25b6}'),
            "the draw reads no header, no marker before the thumbnail job: {screen}"
        );

        let rect = app.gallery.thumb_box();
        app.spawn_thumb_decode(0, rect);
        app.spawn_thumb_decode(1, rect);
        crate::test_support::wait_until(|| {
            app.poll_thumbnails();
            app.animated.len() == 2
        });
        app.mark_dirty();
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, "\u{25b6} anim.gif");
        assert!(!screen.contains("\u{25b6} still.gif"), "{screen}");

        app.gallery.show_filenames = false;
        app.mark_dirty();
        let buffer = render_buffer(&mut app, None, 100, 30);
        let cell = app.gallery.cell_rect(0);
        let below = buffer[(cell.x + cell.width / 2, cell.y + cell.height)].symbol();
        assert_eq!(below, "\u{25b6}", "{}", screen_rows(&buffer).join("\n"));
        let still_cell = app.gallery.cell_rect(1);
        let below_still = buffer[(
            still_cell.x + still_cell.width / 2,
            still_cell.y + still_cell.height,
        )]
            .symbol();
        assert_ne!(below_still, "\u{25b6}");
    }

    #[test]
    fn gallery_shortens_a_long_label() {
        let long = format!("{}.jpg", "x".repeat(80));
        let mut app = app_with(&[long.as_str()]);
        app.gallery.show_filenames = true;
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, "xx\u{2026}");
    }

    #[test]
    fn gallery_asks_before_a_delete() {
        let mut app = app_with(&["a.jpg", "b.jpg"]);
        app.pending_delete = Some(pending(&["a.jpg"], DeleteMode::Trash));
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, " Move to trash a.jpg? [y/N] ");

        app.pending_delete = Some(pending(&["a.jpg", "b.jpg"], DeleteMode::Permanent));
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, " Permanently delete 2 files? ");
    }

    #[test]
    fn gallery_shows_the_delete_error_before_the_error() {
        let mut app = app_with(&["a.jpg"]);
        app.error = Some("scan failed".to_owned());
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, " Error: scan failed ");

        app.delete_error = Some("no trash".to_owned());
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, " Delete error: no trash ");
        assert_hides(&screen, "scan failed");
    }

    #[test]
    fn gallery_status_note_replaces_the_hints() {
        let mut app = app_with(&["a.jpg"]);
        let screen = render(&mut app, Some("1 file moved to trash"), 100, 30);
        assert_shows(&screen, " a.jpg [1/1]  1 file moved to trash ");
        assert_hides(&screen, "Enter: open");
    }

    #[test]
    fn fullscreen_shows_the_name_and_the_position() {
        let mut app = app_with(&["a.jpg", "b.jpg"]);
        app.mode = ViewMode::Fullscreen;
        app.current = 1;
        app.loaded = Some(RgbaImage::new(30, 20));
        let screen = render(&mut app, None, 100, 30);
        assert!(
            status_row(&screen).trim_end() == " b.jpg [2/2]",
            "the decoded size is not the file size, the facts come from the file: {screen}"
        );
        assert_eq!(
            app.image_rect,
            Rect::new(0, 0, 100, 29),
            "no frame, 1 status row"
        );
        assert_eq!(
            screen.matches(" b.jpg ").count(),
            1,
            "the name only in the status row"
        );
    }

    #[test]
    fn fullscreen_shows_the_zoom() {
        let mut app = app_with(&["a.jpg"]);
        app.mode = ViewMode::Fullscreen;
        app.zoom = 2.0;
        let screen = render(&mut app, None, 100, 30);
        assert_eq!(status_row(&screen).trim_end(), " a.jpg [1/1] 200%");
    }

    #[test]
    fn fullscreen_shows_the_slideshow_state_and_interval() {
        let mut app = app_with(&["a.jpg"]);
        app.mode = ViewMode::Fullscreen;
        app.start_slideshow();
        let interval = format!("{:.1}s", app.slideshow_interval.as_secs_f64());
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, &format!("\u{25b6} {interval}"));
        assert_hides(&screen, "space: pause");

        app.slideshow_toggle_pause();
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, "\u{23f8}");
        assert_shows(&screen, &interval);
    }

    #[test]
    fn fullscreen_asks_before_a_delete_and_shows_errors() {
        let mut app = app_with(&["a.jpg", "b.jpg"]);
        app.mode = ViewMode::Fullscreen;
        app.pending_delete = Some(pending(&["a.jpg"], DeleteMode::Permanent));
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, " Permanently delete a.jpg? [y/N] ");

        app.pending_delete = Some(pending(&[], DeleteMode::Trash));
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, " Move to trash 0 files? [y/N] ");

        app.pending_delete = None;
        app.error = Some("decode failed".to_owned());
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, " Error: decode failed ");

        app.delete_error = Some("no trash".to_owned());
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, " Delete error: no trash ");
    }

    #[test]
    fn fullscreen_uses_the_terminal_defaults_and_the_ansi_roles() {
        let mut app = app_with(&["a.jpg"]);
        app.mode = ViewMode::Fullscreen;
        let buffer = render_buffer(&mut app, None, 100, 30);
        assert_terminal_background(&buffer);
        assert_symbol(&buffer, (0, 0), " ", Color::Reset, Modifier::empty());
        assert_symbol(&buffer, (0, 29), " ", Color::Reset, Modifier::empty());
        assert_symbol(&buffer, (1, 29), "a", Color::Reset, Modifier::empty());

        app.pending_delete = Some(pending(&["a.jpg"], DeleteMode::Trash));
        let buffer = render_buffer(&mut app, None, 100, 30);
        assert_ansi(cell_of(&buffer, "Move to trash a.jpg?"), Color::Red);
    }

    #[cfg(feature = "video")]
    #[test]
    fn video_status_bar_shows_the_time_the_position_and_the_note() {
        let mut app = app_with(&["clip.mp4", "b.jpg"]);
        app.mode = ViewMode::Video;
        let screen = render(&mut app, None, 100, 30);
        assert!(
            status_row(&screen).starts_with(" \u{25b6} clip.mp4 00:00/00:00 [1/2] "),
            "{screen}"
        );
        assert_hides(&screen, "Space: pause");
        assert_eq!(
            app.image_rect,
            Rect::new(0, 0, 100, 29),
            "no frame, 1 status row"
        );

        let screen = render(&mut app, Some("mpv not found"), 100, 30);
        assert_shows(&screen, " mpv not found ");

        app.error = Some("no video stream".to_owned());
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, " Error: no video stream ");
    }

    #[cfg(feature = "video")]
    #[test]
    fn video_status_bar_shows_the_player_position_and_its_error() {
        use crate::player::Player;
        use crate::test_support::video::{clip_info, fake_decoder, shared};

        let mut app = app_with(&["clip.mp4", "b.jpg"]);
        app.mode = ViewMode::Video;
        let mut info = clip_info();
        info.duration = 3725.0;
        let decoder = fake_decoder(info);
        let mut player = Player::open(
            shared(&decoder),
            std::path::Path::new("/bin/ffmpeg"),
            std::path::Path::new("/videos/clip.mp4"),
            (64, 48),
            &[],
            true,
        )
        .unwrap();
        decoder.feed(vec![0; 64 * 48 * 4]);
        assert!(player.poll());
        player.toggle_pause();
        app.video = Some(player);
        let screen = render(&mut app, None, 100, 30);
        assert!(
            status_row(&screen).starts_with(" \u{23f8} clip.mp4 00:00/1:02:05 [1/2] "),
            "{screen}"
        );
        assert_eq!(crate::file_meta::format_time(65.9), "01:05");

        app.video.as_mut().unwrap().toggle_pause();
        decoder.end();
        assert!(
            !app.video.as_mut().unwrap().poll(),
            "the end of the clip: a repeat"
        );
        decoder.end();
        assert!(
            !app.video.as_mut().unwrap().poll(),
            "no frame: the software retry"
        );
        decoder.end();
        assert!(
            !app.video.as_mut().unwrap().poll(),
            "no frame again: the error"
        );
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, " Error: ffmpeg could not decode clip.mp4 ");
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

    #[test]
    fn help_popup_lists_the_keys_of_each_view_in_two_columns() {
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
        assert_shows(&screen, " press any key to close");
        #[cfg(feature = "video")]
        {
            assert_shows(&screen, " Video player (no sound)");
            assert_shows(&screen, "Seek 5 s back / forward");
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
    fn help_popup_falls_back_to_one_column_in_a_narrow_terminal() {
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
    fn help_popup_clips_inside_a_narrow_and_short_terminal() {
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

    #[cfg(feature = "video")]
    #[test]
    fn a_video_cell_gets_the_play_marker_after_its_thumbnail() {
        let mut app = app_with(&["a.jpg", "clip.mp4"]);
        app.thumb_cache.insert(0, RgbaImage::new(4, 4));
        let buffer = render_buffer(&mut app, None, 100, 30);
        let cell = app.gallery.cell_rect(1);
        let below = |buffer: &Buffer| {
            buffer[(cell.x + cell.width / 2, cell.y + cell.height)]
                .symbol()
                .to_owned()
        };
        assert_ne!(below(&buffer), "\u{25b6}", "no marker before the thumbnail");

        app.thumb_cache.insert(1, RgbaImage::new(4, 4));
        let buffer = render_buffer(&mut app, None, 100, 30);
        assert_eq!(below(&buffer), "\u{25b6}");
        let still = app.gallery.cell_rect(0);
        let below_still = buffer[(still.x + still.width / 2, still.y + still.height)].symbol();
        assert_ne!(below_still, "\u{25b6}", "a still image gets no marker");

        app.gallery.show_filenames = true;
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, "\u{25b6} clip.mp4");
        assert!(!screen.contains("\u{25b6} a.jpg"), "{screen}");
    }
}
