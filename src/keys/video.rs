//! The keys of the terminal video player.

use super::step_key;
use crate::app::App;
use crate::mpv::Step;
use crate::video_handoff::focus_gallery_on_current;
use crossterm::event::{KeyCode, KeyEvent};
use std::io;

/// The player has no pause and no seek: `p` opens the video in the mpv window.
///
/// `q` and Esc stop a running slideshow and go back. A step keeps the slideshow and gives the
/// new file the full interval, as in the fullscreen view.
pub(crate) fn handle_video_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    match key.code {
        KeyCode::Char(' ') => app.slideshow_toggle_pause(),
        KeyCode::Left | KeyCode::Char('h' | 'j' | 'N') => step(app, Step::Previous)?,
        KeyCode::Right | KeyCode::Char('k' | 'l' | 'n') => step(app, Step::Next)?,
        KeyCode::Char('q') | KeyCode::Esc => leave_player(app)?,
        KeyCode::Char('p') => {
            app.graphics_delete_all()?;
            app.request_video_window();
        }
        KeyCode::Char('?') => {
            app.graphics_delete_all()?;
            app.help_visible = true;
        }
        KeyCode::Home | KeyCode::End => {
            app.graphics_delete_all()?;
            step_key(app, key.code);
        }
        _ => {}
    }
    Ok(false)
}

/// A running slideshow stops.
pub(super) fn leave_player(app: &mut App) -> io::Result<()> {
    app.graphics_delete_all()?;
    app.stop_slideshow();
    // The player has no controls. The fullscreen placeholder is no step back.
    app.exit_video();
    focus_gallery_on_current(app);
    Ok(())
}

/// The end of the list keeps the video.
pub(super) fn step(app: &mut App, step: Step) -> io::Result<()> {
    app.graphics_delete_all()?;
    if !app.step_from_video(step) {
        app.needs_render = true;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::app::{App, ViewMode};
    use crate::player::Player;
    use crate::test_support::video::{FakeDecoder, clip_info, fake_decoder, shared};
    use crate::test_support::{char_key, gallery_app, press};
    use crossterm::event::KeyCode;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

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

    fn with_player(app: &mut App) -> Rc<FakeDecoder> {
        let decoder = fake_decoder(clip_info());
        app.video = Some(
            Player::open(
                shared(&decoder),
                Path::new("/bin/ffmpeg"),
                app.current_path(),
                (64, 48),
                &[],
                true,
            )
            .unwrap(),
        );
        decoder
    }

    #[test]
    fn n_steps_and_home_end_jump() {
        let mut app = video_app();
        let decoder = with_player(&mut app);

        char_key(&mut app, 'n');
        assert_eq!(app.current, 2);
        assert_eq!(app.mode, ViewMode::Video);
        assert!(app.video.is_none(), "the step stops the player");
        assert!(!decoder.streaming());
        char_key(&mut app, 'N');
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
    fn step_past_end_keeps_video() {
        let mut app = video_app();
        app.enter_video(3);
        app.images[3] = PathBuf::from("/videos/last.mp4");
        with_player(&mut app);
        app.needs_render = false;

        char_key(&mut app, 'n');

        assert_eq!(app.current, 3);
        assert_eq!(app.mode, ViewMode::Video);
        assert!(app.video.is_some(), "the player plays on");
        assert!(app.needs_render);
    }

    #[test]
    fn arrows_step_space_ignored() {
        let mut app = video_app();
        let decoder = with_player(&mut app);
        char_key(&mut app, ' ');
        assert!(
            app.video.as_ref().unwrap().playing(),
            "no pause in the player"
        );
        assert_eq!(decoder.starts().len(), 1, "no seek");

        press(&mut app, KeyCode::Right);
        assert_eq!(app.current, 2);
        assert_eq!(app.mode, ViewMode::Video);
        char_key(&mut app, 'h');
        assert_eq!(app.current, 1);
        char_key(&mut app, 'l');
        assert_eq!(app.current, 2);
        char_key(&mut app, 'j');
        assert_eq!(app.current, 1);
        char_key(&mut app, 'k');
        assert_eq!(app.current, 2);
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.current, 0);
        assert_eq!(app.mode, ViewMode::Fullscreen, "a.jpg is a photo");
    }

    #[test]
    fn p_opens_mpv_window() {
        let mut app = video_app();
        let decoder = with_player(&mut app);

        assert!(!char_key(&mut app, 'p'));

        assert!(app.video.is_none());
        assert!(!decoder.streaming(), "ffmpeg is gone");
        assert_eq!(app.mode, ViewMode::Video, "the run loop opens the window");
        assert!(app.take_video_window_request());
        assert!(
            !app.take_video_window_request(),
            "the request is taken once"
        );
    }

    #[test]
    fn s_starts_slideshow_on_video() {
        let mut app = video_app();
        app.enter_gallery();
        app.gallery.cursor = 1;

        char_key(&mut app, 's');

        assert_eq!(app.mode, ViewMode::Video);
        assert!(app.slideshow_running());
        assert_eq!(app.video_return, crate::app::VideoReturn::Fullscreen);
    }

    #[test]
    fn player_ignores_d_keys() {
        let mut app = video_app();
        let decoder = with_player(&mut app);

        char_key(&mut app, 'd');
        char_key(&mut app, 'D');
        assert!(
            app.selection.is_empty(),
            "viewr is a viewer: d does nothing"
        );
        assert_eq!(app.mode, ViewMode::Video);

        assert!(!char_key(&mut app, 'q'), "q goes back, not out of viewr");
        assert_ne!(app.mode, ViewMode::Video);
        assert!(app.video.is_none());
        assert!(!decoder.streaming());
    }

    #[test]
    fn q_and_esc_stop_slideshow() {
        let mut esc_app = crate::test_support::video::slideshow_video_app();
        press(&mut esc_app, KeyCode::Esc);
        let mut q_app = crate::test_support::video::slideshow_video_app();
        assert!(!char_key(&mut q_app, 'q'));

        assert!(!q_app.slideshow_running());
        assert!(!esc_app.slideshow_running());
        assert_eq!(q_app.mode, esc_app.mode);
        assert_eq!(
            q_app.mode,
            ViewMode::Gallery,
            "one q leaves the player for the grid"
        );
    }

    #[test]
    fn one_q_leaves_stepped_video() {
        let mut app =
            crate::test_support::video::fullscreen_app_on_video(&["a.jpg", "clip.mp4"], false);
        assert!(!char_key(&mut app, 'q'));
        assert_eq!(app.mode, ViewMode::Gallery);
        assert_eq!(
            app.gallery.selected_index(),
            Some(1),
            "the cursor is on the video"
        );
    }
}
