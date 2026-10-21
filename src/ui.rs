use crate::app::{App, ViewMode};
use crate::gallery::LABEL_ROWS;
use crate::styles::STYLES;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

/// Preferred width of the info panel, including its border.
const INFO_PANEL_WIDTH: u16 = 42;

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
fn split_info(area: Rect, app: &App) -> (Rect, Option<Rect>) {
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
                if pending.permanent {
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
            " space: pause  [/]: interval  s: stop  i: info  q: quit "
        } else if placeholder {
            " Enter: play  \u{2190}/\u{2192}: navigate  i: info  ?: help  q: quit "
        } else if app.zoom > 1.0 {
            " hjkl: pan  +/-: zoom  0: reset  ?: help  q: quit "
        } else {
            " \u{2190}/\u{2192}: navigate  +/-: zoom  s: slideshow  i: info  ?: help  q: quit "
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

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(STYLES.border)
        .title(Line::from(Span::styled(
            format!(" {filename} "),
            STYLES.title,
        )))
        .title_bottom(bottom_line);

    let inner = block.inner(area);
    frame.render_widget(block, area);
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
                        " Space: pause  \u{2190}/\u{2192}: navigate  ?: help  q: quit ",
                        STYLES.key,
                    )
                },
                |note| Span::styled(format!(" {note} "), STYLES.text),
            ),
        ])
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(STYLES.border)
        .title(Line::from(Span::styled(
            format!(" {filename} "),
            STYLES.title,
        )))
        .title_bottom(bottom_line);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    app.image_rect = inner;
}

fn draw_gallery(frame: &mut Frame<'_>, app: &mut App, area: Rect, status: Option<&str>) {
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(area);
    let search_rect = chunks[0];
    let gallery_area = chunks[1];
    draw_search_bar(frame, app, search_rect);

    // The title shows the fitted cell size, so the grid layout runs first.
    let gallery_block = Block::default()
        .borders(Borders::ALL)
        .border_style(STYLES.border);
    let grid_rect = gallery_block.inner(gallery_area);
    app.gallery.update_grid(grid_rect);

    let filtered_count = app.gallery.filtered_indices.len();
    let total = app.images.len();
    let scanning = if app.is_scanning() {
        ", scanning\u{2026}"
    } else {
        ""
    };
    let thumb_size = format!("{}x{}", app.gallery.thumb_cols, app.gallery.thumb_rows);
    let gallery_title = if app.gallery.search_query.is_empty() {
        format!(" Gallery [{total} images{scanning}] [{thumb_size}] ")
    } else {
        format!(" Gallery [{filtered_count}/{total} matches{scanning}] [{thumb_size}] ")
    };

    let bottom_line = gallery_bottom_line(app, filtered_count, status);

    let gallery_block = gallery_block
        .title(Line::from(Span::styled(gallery_title, STYLES.title)))
        .title_bottom(bottom_line);
    frame.render_widget(gallery_block, gallery_area);

    draw_thumbnail_cells(frame, app, grid_rect);
}

/// The search line above the grid: the query while typing, else the selection count.
fn draw_search_bar(frame: &mut Frame<'_>, app: &App, search_rect: Rect) {
    let search_block = Block::default()
        .borders(Borders::ALL)
        .border_style(STYLES.border)
        .title(Line::from(Span::styled(" Search ", STYLES.title)));

    let search_inner = search_block.inner(search_rect);
    frame.render_widget(search_block, search_rect);

    if app.gallery.search_active {
        let match_count = app.gallery.filtered_indices.len();
        let total = app.images.len();
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("/{}\u{2588}", app.gallery.search_query),
                    STYLES.text,
                ),
                Span::styled(format!("  {match_count}/{total} matches"), STYLES.dim),
            ])),
            search_inner,
        );
    } else if !app.selection.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {} selected", app.selection.len()),
                STYLES.dim,
            ))),
            search_inner,
        );
    } else {
        frame.render_widget(Paragraph::new(""), search_inner);
    }
}

