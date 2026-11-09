//! The tests of the run loop: the waits, the input step, the mouse capture and the renders.
use super::{
    BUSY_POLL, CACHE_READ_POLL, FIRST_PICTURE_POLL, IDLE_POLL, LoopState, SHAPE_POLL,
    SHAPE_POLL_WINDOW, clears_status, draw_ui, handle_input, mouse_capture_wanted, next_timeout,
    sync_mouse_capture, waits_for_a_fast_shape, window_open,
};
use crate::app::ViewMode;
use crate::config::Config;
use crate::test_support::{app, app_with, app_with_files, gallery_app, test_dir, wait_until};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use std::time::{Duration, Instant};

#[test]
fn leftover_emits_need_no_wait() {
    let mut app = app_with(&["a.png"]);
    app.scan_complete = true;
    assert_eq!(next_timeout(&app, &[0]), Duration::ZERO);
    app.mode = ViewMode::Fullscreen;
    assert!(
        next_timeout(&app, &[0]) > Duration::ZERO,
        "the fullscreen view writes no grid pictures"
    );
}

#[test]
fn fullscreen_emit_waits_idle() {
    let mut app = app_with(&["a.png"]);
    app.scan_complete = true;
    app.mode = ViewMode::Fullscreen;
    app.needs_render = false;
    let mut pending = vec![0];
    assert!(!super::render_images(&mut app, &mut pending).unwrap());
    assert!(pending.is_empty());
    assert_eq!(next_timeout(&app, &pending), IDLE_POLL);
}

#[test]
fn key_press_clears_scan_error() {
    // The loop clears the scan error where it clears the status note: on `clears_status`.
    let mut app = app_with(&["a.png"]);
    app.scan_error = Some("denied".to_owned());
    app.enter_fullscreen_at(0);
    assert!(app.scan_error.is_some(), "the automatic work keeps it");
    let press = Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    if clears_status(&press) {
        app.scan_error = None;
    }
    assert!(app.scan_error.is_none());
}

#[test]
fn only_key_press_clears_note() {
    let press = Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    let release = Event::Key(KeyEvent::new_with_kind(
        KeyCode::Char('a'),
        KeyModifiers::NONE,
        KeyEventKind::Release,
    ));
    assert!(clears_status(&press));
    assert!(!clears_status(&release));
    assert!(!clears_status(&Event::Resize(80, 24)));
}

/// A loop job alone polls fast, so its frames go out soon after its ffmpeg run.
#[test]
fn loop_job_alone_polls_busy() {
    let mut app = gallery_app(3);
    app.pretend_loop_job(0);
    assert!(app.has_pending_work(), "the folder prefetch still waits");
    assert_eq!(next_timeout(&app, &[]), BUSY_POLL);
}

#[test]
fn slideshow_wakes_for_next_slide() {
    let mut app = gallery_app(3);
    assert_eq!(next_timeout(&app, &[]), IDLE_POLL);
    app.slideshow_interval = Duration::from_millis(5);
    app.start_slideshow();
    assert!(next_timeout(&app, &[]) <= Duration::from_millis(5));
}

#[test]
fn folder_rest_wakes_for_prefetch() {
    let temp = test_dir();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let folder = root.join("sub");
    std::fs::create_dir(&folder).unwrap();
    for name in ["a.png", "b.png"] {
        image::RgbaImage::new(4, 3).save(folder.join(name)).unwrap();
    }
    let mut app = app();
    app.gallery.update_grid(Rect::new(0, 0, 120, 40));
    app.switch_to_dir(&root);
    wait_until(|| {
        app.refresh_from_scanner();
        app.poll_cell_shape();
        app.scan_complete && !app.has_pending_work()
    });
    assert_eq!(app.filenames, ["..", "sub"]);
    app.gallery.cursor = 1;
    assert_eq!(next_timeout(&app, &[]), IDLE_POLL);

    let rested = Instant::now()
        .checked_sub(Duration::from_millis(290))
        .unwrap();
    app.tick_folder_prefetch(rested);
    assert!(next_timeout(&app, &[]) < Duration::from_millis(100));
}

