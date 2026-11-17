//! The key presses: the overlays, and the dispatch to the handler of the view mode.

pub(crate) mod fullscreen;
pub(crate) mod gallery;
pub(crate) mod mouse;
#[cfg(feature = "video")]
pub(crate) mod video;

use crate::app::{App, ViewMode};
use crate::pictures::kitty::query::LateAnswers;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use fullscreen::handle_fullscreen_key;
use gallery::{handle_gallery_key, handle_search_key};
use mouse::handle_mouse;
use std::io;
use std::time::Instant;
#[cfg(feature = "video")]
use video::handle_video_key;

/// Drops a Kitty probe answer or an OSC 10 answer that comes after the timeout.
///
/// crossterm reads a late `ESC _ G i=31 ; OK ESC \\` as the keys Alt+`_`, `G`, `i`, `=`, ...
/// and Alt+`\\`, and a late `ESC ] 10 ; rgb:... BEL` as Alt+`]`, `1`, `0`, ... and Ctrl+`g`.
/// Unfiltered, those keys zoom, jump and open the info panel. Nothing in phewr uses Alt+`_` or
/// Alt+`]`, and a key that fits no answer ends the swallow.
///
/// The filter counts the owed Kitty answers and the owed OSC 10 answer apart. It swallows an
/// Alt+`_` answer only while a Kitty answer is owed, and an Alt+`]` answer only while the OSC 10
/// answer is owed.
///
/// The run loop reads every Kitty answer with `AnswerKeys` first, so the self check gets each
/// `ENOENT`. Each Kitty answer that it read counts here as one late Kitty answer less
/// ([`LateAnswerFilter::kitty_answers_came`]), and never as the OSC 10 answer. Only a cut Kitty
/// answer that `AnswerKeys` gives back reaches this filter.
///
/// Ghostty 1.4 sends DA1 before the OSC 10 answer and the Kitty answers, so the start read waits
/// `LATE_ANSWER_GRACE` after DA1. When the OSC 10 answer is still missing then, it stays owed, and
/// this filter drops it when it comes.
#[derive(Debug, Default)]
pub(crate) struct LateAnswerFilter {
    /// The owed answers. Each one ends with Alt+`\\`, an OSC answer also with Ctrl+`g`.
    owed: LateAnswers,
    /// The opener of the answer in progress: `_` for Kitty, `]` for OSC.
    inside: Option<char>,
}

impl LateAnswerFilter {
    pub(crate) const fn new(owed: LateAnswers) -> Self {
        Self { owed, inside: None }
    }

