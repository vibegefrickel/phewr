//! The status row of each view: a question, an error or the position at the left, and the facts
//! of the selected file at the right.

use super::STYLES;
use crate::app::{App, DeleteMode};
use ratatui::{prelude::*, widgets::Paragraph};

/// The status row: `line` at the left, the facts of the file at the right. The facts give way
/// to a status note, a question, an error and a line too long for both.
pub(super) fn draw_status_row(
    frame: &mut Frame<'_>,
    app: &App,
    area: Rect,
    line: Line<'static>,
    status: Option<&str>,
) {
    let busy = status.is_some()
        || app.error.is_some()
        || app.scan_error.is_some()
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

/// The status row of the fullscreen view: a delete question, an error, or the name, the position,
/// the zoom, the slideshow state and the status note.
pub(super) fn fullscreen_status_line(app: &App, status: Option<&str>) -> Line<'static> {
    if let Some(line) = question_or_error(app) {
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
    spans.extend(status.map(|note| Span::styled(format!(" {note} "), STYLES.text)));
    Line::from(spans)
}

/// The glyph of the video status row while the player plays.
#[cfg(feature = "video")]
const PLAYING_GLYPH: &str = "\u{25b6}";
/// The glyph of the video status row after the player reached the end of a clip that does not
/// loop. The player has no pause, so the glyph means "finished".
#[cfg(feature = "video")]
const FINISHED_GLYPH: &str = "\u{23f8}";

/// The status row of the terminal player: an error, or the state, the name, the time, the
/// position and the status note.
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
    // The dropped frames show in a debug build only, so the maintainer sees a slow terminal.
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

/// The status row of the gallery: a delete question, an error, the search query while typing,
/// or name, position, entry counts, selection count, scan note and the status note.
pub(super) fn gallery_status_line(
    app: &App,
    filtered_count: usize,
    status: Option<&str>,
) -> Line<'static> {
    if let Some(line) = question_or_error(app) {
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

/// The delete question, the delete error, the error or the scan error, in this order. They replace the rest of
/// the status row in the gallery and the fullscreen view.
fn question_or_error(app: &App) -> Option<Line<'static>> {
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
        return Some(Line::from(Span::styled(prompt, STYLES.error)));
    }
    if let Some(err) = &app.delete_error {
        return Some(Line::from(Span::styled(
            format!(" Delete error: {err} "),
            STYLES.error,
        )));
    }
    let err = app.error.as_ref().or(app.scan_error.as_ref())?;
    Some(Line::from(Span::styled(
        format!(" Error: {err} "),
        STYLES.error,
    )))
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

/// The folder and the image count of a grid with folders. `..` does not count.
fn entry_counts(app: &App) -> String {
    let folders = subfolder_count(app);
    let folder_word = if folders == 1 { "folder" } else { "folders" };
    let images = app.file_count();
    let image_word = if images == 1 { "image" } else { "images" };
    format!(" {folders} {folder_word}, {images} {image_word} ")
}

#[cfg(test)]
mod tests {
    use crate::app::{DeleteMode, PendingDelete, ViewMode};
    use crate::ui::tests::{
        app_with, assert_ansi, assert_default_fg, assert_hides, assert_shows, assert_symbol,
        assert_terminal_background, cell_of, render, render_buffer, screen_rows, status_row,
    };
    use image::RgbaImage;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier};
    use std::path::PathBuf;

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
    fn gallery_shows_the_scan_error_after_the_error() {
        let mut app = app_with(&["a.jpg"]);
        app.scan_error = Some("denied".to_owned());
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, " Error: denied ");

        app.error = Some("scan failed".to_owned());
        let screen = render(&mut app, None, 100, 30);
        assert_shows(&screen, " Error: scan failed ");
        assert_hides(&screen, "denied");
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
        app.video = Some(player);
        let screen = render(&mut app, None, 100, 30);
        assert!(
            status_row(&screen).starts_with(" \u{25b6} clip.mp4 00:00/1:02:05 [1/2] "),
            "{screen}"
        );
        assert_eq!(crate::file_meta::format_time(65.9), "01:05");

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
}