#[cfg(feature = "video")]
#[test]
fn mpv_window_polls_every_100ms() {
    let mut app = gallery_app(2);
    assert_eq!(next_timeout(&app, &[]), IDLE_POLL);
    let (_exit, exit_rx) = std::sync::mpsc::channel();
    app.watch_video_window(exit_rx, false);
    assert_eq!(next_timeout(&app, &[]), super::WINDOW_POLL);
}

#[test]
fn cache_read_polls_every_2ms() {
    let dir = test_dir();
    let names = ["a.png", "b.png", "c.png", "d.png", "e.png"];
    let mut app = app_with_files(dir.path(), &names);
    app.scan_complete = true;
    for index in 0..names.len() {
        app.spawn_thumb_decode(index, Rect::new(0, 0, 1, 1));
    }
    assert!(app.cache_reads_pending(), "the fifth cell waits and reads");
    assert_eq!(next_timeout(&app, &[]), CACHE_READ_POLL);
}

#[test]
fn first_picture_polls_every_2ms() {
    let dir = test_dir();
    let mut app = app_with_files(dir.path(), &["a.png", "b.png"]);
    app.scan_complete = true;
    app.gallery.update_grid(Rect::new(0, 0, 80, 24));
    app.spawn_thumb_decode(0, Rect::new(0, 0, 1, 1));
    assert!(app.waits_for_the_first_picture());
    assert_eq!(next_timeout(&app, &[]), FIRST_PICTURE_POLL);

    app.thumb_cache.insert(1, image::RgbaImage::new(1, 1));
    assert!(!app.waits_for_the_first_picture(), "one picture shows");
}

#[test]
fn shape_read_polls_every_1ms() {
    let dir = test_dir();
    let mut app = app();
    let path = dir.path().join("a.png");
    image::RgbaImage::new(30, 40).save(&path).unwrap();
    app.images.push(path);
    app.filenames.push("a.png".to_owned());

    app.request_cell_shape();
    assert_eq!(next_timeout(&app, &[]), SHAPE_POLL);
    wait_until(|| {
        app.poll_cell_shape();
        !app.cell_shape_pending()
    });
    assert!(next_timeout(&app, &[]) > SHAPE_POLL);
}

#[test]
fn slow_shape_read_waits_normally() {
    assert!(!waits_for_a_fast_shape(None), "no read runs");
    assert!(waits_for_a_fast_shape(Some(Duration::ZERO)));
    assert!(!waits_for_a_fast_shape(Some(SHAPE_POLL_WINDOW)));
}

#[test]
fn window_releases_mouse() {
    let mut state = LoopState::new(&Config::default());
    assert!(state.mouse_captured);
    let mut out = Vec::new();

    sync_mouse_capture(&mut out, &mut state, false).unwrap();
    assert!(out.is_empty(), "nothing changed");
    sync_mouse_capture(&mut out, &mut state, true).unwrap();
    assert!(!state.mouse_captured);
    assert!(
        String::from_utf8_lossy(&out).contains("\x1b[?1000l"),
        "{out:?}"
    );
    out.clear();
    sync_mouse_capture(&mut out, &mut state, true).unwrap();
    assert!(out.is_empty(), "released once");
    sync_mouse_capture(&mut out, &mut state, false).unwrap();
    assert!(state.mouse_captured);
    assert!(
        String::from_utf8_lossy(&out).contains("\x1b[?1000h"),
        "{out:?}"
    );
}

#[test]
fn mouse_off_writes_no_capture() {
    let config = Config {
        mouse: false,
        ..Config::default()
    };
    let mut state = LoopState::new(&config);
    let mut out = Vec::new();
    for window in [false, true, false] {
        sync_mouse_capture(&mut out, &mut state, window).unwrap();
    }
    assert!(out.is_empty());
    assert!(!mouse_capture_wanted(false, false));
    assert!(mouse_capture_wanted(true, false));
    assert!(!mouse_capture_wanted(true, true));
}