    /// True when `event` belongs to a late answer.
    pub(crate) fn swallows(&mut self, event: &Event) -> bool {
        let Event::Key(key) = event else {
            return false;
        };
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char(open @ ('_' | ']'))
                if alt && self.inside.is_none() && self.owes(open) =>
            {
                self.inside = Some(open);
                true
            }
            KeyCode::Char('\\') if alt && self.inside.is_some() => self.end_answer(),
            KeyCode::Char('g') if ctrl && self.inside == Some(']') => self.end_answer(),
            KeyCode::Char(c)
                if self.inside.is_some() && !alt && c.is_ascii() && !c.is_ascii_control() =>
            {
                true
            }
            _ => {
                self.inside = None;
                false
            }
        }
    }

    /// `count` Kitty answers came through `AnswerKeys`: as many Kitty answers less are owed.
    pub(crate) const fn kitty_answers_came(&mut self, count: usize) {
        self.owed.kitty = self.owed.kitty.saturating_sub(count);
    }

    /// True when an answer with the opener `open` is owed.
    const fn owes(&self, open: char) -> bool {
        if open == ']' {
            self.owed.foreground
        } else {
            self.owed.kitty > 0
        }
    }

    /// The last key of an answer: that answer is no longer owed.
    fn end_answer(&mut self) -> bool {
        if self.inside.take() == Some(']') {
            self.owed.foreground = false;
        } else {
            self.owed.kitty = self.owed.kitty.saturating_sub(1);
        }
        true
    }
}

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
    use super::{LateAnswerFilter, LateAnswers, handle_event};
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

    /// A filter that owes `kitty` Kitty answers and, with `foreground`, the OSC 10 answer.
    fn owing(kitty: usize, foreground: bool) -> LateAnswerFilter {
        LateAnswerFilter::new(LateAnswers { kitty, foreground })
    }

    #[test]
    fn armed_filter_drops_late_answer() {
        let mut filter = owing(1, false);
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

    /// A start that owed two answers and got none in time drops both late answers.
    #[test]
    fn filter_drops_two_late_answers() {
        let mut filter = owing(2, false);
        for key in late_answer_keys("\x1b_Gi=31;OK\x1b\\\x1b_Gi=32;OK\x1b\\") {
            assert!(filter.swallows(&key), "{key:?}");
        }
        let alt_underscore = Event::Key(KeyEvent::new(KeyCode::Char('_'), KeyModifiers::ALT));
        assert!(!filter.swallows(&alt_underscore), "two answers at most");
    }

    /// A late OSC 10 answer ends with BEL (Ctrl+`g`) or with `ESC \`, and the Kitty answer follows.
    #[test]
    fn filter_drops_late_foreground_answer() {
        let bel = Event::Key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
        let mut filter = owing(1, true);
        let mut keys = late_answer_keys("\x1b]10;rgb:ffff/ffff/ffff");
        keys.push(bel.clone());
        keys.extend(late_answer_keys("\x1b_Gi=31;OK\x1b\\"));
        for key in keys {
            assert!(filter.swallows(&key), "{key:?}");
        }
        let zero = Event::Key(KeyEvent::new(KeyCode::Char('0'), KeyModifiers::NONE));
        assert!(!filter.swallows(&zero), "both answers came");

        let mut filter = owing(0, true);
        for key in late_answer_keys("\x1b]10;rgb:ff/ff/ff\x1b\\") {
            assert!(filter.swallows(&key), "{key:?}");
        }
        assert!(
            !filter.swallows(&bel),
            "Ctrl+g outside an answer is a real key"
        );
    }

    #[test]
    fn filter_drops_error_answer() {
        let mut filter = owing(1, false);
        for key in late_answer_keys("\x1b_Gi=31;EBADF:could not open file\x1b\\") {
            assert!(filter.swallows(&key), "{key:?}");
        }
    }

    #[test]
    fn foreign_key_ends_swallow() {
        let mut filter = owing(1, false);
        let alt_underscore = Event::Key(KeyEvent::new(KeyCode::Char('_'), KeyModifiers::ALT));
        assert!(filter.swallows(&alt_underscore));
        let enter = Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!filter.swallows(&enter), "Enter is a real key");
        let g = Event::Key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT));
        assert!(!filter.swallows(&g), "the swallow ended with Enter");
        assert!(!filter.swallows(&Event::Resize(80, 24)));
    }

    #[test]
    fn idle_filter_passes_every_key() {
        let mut filter = LateAnswerFilter::default();
        for key in late_answer_keys("\x1b_Gi=31;OK\x1b\\") {
            assert!(!filter.swallows(&key), "{key:?}");
        }
        let mut filter = owing(0, false);
        let alt_underscore = Event::Key(KeyEvent::new(KeyCode::Char('_'), KeyModifiers::ALT));
        assert!(!filter.swallows(&alt_underscore));
    }

    /// The Kitty answers that `AnswerKeys` read, also a self check `ENOENT` that nobody owes,
    /// never pay for the owed OSC 10 answer.
    #[test]
    fn kitty_answers_leave_the_foreground_owed() {
        let mut filter = owing(1, true);
        filter.kitty_answers_came(3);
        for key in late_answer_keys("\x1b]10;rgb:ff/ff/ff\x1b\\") {
            assert!(filter.swallows(&key), "{key:?}");
        }
        let zero = Event::Key(KeyEvent::new(KeyCode::Char('0'), KeyModifiers::NONE));
        assert!(!filter.swallows(&zero), "the OSC 10 answer came");
    }

    /// Each opener needs its own owed answer.
    #[test]
    fn filter_swallows_only_the_owed_kind() {
        let alt_underscore = Event::Key(KeyEvent::new(KeyCode::Char('_'), KeyModifiers::ALT));
        let alt_bracket = Event::Key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT));
        let mut filter = owing(0, true);
        assert!(!filter.swallows(&alt_underscore), "no Kitty answer owed");
        let mut filter = owing(1, false);
        assert!(!filter.swallows(&alt_bracket), "no OSC 10 answer owed");
        for key in late_answer_keys("\x1b]10;rgb:ff/ff/ff\x1b\\") {
            assert!(!filter.swallows(&key), "{key:?}");
        }
        for key in late_answer_keys("\x1b_Gi=31;OK\x1b\\") {
            assert!(filter.swallows(&key), "{key:?}");
        }
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

        assert_eq!(app.gallery.search_query, "t");
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
