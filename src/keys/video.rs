//! The keys of the terminal video player.

use super::step_key;
use crate::app::App;
use crate::player::SEEK_STEP;
use crate::video_handoff::focus_gallery_on_current;
use crossterm::event::{KeyCode, KeyEvent};
use std::io;

/// Keys of the player: pause, seek, the steps through the list, the mpv window.
///
/// `q` and Esc stop a running slideshow and go back: the user wants out. A step to another file
/// keeps the slideshow and gives that file the full interval, like in the fullscreen view.
/// `n` and `N` step like the same keys in mpv. `p` opens the video in the mpv window, with sound.
pub(crate) fn handle_video_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    match key.code {
        KeyCode::Char(' ') => {
            if let Some(ref mut v) = app.video {
                v.toggle_pause();
                app.needs_render = true;
            }
        }
        KeyCode::Left | KeyCode::Char('h') => seek(app, -SEEK_STEP),
        KeyCode::Right | KeyCode::Char('l') => seek(app, SEEK_STEP),
        KeyCode::Char('q') | KeyCode::Esc => leave_player(app)?,
        KeyCode::Char('p') => {
            app.graphics_delete_all()?;
            app.request_video_window();
        }
        KeyCode::Char('?') => {
            app.graphics_delete_all()?;
            app.help_visible = true;
        }
        KeyCode::Char('n') => {
            app.graphics_delete_all()?;
            step_from_player(app, crate::mpv::Step::Next);
        }
        KeyCode::Char('N') => {
            app.graphics_delete_all()?;
            step_from_player(app, crate::mpv::Step::Previous);
        }
        KeyCode::Home | KeyCode::End => {
            app.graphics_delete_all()?;
            step_key(app, key.code);
        }
        _ => {}
    }
    Ok(false)
}

/// Back to the view that opened the player, with the gallery cursor on the file.
pub(super) fn leave_player(app: &mut App) -> io::Result<()> {
    app.graphics_delete_all()?;
    app.stop_slideshow();
    app.return_from_video();
    focus_gallery_on_current(app);
    Ok(())
}

/// Move the position of the player by `seconds`.
pub(super) fn seek(app: &mut App, seconds: f64) {
    if let Some(ref mut v) = app.video {
        v.seek_by(seconds);
        app.needs_render = true;
    }
}

/// Step to the neighbor in the list, as `n` and `N` do in mpv. The end of the list keeps the
/// video, and the slideshow countdown starts again for the new file.
fn step_from_player(app: &mut App, step: crate::mpv::Step) {
    if !app.step_from_video(step) {
        app.needs_render = true;
    }
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

    /// Give `app` a player with a fake decoder on the current video.
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
    fn n_and_shift_n_step_between_the_files_and_home_and_end_jump() {
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
    fn a_step_past_the_end_of_the_list_keeps_the_video() {
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
    fn space_pauses_and_the_arrows_seek() {
        let mut app = video_app();
        char_key(&mut app, ' ');
        press(&mut app, KeyCode::Right);
        assert!(app.video.is_none(), "without a player, the keys do nothing");
        assert_eq!(app.current, 1);

        let decoder = with_player(&mut app);
        char_key(&mut app, ' ');
        assert!(!app.video.as_ref().unwrap().playing());
        char_key(&mut app, ' ');
        assert!(app.video.as_ref().unwrap().playing());

        let mut info = clip_info();
        info.duration = 60.0;
        let decoder_long = fake_decoder(info);
        drop(decoder);
        app.video = Some(
            Player::open(
                shared(&decoder_long),
                Path::new("/bin/ffmpeg"),
                Path::new("/videos/clip.mp4"),
                (64, 48),
                &[],
                true,
            )
            .unwrap(),
        );
        press(&mut app, KeyCode::Right);
        assert!(
            decoder_long.last_args().contains(" -ss 5.000 "),
            "{}",
            decoder_long.last_args()
        );
        char_key(&mut app, 'l');
        assert!(decoder_long.last_args().contains(" -ss 10.000 "));
        press(&mut app, KeyCode::Left);
        assert!(decoder_long.last_args().contains(" -ss 5.000 "));
        char_key(&mut app, 'h');
        assert!(!decoder_long.last_args().contains("-ss"));
        assert_eq!(decoder_long.starts().len(), 5);
        assert_eq!(app.mode, ViewMode::Video, "a seek never leaves the video");
    }

    #[test]
    fn p_stops_the_player_and_asks_for_the_mpv_window() {
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
    fn the_player_has_no_delete_keys_and_q_goes_back() {
        let mut app = video_app();
        let decoder = with_player(&mut app);

        char_key(&mut app, 'd');
        char_key(&mut app, 'D');
        assert!(
            app.pending_delete.is_none(),
            "viewr is a viewer: no delete in the player"
        );
        assert_eq!(app.mode, ViewMode::Video);

        assert!(!char_key(&mut app, 'q'), "q goes back, not out of viewr");
        assert_ne!(app.mode, ViewMode::Video);
        assert!(app.video.is_none());
        assert!(!decoder.streaming());
    }

    #[test]
    fn q_and_esc_in_the_player_during_a_slideshow_stop_it() {
        let mut esc_app = crate::test_support::video::slideshow_video_app();
        press(&mut esc_app, KeyCode::Esc);
        let mut q_app = crate::test_support::video::slideshow_video_app();
        assert!(!char_key(&mut q_app, 'q'));

        assert!(!q_app.slideshow_running());
        assert!(!esc_app.slideshow_running());
        assert_eq!(q_app.mode, esc_app.mode);
        assert_eq!(q_app.mode, ViewMode::Fullscreen);
        assert!(q_app.shows_video_placeholder());
    }
}
