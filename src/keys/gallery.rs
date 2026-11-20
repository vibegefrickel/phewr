//! The keys of the thumbnail grid and of its search line.

use crate::app::App;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::io;
use tuilibr::text_field::edit_for_key;

/// The keys of the search line: Enter applies the filter, Esc clears it, and the readline keys of
/// `edit_for_key` edit the field. Only a change of the text runs the filter again.
pub(super) fn handle_search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.gallery.search_active = false;
            if app.gallery.search_query.clear() {
                app.update_filter();
            }
        }
        KeyCode::Enter => {
            app.gallery.search_active = false;
        }
        _ => {
            let Some(edit) = edit_for_key(&key) else {
                return;
            };
            // The loop draws the text on each pass, so a cursor move needs no picture write.
            let before = app.gallery.search_query.text().to_owned();
            if app.gallery.search_query.apply(edit) && app.gallery.search_query.text() != before {
                app.update_filter();
            }
        }
    }
}

pub(super) fn handle_gallery_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    let prev_offset = app.gallery.scroll_offset;
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Char('?') => {
            app.graphics_delete_all()?;
            app.help_visible = true;
        }
        KeyCode::Char('/') => {
            app.gallery.search_active = true;
        }
        // The next grid write deletes the old pictures.
        KeyCode::Enter if app.selected_is_folder() => {
            app.enter_selected_folder();
        }
        KeyCode::Backspace => {
            app.go_to_parent();
        }
        KeyCode::Enter => {
            app.graphics_delete_all()?;
            app.enter_fullscreen_selected();
        }
        KeyCode::Char('s') if app.selected_is_folder() => {}
        KeyCode::Char('s') => {
            app.graphics_delete_all()?;
            app.enter_fullscreen_selected();
            if app.in_slideshow_view() {
                app.start_slideshow();
            }
        }
        #[cfg(feature = "video")]
        KeyCode::Char('p') if app.selected_is_video() => {
            app.graphics_delete_all()?;
            app.enter_fullscreen_selected();
            app.request_video_window();
        }
        KeyCode::Tab => app.toggle_info()?,
        KeyCode::Char('B') => app.toggle_layout()?,
        // A zoom marks the grid dirty, and the next grid write deletes the old pictures.
        KeyCode::Char('+' | '=') => {
            app.thumb_zoom_in();
        }
        KeyCode::Char('-' | '_') => {
            app.thumb_zoom_out();
        }
        KeyCode::Char('0') => {
            app.thumb_zoom_reset();
        }
        KeyCode::Char(' ') => {
            app.toggle_selection_at_cursor();
        }
        KeyCode::Char('a') => {
            app.select_all_filtered();
        }
        KeyCode::Char('A') => {
            app.clear_selection();
        }
        code => {
            if move_cursor(app, code, ctrl) {
                app.pre_decode_hovered();
            }
        }
    }
    if app.gallery.scroll_offset != prev_offset {
        app.needs_render = true;
    }
    Ok(false)
}

