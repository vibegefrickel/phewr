//! The tests of the status row of each view.
use crate::app::ViewMode;
use crate::ui::tests::{
    app_with, assert_ansi, assert_default_fg, assert_hides, assert_shows, assert_symbol,
    assert_terminal_background, cell_of, render, render_buffer, screen_rows, status_row,
};
use image::RgbaImage;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

#[test]
fn gallery_shows_position_without_hints() {
    let mut app = app_with(&["a.jpg", "b.jpg"]);
    app.gallery.show_filenames = true;
    let screen = render(&mut app, None, 100, 30);
    assert_eq!(status_row(&screen).trim_end(), " a.jpg [1/2]", "{screen}");
    assert_hides(&screen, "?: help");
    assert_shows(&screen, "b.jpg");
    assert_hides(&screen, "Gallery");
    assert_hides(&screen, "Search");
    assert_eq!(app.gallery.grid_rect, Rect::new(0, 0, 100, 29), "no frame");
}

#[test]
fn empty_folder_shows_note() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = app_with(&[]);
    app.scan_complete = true;
    let screen = render(&mut app, None, 100, 30);
    assert!(
        status_row(&screen).starts_with(" No images or folders here"),
        "{screen}"
    );
    assert_hides(&screen, "[1/");

    app.switch_to_dir(temp.path());
    crate::test_support::wait_until(|| {
        app.refresh_from_scanner();
        !app.is_scanning()
    });
    assert_eq!(app.filenames, [".."], "the scan lists only the parent");
    let screen = render(&mut app, None, 100, 30);
    assert_shows(&screen, "No images or folders here");
    assert_hides(&screen, "Backspace");

    std::fs::create_dir(temp.path().join("sub")).unwrap();
    app.switch_to_dir(temp.path());
    crate::test_support::wait_until(|| {
        app.refresh_from_scanner();
        !app.is_scanning()
    });
    let screen = render(&mut app, None, 100, 30);
    assert_hides(&screen, "No images or folders here");
}

#[test]
fn search_shows_query_and_matches() {
    let mut app = app_with(&["a.jpg", "b.jpg"]);
    app.gallery.search_active = true;
    app.gallery.search_query = "b".to_owned();
    app.gallery.filtered_indices = vec![1];
    let screen = render(&mut app, None, 100, 30);
    assert!(
        status_row(&screen).trim_end() == " /b\u{2588}  1/2 matches",
        "{screen}"
    );

    // After Enter, the filter stays and the position counts the matches.
    app.gallery.search_active = false;
    let screen = render(&mut app, None, 100, 30);
    assert!(status_row(&screen).starts_with(" b.jpg [1/1] "), "{screen}");
}

#[test]
fn gallery_shows_selection_count() {
    let mut app = app_with(&["a.jpg", "b.jpg"]);
    app.gallery.show_filenames = true;
    app.selection.insert(app.images[1].clone());
    let screen = render(&mut app, None, 100, 30);
    assert!(
        status_row(&screen).trim_end() == " a.jpg [1/2]  1 selected",
        "{screen}"
    );
    assert_shows(&screen, "\u{25cf} b.jpg");
}

#[test]
fn gallery_row_holds_search_and_scan() {
    let mut app = app_with(&["a.jpg", "b.jpg"]);
    app.scan_complete = false;
    let screen = render(&mut app, None, 100, 30);
    assert!(
        status_row(&screen).trim_end() == " a.jpg [1/2]  scanning\u{2026}",
        "{screen}"
    );

    app.selection.insert(app.images[0].clone());
    app.gallery.search_active = true;
    app.gallery.search_query = "a".to_owned();
    app.gallery.filtered_indices = vec![0];
    let buffer = render_buffer(&mut app, None, 100, 30);
    let screen = screen_rows(&buffer).join("\n");
    assert!(
        status_row(&screen).trim_end() == " /a\u{2588}  1/2 matches  1 selected  scanning\u{2026}",
        "{screen}"
    );
    assert_default_fg(cell_of(&buffer, "/a\u{2588}"), Modifier::empty());
    assert_ansi(cell_of(&buffer, "1/2 matches"), Color::DarkGray);
    assert_ansi(cell_of(&buffer, "1 selected"), Color::Yellow);
    assert_ansi(cell_of(&buffer, "scanning"), Color::DarkGray);
}

