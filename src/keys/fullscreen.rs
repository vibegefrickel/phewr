//! The keys of the fullscreen view.

use crate::app::App;
use crossterm::event::{KeyCode, KeyEvent};
use std::io;

/// Keys of the fullscreen view: navigation, zoom and pan, slideshow, info, delete.
pub(crate) fn handle_fullscreen_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    match key.code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Char('?') => {
            app.graphics_delete_all()?;
            app.help_visible = true;
        }
        KeyCode::Esc if app.slideshow_running() => app.stop_slideshow(),
        KeyCode::Esc => {
            app.graphics_delete_all()?;
            app.enter_gallery();
        }
        KeyCode::Char('i') => {
            app.graphics_delete_all()?;
            app.toggle_info();
        }
        KeyCode::Enter if app.shows_video_placeholder() => {
            app.stop_slideshow();
            app.enter_fullscreen_at(app.current);
        }
        KeyCode::Char('s') => app.toggle_slideshow(),
        KeyCode::Char(' ') => app.slideshow_toggle_pause(),
        KeyCode::Char('[') => app.slideshow_adjust_interval(-1.0),
        KeyCode::Char(']') => app.slideshow_adjust_interval(1.0),
        KeyCode::Char('+' | '=') => app.zoom_in(),
        KeyCode::Char('-' | '_') => app.zoom_out(),
        KeyCode::Char('0') => app.reset_zoom(),
        KeyCode::Left | KeyCode::Char('h') if app.zoom > 1.0 => app.pan(-1.0, 0.0),
        KeyCode::Right | KeyCode::Char('l') if app.zoom > 1.0 => app.pan(1.0, 0.0),
        KeyCode::Up | KeyCode::Char('k') if app.zoom > 1.0 => app.pan(0.0, -1.0),
        KeyCode::Down | KeyCode::Char('j') if app.zoom > 1.0 => app.pan(0.0, 1.0),
        KeyCode::Left | KeyCode::Char('h') => {
            app.prev();
            app.slideshow_touch();
        }
        KeyCode::Right | KeyCode::Char('l') => {
            app.next();
            app.slideshow_touch();
        }
        KeyCode::Home => {
            app.graphics_delete_all()?;
            app.first();
            app.slideshow_touch();
        }
        KeyCode::End => {
            app.graphics_delete_all()?;
            app.last();
            app.slideshow_touch();
        }
        KeyCode::Char('d' | 'D') => {
            app.begin_delete(matches!(key.code, KeyCode::Char('D')));
        }
        _ => {}
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use crate::app::{App, ViewMode};
    use crate::test_support::{char_key, fullscreen_app, press};
    use crossterm::event::KeyCode;

    fn zoom_is_one(app: &App) -> bool {
        (app.zoom - 1.0).abs() < f64::EPSILON
    }

    #[test]
    fn fullscreen_arrows_step_through_the_photos() {
        let mut app = fullscreen_app(4, 1);

        press(&mut app, KeyCode::Right);
        assert_eq!(app.current, 2);
        char_key(&mut app, 'l');
        assert_eq!(app.current, 3);
        char_key(&mut app, 'l');
        assert_eq!(app.current, 3, "the last photo stays");
        press(&mut app, KeyCode::Left);
        assert_eq!(app.current, 2);
        char_key(&mut app, 'h');
        assert_eq!(app.current, 1);
        press(&mut app, KeyCode::End);
        assert_eq!(app.current, 3);
        press(&mut app, KeyCode::Home);
        assert_eq!(app.current, 0);
        assert_eq!(app.mode, ViewMode::Fullscreen);
    }

    #[test]
    fn fullscreen_zoom_keys_and_pan_keys() {
        let mut app = fullscreen_app(3, 1);

        char_key(&mut app, '+');
        let zoom = app.zoom;
        assert!(zoom > 1.0);
        char_key(&mut app, '=');
        assert!(app.zoom > zoom);

        for code in [
            KeyCode::Left,
            KeyCode::Char('h'),
            KeyCode::Right,
            KeyCode::Char('l'),
            KeyCode::Up,
            KeyCode::Char('k'),
            KeyCode::Down,
            KeyCode::Char('j'),
        ] {
            app.needs_render = false;
            press(&mut app, code);
            assert_eq!(app.current, 1, "{code:?} pans and does not step");
            assert!(app.needs_render, "{code:?} pans");
        }

        let zoomed = app.zoom;
        char_key(&mut app, '-');
        assert!(app.zoom < zoomed, "- zooms out");
        char_key(&mut app, '_');
        char_key(&mut app, '_');
        char_key(&mut app, '_');
        assert!(zoom_is_one(&app), "zoom out stops at 1");

        char_key(&mut app, '+');
        char_key(&mut app, '0');
        assert!(zoom_is_one(&app));
    }

    #[test]
    fn fullscreen_slideshow_keys() {
        let mut app = fullscreen_app(3, 0);

        char_key(&mut app, 's');
        assert!(app.slideshow_running());
        char_key(&mut app, ' ');
        assert!(app.slideshow.as_ref().unwrap().paused);
        char_key(&mut app, ' ');
        assert!(!app.slideshow.as_ref().unwrap().paused);

        let interval = app.slideshow.as_ref().unwrap().interval;
        char_key(&mut app, ']');
        assert!(app.slideshow.as_ref().unwrap().interval > interval);
        char_key(&mut app, '[');
        assert_eq!(app.slideshow.as_ref().unwrap().interval, interval);

        press(&mut app, KeyCode::Right);
        assert_eq!(app.current, 1);
        assert!(app.slideshow_running(), "a step keeps the slideshow");

        press(&mut app, KeyCode::Esc);
        assert!(!app.slideshow_running());
        assert_eq!(
            app.mode,
            ViewMode::Fullscreen,
            "Esc stops the slideshow first"
        );

        char_key(&mut app, 's');
        char_key(&mut app, 's');
        assert!(!app.slideshow_running(), "s toggles");

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, ViewMode::Gallery);
    }

    #[test]
    fn fullscreen_info_help_quit_and_other_keys() {
        let mut app = fullscreen_app(2, 0);
        let info = app.info_visible;

        char_key(&mut app, 'i');
        assert_ne!(app.info_visible, info);
        char_key(&mut app, '?');
        assert!(app.help_visible);
        char_key(&mut app, 'x');
        assert!(!app.help_visible);

        press(&mut app, KeyCode::Enter);
        char_key(&mut app, 'x');
        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert_eq!(app.current, 0);
        assert!(char_key(&mut app, 'q'));
    }
}
