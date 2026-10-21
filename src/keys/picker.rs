//! The keys of the directory picker.

use crate::app::App;
use crate::picker::PickerState;
use crossterm::event::KeyCode;

pub(super) fn handle_picker_key(app: &mut App, code: KeyCode) -> bool {
    let filter_active = app.picker.as_ref().is_some_and(|p| p.filter_active);

    if filter_active {
        let Some(picker) = app.picker.as_mut() else {
            return false;
        };
        match code {
            KeyCode::Esc => {
                picker.filter_active = false;
                picker.filter.clear();
                picker.rebuild_filter();
                app.needs_render = true;
            }
            KeyCode::Enter => {
                picker.filter_active = false;
                app.needs_render = true;
            }
            KeyCode::Backspace => {
                picker.filter.pop();
                picker.rebuild_filter();
                app.needs_render = true;
            }
            KeyCode::Char(c) => {
                picker.filter.push(c);
                picker.rebuild_filter();
                app.needs_render = true;
            }
            _ => {}
        }
        return false;
    }

    match code {
        KeyCode::Char('q') => return true,
        KeyCode::Esc if !app.images.is_empty() => {
            app.close_picker();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            with_picker(app, PickerState::move_up);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            with_picker(app, PickerState::move_down);
        }
        KeyCode::Home | KeyCode::Char('g') => {
            with_picker(app, PickerState::move_first);
        }
        KeyCode::End | KeyCode::Char('G') => {
            with_picker(app, PickerState::move_last);
        }
        KeyCode::PageUp => {
            with_picker(app, |picker| picker.move_page_up(10));
        }
        KeyCode::PageDown => {
            with_picker(app, |picker| picker.move_page_down(10));
        }
        KeyCode::Left | KeyCode::Char('h') => {
            let target = app.picker.as_mut().and_then(PickerState::ascend);
            if let Some(t) = target {
                app.switch_to_dir(&t);
            }
        }
        KeyCode::Right | KeyCode::Char('l') => {
            let target = app.picker.as_mut().and_then(|p| {
                let sel = p.selected()?;
                if sel.is_parent {
                    return p.ascend();
                }
                p.enter_selected()
            });
            if let Some(t) = target {
                app.switch_to_dir(&t);
            }
        }
        KeyCode::Enter => {
            let target = app
                .picker
                .as_ref()
                .and_then(|picker| picker.selected())
                .map(|entry| entry.path.clone());
            if let Some(target) = target {
                app.switch_to_dir(&target);
                app.close_picker();
            }
        }
        KeyCode::Char('/') => {
            with_picker(app, |picker| {
                picker.filter_active = true;
                picker.filter.clear();
                picker.rebuild_filter();
            });
        }
        KeyCode::Char('?') => {
            app.help_visible = true;
            app.needs_render = true;
        }
        _ => {}
    }
    false
}

/// Change the picker with `change` and draw it again. Without a picker, nothing happens.
fn with_picker(app: &mut App, change: impl FnOnce(&mut PickerState)) {
    if let Some(picker) = app.picker.as_mut() {
        change(picker);
        app.needs_render = true;
    }
}

#[cfg(test)]
mod tests {
    use super::handle_picker_key;
    use crate::app::{App, ViewMode};
    use crate::image_list::SharedImageList;
    use crate::test_support::{char_key, press, synthetic_tree};
    use crossterm::event::KeyCode;
    use std::fs;
    use std::path::Path;
    use std::time::Duration;
    use tempfile::TempDir;

    fn picker_app(root: &Path) -> App {
        let list = SharedImageList::new();
        let mut app = App::new((8, 16), list);
        app.initial_dir = Some(root.to_path_buf());
        app.open_picker();
        app
    }

    /// Drop the tree after a short wait: `switch_to_dir` starts a scanner thread in it.
    fn cleanup(temp: TempDir) {
        std::thread::sleep(Duration::from_millis(20));
        drop(temp);
    }

    fn select_path(app: &mut App, target: &Path) {
        let picker = app.picker.as_mut().unwrap();
        let entry_index = picker
            .entries
            .iter()
            .position(|entry| entry.path == target)
            .unwrap();
        picker.cursor = picker
            .filtered_indices
            .iter()
            .position(|index| *index == entry_index)
            .unwrap();
    }

    #[test]
    fn enter_chooses_directory_and_closes_picker() {
        let (temp, root, child) = synthetic_tree();
        let mut app = picker_app(&root);
        select_path(&mut app, &child);

        assert!(!handle_picker_key(&mut app, KeyCode::Enter));
        assert_eq!(app.mode, ViewMode::Gallery);
        assert!(app.picker.is_none());
        assert_eq!(app.current_dir, Some(fs::canonicalize(&child).unwrap()));

        cleanup(temp);
    }