#[cfg(feature = "video")]
#[test]
fn help_popup_holds_player() {
    use crate::app::ViewMode;
    use crate::player::Player;
    use crate::test_support::video::{clip_info, fake_decoder, shared, video_app};
    use std::path::Path;

    let mut app = video_app();
    app.image_rect = Rect::new(0, 0, 100, 40);
    let decoder = fake_decoder(clip_info());
    app.video = Some(
        Player::open(
            shared(&decoder),
            Path::new("/bin/ffmpeg"),
            Path::new("/videos/clip.mp4"),
            (64, 48),
            &[],
            true,
        )
        .unwrap(),
    );
    assert!(!crate::test_support::char_key(&mut app, '?'));
    assert!(app.help_visible);
    assert!(
        crate::test_support::text(&app.graphics_out.take_bytes()).contains("a=d"),
        "the help deletes the pictures first"
    );
    decoder.feed(vec![1; 64 * 48 * 4]);
    app.needs_render = false;
    let mut pending = Vec::new();

    super::poll_background(&mut app, &mut pending);
    assert!(!app.needs_render, "no frame while the help is open");
    assert!(app.video.as_ref().unwrap().current_frame.is_none());
    app.needs_render = true;
    super::render_images(&mut app, &mut pending).unwrap();
    assert!(app.graphics_out.take_bytes().is_empty());

    assert!(!crate::test_support::char_key(&mut app, 'x'));
    assert!(!app.help_visible);
    assert_eq!(app.mode, ViewMode::Video);
    super::poll_background(&mut app, &mut pending);
    assert!(app.needs_render, "the frame comes after the help");
    super::render_images(&mut app, &mut pending).unwrap();
    let out = app.graphics_out.take_bytes();
    for path in crate::encoder::kitty::temp_file_paths(&out) {
        std::fs::remove_file(path).unwrap();
    }
    assert!(crate::test_support::text(&out).contains("a=T,"));
}

/// A thumbnail that arrives while the help popup is open waits for the close. The close
/// renders the grid again, and that write places the picture.
#[test]
fn help_popup_holds_late_thumbnails() {
    let mut app = app_with(&["a.jpg"]);
    app.scan_complete = true;
    app.gallery.zoom_rows = 4;
    app.gallery.update_grid(Rect::new(0, 0, 106, 29));
    let image = image::RgbaImage::from_pixel(160, 96, image::Rgba([9, 9, 9, 255]));
    let picture = crate::app::thumbnails::pad_for_grid(image, (160, 96), (8, 16), true);
    app.thumb_cache.insert(0, picture);
    assert!(!crate::test_support::char_key(&mut app, '?'));
    assert!(app.help_visible);
    app.graphics_out.take_bytes();

    let mut pending = vec![0];
    app.needs_render = true;
    assert!(!super::render_images(&mut app, &mut pending).unwrap());
    assert!(
        app.graphics_out.take_bytes().is_empty(),
        "no picture while the help is open"
    );
    assert!(pending.is_empty(), "the close renders the grid again");

    assert!(!crate::test_support::char_key(&mut app, 'x'));
    assert!(!app.help_visible);
    assert!(super::render_images(&mut app, &mut pending).unwrap());
    let out = app.graphics_out.take_bytes();
    for path in crate::encoder::kitty::temp_file_paths(&out) {
        std::fs::remove_file(path).unwrap();
    }
    assert!(
        crate::test_support::text(&out).contains("a=T,"),
        "the picture comes after the help"
    );
}

#[test]
fn state_starts_without_status() {
    let config = Config {
        video_player: crate::config::VideoPlayer::Window,
        ..Config::default()
    };

    let state = LoopState::new(&config);

    assert_eq!(state.status, None);
    #[cfg(feature = "video")]
    assert_eq!(state.video.player, crate::config::VideoPlayer::Window);
}

#[test]
fn late_answer_keeps_note() {
    let mut app = gallery_app(3);
    let mut state = LoopState::new(&Config::default());
    state.late_answers = crate::keys::LateAnswerFilter::new(1);
    state.status = Some("note".into());
    let alt_underscore = Event::Key(KeyEvent::new(KeyCode::Char('_'), KeyModifiers::ALT));
    assert!(!handle_input(&mut app, &mut state, &alt_underscore).unwrap());
    assert_eq!(state.status.as_deref(), Some("note"));
}