/// Move the grid cursor. Returns `false` for a key that moves nothing.
fn move_cursor(app: &mut App, code: KeyCode, ctrl: bool) -> bool {
    match code {
        KeyCode::Left | KeyCode::Char('h') => app.gallery.move_left(),
        KeyCode::Right | KeyCode::Char('l') => app.gallery.move_right(),
        KeyCode::Up | KeyCode::Char('k') => app.gallery.move_up(),
        KeyCode::Down | KeyCode::Char('j') => app.gallery.move_down(),
        KeyCode::PageUp => app.gallery.move_page_up(),
        KeyCode::Char('b') if ctrl => app.gallery.move_page_up(),
        KeyCode::PageDown => app.gallery.move_page_down(),
        KeyCode::Char('f') if ctrl => app.gallery.move_page_down(),
        KeyCode::Char('g') | KeyCode::Home => app.gallery.move_to_first(),
        KeyCode::Char('G') | KeyCode::End => app.gallery.move_to_last(),
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use crate::app::ViewMode;

    use crate::test_support::{
        char_key, folder_app, gallery_app, press, press_with, text, wait_until,
    };
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::fs;
    use tuilibr::frame::Layout;

    #[test]
    fn enter_and_backspace_walk_folders() {
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        fs::create_dir(root.join("sub")).unwrap();
        // Two photos: one image opens the fullscreen view.
        fs::write(root.join("sub").join("photo.png"), b"synthetic").unwrap();
        fs::write(root.join("sub").join("other.png"), b"synthetic").unwrap();
        let mut app = gallery_app(0);
        app.switch_to_dir(&root);
        wait_until(|| {
            app.refresh_from_scanner();
            !app.is_scanning()
        });
        assert_eq!(app.filenames, ["..", "sub"]);

        // Space, a and s do nothing on a folder.
        press(&mut app, KeyCode::Right);
        char_key(&mut app, ' ');
        char_key(&mut app, 'a');
        assert!(app.selection.is_empty());
        char_key(&mut app, 's');
        assert_eq!(app.mode, ViewMode::Gallery);
        assert!(app.slideshow.is_none());
        assert!(
            app.graphics_out.take_bytes().is_empty(),
            "no delete of the pictures"
        );

        press(&mut app, KeyCode::Enter);
        assert_eq!(app.current_dir.as_deref(), Some(root.join("sub").as_path()));
        wait_until(|| {
            app.refresh_from_scanner();
            !app.is_scanning()
        });
        assert_eq!(app.filenames, ["..", "other.png", "photo.png"]);

        press(&mut app, KeyCode::Backspace);
        wait_until(|| {
            app.refresh_from_scanner();
            !app.is_scanning()
        });
        assert_eq!(app.current_dir.as_deref(), Some(root.as_path()));
        assert_eq!(app.gallery.cursor, 1, "the cursor is on sub");
    }

    #[test]
    fn enter_on_file_opens_fullscreen() {
        let mut app = folder_app(&["..", "sub"], &["a.jpg", "b.jpg"]);
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.mode,
            ViewMode::Gallery,
            "Enter on .. switches, nothing to see"
        );
        let mut app = folder_app(&["..", "sub"], &["a.jpg", "b.jpg"]);
        app.gallery.cursor = 2;
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert_eq!(app.current, 2);
        // The steps stop at the first file.
        press(&mut app, KeyCode::Left);
        assert_eq!(app.current, 2);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.current, 3);
        press(&mut app, KeyCode::Home);
        assert_eq!(app.current, 2);
    }

    #[test]
    fn arrows_move_cursor() {
        let mut app = gallery_app(9);

        press(&mut app, KeyCode::Right);
        char_key(&mut app, 'l');
        assert_eq!(app.gallery.cursor, 2);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.gallery.cursor, 5);
        char_key(&mut app, 'j');
        assert_eq!(app.gallery.cursor, 8);
        assert_eq!(app.gallery.scroll_offset, 1);
        assert!(app.needs_render, "a scroll redraws the pictures");
        press(&mut app, KeyCode::Left);
        char_key(&mut app, 'h');
        assert_eq!(app.gallery.cursor, 6);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.gallery.cursor, 3);
        char_key(&mut app, 'k');
        assert_eq!(app.gallery.cursor, 0);
        assert_eq!(app.gallery.scroll_offset, 0);
    }

    #[test]
    fn jumps_by_page_and_to_ends() {
        let mut app = gallery_app(9);

        press(&mut app, KeyCode::PageDown);
        assert_eq!(app.gallery.cursor, 6);
        press(&mut app, KeyCode::PageUp);
        assert_eq!(app.gallery.cursor, 0);
        press_with(&mut app, KeyCode::Char('f'), KeyModifiers::CONTROL);
        assert_eq!(app.gallery.cursor, 6);
        press_with(&mut app, KeyCode::Char('b'), KeyModifiers::CONTROL);
        assert_eq!(app.gallery.cursor, 0);
        char_key(&mut app, 'f');
        assert_eq!(app.gallery.cursor, 0, "f without Ctrl does nothing");

        char_key(&mut app, 'G');
        assert_eq!(app.gallery.cursor, 8);
        char_key(&mut app, 'b');
        assert_eq!(app.gallery.cursor, 8, "b without Ctrl does nothing");
        char_key(&mut app, 'g');
        assert_eq!(app.gallery.cursor, 0);
        press(&mut app, KeyCode::End);
        assert_eq!(app.gallery.cursor, 8);
        press(&mut app, KeyCode::Home);
        assert_eq!(app.gallery.cursor, 0);
    }

    #[test]
    fn selection_keys_and_delete() {
        let mut app = gallery_app(4);

        char_key(&mut app, ' ');
        assert!(app.is_marked(0));
        char_key(&mut app, ' ');
        assert!(!app.is_marked(0));
        char_key(&mut app, 'a');
        assert_eq!(app.selection.len(), 4);
        char_key(&mut app, 'A');
        assert!(app.selection.is_empty());

        press(&mut app, KeyCode::Right);
        char_key(&mut app, ' ');
        press(&mut app, KeyCode::Right);
        char_key(&mut app, ' ');
        assert_eq!(app.selection.len(), 2);
    }

    #[test]
    fn enter_and_s_open_fullscreen() {
        let mut app = gallery_app(4);
        press(&mut app, KeyCode::Right);

        press(&mut app, KeyCode::Enter);
        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert_eq!(app.current, 1);
        assert!(!app.slideshow_running());

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, ViewMode::Gallery);
        char_key(&mut app, 's');
        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert!(app.slideshow_running());
    }

    #[test]
    fn s_without_photo_starts_nothing() {
        let mut app = gallery_app(0);

        char_key(&mut app, 's');

        assert_eq!(app.mode, ViewMode::Gallery);
        assert!(!app.slideshow_running());
    }

    #[test]
    fn info_help_quit_and_other_keys() {
        let mut app = gallery_app(2);
        let info = app.info_visible;

        press(&mut app, KeyCode::Tab);
        assert_ne!(app.info_visible, info);
        assert!(text(&app.graphics_out.take_bytes()).contains("a=d"));
        char_key(&mut app, 'B');
        assert_eq!(app.pane_layout, Layout::Minimal);
        assert!(text(&app.graphics_out.take_bytes()).contains("a=d"));
        char_key(&mut app, 'B');
        assert_eq!(app.pane_layout, Layout::Maximal);
        char_key(&mut app, '?');
        assert!(app.help_visible);
        char_key(&mut app, 'i');
        assert!(!app.help_visible);
        char_key(&mut app, 'i');
        assert_eq!(app.mode, ViewMode::Gallery);

        assert!(!press(&mut app, KeyCode::Esc), "Esc does not quit the grid");
        assert!(char_key(&mut app, 'q'));
    }

    /// `n` is free: it changes no layout.
    #[test]
    fn n_changes_nothing() {
        let mut app = gallery_app(2);
        let layout = (app.gallery.thumb_rows, app.gallery.grid_cols);
        let thumb_rect = app.gallery.thumb_rect(0);

        char_key(&mut app, 'n');

        assert_eq!((app.gallery.thumb_rows, app.gallery.grid_cols), layout);
        assert_eq!(app.gallery.thumb_rect(0), thumb_rect);
        assert_eq!(app.mode, ViewMode::Gallery);
    }

    /// The zoom keys stop at two rows and at the most rows that fit the terminal.
    #[test]
    fn zoom_keys_stop_at_limits() {
        let mut app = gallery_app(2);
        for _ in 0..40 {
            char_key(&mut app, '+');
        }
        assert_eq!(app.gallery.zoom_rows, 2);
        char_key(&mut app, '+');
        assert_eq!(app.gallery.zoom_rows, 2);

        for _ in 0..40 {
            char_key(&mut app, '-');
        }
        assert_eq!(
            app.gallery.zoom_rows, 3,
            "24 rows: 3 rows of 6 are the last level with the large icon"
        );
        char_key(&mut app, '-');
        assert_eq!(app.gallery.zoom_rows, 3);
    }

    #[test]
    fn modifier_keys_do_not_type() {
        let mut app = gallery_app(3);
        char_key(&mut app, '/');
        char_key(&mut app, 'q');
        press_with(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
        press_with(&mut app, KeyCode::Char('w'), KeyModifiers::ALT);
        assert_eq!(app.gallery.search_query.text(), "q");
    }

    #[test]
    fn search_keys_edit_query() {
        let mut app = gallery_app(3);

        char_key(&mut app, '/');
        assert!(app.gallery.search_active);
        char_key(&mut app, 'q');
        char_key(&mut app, '7');
        assert_eq!(
            app.gallery.search_query.text(),
            "q7",
            "q is text in the search"
        );
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.gallery.search_query.text(), "q");
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.gallery.search_query.text(), "q");
        press(&mut app, KeyCode::Enter);
        assert!(!app.gallery.search_active);
        assert_eq!(
            app.gallery.search_query.text(),
            "q",
            "Enter keeps the filter"
        );

        char_key(&mut app, '/');
        press(&mut app, KeyCode::Esc);
        assert!(!app.gallery.search_active);
        assert!(app.gallery.search_query.is_empty(), "Esc clears the filter");
        assert_eq!(app.gallery.filtered_indices, [0, 1, 2]);

        char_key(&mut app, '/');
        press(&mut app, KeyCode::Backspace);
        press(&mut app, KeyCode::Esc);
        assert!(!app.gallery.search_active);
        assert_eq!(app.mode, ViewMode::Gallery);
    }

    /// Types `text` into an open search line.
    fn type_text(app: &mut crate::app::App, text: &str) {
        for character in text.chars() {
            char_key(app, character);
        }
    }

    #[test]
    fn search_cursor_moves_and_inserts() {
        let mut app = gallery_app(3);
        char_key(&mut app, '/');
        type_text(&mut app, "bc");
        press(&mut app, KeyCode::Home);
        assert_eq!(app.gallery.search_query.cursor(), 0);
        char_key(&mut app, 'a');
        assert_eq!(app.gallery.search_query.text(), "abc");
        press(&mut app, KeyCode::End);
        press(&mut app, KeyCode::Left);
        char_key(&mut app, 'x');
        assert_eq!(app.gallery.search_query.text(), "abxc");
        press(&mut app, KeyCode::Right);
        assert_eq!(app.gallery.search_query.cursor(), 4);
        press_with(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert_eq!(
            app.gallery.search_query.cursor(),
            0,
            "Ctrl-A goes to the start"
        );
        press_with(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL);
        assert_eq!(
            app.gallery.search_query.cursor(),
            4,
            "Ctrl-E goes to the end"
        );
    }

    #[test]
    fn search_word_keys() {
        let mut app = gallery_app(3);
        char_key(&mut app, '/');
        type_text(&mut app, "red car");
        press_with(&mut app, KeyCode::Left, KeyModifiers::ALT);
        assert_eq!(
            app.gallery.search_query.cursor(),
            4,
            "Alt-Left goes to the word start"
        );
        press_with(&mut app, KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert_eq!(
            app.gallery.search_query.text(),
            "red ",
            "Ctrl-K deletes to the end"
        );
        type_text(&mut app, "big car");
        press_with(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
        assert_eq!(
            app.gallery.search_query.text(),
            "red big ",
            "Ctrl-W deletes a word"
        );
        press_with(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert!(
            app.gallery.search_query.is_empty(),
            "Ctrl-U deletes to the start"
        );
        assert!(
            app.gallery.search_active,
            "an edit keeps the search line open"
        );
    }

    #[test]
    fn search_cursor_move_keeps_the_filter() {
        let mut app = gallery_app(3);
        char_key(&mut app, '/');
        // An empty query resets the list at once, so a run of the filter shows in the list.
        app.gallery.filtered_indices = vec![1];
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Home);
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.gallery.filtered_indices, [1], "no edit runs no filter");
        char_key(&mut app, 'a');
        press(&mut app, KeyCode::Backspace);
        assert_eq!(
            app.gallery.filtered_indices,
            [0, 1, 2],
            "an edit runs the filter"
        );
    }
}
