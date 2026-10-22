//! The key presses: the overlays, and the dispatch to the handler of the view mode.

pub(crate) mod fullscreen;
pub(crate) mod gallery;
pub(crate) mod mouse;
#[cfg(feature = "video")]
pub(crate) mod video;

use crate::app::{App, DeleteMode, ViewMode};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use fullscreen::handle_fullscreen_key;
use gallery::{handle_gallery_key, handle_search_key};
use mouse::handle_mouse;
use std::io;
use std::time::Instant;
#[cfg(feature = "video")]
use video::handle_video_key;

/// Drops the answer to the Kitty probe of `detect_transfer` when it comes after the timeout.
///
/// A late answer reaches crossterm, which reads `ESC _ G i=31 ; OK ESC \\` as the keys Alt+`_`,
/// `G`, `i`, `=`, ... and Alt+`\\`. Those keys would zoom, jump and open the info panel. The
/// filter swallows the keys from Alt+`_` to Alt+`\\`, when the probe got no answer in time.
/// Nothing in viewr uses Alt+`_`, and a key that does not fit an answer ends the swallow.
#[derive(Debug, Default)]
pub(crate) struct LateAnswerFilter {
    armed: bool,
    inside: bool,
}

impl LateAnswerFilter {
    /// `armed` when the probe went out and no answer came back in time.
    pub(crate) fn new(armed: bool) -> Self {
        Self {
            armed,
            inside: false,
        }
    }

    /// True when `event` belongs to the late answer and the loop must drop it.
    pub(crate) fn swallows(&mut self, event: &Event) -> bool {
        if !self.armed {
            return false;
        }
        let Event::Key(key) = event else {
            return false;
        };
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('_') if alt && !self.inside => {
                self.inside = true;
                true
            }
            KeyCode::Char('\\') if alt && self.inside => {
                self.inside = false;
                self.armed = false;
                true
            }
            KeyCode::Char(c) if self.inside && !alt && c.is_ascii() && !c.is_ascii_control() => {
                true
            }
            _ => {
                self.inside = false;
                false
            }
        }
    }
}

pub(crate) fn handle_event(app: &mut App, event: &Event) -> io::Result<bool> {
    handle_event_at(app, event, Instant::now())
}

/// `handle_event` with the clock of the double-click, so that a test needs no sleep.
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

/// Route a key press: overlays (help, delete question) win over the view mode.
fn handle_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    if app.help_visible {
        app.help_visible = false;
        app.needs_render = true;
    } else if app.pending_delete.is_some() {
        match key.code {
            KeyCode::Char('y' | 'Y') => {
                app.graphics_delete_all()?;
                app.confirm_delete();
                if app.scan_complete && app.images.is_empty() {
                    return Ok(true);
                }
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                app.cancel_delete();
            }
            _ => {}
        }
    } else {
        return match app.mode {
            ViewMode::Fullscreen => handle_fullscreen_key(app, key),
            #[cfg(feature = "video")]
            ViewMode::Video => handle_video_key(app, key),
            ViewMode::Gallery if app.gallery.search_active => {
                handle_search_key(app, key.code);
                Ok(false)
            }
            ViewMode::Gallery => handle_gallery_key(app, key),
        };
    }
    Ok(false)
}

/// The steps through the list that the fullscreen view and the video player share. A step
/// restarts the slideshow countdown, so that the new file gets the full interval.
fn step_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Left | KeyCode::Char('h') => app.prev(),
        KeyCode::Right | KeyCode::Char('l') => app.next(),
        KeyCode::Home => app.first(),
        KeyCode::End => app.last(),
        _ => return,
    }
    app.slideshow_touch();
}

/// `d` asks before the trash, `D` before the permanent delete.
fn delete_key(app: &mut App, code: KeyCode) {
    app.begin_delete(if code == KeyCode::Char('D') {
        DeleteMode::Permanent
    } else {
        DeleteMode::Trash
    });
}

#[cfg(test)]
mod tests {
    use super::{LateAnswerFilter, handle_event};
    use crate::app::{DeleteMode, ViewMode};
    use crate::test_support::{char_key, fullscreen_app, gallery_app, press};
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn resize_marks_the_pictures_dirty() {
        let mut app = gallery_app(3);
        app.graphics_storage_dirty = false;

        assert!(!handle_event(&mut app, &Event::Resize(80, 24)).unwrap());

        assert!(app.graphics_storage_dirty);
        assert!(app.needs_render);
    }

    /// The key events that crossterm makes of a late probe answer.
    fn late_answer_keys(answer: &str) -> Vec<Event> {
        let mut keys = Vec::new();
        let mut alt = false;
        for c in answer.chars() {
            if c == '\x1b' {
                alt = true;
                continue;
            }
            let mut modifiers = if alt {
                KeyModifiers::ALT
            } else {
                KeyModifiers::NONE
            };
            if c.is_ascii_uppercase() {
                modifiers |= KeyModifiers::SHIFT;
            }
            keys.push(Event::Key(KeyEvent::new(KeyCode::Char(c), modifiers)));
            alt = false;
        }
        keys
    }

    #[test]
    fn an_armed_filter_drops_the_late_probe_answer_and_keeps_the_keys_around_it() {
        let mut filter = LateAnswerFilter::new(true);
        let j = Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert!(!filter.swallows(&j), "a key before the answer");

        for key in late_answer_keys("\x1b_Gi=31;OK\x1b\\") {
            assert!(filter.swallows(&key), "{key:?}");
        }
        assert!(!filter.swallows(&j), "a key after the answer");
        let alt_underscore = Event::Key(KeyEvent::new(KeyCode::Char('_'), KeyModifiers::ALT));
        assert!(
            !filter.swallows(&alt_underscore),
            "one answer at most, then every key passes"
        );
    }