#[test]
fn input_clears_note_and_quits() {
    let mut app = gallery_app(3);
    let mut state = LoopState::new(&Config::default());
    state.status = Some("note".into());
    app.scan_error = Some("unreadable".into());
    let q = Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(handle_input(&mut app, &mut state, &q).unwrap());
    assert_eq!(state.status, None);
    assert_eq!(app.scan_error, None);
}

#[test]
fn resize_keeps_note_and_redraws() {
    let mut app = gallery_app(3);
    let mut state = LoopState::new(&Config::default());
    state.status = Some("note".into());
    app.needs_render = false;
    assert!(!handle_input(&mut app, &mut state, &Event::Resize(80, 24)).unwrap());
    assert_eq!(state.status.as_deref(), Some("note"));
    assert!(app.needs_render);
}

#[test]
fn mouse_press_clears_note() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

    let event = |kind| {
        Event::Mouse(MouseEvent {
            kind,
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        })
    };
    assert!(clears_status(&event(MouseEventKind::Down(
        MouseButton::Left
    ))));
    assert!(!clears_status(&event(MouseEventKind::Up(
        MouseButton::Left
    ))));
}

#[test]
fn idle_gallery_plays_loops() {
    let mut app = gallery_app(3);
    app.needs_render = false;
    let mut pending = Vec::new();
    assert!(!super::render_images(&mut app, &mut pending).unwrap());
    assert!(
        app.graphics_out.take_bytes().is_empty(),
        "no loop, no write"
    );
}

#[test]
fn loaded_image_starts_prefetch() {
    let dir = test_dir();
    let mut app = app_with_files(dir.path(), &["a.png", "b.png"]);
    app.image_rect = Rect::new(0, 0, 10, 10);
    app.enter_fullscreen_at(0);
    let mut pending = Vec::new();
    wait_until(|| {
        super::poll_background(&mut app, &mut pending);
        app.needs_render = true;
        super::render_images(&mut app, &mut pending).unwrap();
        app.loaded.is_some()
    });
    app.needs_render = true;
    assert!(super::render_images(&mut app, &mut pending).unwrap());
    assert!(
        app.prefetcher.has_jobs(),
        "the neighbor decodes after the image"
    );
}

#[cfg(feature = "video")]
#[test]
fn player_clock_sets_wait() {
    use crate::player::Player;
    use crate::test_support::video::{clip_info, fake_decoder, shared, video_app};
    use std::path::Path;

    let mut app = video_app();
    let decoder = fake_decoder(clip_info());
    app.video = Some(
        Player::open(
            shared(&decoder),
            Path::new("/bin/ffmpeg"),
            Path::new("/videos/clip.mp4"),
            (64, 48),
            &[],
            true,
        )
        .unwrap(),
    );
    let frame_wait = app.video.as_ref().unwrap().time_until_next_frame();
    assert!(next_timeout(&app, &[]) <= frame_wait);
}

#[test]
fn window_closed_without_mpv() {
    assert!(!window_open(&app()));
}

#[test]
fn draw_records_fullscreen_box() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut app = gallery_app(3);
    let state = LoopState::new(&Config::default());
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal
        .draw(|frame| draw_ui(frame, &mut app, &state))
        .unwrap();
    assert_eq!(app.fullscreen_box, Rect::new(0, 0, 80, 23));
    app.info_visible = true;
    terminal
        .draw(|frame| draw_ui(frame, &mut app, &state))
        .unwrap();
    assert_eq!(app.fullscreen_box, Rect::new(0, 0, 54, 23));
}

#[test]
fn fullscreen_render_draws_chrome_again() {
    let mut app = crate::test_support::fullscreen_app(3, 0);
    app.needs_render = true;
    let mut pending = vec![1];
    assert!(super::render_images(&mut app, &mut pending).unwrap());
    assert!(!app.needs_render);
    assert!(pending.is_empty());
}

#[test]
fn pending_emits_go_out_without_render() {
    let mut app = gallery_app(3);
    app.needs_render = false;
    let mut pending = vec![0];
    assert!(!super::render_images(&mut app, &mut pending).unwrap());
    assert!(
        pending.is_empty(),
        "a cell without a thumbnail writes nothing"
    );
}
