//! The tests of the run loop: the waits, the input step, the mouse capture and the renders.
use super::{
    BUSY_POLL, CACHE_READ_POLL, FIRST_PICTURE_POLL, IDLE_POLL, LoopState, SHAPE_POLL,
    SHAPE_POLL_WINDOW, clears_status, draw_ui, handle_input, mouse_capture_wanted, next_timeout,
    release_held, sync_mouse_capture, waits_for_a_fast_shape, window_open,
};
use crate::app::ViewMode;
use crate::config::Config;
use crate::test_support::{app, app_with, app_with_files, gallery_app, test_dir, wait_until};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use std::time::{Duration, Instant};
use tuilibr::pictures::kitty::answers::ANSWER_WAIT;
use tuilibr::pictures::kitty::query::LateAnswers;
use tuilibr::pictures::kitty::screen::Screen;
use tuilibr::pictures::test_support::key_events;
use tuilibr::terminal::answers::LateAnswerFilter;

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
    for path in tuilibr::pictures::kitty::protocol::temp_file_paths(&out) {
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
    let picture = tuilibr::pictures::resize::padding::pad_for_grid(image, (160, 96), (8, 16), true);
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
    for path in tuilibr::pictures::kitty::protocol::temp_file_paths(&out) {
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

/// A filter that owes one late Kitty answer and no OSC 10 answer.
fn owing_kitty_answer() -> LateAnswerFilter {
    LateAnswerFilter::new(LateAnswers {
        kitty: 1,
        foreground: false,
    })
}

#[test]
fn late_answer_keeps_note() {
    let mut app = gallery_app(3);
    let mut state = LoopState::new(&Config::default());
    state.answers = owing_kitty_answer();
    state.status = Some("note".into());
    let alt_underscore = Event::Key(KeyEvent::new(KeyCode::Char('_'), KeyModifiers::ALT));
    assert!(!handle_input(&mut app, &mut state, &alt_underscore).unwrap());
    assert_eq!(state.status.as_deref(), Some("note"));
}

#[test]
fn answer_marks_a_lost_picture_and_keys_pass() {
    let mut app = gallery_app(3);
    app.screen = Screen::default().with_self_check(true);
    let mut state = LoopState::new(&Config::default());
    app.screen.record(7, 16);
    let mut input = b"\x1b_Gi=7,p=1;ENOENT: image not found\x1b\\".to_vec();
    input.extend_from_slice(b"\x1b_Gi=9;OK\x1b\\");
    for event in key_events(&input) {
        assert!(!handle_input(&mut app, &mut state, &event).unwrap());
    }
    assert!(app.screen.has_lost());
    assert!(!app.screen.stores(7));
    assert_eq!(
        app.gallery.cursor, 0,
        "no key of the answers moved the cursor"
    );
    let q = Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(handle_input(&mut app, &mut state, &q).unwrap());
}

#[test]
fn a_broken_answer_goes_when_the_input_is_quiet() {
    let mut app = gallery_app(30);
    let mut state = LoopState::new(&Config::default());
    // A cut answer whose letters are keys of phewr: `j` moves, `q` quits.
    for event in key_events(b"\x1b_Gi=1;jjq") {
        assert!(!handle_input(&mut app, &mut state, &event).unwrap());
    }
    let wait = state.answers.wait(Instant::now()).unwrap();
    assert!(wait <= ANSWER_WAIT);
    assert!(!release_held(&mut app, &mut state, Instant::now() + ANSWER_WAIT).unwrap());
    assert_eq!(state.answers.wait(Instant::now()), None);
    assert_eq!(app.gallery.cursor, 0, "no key of the cut answer ran");
    let j = Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert!(!handle_input(&mut app, &mut state, &j).unwrap());
    assert_ne!(app.gallery.cursor, 0, "the next key runs");
}

#[test]
fn a_quiet_input_gives_a_lone_alt_key_back() {
    let mut app = gallery_app(3);
    let mut state = LoopState::new(&Config::default());
    for event in key_events(b"\x1b_") {
        assert!(!handle_input(&mut app, &mut state, &event).unwrap());
    }
    assert!(state.answers.wait(Instant::now()).is_some(), "Alt+_ waits");
    assert!(!release_held(&mut app, &mut state, Instant::now() + ANSWER_WAIT).unwrap());
    assert_eq!(state.answers.wait(Instant::now()), None);
}

#[test]
fn self_check_answer_wins_over_an_owed_probe_answer() {
    let mut app = gallery_app(3);
    app.screen = Screen::default().with_self_check(true);
    app.screen.record(7, 16);
    let mut state = LoopState::new(&Config::default());
    // The probe answer timed out and never comes. The first Kitty answer is an ENOENT.
    state.answers = owing_kitty_answer();
    for event in key_events(b"\x1b_Gi=7,p=1;ENOENT: image not found\x1b\\") {
        assert!(!handle_input(&mut app, &mut state, &event).unwrap());
    }
    assert!(app.screen.has_lost(), "the screen gets the answer");
    // The answer counted as the owed one: the filter swallows no later Alt+`_` sequence.
    for event in key_events(b"\x1b_x") {
        assert!(!handle_input(&mut app, &mut state, &event).unwrap());
    }
    let q = Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(handle_input(&mut app, &mut state, &q).unwrap());
}

#[test]
fn late_probe_answer_then_self_check_answer() {
    let mut app = gallery_app(3);
    app.screen = Screen::default().with_self_check(true);
    app.screen.record(7, 16);
    let mut state = LoopState::new(&Config::default());
    state.answers = owing_kitty_answer();
    let mut input = b"\x1b_Gi=31;OK\x1b\\".to_vec();
    input.extend_from_slice(b"\x1b_Gi=7,p=1;ENOENT: image not found\x1b\\");
    for event in key_events(&input) {
        assert!(!handle_input(&mut app, &mut state, &event).unwrap());
    }
    assert!(app.screen.has_lost());
    assert_eq!(app.gallery.cursor, 0);
}

#[test]
fn self_check_answer_leaves_a_late_foreground_owed() {
    let mut app = gallery_app(3);
    app.screen = Screen::default().with_self_check(true);
    app.screen.record(7, 16);
    let mut state = LoopState::new(&Config::default());
    state.answers = LateAnswerFilter::new(LateAnswers {
        kitty: 0,
        foreground: true,
    });
    // An ENOENT that nobody owes, then the late OSC 10 answer. Its `/` opens the search.
    let mut input = b"\x1b_Gi=7,p=1;ENOENT: image not found\x1b\\".to_vec();
    input.extend_from_slice(b"\x1b]10;rgb:ff/ff/ff\x1b\\");
    for event in key_events(&input) {
        assert!(!handle_input(&mut app, &mut state, &event).unwrap());
    }
    assert!(!release_held(&mut app, &mut state, Instant::now() + ANSWER_WAIT).unwrap());
    assert!(app.screen.has_lost());
    assert!(!app.gallery.search_active, "no key of the answer ran");
    let q = Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(
        handle_input(&mut app, &mut state, &q).unwrap(),
        "the answer ended"
    );
}

#[test]
fn held_keys_come_back_with_the_next_event() {
    let mut app = gallery_app(3);
    let mut state = LoopState::new(&Config::default());
    // Alt+`_` can start an answer. The `q` after it quits.
    for event in key_events(b"\x1b_") {
        assert!(!handle_input(&mut app, &mut state, &event).unwrap());
    }
    let q = Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(handle_input(&mut app, &mut state, &q).unwrap());
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

/// The fullscreen box is the main pane: the whole area without the info pane, inside its box in
/// the maximal layout with it, right of the divider in the minimal layout.
#[test]
fn draw_records_fullscreen_box() {
    use ratatui::{Terminal, backend::TestBackend};
    use tuilibr::frame::Layout;

    let mut app = gallery_app(3);
    let state = LoopState::new(&Config::default());
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let mut boxes = Vec::new();
    for (layout, info) in [
        (Layout::Maximal, false),
        (Layout::Maximal, true),
        (Layout::Minimal, false),
        (Layout::Minimal, true),
    ] {
        app.pane_layout = layout;
        app.info_visible = info;
        terminal
            .draw(|frame| {
                draw_ui(frame, &mut app, &state);
            })
            .unwrap();
        boxes.push(app.fullscreen_box);
    }
    assert_eq!(
        boxes,
        [
            Rect::new(0, 0, 80, 22),
            Rect::new(37, 1, 42, 20),
            Rect::new(0, 0, 80, 22),
            Rect::new(37, 0, 43, 22),
        ]
    );
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

/// An unknown `[gallery] layout` value leaves a note in the status row at the start.
#[test]
fn unknown_layout_note_shows_at_start() {
    use ratatui::{Terminal, backend::TestBackend};

    let note = "unknown layout \"boxed\", maximal";
    let config = Config {
        layout_note: Some(note.to_owned()),
        ..Config::default()
    };
    let state = LoopState::new(&config);
    assert_eq!(state.status.as_deref(), Some(note));
    let mut app = gallery_app(3);
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    terminal
        .draw(|frame| {
            draw_ui(frame, &mut app, &state);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let status_row: String = (0..100).map(|x| buffer[(x, 23)].symbol()).collect();
    assert!(status_row.contains(note), "{status_row}");
}

/// A writer into the byte buffer of the Kitty output, so the backend and the Kitty writes share
/// one stream in the order of the terminal.
struct SharedStream(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for SharedStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A Kitty placement moves the terminal cursor, so the draw pass ends with the cursor on the
/// search field.
#[test]
fn draw_pass_puts_the_cursor_back_on_the_search_field() {
    use ratatui::backend::CrosstermBackend;
    use ratatui::{Terminal, TerminalOptions, Viewport};
    use tuilibr::pictures::kitty::GraphicsOutput;

    let mut app = app_with(&["a.jpg"]);
    app.scan_complete = true;
    app.gallery.zoom_rows = 4;
    let image = image::RgbaImage::from_pixel(160, 96, image::Rgba([9, 9, 9, 255]));
    let picture = tuilibr::pictures::resize::padding::pad_for_grid(image, (160, 96), (8, 16), true);
    app.thumb_cache.insert(0, picture);
    app.gallery.search_active = true;
    app.gallery.search_query = tuilibr::text_field::TextField::from("ab");
    let GraphicsOutput::Buffer(stream) = &app.graphics_out else {
        panic!("a test app writes to a buffer");
    };
    let stream = std::sync::Arc::clone(stream);
    let mut terminal = Terminal::with_options(
        CrosstermBackend::new(SharedStream(std::sync::Arc::clone(&stream))),
        TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, 0, 106, 31)),
        },
    )
    .unwrap();
    let state = LoopState::new(&Config::default());
    // A thumbnail that arrives later goes out without a second draw, and no draw moves the
    // cursor after it.
    let mut pending = vec![0];
    app.needs_render = false;

    super::draw_pass(&mut terminal, &mut app, &state, &mut pending).unwrap();
    let out = app.graphics_out.take_bytes();
    for path in tuilibr::pictures::kitty::protocol::temp_file_paths(&out) {
        std::fs::remove_file(path).unwrap();
    }
    let text = crate::test_support::text(&out);
    let placement = text.find("a=p,").or_else(|| text.find("a=T,"));
    assert!(placement.is_some(), "the pass places the thumbnail");
    // The omnibox is the last row, and the cursor sits after `/ab`: CUP is 1-based.
    let field = "\x1b[31;4H";
    let cursor_set = text
        .find(field)
        .expect("the draw sets the cursor on the field");
    let placement = placement.unwrap();
    assert!(
        cursor_set < placement,
        "the draw sets the cursor before the Kitty write"
    );
    let save = text.find("\x1b7").expect("the update saves the cursor");
    assert!(
        cursor_set < save && save < placement,
        "the save comes after the cursor"
    );
    let restore = text.rfind("\x1b8").expect("the update restores the cursor");
    assert!(restore > placement, "the restore comes after the placement");
    let after = text.get(restore..).unwrap_or_default();
    assert!(
        !after.contains('H'),
        "no write moves the cursor after the restore: {after:?}"
    );
}
