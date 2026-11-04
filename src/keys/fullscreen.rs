//! The keys of the fullscreen view.

use super::step_key;
use crate::app::App;
use crossterm::event::{KeyCode, KeyEvent};
use std::io;

pub(crate) fn handle_fullscreen_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    match key.code {
        KeyCode::Char('?') => {
            app.graphics_delete_all()?;
            app.help_visible = true;
        }
        // `Esc` stops a running slideshow first. `q` goes back in one press: `enter_gallery` ends
        // the slideshow.
        KeyCode::Esc if app.slideshow_running() => app.stop_slideshow(),
        KeyCode::Char('q') | KeyCode::Esc => {
            app.graphics_delete_all()?;
            app.enter_gallery();
        }
        KeyCode::Tab => {
            app.graphics_delete_all()?;
            app.toggle_info();
        }
        KeyCode::Enter => {
            play_in_place(app)?;
        }
        #[cfg(feature = "video")]
        KeyCode::Char('p') if app.fullscreen_on_live() => {
            app.enter_motion_at(app.current);
            app.request_video_window();
        }
        #[cfg(feature = "video")]
        KeyCode::Char('p') if app.shows_video_placeholder() || app.fullscreen_on_animation() => {
            app.enter_video(app.current);
            app.request_video_window();
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
        KeyCode::Left | KeyCode::Right | KeyCode::Char('h' | 'j' | 'k' | 'l') => {
            step_key(app, key.code);
        }
        KeyCode::Home | KeyCode::End => {
            app.graphics_delete_all()?;
            step_key(app, key.code);
        }
        _ => {}
    }
    Ok(false)
}

/// Enter and a click on the middle of the picture: play the video of the placeholder, the
/// animation or the motion of a Live Photo. `false` when the file has nothing to play.
pub(super) fn play_in_place(app: &mut App) -> io::Result<bool> {
    if on_live(app) {
        // The still stays on the screen until the first frame of the motion replaces it.
        app.stop_slideshow();
        enter_motion(app);
        return Ok(true);
    }
    if !app.shows_video_placeholder() && !on_animation(app) {
        return Ok(false);
    }
    if !app.shows_video_placeholder() {
        app.graphics_delete_all()?;
    }
    app.stop_slideshow();
    app.enter_fullscreen_at(app.current);
    Ok(true)
}

/// A motion plays only with the `video` feature, and without it the scan lists no `.mov`.
#[cfg(feature = "video")]
fn on_live(app: &App) -> bool {
    app.fullscreen_on_live()
}

#[cfg(not(feature = "video"))]
fn on_live(_app: &App) -> bool {
    false
}

#[cfg(feature = "video")]
fn enter_motion(app: &mut App) {
    app.enter_motion_at(app.current);
}

#[cfg(not(feature = "video"))]
fn enter_motion(_app: &mut App) {}

/// An animation plays only with the `video` feature.
#[cfg(feature = "video")]
fn on_animation(app: &App) -> bool {
    app.fullscreen_on_animation()
}

#[cfg(not(feature = "video"))]
fn on_animation(_app: &App) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use crate::app::{App, ViewMode};
    use crate::test_support::{char_key, fullscreen_app, press, text};
    use crossterm::event::KeyCode;

    #[test]
    fn hj_step_back_and_kl_forward() {
        let mut app = fullscreen_app(4, 1);
        char_key(&mut app, 'k');
        assert_eq!(app.current, 2);
        char_key(&mut app, 'l');
        assert_eq!(app.current, 3);
        char_key(&mut app, 'j');
        assert_eq!(app.current, 2);
        char_key(&mut app, 'h');
        assert_eq!(app.current, 1);
        assert_eq!(app.mode, ViewMode::Fullscreen);
    }

    fn zoom_is_one(app: &App) -> bool {
        (app.zoom - 1.0).abs() < f64::EPSILON
    }

    #[test]
    fn arrows_step_through_photos() {
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
    fn zoom_and_pan_keys() {
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
    fn slideshow_keys() {
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
    fn info_help_quit_and_other_keys() {
        let mut app = fullscreen_app(2, 0);
        let info = app.info_visible;

        press(&mut app, KeyCode::Tab);
        assert_ne!(app.info_visible, info);
        char_key(&mut app, '?');
        assert!(app.help_visible);
        char_key(&mut app, 'x');
        assert!(!app.help_visible);

        press(&mut app, KeyCode::Enter);
        char_key(&mut app, 'x');
        assert_eq!(app.mode, ViewMode::Fullscreen);
        assert_eq!(app.current, 0);
    }

    #[test]
    fn q_returns_to_gallery_then_quits() {
        let mut app = fullscreen_app(2, 1);
        app.graphics_out.take_bytes();

        assert!(!char_key(&mut app, 'q'), "q leaves the view, not phewr");
        assert_eq!(app.mode, ViewMode::Gallery);
        assert!(app.needs_render);
        let out = text(&app.graphics_out.take_bytes());
        assert!(
            out.contains("\x1b_Ga=d,d=A,"),
            "the image goes first: {out}"
        );

        assert!(char_key(&mut app, 'q'), "q in the gallery quits phewr");
    }

    #[test]
    fn q_stops_slideshow_and_returns() {
        let mut app = fullscreen_app(2, 0);
        char_key(&mut app, 's');
        assert!(app.slideshow_running());

        assert!(!char_key(&mut app, 'q'));
        assert!(!app.slideshow_running());
        assert_eq!(app.mode, ViewMode::Gallery);
    }
}
