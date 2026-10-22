use crate::app::{App, DeleteMode, ViewMode};
use crate::gallery::LABEL_ROWS;
use crate::styles::STYLES;
use ratatui::{
    prelude::*,
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};

/// Preferred width of the info panel, including its border.
const INFO_PANEL_WIDTH: u16 = 42;
/// The status row at the bottom of the gallery, the fullscreen view and the video view.
const STATUS_ROWS: u16 = 1;
/// The icon of a grid cell whose image thumbnail is not decoded yet.
const IMAGE_LOADING_ICON: &str = "\u{25cc}";
/// The icon of a grid cell whose video thumbnail is not decoded yet.
const VIDEO_LOADING_ICON: &str = "\u{25b6}";
/// The symbol in the center of a folder cell: a small right-pointing triangle, one cell wide.
const FOLDER_ICON: &str = "\u{25b8}";

/// Draw the whole frame. `status` is a one-line note of the run loop, in place of the key hints.
pub(crate) fn draw(frame: &mut Frame<'_>, app: &mut App, status: Option<&str>) {
    let area = frame.area();
    let (main, info) = split_info(area, app);
    match app.mode {
        ViewMode::Gallery => draw_gallery(frame, app, main, status),
        ViewMode::Fullscreen => draw_fullscreen(frame, app, main, status),
        ViewMode::Picker => draw_picker(frame, app),
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
/// The picker never shows it, and very narrow terminals keep the full width.
pub(crate) fn split_info(area: Rect, app: &App) -> (Rect, Option<Rect>) {
    if !app.info_visible || app.mode == ViewMode::Picker {
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

fn draw_picker(frame: &mut Frame<'_>, app: &mut App) {
    let area = frame.area();
    let has_photos = !app.images.is_empty();

    let Some(picker) = app.picker.as_mut() else {
        return;
    };

    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(area);
    let filter_rect = chunks[0];
    let list_area = chunks[1];

    let filter_block = Block::default()
        .borders(Borders::ALL)
        .border_style(STYLES.border)
        .title(Line::from(Span::styled(" Filter ", STYLES.title)));
    let filter_inner = filter_block.inner(filter_rect);
    frame.render_widget(filter_block, filter_rect);

    if picker.filter_active {
        let match_count = picker.filtered_indices.len();
        let total = picker.entries.len();
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("/{}\u{2588}", picker.filter), STYLES.text),
                Span::styled(format!("  {match_count}/{total} matches"), STYLES.dim),
            ])),
            filter_inner,
        );
    } else if !picker.filter.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" /{}", picker.filter), STYLES.text),
                Span::styled(
                    format!(
                        "  {}/{} matches",
                        picker.filtered_indices.len(),
                        picker.entries.len()
                    ),
                    STYLES.dim,
                ),
            ])),
            filter_inner,
        );
    } else {
        frame.render_widget(Paragraph::new(""), filter_inner);
    }

    let title_path = picker.current_dir.display().to_string();
    let count = picker.filtered_indices.len();
    let list_title = format!(" Directory: {title_path} [{count}] ");

    let hint = if picker.filter_active {
        " Esc: cancel  Enter: confirm  Backspace: delete "
    } else if has_photos {
        " Enter: choose  /: filter  ?: help  q: back "
    } else {
        " Enter: choose  /: filter  ?: help  q: quit "
    };

    let bottom_line = if let Some(err) = &picker.error {
        Line::from(Span::styled(format!(" {err} "), STYLES.error))
    } else {
        Line::from(Span::styled(hint, STYLES.key))
    };

    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_style(STYLES.border)
        .title(Line::from(Span::styled(list_title, STYLES.title)))
        .title_bottom(bottom_line);

    let list_inner = list_block.inner(list_area);
    frame.render_widget(list_block, list_area);

    let visible_height = list_inner.height as usize;
    picker.adjust_scroll(visible_height);

    let lines: Vec<Line<'_>> = picker
        .visible_slice(visible_height)
        .iter()
        .enumerate()
        .map(|(vis, &real)| {
            let entry = &picker.entries[real];
            let is_cursor = picker.scroll_offset + vis == picker.cursor;
            let marker = if is_cursor { "\u{25b6} " } else { "  " };
            let label = if entry.is_parent {
                "..".to_owned()
            } else {
                format!("{}/", entry.name)
            };
            let style = if is_cursor {
                STYLES.selection
            } else {
                STYLES.text
            };
            Line::from(vec![
                Span::styled(marker.to_owned(), style),
                Span::styled(label, style),
            ])
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), list_inner);
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
        let dims = app
            .loaded
            .as_ref()
            .map(|img| format!(" {}x{}", img.width(), img.height()))
            .unwrap_or_default();
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
        let hint = if app.slideshow.is_some() {
            " space: pause  [/]: interval  s/Esc: stop  q: back  i: info "
        } else if placeholder {
            " Enter: play  \u{2190}/\u{2192}: navigate  i: info  ?: help  q/Esc: back "
        } else if app.zoom > 1.0 {
            " hjkl: pan  +/-: zoom  0: reset  ?: help  q/Esc: back "
        } else {
            " \u{2190}/\u{2192}: navigate  +/-: zoom  s: slideshow  i: info  ?: help  q/Esc: back "
        };
        Line::from(vec![
            Span::styled(
                format!(
                    " {} [{}/{}]{dims}{zoom}{slideshow} ",
                    filename,
                    app.current + 1,
                    app.images.len(),
                ),
                STYLES.text,
            ),
            status_note.map_or_else(
                || Span::styled(hint, STYLES.key),
                |note| Span::styled(format!(" {note} "), STYLES.text),
            ),
        ])
    };

    let inner = content_area(area);
    frame.render_widget(Paragraph::new(bottom_line), status_area(area));
    if placeholder {
        let middle = Rect {
            y: inner.y + inner.height / 2,
            height: inner.height.min(1),
            ..inner
        };
        frame.render_widget(
            Paragraph::new(Span::styled("video: press Enter to play", STYLES.dim))
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

    let (status, pts, duration, fps) = match &app.video {
        Some(v) => {
            let status = if v.playing { "\u{25b6}" } else { "\u{23f8}" };
            (status, v.current_pts, v.duration, v.fps)
        }
        None => ("\u{25b6}", 0.0, 0.0, 0.0),
    };

    let fmt_time = |secs: f64| -> String {
        let s = secs as u64;
        format!("{:02}:{:02}", s / 60, s % 60)
    };

    let bottom_line = if let Some(err) = &app.error {
        Line::from(Span::styled(format!(" Error: {err} "), STYLES.error))
    } else {
        Line::from(vec![
            Span::styled(
                format!(
                    " {} {} {}/{} [{:.0}fps] [{}/{}] ",
                    status,
                    filename,
                    fmt_time(pts),
                    fmt_time(duration),
                    fps,
                    app.current + 1,
                    app.images.len(),
                ),
                STYLES.text,
            ),
            status_note.map_or_else(
                || {
                    Span::styled(
                        " Space: pause  \u{2190}/\u{2192}: navigate  ?: help  q: back ",
                        STYLES.key,
                    )
                },
                |note| Span::styled(format!(" {note} "), STYLES.text),
            ),
        ])
    };

    frame.render_widget(Paragraph::new(bottom_line), status_area(area));
    app.image_rect = content_area(area);
}

fn draw_gallery(frame: &mut Frame<'_>, app: &mut App, area: Rect, status: Option<&str>) {
    let grid_rect = content_area(area);
    app.gallery.update_grid(grid_rect);
    let filtered_count = app.gallery.filtered_indices.len();
    frame.render_widget(
        Paragraph::new(gallery_status_line(app, filtered_count, status)),
        status_area(area),
    );
    draw_thumbnail_cells(frame, app, grid_rect);
}

/// The status row of the gallery: a delete question, an error, the search query while typing,
/// or name, position, selection count, scan note and hints.
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
            format!(" {action} {name}? ")
        } else {
            format!(" {action} {} files? ", pending.paths.len())
        };
        return Line::from(vec![
            Span::styled(prompt, STYLES.error),
            Span::styled(" y: confirm  n/Esc: cancel ", STYLES.key),
        ]);
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
    spans.push(if app.gallery.search_active {
        Span::styled(" Esc: cancel  Enter: confirm ", STYLES.key)
    } else if let Some(status) = status {
        Span::styled(format!(" {status} "), STYLES.text)
    } else {
        Span::styled(
            " Enter: open  +/-: zoom  n: names  i: info  /: search  ?: help  q: quit ",
            STYLES.key,
        )
    });
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
fn entry_counts(app: &App) -> String {
    let folders = app.folder_count.saturating_sub(usize::from(
        app.filenames.first().is_some_and(|name| name == ".."),
    ));
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
fn status_area(area: Rect) -> Rect {
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
            draw_loading_icon(frame, app, vis_idx, img_idx, grid_rect);
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
}

/// A dim icon in the center of a cell without a thumbnail. Kitty draws the thumbnail above the
/// text, so the icon needs no redraw when the thumbnail arrives.
fn draw_loading_icon(
    frame: &mut Frame<'_>,
    app: &App,
    vis_idx: usize,
    img_idx: usize,
    grid_rect: Rect,
) {
    let cell = app.gallery.cell_rect(vis_idx);
    let center = Position::new(cell.x + cell.width / 2, cell.y + cell.height / 2);
    if !grid_rect.contains(center) {
        return;
    }
    let icon = if app.is_video_at(img_idx) {
        VIDEO_LOADING_ICON
    } else {
        IMAGE_LOADING_ICON
    };
    frame
        .buffer_mut()
        .set_string(center.x, center.y, icon, STYLES.dim);
}

/// A folder cell: a dim symbol at the center and the name below it. The name goes inside the
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
    let center = Position::new(cell.x + cell.width / 2, cell.y + cell.height / 2);
    if !grid_rect.contains(center) {
        return;
    }
    frame
        .buffer_mut()
        .set_string(center.x, center.y, FOLDER_ICON, STYLES.dim);
    if app.gallery.show_filenames {
        return;
    }
    let name_rect = Rect {
        y: center.y + 1,
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

/// The label of the entry `img_idx`, cut to `width` with a margin, with a dot for a mark.
fn label_text(app: &App, img_idx: usize, width: u16) -> String {
    let is_marked = app.is_marked(img_idx);
    let raw_filename = app.filenames[img_idx].clone();
    let prefix_len = if is_marked { 2 } else { 0 };
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
    if is_marked {
        format!("\u{25cf} {trimmed}")
    } else {
        trimmed
    }
}

fn draw_help_popup(frame: &mut Frame<'_>) {
    let area = frame.area();

    let help_lines = [
        ("", "Gallery"),
        ("h/j/k/l", "Navigate grid"),
        ("g / G", "Jump to first / last"),
        ("Home / End", "Jump to first / last"),
        ("Enter", "Open fullscreen or folder"),
        ("Backspace", "Parent folder"),
        ("+ / -", "Larger / smaller thumbnails"),
        ("0", "Reset thumbnail size"),
        ("n", "Toggle filename labels"),
        ("i", "Toggle info panel"),
        ("s", "Start slideshow from cursor"),
        ("Space", "Toggle selection"),
        ("a / A", "Select all / clear all"),
        ("d / D", "Trash / permanently delete selection"),
        ("o", "Open directory picker"),
        ("/", "Search"),
        ("?", "Toggle help"),
        ("q / Esc", "Quit"),
        ("", ""),
        ("", "Search"),
        ("<type>", "Filter by filename"),
        ("Esc", "Cancel search"),
        ("Enter", "Confirm filter"),
        ("Backspace", "Delete character"),
        ("", ""),
        ("", "Fullscreen"),
        ("\u{2190} / \u{2192}", "Previous / next image"),
        ("h / l", "Previous / next image"),
        ("Home / End", "Jump to first / last"),
        ("+ / -", "Zoom in / out"),
        ("0", "Reset zoom to fit"),
        ("h/j/k/l", "Pan when zoomed"),
        ("i", "Toggle info panel"),
        ("s", "Start / stop slideshow"),
        ("Space", "Pause / resume slideshow"),
        ("[ / ]", "Shorter / longer slide interval"),
        ("d / D", "Trash / permanently delete image"),
        ("q / Esc", "Back to gallery"),
        ("?", "Toggle help"),
        #[cfg(feature = "video")]
        ("", ""),
        #[cfg(feature = "video")]
        ("", "Video in mpv ([video] player)"),
        #[cfg(feature = "video")]
        ("q", "Back to gallery"),
        #[cfg(feature = "video")]
        ("Space", "Pause / resume"),
        #[cfg(feature = "video")]
        ("\u{2190} / \u{2192}", "Seek back / forward"),
        #[cfg(feature = "video")]
        ("f", "Fullscreen (window player)"),
        #[cfg(feature = "video")]
        ("", ""),
        #[cfg(feature = "video")]
        ("", "Video in the built-in player"),
        #[cfg(feature = "video")]
        ("Space", "Pause / resume"),
        #[cfg(feature = "video")]
        ("\u{2190} / \u{2192}", "Previous / next file"),
        #[cfg(feature = "video")]
        ("h / l", "Previous / next file"),
        #[cfg(feature = "video")]
        ("q / Esc", "Back to gallery"),
        ("", ""),
        ("", "Directory picker"),
        ("j / k", "Move cursor"),
        ("Enter", "Choose directory and open gallery"),
        ("l / Right", "Descend into directory"),
        ("h", "Parent directory"),
        ("g / G", "First / last"),
        ("/", "Filter names"),
        ("q / Esc", "Back to gallery"),
    ];

    let height = (help_lines.len() as u16 + 4).min(area.height.saturating_sub(2));
    let width = 52.min(area.width.saturating_sub(4));
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

    let key_width = 12;
    let lines: Vec<Line<'_>> = help_lines
        .iter()
        .map(|(key, desc)| {
            if key.is_empty() {
                Line::from(Span::styled(format!(" {desc}"), STYLES.title))
            } else {
                Line::from(vec![
                    Span::styled(format!(" {key:>key_width$}  "), STYLES.key),
                    Span::styled(*desc, STYLES.text),
                ])
            }
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), chunks[0]);
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
    use crate::image_list::SharedImageList;
    use crate::picker::PickerState;
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
    fn gallery_status_row_shows_the_position_and_the_hints() {
        let mut app = app_with(&["a.jpg", "b.jpg"]);
        app.gallery.show_filenames = true;
        let screen = render(&mut app, None, 100, 30);
        assert!(
            status_row(&screen).starts_with(" a.jpg [1/2]  Enter: open  +/-: zoom"),
            "{screen}"
        );
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
    fn an_empty_gallery_shows_the_hints_without_a_position() {
        let mut app = app_with(&[]);
        let screen = render(&mut app, None, 100, 30);
        assert!(status_row(&screen).starts_with(" Enter: open"), "{screen}");
        assert_hides(&screen, "[1/");
    }

    #[test]
    fn gallery_search_shows_the_query_and_the_matches() {
        let mut app = app_with(&["a.jpg", "b.jpg"]);
        app.gallery.search_active = true;
        app.gallery.search_query = "b".to_owned();
        app.gallery.filtered_indices = vec![1];
        let screen = render(&mut app, None, 100, 30);
        assert!(
            status_row(&screen)
                .starts_with(" /b\u{2588}  1/2 matches  Esc: cancel  Enter: confirm "),
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
            status_row(&screen).starts_with(" a.jpg [1/2]  1 selected  Enter: open"),
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
            status_row(&screen).starts_with(" a.jpg [1/2]  scanning\u{2026}  Enter: open"),
            "{screen}"
        );

        app.selection.insert(app.images[0].clone());
        app.gallery.search_active = true;
        app.gallery.search_query = "a".to_owned();
        app.gallery.filtered_indices = vec![0];
        let buffer = render_buffer(&mut app, None, 100, 30);
        let screen = screen_rows(&buffer).join("\n");
        assert!(
            status_row(&screen).starts_with(
                " /a\u{2588}  1/2 matches  1 selected  scanning\u{2026}  Esc: cancel  Enter: confirm "
            ),
            "{screen}"
        );
        assert_default_fg(cell_of(&buffer, "/a\u{2588}"), Modifier::empty());
        assert_ansi(cell_of(&buffer, "1/2 matches"), Color::DarkGray);
        assert_ansi(cell_of(&buffer, "1 selected"), Color::Yellow);
        assert_ansi(cell_of(&buffer, "scanning"), Color::DarkGray);
        assert_ansi(cell_of(&buffer, "Esc: cancel"), Color::Cyan);
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

    #[test]
    fn an_empty_image_cell_shows_a_dim_loading_icon_at_its_center() {
        let mut app = app_with(&["a.jpg", "b.jpg"]);
        app.thumb_cache.insert(1, RgbaImage::new(4, 4));
        let buffer = render_buffer(&mut app, None, 100, 30);
        let screen = screen_rows(&buffer).join("\n");
        let center = cell_center(&app, 0);
        assert_symbol(
            &buffer,
            center,
            "\u{25cc}",
            Color::DarkGray,
            Modifier::empty(),
        );
        assert_eq!(
            screen.matches('\u{25cc}').count(),
            1,
            "the cached cell has no icon"
        );
        let cached = cell_center(&app, 1);
        assert_symbol(&buffer, cached, " ", Color::Reset, Modifier::empty());
    }

    #[test]
    fn a_folder_cell_shows_a_dim_symbol_and_its_name() {
        let mut app = folder_app(&["..", "summer"], &["a.jpg"]);
        app.thumb_cache.insert(2, RgbaImage::new(4, 4));
        let buffer = render_buffer(&mut app, None, 100, 30);
        let screen = screen_rows(&buffer).join("\n");
        for vis_index in 0..2 {
            let center = cell_center(&app, vis_index);
            assert_symbol(
                &buffer,
                center,
                "\u{25b8}",
                Color::DarkGray,
                Modifier::empty(),
            );
        }
        assert_eq!(
            screen.matches('\u{25b8}').count(),
            2,
            "the file cell has no symbol"
        );
        assert_eq!(
            screen.matches('\u{25cc}').count(),
            0,
            "a folder has no loading icon"
        );
        // The names sit inside the cells, one row below the symbol, also without labels.
        let (x, y) = cell_center(&app, 1);
        let row = &screen_rows(&buffer)[usize::from(y) + 1];
        assert!(row.contains("summer"), "{screen}");
        assert!(row.contains(".."), "{screen}");
        let name_cell = &buffer[(x, y + 1)];
        assert_eq!(name_cell.symbol(), "m", "the name is centered: sum|mer");
        assert_hides(&screen, "a.jpg");
        assert!(
            status_row(&screen).starts_with(" .. [1/3]  1 folder, 1 image  Enter: open"),
            "{screen}"
        );

        // With labels, the names move to the label row, and the selected border wins.
        app.gallery.toggle_filenames();
        app.gallery.cursor = 1;
        let buffer = render_buffer(&mut app, None, 100, 30);
        let screen = screen_rows(&buffer).join("\n");
        let (_, y) = cell_center(&app, 1);
        let row = &screen_rows(&buffer)[usize::from(y) + 1];
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
    fn an_empty_video_cell_shows_a_dim_play_icon_at_its_center() {
        let mut app = app_with(&["a.jpg", "clip.mp4"]);
        let buffer = render_buffer(&mut app, None, 100, 30);
        let center = cell_center(&app, 1);
        assert_symbol(
            &buffer,
            center,
            "\u{25b6}",
            Color::DarkGray,
            Modifier::empty(),
        );
        assert_symbol(
            &buffer,
            cell_center(&app, 0),
            "\u{25cc}",
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
        assert_ansi(cell_of(&buffer, "Enter: open"), Color::Cyan);

        // On a marked cell under the cursor, the cursor style wins for the label.
        app.selection.insert(app.images[0].clone());
        let buffer = render_buffer(&mut app, None, 100, 30);
        assert_ansi_bold(cell_of(&buffer, "\u{25cf} a.jpg"), Color::Blue);

        app.error = Some("scan failed".to_owned());
        let buffer = render_buffer(&mut app, None, 100, 30);
        assert_ansi(cell_of(&buffer, "Error: scan failed"), Color::Red);
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
        assert_shows(&screen, " Move to trash a.jpg?  y: confirm  n/Esc: cancel ");

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
    fn fullscreen_shows_the_name_the_position_and_the_size() {
        let mut app = app_with(&["a.jpg", "b.jpg"]);
        app.mode = ViewMode::Fullscreen;
        app.current = 1;
        app.loaded = Some(RgbaImage::new(30, 20));
        let screen = render(&mut app, None, 100, 30);
        assert!(
            status_row(&screen).starts_with(" b.jpg [2/2] 30x20  \u{2190}/\u{2192}: navigate"),
            "{screen}"
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
    fn fullscreen_shows_the_zoom_and_the_pan_hints() {
        let mut app = app_with(&["a.jpg"]);
        app.mode = ViewMode::Fullscreen;
        app.zoom = 2.0;
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, " a.jpg [1/1] 200%  hjkl: pan");
    }

    #[test]
    fn fullscreen_shows_the_slideshow_state_and_interval() {
        let mut app = app_with(&["a.jpg"]);
        app.mode = ViewMode::Fullscreen;
        app.start_slideshow();
        let interval = format!("{:.1}s", app.slideshow_interval.as_secs_f64());
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, &format!("\u{25b6} {interval}"));
        assert_shows(&screen, "space: pause  [/]: interval  s/Esc: stop  q: back");

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
        assert_ansi(cell_of(&buffer, "\u{2190}/\u{2192}: navigate"), Color::Cyan);

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
            status_row(&screen).starts_with(" \u{25b6} clip.mp4 00:00/00:00 [0fps] [1/2] "),
            "{screen}"
        );
        assert_shows(&screen, "Space: pause");
        assert_eq!(
            app.image_rect,
            Rect::new(0, 0, 100, 29),
            "no frame, 1 status row"
        );

        let screen = render(&mut app, Some("mpv not found"), 100, 30);
        assert_shows(&screen, " mpv not found ");
        assert_hides(&screen, "Space: pause");

        app.error = Some("no video stream".to_owned());
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, " Error: no video stream ");
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
    fn help_popup_lists_the_keys() {
        let mut app = app_with(&["a.jpg"]);
        app.help_visible = true;
        let screen = render(&mut app, None, 100, 120);
        assert_shows(&screen, " Help ");
        assert_shows(&screen, "     h/j/k/l  Navigate grid");
        assert_shows(&screen, "Open fullscreen or folder");
        assert_shows(&screen, "   Backspace  Parent folder");
        assert_shows(&screen, " Directory picker");
        assert_shows(&screen, " press any key to close");

        let buffer = render_buffer(&mut app, None, 100, 120);
        assert_terminal_background(&buffer);
        assert_default_fg(cell_of(&buffer, "Help "), Modifier::BOLD);
        assert_default_fg(cell_of(&buffer, "Gallery   "), Modifier::BOLD);
        assert_ansi(cell_of(&buffer, "h/j/k/l"), Color::Cyan);
        assert_default_fg(cell_of(&buffer, "Navigate grid"), Modifier::empty());
        assert_ansi(cell_of(&buffer, "press any key"), Color::DarkGray);
    }

    #[test]
    fn directory_picker_lists_the_directories_filter_and_error() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        std::fs::create_dir(dir.join("alpha")).unwrap();
        std::fs::create_dir(dir.join("beta")).unwrap();
        let mut app = app_with(&["a.jpg"]);
        app.info_visible = true;
        app.mode = ViewMode::Picker;
        app.picker = Some(PickerState::new(dir.clone()));

        let width = u16::try_from(dir.as_os_str().len()).unwrap() + 60;
        let screen = render(&mut app, None, width, 20);
        assert_shows(&screen, " Filter ");
        assert_shows(&screen, &format!(" Directory: {} [3] ", dir.display()));
        assert_shows(&screen, "\u{25b6} ..");
        assert_shows(&screen, "  alpha/");
        assert_shows(&screen, "  beta/");
        assert_shows(&screen, " Enter: choose  /: filter");
        assert_hides(&screen, " Info ");
        let buffer = render_buffer(&mut app, None, width, 20);
        assert_terminal_background(&buffer);
        assert_default_fg(cell_of(&buffer, "\u{25b6} .."), Modifier::REVERSED);
        assert_default_fg(cell_of(&buffer, "alpha/"), Modifier::empty());
        assert_ansi(cell_of(&buffer, "Enter: choose"), Color::Cyan);

        let picker = app.picker.as_mut().unwrap();
        picker.filter = "bet".to_owned();
        picker.filter_active = true;
        picker.rebuild_filter();
        let screen = render(&mut app, None, width, 20);
        assert_shows(&screen, "/bet\u{2588}  1/3 matches");
        assert_shows(&screen, " Esc: cancel  Enter: confirm  Backspace: delete ");
        assert_shows(&screen, "\u{25b6} beta/");

        let picker = app.picker.as_mut().unwrap();
        picker.filter_active = false;
        picker.error = Some("permission denied".to_owned());
        let screen = render(&mut app, None, width, 20);
        assert_shows(&screen, " /bet  1/3 matches");
        assert_shows(&screen, " permission denied ");
        let buffer = render_buffer(&mut app, None, width, 20);
        assert_ansi(cell_of(&buffer, "permission denied"), Color::Red);
        assert_ansi(cell_of(&buffer, "1/3 matches"), Color::DarkGray);

        app.picker = None;
        let screen = render(&mut app, None, width, 20);
        assert_hides(&screen, " Filter ");
    }
}