    #[test]
    fn an_error_answer_with_a_message_is_dropped_too() {
        let mut filter = LateAnswerFilter::new(true);
        for key in late_answer_keys("\x1b_Gi=31;EBADF:could not open file\x1b\\") {
            assert!(filter.swallows(&key), "{key:?}");
        }
    }

    #[test]
    fn a_key_that_fits_no_answer_ends_the_swallow() {
        let mut filter = LateAnswerFilter::new(true);
        let alt_underscore = Event::Key(KeyEvent::new(KeyCode::Char('_'), KeyModifiers::ALT));
        assert!(filter.swallows(&alt_underscore));
        let enter = Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!filter.swallows(&enter), "Enter is a real key");
        let g = Event::Key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT));
        assert!(!filter.swallows(&g), "the swallow ended with Enter");
        assert!(!filter.swallows(&Event::Resize(80, 24)));
    }

    #[test]
    fn a_filter_without_a_late_answer_passes_every_key() {
        let mut filter = LateAnswerFilter::default();
        for key in late_answer_keys("\x1b_Gi=31;OK\x1b\\") {
            assert!(!filter.swallows(&key), "{key:?}");
        }
        let mut filter = LateAnswerFilter::new(false);
        let alt_underscore = Event::Key(KeyEvent::new(KeyCode::Char('_'), KeyModifiers::ALT));
        assert!(!filter.swallows(&alt_underscore));
    }

    #[test]
    fn key_releases_and_other_events_change_nothing() {
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
    fn key_handlers_write_the_kitty_sequences_to_the_app_output() {
        const DELETE_ALL: &[u8] = b"\x1b_Ga=d,d=A,q=2\x1b\\";
        let mut app = gallery_app(3);
        app.terminal_images.insert(5, 16);

        char_key(&mut app, '?');
        assert_eq!(app.graphics_out.take_bytes(), DELETE_ALL);
        assert!(app.terminal_images.is_empty());
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
    fn any_key_closes_the_help_and_does_nothing_else() {
        let mut app = gallery_app(3);

        assert!(!char_key(&mut app, '?'));
        assert!(app.help_visible);
        assert!(!char_key(&mut app, 'q'), "q only closes the help");
        assert!(!app.help_visible);
        assert!(app.needs_render);
        assert_eq!(app.mode, ViewMode::Gallery);
    }

    #[test]
    fn t_is_text_in_the_gallery_search() {
        let mut app = gallery_app(2);
        char_key(&mut app, '/');

        char_key(&mut app, 't');

        assert_eq!(app.gallery.search_query, "t");
    }

    #[test]
    fn t_is_no_key_in_the_gallery_and_the_fullscreen_view() {
        for mut app in [gallery_app(2), fullscreen_app(2, 0)] {
            let mode = app.mode;
            app.graphics_out.take_bytes();

            assert!(!char_key(&mut app, 't'));

            assert_eq!(app.mode, mode);
            assert!(app.graphics_out.take_bytes().is_empty());
        }
    }

    #[test]
    fn fullscreen_delete_asks_and_n_or_esc_cancel() {
        let mut app = fullscreen_app(3, 1);

        char_key(&mut app, 'd');
        let pending = app.pending_delete.as_ref().unwrap();
        assert_eq!(pending.paths, [PathBuf::from("/photos/photo-01.jpg")]);
        assert!(pending.from_fullscreen);
        assert_eq!(pending.mode, DeleteMode::Trash);

        char_key(&mut app, 'x');
        assert!(app.pending_delete.is_some(), "other keys keep the question");
        char_key(&mut app, 'n');
        assert!(app.pending_delete.is_none());

        char_key(&mut app, 'D');
        assert_eq!(
            app.pending_delete.as_ref().unwrap().mode,
            DeleteMode::Permanent
        );
        char_key(&mut app, 'N');
        assert!(app.pending_delete.is_none());

        char_key(&mut app, 'D');
        press(&mut app, KeyCode::Esc);
        assert!(app.pending_delete.is_none());
        assert_eq!(
            app.mode,
            ViewMode::Fullscreen,
            "Esc only cancels the question"
        );
        assert_eq!(app.images.len(), 3);
    }

    /// A temp directory with small files that the test can delete.
    fn deletable_files(names: &[&str]) -> (TempDir, Vec<PathBuf>) {
        let dir = tempfile::tempdir().unwrap();
        let paths = names
            .iter()
            .map(|name| {
                let path = dir.path().join(name);
                fs::write(&path, b"not an image").unwrap();
                path
            })
            .collect();
        (dir, paths)
    }

    #[test]
    fn y_deletes_permanently_and_the_last_delete_quits() {
        let (_dir, paths) = deletable_files(&["a.jpg", "b.jpg"]);
        let mut app = gallery_app(0);
        app.images.clone_from(&paths);
        app.filenames = vec!["a.jpg".to_owned(), "b.jpg".to_owned()];
        app.gallery.filtered_indices = vec![0, 1];

        char_key(&mut app, 'D');
        assert!(!char_key(&mut app, 'y'));
        assert!(!paths[0].exists());
        assert_eq!(app.images, [paths[1].clone()]);
        assert!(app.pending_delete.is_none());

        char_key(&mut app, 'D');
        assert!(char_key(&mut app, 'Y'), "an empty scanned list ends viewr");
        assert!(!paths[1].exists());
        assert!(app.images.is_empty());
    }
}