#[test]
fn file_facts_show_until_note() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = crate::test_support::app_with_files(dir.path(), &["a.png"]);
    app.request_meta();
    crate::test_support::wait_until(|| {
        app.poll_meta();
        !app.meta_pending()
    });
    let buffer = render_buffer(&mut app, None, 100, 30);
    let screen = screen_rows(&buffer).join("\n");
    let row = status_row(&screen);
    assert!(row.starts_with(" a.png [1/1] "), "{screen}");
    assert!(row.contains("8\u{d7}8 \u{b7} 0.0 MP \u{b7} "), "{row}");
    assert!(row.contains(" \u{b7} PNG \u{b7} "), "{row}");
    assert_eq!(
        row.trim_end().chars().count(),
        99,
        "the facts end at the right edge"
    );
    assert_ansi(cell_of(&buffer, "8\u{d7}8"), Color::DarkGray);

    let screen = render(&mut app, Some("copied"), 100, 30);
    assert_hides(&screen, "PNG");
    assert_shows(&screen, "copied");
}

#[test]
fn facts_count_display_columns() {
    // Two wide characters are 4 columns and 2 chars.
    assert!(super::facts_fit(6, "\u{5199}\u{771f}", 10));
    assert!(!super::facts_fit(7, "\u{5199}\u{771f}", 10));
    assert!(super::facts_fit(6, "abcd", 10));
}

#[test]
fn scan_error_comes_after_error() {
    let mut app = app_with(&["a.jpg"]);
    app.scan_error = Some("denied".to_owned());
    let screen = render(&mut app, None, 100, 30);
    assert_shows(&screen, " Error: denied ");

    app.error = Some("scan failed".to_owned());
    let screen = render(&mut app, None, 100, 30);
    assert_shows(&screen, " Error: scan failed ");
    assert_hides(&screen, "denied");
}

#[test]
fn note_replaces_hints() {
    let mut app = app_with(&["a.jpg"]);
    let screen = render(&mut app, Some("1 file marked"), 100, 30);
    assert_shows(&screen, " a.jpg [1/1]  1 file marked ");
    assert_hides(&screen, "Enter: open");
}

#[test]
fn fullscreen_shows_name_and_position() {
    let mut app = app_with(&["a.jpg", "b.jpg"]);
    app.mode = ViewMode::Fullscreen;
    app.current = 1;
    app.loaded = Some(RgbaImage::new(30, 20).into());
    let screen = render(&mut app, None, 100, 30);
    assert!(
        status_row(&screen).trim_end() == " b.jpg [2/2]",
        "the decoded size is not the file size, the facts come from the file: {screen}"
    );
    assert_eq!(
        app.image_rect,
        Rect::new(0, 0, 100, 29),
        "no frame, 1 status row"
    );
    assert_eq!(
        screen.matches(" b.jpg ").count(),
        1,
        "the name only in the status row"
    );
}

#[test]
fn fullscreen_shows_zoom() {
    let mut app = app_with(&["a.jpg"]);
    app.mode = ViewMode::Fullscreen;
    app.zoom = 2.0;
    let screen = render(&mut app, None, 100, 30);
    assert_eq!(status_row(&screen).trim_end(), " a.jpg [1/1] 200%");
}