/// The bottom border of the grid: a delete question, an error, or name, position and hints.
fn gallery_bottom_line(app: &App, filtered_count: usize, status: Option<&str>) -> Line<'static> {
    let selected_name = app
        .gallery
        .selected_index()
        .and_then(|i| app.images[i].file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    if let Some(pending) = &app.pending_delete {
        let action = if pending.permanent {
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
        Line::from(vec![
            Span::styled(prompt, STYLES.error),
            Span::styled(" y: confirm  n/Esc: cancel ", STYLES.key),
        ])
    } else if let Some(err) = &app.delete_error {
        Line::from(Span::styled(format!(" Delete error: {err} "), STYLES.error))
    } else if let Some(err) = &app.error {
        Line::from(Span::styled(format!(" Error: {err} "), STYLES.error))
    } else {
        let hint = if app.gallery.search_active {
            Span::styled(" Esc: cancel  Enter: confirm ", STYLES.key)
        } else if let Some(status) = status {
            Span::styled(format!(" {status} "), STYLES.text)
        } else {
            Span::styled(
                " Enter: open  +/-: zoom  n: names  i: info  /: search  ?: help  q: quit ",
                STYLES.key,
            )
        };
        if filtered_count == 0 {
            Line::from(hint)
        } else {
            Line::from(vec![
                Span::styled(
                    format!(
                        " {} [{}/{}] ",
                        selected_name,
                        app.gallery.cursor + 1,
                        filtered_count,
                    ),
                    STYLES.text,
                ),
                hint,
            ])
        }
    }
}

/// One bordered cell per visible thumbnail, with an optional filename label; Kitty draws the pictures.
fn draw_thumbnail_cells(frame: &mut Frame<'_>, app: &mut App, grid_rect: Rect) {
    app.gallery.update_grid(grid_rect);
    let thumb_cols = app.gallery.thumb_cols;
    let thumb_rows = app.gallery.thumb_rows;
    let cell_height_total = app.gallery.cell_height_total();

    for (vis_idx, img_idx) in app.gallery.visible_items().collect::<Vec<_>>() {
        let col = vis_idx % app.gallery.grid_cols;
        let row = vis_idx / app.gallery.grid_cols;

        let cell_x = grid_rect.x + (col as u16) * thumb_cols;
        let cell_y = grid_rect.y + (row as u16) * cell_height_total;

        let is_selected =
            vis_idx + app.gallery.scroll_offset * app.gallery.grid_cols == app.gallery.cursor;
        let is_marked = app.is_marked(img_idx);

        // Only the selected cell and the marked cells get a border. The selected style wins.
        let border_style = if is_selected {
            Some(STYLES.cursor_cell)
        } else if is_marked {
            Some(STYLES.marked)
        } else {
            None
        };
        if let Some(border_style) = border_style {
            let thumb_block = Block::default()
                .borders(Borders::ALL)
                .border_style(border_style);
            let block_rect = Rect {
                x: cell_x,
                y: cell_y,
                width: thumb_cols.min(grid_rect.right().saturating_sub(cell_x)),
                height: thumb_rows.min(grid_rect.bottom().saturating_sub(cell_y)),
            };
            frame.render_widget(thumb_block, block_rect);
        }

        if !app.gallery.show_filenames {
            continue;
        }

        let raw_filename = app.images[img_idx]
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let prefix_len = if is_marked { 2 } else { 0 };
        let max_label = (thumb_cols as usize).saturating_sub(2 + prefix_len);
        let trimmed = if raw_filename.chars().count() > max_label {
            let head: String = raw_filename
                .chars()
                .take(max_label.saturating_sub(1))
                .collect();
            format!("{head}\u{2026}")
        } else {
            raw_filename
        };
        let filename = if is_marked {
            format!("\u{25cf} {trimmed}")
        } else {
            trimmed
        };

        let label_y = cell_y + thumb_rows;
        if label_y < grid_rect.bottom() {
            let label_rect = Rect {
                x: cell_x,
                y: label_y,
                width: thumb_cols.min(grid_rect.right().saturating_sub(cell_x)),
                height: LABEL_ROWS,
            };
            let label_style = if is_selected {
                STYLES.cursor_cell
            } else if is_marked {
                STYLES.marked
            } else {
                STYLES.text
            };
            frame.render_widget(
                Paragraph::new(filename)
                    .style(label_style)
                    .alignment(Alignment::Center),
                label_rect,
            );
        }
    }
}

fn draw_help_popup(frame: &mut Frame<'_>) {
    let area = frame.area();

    let help_lines = [
        ("", "Gallery"),
        ("h/j/k/l", "Navigate grid"),
        ("g / G", "Jump to first / last"),
        ("Home / End", "Jump to first / last"),
        ("Enter", "Open fullscreen"),
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
        ("Esc", "Back to gallery"),
        ("?", "Toggle help"),
        ("q", "Quit"),
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
        ("Esc", "Back to gallery"),
        #[cfg(feature = "video")]
        ("q", "Quit"),
        ("", ""),
        ("", "Directory picker"),
        ("j / k", "Move cursor"),
        ("Enter", "Choose directory and open gallery"),
        ("l / Right", "Descend into directory"),
        ("h", "Parent directory"),
        ("g / G", "First / last"),
        ("/", "Filter names"),
        ("Esc", "Back to gallery"),
        ("q", "Quit"),
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
    use crate::app::{App, PendingDelete, ViewMode};
    use crate::image_list::SharedImageList;
    use crate::picker::PickerState;
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
        assert!(
            buffer.content().iter().all(|cell| cell.bg == Color::Reset),
            "a cell sets a background color"
        );
    }

    /// Fail when the cell does not carry the terminal foreground and `modifier`.
    fn assert_default_fg(cell: &Cell, modifier: Modifier) {
        assert_eq!((cell.fg, cell.modifier), (Color::Reset, modifier));
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
    fn pending(names: &[&str], permanent: bool) -> PendingDelete {
        PendingDelete {
            paths: names
                .iter()
                .map(|name| PathBuf::from("/photos").join(name))
                .collect(),
            from_fullscreen: false,
            permanent,
        }
    }

    #[test]
    fn gallery_shows_the_count_the_size_the_position_and_the_hints() {
        let mut app = app_with(&["a.jpg", "b.jpg"]);
        app.gallery.show_filenames = true;
        let screen = render(&mut app, None, 100, 30);
        let size = format!("[{}x{}]", app.gallery.thumb_cols, app.gallery.thumb_rows);
        assert_shows(&screen, &format!(" Gallery [2 images] {size} "));
        assert_shows(&screen, " Search ");
        assert_shows(&screen, " a.jpg [1/2] ");
        assert_shows(&screen, "Enter: open  +/-: zoom");
        assert_shows(&screen, "b.jpg");

        app.scan_complete = false;
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, "[2 images, scanning\u{2026}]");
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
        assert_shows(&screen, " Gallery [0 images] ");
        assert_shows(&screen, "\u{2514} Enter: open");
        assert_hides(&screen, "[1/");
    }

    #[test]
    fn gallery_search_shows_the_query_and_the_matches() {
        let mut app = app_with(&["a.jpg", "b.jpg"]);
        app.gallery.search_active = true;
        app.gallery.search_query = "b".to_owned();
        app.gallery.filtered_indices = vec![1];
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, "/b\u{2588}  1/2 matches");
        assert_shows(&screen, " Gallery [1/2 matches] ");
        assert_shows(&screen, " b.jpg [1/1]  Esc: cancel  Enter: confirm ");
    }

    #[test]
    fn gallery_shows_the_selection_count_and_marks_the_label() {
        let mut app = app_with(&["a.jpg", "b.jpg"]);
        app.gallery.show_filenames = true;
        app.selection.insert(app.images[1].clone());
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, " 1 selected");
        assert_shows(&screen, "\u{25cf} b.jpg");
    }

    #[test]
    fn gallery_draws_a_border_only_around_the_selected_and_the_marked_cells() {
        let mut app = app_with(&["a.jpg", "b.jpg", "c.jpg"]);
        let screen = render(&mut app, None, 100, 30);
        assert_eq!(
            screen.matches('\u{250c}').count(),
            3,
            "search bar + gallery block + cursor:\n{screen}"
        );

        app.selection.insert(app.images[2].clone());
        let screen = render(&mut app, None, 100, 30);
        assert_eq!(
            screen.matches('\u{250c}').count(),
            4,
            "+ one marked cell:\n{screen}"
        );
    }

    #[test]
    fn gallery_uses_the_terminal_defaults_and_the_ansi_roles() {
        let mut app = app_with(&["a.jpg", "b.jpg", "c.jpg"]);
        app.gallery.show_filenames = true;
        app.selection.insert(app.images[1].clone());
        let buffer = render_buffer(&mut app, None, 100, 30);
        assert_terminal_background(&buffer);

        // The grid starts inside the search bar (3 rows) and the gallery border.
        let (left, top) = (1, 4);
        let cursor_corner = &buffer[(left, top)];
        assert_eq!(cursor_corner.symbol(), "\u{250c}");
        assert_default_fg(cursor_corner, Modifier::BOLD);
        let marked_corner = &buffer[(left + app.gallery.thumb_cols, top)];
        assert_eq!(marked_corner.symbol(), "\u{250c}");
        assert_ansi(marked_corner, Color::Yellow);

        assert_default_fg(cell_of(&buffer, "a.jpg"), Modifier::BOLD);
        assert_ansi(cell_of(&buffer, "\u{25cf} b.jpg"), Color::Yellow);
        assert_default_fg(cell_of(&buffer, "c.jpg"), Modifier::empty());
        assert_ansi(&buffer[(0, 0)], Color::DarkGray);
        assert_default_fg(cell_of(&buffer, "Gallery ["), Modifier::BOLD);
        assert_ansi(cell_of(&buffer, "1 selected"), Color::DarkGray);
        assert_ansi(cell_of(&buffer, "Enter: open"), Color::Cyan);

        // On a marked cell under the cursor, the cursor style wins for the border and the label.
        app.selection.insert(app.images[0].clone());
        let buffer = render_buffer(&mut app, None, 100, 30);
        assert_default_fg(&buffer[(left, top)], Modifier::BOLD);
        assert_default_fg(cell_of(&buffer, "\u{25cf} a.jpg"), Modifier::BOLD);

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
        app.pending_delete = Some(pending(&["a.jpg"], false));
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, " Move to trash a.jpg?  y: confirm  n/Esc: cancel ");

        app.pending_delete = Some(pending(&["a.jpg", "b.jpg"], true));
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
        assert_shows(&screen, " b.jpg ");
        assert_shows(&screen, " b.jpg [2/2] 30x20  \u{2190}/\u{2192}: navigate");
        assert_eq!(app.image_rect, Rect::new(1, 1, 98, 28));
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
        assert_shows(&screen, "space: pause  [/]: interval  s: stop");

        app.slideshow_toggle_pause();
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, "\u{23f8}");
        assert_shows(&screen, &interval);
    }

    #[test]
    fn fullscreen_asks_before_a_delete_and_shows_errors() {
        let mut app = app_with(&["a.jpg", "b.jpg"]);
        app.mode = ViewMode::Fullscreen;
        app.pending_delete = Some(pending(&["a.jpg"], true));
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, " Permanently delete a.jpg? [y/N] ");

        app.pending_delete = Some(pending(&[], false));
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
        assert_ansi(&buffer[(0, 0)], Color::DarkGray);
        assert_default_fg(cell_of(&buffer, "a.jpg "), Modifier::BOLD);
        assert_default_fg(cell_of(&buffer, "a.jpg ["), Modifier::empty());
        assert_ansi(cell_of(&buffer, "\u{2190}/\u{2192}: navigate"), Color::Cyan);

        app.pending_delete = Some(pending(&["a.jpg"], false));
        let buffer = render_buffer(&mut app, None, 100, 30);
        assert_ansi(cell_of(&buffer, "Move to trash a.jpg?"), Color::Red);
    }

    #[cfg(feature = "video")]
    #[test]
    fn video_status_bar_shows_the_time_the_position_and_the_note() {
        let mut app = app_with(&["clip.mp4", "b.jpg"]);
        app.mode = ViewMode::Video;
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, " clip.mp4 ");
        assert_shows(&screen, "\u{25b6} clip.mp4 00:00/00:00 [0fps] [1/2] ");
        assert_shows(&screen, "Space: pause");
        assert_eq!(app.image_rect, Rect::new(1, 1, 98, 28));

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