    #[test]
    fn right_descends_without_closing_picker() {
        let (temp, root, child) = synthetic_tree();
        let mut app = picker_app(&root);
        select_path(&mut app, &child);

        assert!(!handle_picker_key(&mut app, KeyCode::Right));
        assert_eq!(app.mode, ViewMode::Picker);
        assert_eq!(
            app.picker.as_ref().map(|picker| &picker.current_dir),
            Some(&fs::canonicalize(&child).unwrap())
        );

        cleanup(temp);
    }

    /// A picker in a temp directory with the directories a, b and c.
    fn picker_tree() -> (TempDir, App) {
        let (temp, root, child) = synthetic_tree();
        fs::remove_dir(child).unwrap();
        for name in ["a", "b", "c"] {
            fs::create_dir(root.join(name)).unwrap();
        }
        let app = picker_app(&root);
        (temp, app)
    }

    fn picker_cursor(app: &App) -> usize {
        app.picker.as_ref().unwrap().cursor
    }

    #[test]
    fn picker_keys_move_the_cursor() {
        let (temp, mut app) = picker_tree();

        press(&mut app, KeyCode::Down);
        char_key(&mut app, 'j');
        assert_eq!(picker_cursor(&app), 2);
        press(&mut app, KeyCode::Up);
        assert_eq!(picker_cursor(&app), 1);
        char_key(&mut app, 'k');
        assert_eq!(picker_cursor(&app), 0);
        char_key(&mut app, 'G');
        assert_eq!(picker_cursor(&app), 3, "the parent entry and a, b, c");
        char_key(&mut app, 'g');
        assert_eq!(picker_cursor(&app), 0);
        press(&mut app, KeyCode::End);
        assert_eq!(picker_cursor(&app), 3);
        press(&mut app, KeyCode::Home);
        assert_eq!(picker_cursor(&app), 0);
        press(&mut app, KeyCode::PageDown);
        assert_eq!(picker_cursor(&app), 3);
        press(&mut app, KeyCode::PageUp);
        assert_eq!(picker_cursor(&app), 0);

        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, ViewMode::Picker, "Esc needs photos to go back to");
        char_key(&mut app, '?');
        assert!(app.help_visible);
        char_key(&mut app, 'x');
        assert!(char_key(&mut app, 'q'));

        cleanup(temp);
    }

    #[test]
    fn picker_filter_keys() {
        let (temp, mut app) = picker_tree();

        char_key(&mut app, '/');
        assert!(app.picker.as_ref().unwrap().filter_active);
        char_key(&mut app, 'b');
        char_key(&mut app, 't');
        assert_eq!(app.picker.as_ref().unwrap().filter, "bt");
        press(&mut app, KeyCode::Backspace);
        press(&mut app, KeyCode::Tab);
        let picker = app.picker.as_ref().unwrap();
        assert_eq!(picker.filter, "b");
        assert_eq!(picker.selected().unwrap().name, "b");

        press(&mut app, KeyCode::Enter);
        let picker = app.picker.as_ref().unwrap();
        assert!(!picker.filter_active);
        assert_eq!(picker.filter, "b", "Enter keeps the filter");
        assert_eq!(app.mode, ViewMode::Picker);

        char_key(&mut app, '/');
        assert!(
            app.picker.as_ref().unwrap().filter.is_empty(),
            "/ starts anew"
        );
        char_key(&mut app, 'c');
        press(&mut app, KeyCode::Esc);
        let picker = app.picker.as_ref().unwrap();
        assert!(!picker.filter_active);
        assert!(picker.filter.is_empty());
        assert_eq!(picker.filtered_indices.len(), 4);

        cleanup(temp);
    }

    #[test]
    fn picker_left_and_the_parent_entry_go_up() {
        let (temp, root, child) = synthetic_tree();
        let grandchild = child.join("grandchild");
        fs::create_dir(&grandchild).unwrap();
        let mut app = picker_app(&grandchild);

        assert_eq!(picker_cursor(&app), 0, "the cursor is on the parent entry");
        press(&mut app, KeyCode::Right);
        assert_eq!(app.picker.as_ref().unwrap().current_dir, child);
        assert_eq!(app.current_dir.as_ref(), Some(&child));

        char_key(&mut app, 'l');
        assert_eq!(app.picker.as_ref().unwrap().current_dir, root);

        select_path(&mut app, &child);
        char_key(&mut app, 'l');
        assert_eq!(app.picker.as_ref().unwrap().current_dir, child);
        char_key(&mut app, 'h');
        assert_eq!(app.picker.as_ref().unwrap().current_dir, root);
        assert_eq!(app.mode, ViewMode::Picker);

        cleanup(temp);
    }
}
