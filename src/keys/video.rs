//! The keys of the built-in video player.

use crate::app::App;
use crate::video_handoff::focus_gallery_on_current;
use crossterm::event::{KeyCode, KeyEvent};
use std::io;

/// Keys of the video player: pause, navigation, delete.
///
/// Esc stops a running slideshow, like `q` in mpv: the user wants out. A step to another file
/// keeps the slideshow and gives that file the full interval, like in the fullscreen view.
pub(crate) fn handle_video_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    match key.code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Char(' ') => {
            if let Some(ref mut v) = app.video {
                v.toggle_pause();
                app.needs_render = true;
            }
        }
        KeyCode::Esc => {
            app.graphics_delete_all()?;
            app.stop_slideshow();
            app.return_from_video();
            focus_gallery_on_current(app);
        }
        KeyCode::Char('?') => {
            app.graphics_delete_all()?;
            app.help_visible = true;
        }
        KeyCode::Left | KeyCode::Char('h') => {
            app.graphics_delete_all()?;
            app.prev();
            app.slideshow_touch();
        }
        KeyCode::Right | KeyCode::Char('l') => {
            app.graphics_delete_all()?;
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
    use crate::test_support::{char_key, gallery_app, press};
    use crossterm::event::KeyCode;
    use std::path::PathBuf;

    fn video_app() -> App {
        let mut app = gallery_app(0);
        app.images = ["a.jpg", "clip.mp4", "other.mp4", "b.jpg"]
            .into_iter()
            .map(|name| PathBuf::from("/videos").join(name))
            .collect();
        app.filenames = ["a.jpg", "clip.mp4", "other.mp4", "b.jpg"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        app.gallery.filtered_indices = vec![0, 1, 2, 3];
        app.enter_video(1);
        app
    }

    #[test]
    fn video_keys_step_between_the_videos() {
        let mut app = video_app();

        char_key(&mut app, ' ');
        assert!(app.video.is_none(), "space without a player does nothing");
        press(&mut app, KeyCode::Right);
        assert_eq!(app.current, 2);
        assert_eq!(app.mode, ViewMode::Video);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.current, 1);
        char_key(&mut app, 'l');
        assert_eq!(app.current, 2);
        char_key(&mut app, 'h');
        assert_eq!(app.current, 1);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.current, 1);

        char_key(&mut app, '?');
        assert!(app.help_visible);
        char_key(&mut app, 'x');
        assert!(!app.help_visible);

        press(&mut app, KeyCode::Home);
        assert_eq!(app.current, 0);
        assert_eq!(app.mode, ViewMode::Fullscreen, "a.jpg is a photo");

        let mut app = video_app();
        press(&mut app, KeyCode::End);
        assert_eq!(app.current, 3);
        assert_eq!(app.mode, ViewMode::Fullscreen, "b.jpg is a photo");
    }

    #[test]
    fn gallery_s_on_a_video_starts_the_slideshow_with_the_video() {
        let mut app = video_app();
        app.enter_gallery();
        app.gallery.cursor = 1;

        char_key(&mut app, 's');

        assert_eq!(app.mode, ViewMode::Video);
        assert!(app.slideshow_running());
        assert_eq!(app.video_return, crate::app::VideoReturn::Fullscreen);
    }

    #[test]
    fn video_delete_and_quit_keys() {
        let mut app = video_app();

        char_key(&mut app, 'd');
        let pending = app.pending_delete.as_ref().unwrap();
        assert_eq!(pending.paths, [PathBuf::from("/videos/clip.mp4")]);
        assert!(pending.from_fullscreen && !pending.permanent);
        press(&mut app, KeyCode::Esc);
        assert!(app.pending_delete.is_none());
        assert_eq!(app.mode, ViewMode::Video, "Esc only cancels the question");

        char_key(&mut app, 'D');
        assert!(app.pending_delete.as_ref().unwrap().permanent);
        char_key(&mut app, 'n');

        assert!(char_key(&mut app, 'q'));
    }
}
