//! The key presses: the overlays, and the dispatch to the handler of the view mode.

pub(crate) mod fullscreen;
pub(crate) mod gallery;
pub(crate) mod mouse;
#[cfg(feature = "video")]
pub(crate) mod video;

use crate::app::{App, ViewMode};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use fullscreen::handle_fullscreen_key;
use gallery::{handle_gallery_key, handle_search_key};
use mouse::handle_mouse;
use std::io;
use std::time::Instant;
#[cfg(feature = "video")]
use video::handle_video_key;

pub(crate) fn handle_event(app: &mut App, event: &Event) -> io::Result<bool> {
    handle_event_at(app, event, Instant::now())
}

/// `handle_event` with the double-click clock. A test needs no sleep.
pub(crate) fn handle_event_at(app: &mut App, event: &Event, now: Instant) -> io::Result<bool> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => handle_key(app, *key),
        Event::Mouse(mouse) => handle_mouse(app, *mouse, now),
        Event::Resize(_, _) => {
            app.mark_dirty();
            Ok(false)
        }
        _ => Ok(false),
    }
}

/// Route a key press. The overlays win over the view mode.
fn handle_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    if app.help_visible {
        app.help_visible = false;
        app.needs_render = true;
    } else {
        return match app.mode {
            ViewMode::Fullscreen => handle_fullscreen_key(app, key),
            #[cfg(feature = "video")]
            ViewMode::Video => handle_video_key(app, key),
            ViewMode::Gallery if app.gallery.search_active => {
                handle_search_key(app, key);
                Ok(false)
            }
            ViewMode::Gallery => handle_gallery_key(app, key),
        };
    }
    Ok(false)
}

/// The steps that the fullscreen view and the player share. A step restarts the slideshow
/// countdown.
fn step_key(app: &mut App, code: KeyCode) {
    match code {
        // `h` and `j` step back, `k` and `l` forward, for either hand position.
        KeyCode::Left | KeyCode::Char('h' | 'j') => app.prev(),
        KeyCode::Right | KeyCode::Char('k' | 'l') => app.next(),
        KeyCode::Home => app.first(),
        KeyCode::End => app.last(),
        _ => return,
    }
    app.slideshow_touch();
}

#[cfg(test)]
mod tests {
    use super::handle_event;
    use crate::app::ViewMode;
    use crate::test_support::{char_key, fullscreen_app, gallery_app, press};
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use std::fs;

    #[test]
    fn resize_marks_pictures_dirty() {
        let mut app = gallery_app(3);
        app.graphics_storage_dirty = false;

        assert!(!handle_event(&mut app, &Event::Resize(80, 24)).unwrap());

        assert!(app.graphics_storage_dirty);
        assert!(app.needs_render);
    }

    #[test]
    fn releases_and_other_events_change_nothing() {
        let mut app = gallery_app(3);
        let release = KeyEvent::new_with_kind(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );

        assert!(!handle_event(&mut app, &Event::Key(release)).unwrap());
        assert!(!handle_event(&mut app, &Event::FocusGained).unwrap());
        assert!(!app.needs_render);
    }

    #[test]
    fn handlers_write_to_app_output() {
        const DELETE_ALL: &[u8] = b"\x1b_Ga=d,d=A,q=2\x1b\\";
        let mut app = gallery_app(3);
        app.screen.record(5, 16);

        char_key(&mut app, '?');
        assert_eq!(app.graphics_out.take_bytes(), DELETE_ALL);
        assert_eq!(app.screen.stored_count(), 0);
        char_key(&mut app, 'x');
        assert!(
            app.graphics_out.take_bytes().is_empty(),
            "no picture change"
        );
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.graphics_out.take_bytes(), DELETE_ALL);
        assert_eq!(app.mode, ViewMode::Fullscreen);
    }

    #[test]
    fn any_key_only_closes_help() {
        let mut app = gallery_app(3);

        assert!(!char_key(&mut app, '?'));
        assert!(app.help_visible);
        assert!(!char_key(&mut app, 'q'), "q only closes the help");
        assert!(!app.help_visible);
        assert!(app.needs_render);
        assert_eq!(app.mode, ViewMode::Gallery);
    }

    #[test]
    fn t_is_text_in_search() {
        let mut app = gallery_app(2);
        char_key(&mut app, '/');

        char_key(&mut app, 't');

        assert_eq!(app.gallery.search_query.text(), "t");
    }

    #[test]
    fn t_is_no_key_outside_search() {
        for mut app in [gallery_app(2), fullscreen_app(2, 0)] {
            let mode = app.mode;
            app.graphics_out.take_bytes();

            assert!(!char_key(&mut app, 't'));

            assert_eq!(app.mode, mode);
            assert!(app.graphics_out.take_bytes().is_empty());
        }
    }

    #[test]
    fn d_keys_change_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("a.jpg");
        fs::write(&photo, b"not an image").unwrap();
        for mut app in [gallery_app(0), fullscreen_app(0, 0)] {
            app.images = vec![photo.clone()];
            app.filenames = vec!["a.jpg".to_owned()];
            app.gallery.filtered_indices = vec![0];
            let mode = app.mode;
            app.graphics_out.take_bytes();

            for key in ['d', 'D', 'y'] {
                assert!(!char_key(&mut app, key));
            }

            assert_eq!(app.mode, mode);
            assert_eq!(app.images, std::slice::from_ref(&photo));
            assert!(app.selection.is_empty());
            assert!(app.graphics_out.take_bytes().is_empty());
            assert_eq!(fs::read(&photo).unwrap(), b"not an image");
        }
    }
}