#[test]
fn slideshow_has_no_status_row() {
    let mut app = app_with(&["a.jpg"]);
    app.mode = ViewMode::Fullscreen;
    app.start_slideshow();
    let screen = render(&mut app, None, 100, 30);
    assert_hides(&screen, "a.jpg");

    app.slideshow_toggle_pause();
    app.error = Some("decode failed".to_owned());
    let screen = render(&mut app, Some("a note"), 100, 30);
    assert_hides(&screen, "a.jpg");
    assert_shows(&screen, " Error: decode failed ");
    assert_hides(&screen, "a note");
}

#[test]
fn fullscreen_shows_errors() {
    let mut app = app_with(&["a.jpg", "b.jpg"]);
    app.mode = ViewMode::Fullscreen;
    app.error = Some("decode failed".to_owned());
    let screen = render(&mut app, None, 100, 30);
    assert_shows(&screen, " Error: decode failed ");
}

#[test]
fn fullscreen_uses_ansi_roles() {
    let mut app = app_with(&["a.jpg"]);
    app.mode = ViewMode::Fullscreen;
    let buffer = render_buffer(&mut app, None, 100, 30);
    assert_terminal_background(&buffer);
    assert_symbol(&buffer, (0, 0), " ", Color::Reset, Modifier::empty());
    assert_symbol(&buffer, (0, 29), " ", Color::Reset, Modifier::empty());
    assert_symbol(&buffer, (1, 29), "a", Color::Reset, Modifier::empty());

    app.error = Some("decode failed".to_owned());
    let buffer = render_buffer(&mut app, None, 100, 30);
    assert_ansi(cell_of(&buffer, "Error: decode failed"), Color::Red);
}

#[cfg(feature = "video")]
#[test]
fn video_row_shows_time_and_note() {
    let mut app = app_with(&["clip.mp4", "b.jpg"]);
    app.mode = ViewMode::Video;
    let screen = render(&mut app, None, 100, 30);
    assert!(
        status_row(&screen).starts_with(" \u{25b6} clip.mp4 00:00/00:00 [1/2] "),
        "{screen}"
    );
    assert_hides(&screen, "Space: pause");
    assert_eq!(
        app.image_rect,
        Rect::new(0, 0, 100, 29),
        "no frame, 1 status row"
    );

    let screen = render(&mut app, Some("mpv not found"), 100, 30);
    assert_shows(&screen, " mpv not found ");

    app.error = Some("no video stream".to_owned());
    let screen = render(&mut app, None, 100, 30);
    assert_shows(&screen, " Error: no video stream ");
}

#[cfg(feature = "video")]
#[test]
fn video_row_shows_player_error() {
    use crate::player::Player;
    use crate::test_support::video::{clip_info, fake_decoder, shared};

    let mut app = app_with(&["clip.mp4", "b.jpg"]);
    app.mode = ViewMode::Video;
    let mut info = clip_info();
    info.duration = 3725.0;
    let decoder = fake_decoder(info);
    let mut player = Player::open(
        shared(&decoder),
        std::path::Path::new("/bin/ffmpeg"),
        std::path::Path::new("/videos/clip.mp4"),
        (64, 48),
        &[],
        true,
    )
    .unwrap();
    decoder.feed(vec![0; 64 * 48 * 4]);
    assert!(player.poll());
    app.video = Some(player);
    let screen = render(&mut app, None, 100, 30);
    assert!(
        status_row(&screen).starts_with(" \u{25b6} clip.mp4 00:00/1:02:05 [1/2] "),
        "{screen}"
    );
    assert_eq!(crate::file_meta::format_time(65.9), "01:05");

    decoder.end();
    assert!(
        !app.video.as_mut().unwrap().poll(),
        "the end of the clip: a repeat"
    );
    decoder.end();
    assert!(
        !app.video.as_mut().unwrap().poll(),
        "no frame: the software retry"
    );
    decoder.end();
    assert!(
        !app.video.as_mut().unwrap().poll(),
        "no frame again: the error"
    );
    let screen = render(&mut app, None, 100, 30);
    assert_shows(&screen, " Error: ffmpeg could not decode clip.mp4 ");
}
