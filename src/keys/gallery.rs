//! The keys of the thumbnail grid and of its search line.

use super::delete_key;
use crate::app::App;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::io;

/// Keys while the gallery search line takes input.
pub(super) fn handle_search_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            app.gallery.search_active = false;
            if !app.gallery.search_query.is_empty() {
                app.gallery.search_query.clear();
                app.update_filter();
            }
        }
        KeyCode::Enter => {
            app.gallery.search_active = false;
        }
        KeyCode::Backspace if app.gallery.search_query.pop().is_some() => {
            app.update_filter();
        }
        KeyCode::Char(c) => {
            app.gallery.search_query.push(c);
            app.update_filter();
        }
        _ => {}
    }
}

/// Keys of the thumbnail grid.
pub(super) fn handle_gallery_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    let prev_offset = app.gallery.scroll_offset;
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
        KeyCode::Char('?') => {
            app.graphics_delete_all()?;
            app.help_visible = true;
        }
        KeyCode::Char('/') => {
            app.gallery.search_active = true;
        }
        KeyCode::Enter => {
            app.graphics_delete_all()?;
            app.enter_fullscreen_selected();
        }
        KeyCode::Char('s') => {
            app.graphics_delete_all()?;
            app.enter_fullscreen_selected();
            if app.in_slideshow_view() {
                app.start_slideshow();
            }
        }
        KeyCode::Char('i') => {
            app.graphics_delete_all()?;
            app.toggle_info();
        }
        // A zoom change marks the grid dirty, and the next grid write deletes the old
        // pictures. A key at a limit changes nothing, so it deletes nothing.
        KeyCode::Char('+' | '=') => {
            app.thumb_zoom_in();
        }
        KeyCode::Char('-' | '_') => {
            app.thumb_zoom_out();
        }
        KeyCode::Char('0') => {
            app.thumb_zoom_reset();
        }
        KeyCode::Char('n') => {
            app.toggle_filenames();
        }
        KeyCode::Left | KeyCode::Char('h') => {
            app.gallery.move_left();
            app.pre_decode_hovered();
        }
        KeyCode::Right | KeyCode::Char('l') => {
            app.gallery.move_right();
            app.pre_decode_hovered();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.gallery.move_up();
            app.pre_decode_hovered();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.gallery.move_down();
            app.pre_decode_hovered();
        }
        KeyCode::PageUp | KeyCode::Char('b') if ctrl || matches!(key.code, KeyCode::PageUp) => {
            app.gallery.move_page_up();
            app.pre_decode_hovered();
        }
        KeyCode::PageDown | KeyCode::Char('f') if ctrl || matches!(key.code, KeyCode::PageDown) => {
            app.gallery.move_page_down();
            app.pre_decode_hovered();
        }
        KeyCode::Char('g') | KeyCode::Home => {
            app.gallery.move_to_first();
            app.pre_decode_hovered();
        }
        KeyCode::Char('G') | KeyCode::End => {
            app.gallery.move_to_last();
            app.pre_decode_hovered();
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
        KeyCode::Char('d' | 'D') => delete_key(app, key.code),
        KeyCode::Char('o') => {
            app.graphics_delete_all()?;
            app.open_picker();
        }
        _ => {}
    }
    if app.gallery.scroll_offset != prev_offset {
        app.needs_render = true;
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use crate::app::ViewMode;

    use crate::test_support::{
        char_key, gallery_app, photo_paths, press, press_with, synthetic_tree,
    };
    use crossterm::event::{KeyCode, KeyModifiers};

    #[test]
    fn gallery_arrows_move_the_cursor_in_the_grid() {
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
    fn gallery_jumps_by_page_and_to_the_ends() {
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
    fn gallery_selection_keys_and_a_delete_of_the_selection() {
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
        char_key(&mut app, 'd');
        let pending = app.pending_delete.as_ref().unwrap();
        assert_eq!(pending.paths, photo_paths(3)[1..]);
        assert!(!pending.from_fullscreen);
        char_key(&mut app, 'n');
    }

    #[test]
    fn gallery_enter_and_s_open_the_fullscreen_view() {
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
    fn gallery_s_without_a_photo_starts_no_slideshow() {
        let mut app = gallery_app(0);

        char_key(&mut app, 's');

        assert_eq!(app.mode, ViewMode::Gallery);
        assert!(!app.slideshow_running());
    }

    #[test]
    fn gallery_info_help_quit_and_other_keys() {
        let mut app = gallery_app(2);
        let info = app.info_visible;

        char_key(&mut app, 'i');
        assert_ne!(app.info_visible, info);
        char_key(&mut app, '?');
        assert!(app.help_visible);
        press(&mut app, KeyCode::Tab);
        assert!(!app.help_visible);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.mode, ViewMode::Gallery);

        assert!(char_key(&mut app, 'q'));
        assert!(press(&mut app, KeyCode::Esc));
    }

    #[test]
    fn gallery_n_toggles_the_filename_labels() {
        let mut app = gallery_app(2);
        let compact_thumbs = app.gallery.thumb_rows;
        assert!(!app.gallery.show_filenames);

        app.needs_render = false;
        char_key(&mut app, 'n');
        assert!(app.gallery.show_filenames);
        assert_eq!(app.gallery.thumb_rows, compact_thumbs - 1);
        assert!(app.needs_render);

        char_key(&mut app, 'n');
        assert!(!app.gallery.show_filenames);
        assert_eq!(app.gallery.thumb_rows, compact_thumbs);
    }

    /// The zoom keys stop at two rows and at the most rows that fit the terminal.
    #[test]
    fn gallery_zoom_keys_stop_at_the_limits() {
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
            app.gallery.zoom_rows, 5,
            "24 rows: 1 edge gap, then 3 rows and 1 gap each"
        );
        char_key(&mut app, '-');
        assert_eq!(app.gallery.zoom_rows, 5);
    }

    #[test]
    fn gallery_search_keys_edit_the_query() {
        let mut app = gallery_app(3);

        char_key(&mut app, '/');
        assert!(app.gallery.search_active);
        char_key(&mut app, 'q');
        char_key(&mut app, '7');
        assert_eq!(app.gallery.search_query, "q7", "q is text in the search");
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.gallery.search_query, "q");
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.gallery.search_query, "q");
        press(&mut app, KeyCode::Enter);
        assert!(!app.gallery.search_active);
        assert_eq!(app.gallery.search_query, "q", "Enter keeps the filter");

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

    #[test]
    fn gallery_o_opens_the_directory_picker() {
        let (_temp, root, _child) = synthetic_tree();
        let mut app = gallery_app(2);
        app.initial_dir = Some(root.clone());

        char_key(&mut app, 'o');

        assert_eq!(app.mode, ViewMode::Picker);
        assert_eq!(app.picker.as_ref().unwrap().current_dir, root);
        press(&mut app, KeyCode::Esc);
        assert_eq!(
            app.mode,
            ViewMode::Gallery,
            "Esc closes the picker over photos"
        );
    }

    #[test]
    fn gallery_o_then_q_goes_back_over_photos() {
        let (_temp, root, _child) = synthetic_tree();
        let mut app = gallery_app(2);
        app.initial_dir = Some(root);

        char_key(&mut app, 'o');
        assert!(!char_key(&mut app, 'q'), "q in the picker goes back");
        assert_eq!(app.mode, ViewMode::Gallery);
        assert!(char_key(&mut app, 'q'), "q in the grid quits viewr");
    }
}
