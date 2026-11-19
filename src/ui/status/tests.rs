//! The tests of the status row of each view.
use crate::app::ViewMode;
use crate::ui::tests::{
    app_with, assert_ansi, assert_default_fg, assert_hides, assert_shows, assert_symbol,
    assert_terminal_background, cell_of, omnibox_row, render, render_buffer, screen_rows,
    status_parts, status_row,
};
use image::RgbaImage;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

#[test]
fn gallery_shows_position_without_hints() {
    let mut app = app_with(&["a.jpg", "b.jpg"]);
    let screen = render(&mut app, None, 100, 30);
    assert_eq!(status_parts(&screen), ["NORMAL", "a.jpg [1/2]"], "{screen}");
    assert_hides(&screen, "?: help");
    assert_hides(&screen, "b.jpg");
    assert_hides(&screen, "Gallery");
    assert_hides(&screen, "Search");
    assert_eq!(
        app.gallery.grid_rect,
        Rect::new(1, 1, 98, 26),
        "inside the main box"
    );
}

#[test]
fn empty_folder_shows_note() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = app_with(&[]);
    app.scan_complete = true;
    let screen = render(&mut app, None, 100, 30);
    assert_eq!(
        status_parts(&screen),
        ["NORMAL", "No images or folders here"],
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
    app.gallery.search_query = tuilibr::text_field::TextField::from("b");
    app.gallery.filtered_indices = vec![1];
    let screen = render(&mut app, None, 100, 30);
    assert_eq!(
        status_parts(&screen),
        ["SEARCH", "b.jpg [1/1]"],
        "the search line sits in the omnibox: {screen}"
    );
    let omnibox = omnibox_row(&screen);
    assert!(omnibox.starts_with("/b "), "{omnibox:?}");
    assert!(omnibox.trim_end().ends_with("1/2 matches"), "{omnibox:?}");

    // After Enter, the filter stays and the position counts the matches.
    app.gallery.search_active = false;
    let screen = render(&mut app, None, 100, 30);
    assert_eq!(status_parts(&screen), ["NORMAL", "b.jpg [1/1]"], "{screen}");
}

#[test]
fn gallery_shows_selection_count() {
    let mut app = app_with(&["a.jpg", "b.jpg"]);
    app.selection.insert(app.images[1].clone());
    let screen = render(&mut app, None, 100, 30);
    assert_eq!(
        status_parts(&screen),
        ["NORMAL", "a.jpg [1/2]", "1 selected"],
        "{screen}"
    );
}

#[test]
fn gallery_row_holds_search_and_scan() {
    let mut app = app_with(&["a.jpg", "b.jpg"]);
    app.scan_complete = false;
    let screen = render(&mut app, None, 100, 30);
    assert_eq!(
        status_parts(&screen),
        ["NORMAL", "a.jpg [1/2]", "scanning\u{2026}"],
        "{screen}"
    );

    app.selection.insert(app.images[0].clone());
    app.gallery.search_active = true;
    app.gallery.search_query = tuilibr::text_field::TextField::from("a");
    app.gallery.filtered_indices = vec![0];
    let buffer = render_buffer(&mut app, None, 100, 30);
    let screen = screen_rows(&buffer).join("\n");
    assert_eq!(
        status_parts(&screen),
        ["SEARCH", "a.jpg [1/1]", "1 selected", "scanning\u{2026}"],
        "{screen}"
    );
    assert_eq!(buffer[(1, 29)].symbol(), "a", "{screen}");
    assert_default_fg(&buffer[(1, 29)], Modifier::empty());
    assert_ansi(cell_of(&buffer, "1/2 matches"), Color::DarkGray);
    assert_ansi(cell_of(&buffer, "1 selected"), Color::Yellow);
    assert_ansi(cell_of(&buffer, "scanning"), Color::DarkGray);
}

#[test]
fn file_facts_stay_during_note() {
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
    assert_eq!(
        status_parts(&screen)[..2],
        ["NORMAL", "a.png [1/1]"],
        "{screen}"
    );
    assert!(row.contains("8\u{d7}8 \u{b7} 0.0 MP \u{b7} "), "{row}");
    assert!(row.contains(" \u{b7} PNG \u{b7} "), "{row}");
    let facts = status_parts(&screen).last().copied().unwrap_or_default();
    assert!(
        facts.starts_with("8\u{d7}8") && facts.contains("PNG"),
        "the facts are the last group at the right: {row}"
    );
    assert_ansi(cell_of(&buffer, "8\u{d7}8"), Color::White);

    let screen = render(&mut app, Some("copied"), 100, 30);
    assert!(
        status_row(&screen).contains(" \u{b7} PNG \u{b7} "),
        "{screen}"
    );
    assert_eq!(omnibox_row(&screen).trim(), "copied");

    app.error = Some("broken".to_owned());
    let screen = render(&mut app, None, 100, 30);
    assert!(
        status_row(&screen).contains(" \u{b7} PNG \u{b7} "),
        "{screen}"
    );
    assert_eq!(omnibox_row(&screen).trim(), "Error: broken");
}

#[test]
fn scan_error_comes_after_error() {
    let mut app = app_with(&["a.jpg"]);
    app.scan_error = Some("denied".to_owned());
    let screen = render(&mut app, None, 100, 30);
    assert_eq!(omnibox_row(&screen).trim(), "Error: denied");

    app.error = Some("scan failed".to_owned());
    let screen = render(&mut app, None, 100, 30);
    assert_eq!(omnibox_row(&screen).trim(), "Error: scan failed");
    assert_hides(&screen, "denied");
}

/// An error in the grid sits in the omnibox, and the bar keeps the entry counts.
#[test]
fn grid_error_keeps_the_bar() {
    let mut app = app_with(&["sub", "a.jpg"]);
    app.folder_count = 1;
    let screen = render(&mut app, None, 100, 30);
    assert_eq!(
        status_parts(&screen),
        ["NORMAL", "sub [1/2]", "1 folder, 1 image"],
        "{screen}"
    );
    app.scan_error = Some("denied".to_owned());
    let screen = render(&mut app, None, 100, 30);
    assert_eq!(
        status_parts(&screen),
        ["NORMAL", "sub [1/2]", "1 folder, 1 image"],
        "{screen}"
    );
    assert_eq!(omnibox_row(&screen).trim(), "Error: denied");
}

#[test]
fn note_sits_in_the_omnibox() {
    let mut app = app_with(&["a.jpg"]);
    let screen = render(&mut app, Some("1 file marked"), 100, 30);
    assert_eq!(status_parts(&screen), ["NORMAL", "a.jpg [1/1]"], "{screen}");
    assert_eq!(omnibox_row(&screen).trim(), "1 file marked");
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
        status_parts(&screen) == ["FULLSCREEN", "b.jpg [2/2]"],
        "the decoded size is not the file size, the facts come from the file: {screen}"
    );
    assert_eq!(
        app.image_rect,
        Rect::new(1, 1, 98, 26),
        "inside the main box, above the status row and the omnibox"
    );
    assert_eq!(
        screen.matches(" b.jpg ").count(),
        2,
        "the name in the box title and in the status row"
    );
    assert_shows(&screen, "\u{256d}\u{2500} b.jpg \u{2500}");
}

#[test]
fn fullscreen_shows_zoom() {
    let mut app = app_with(&["a.jpg"]);
    app.mode = ViewMode::Fullscreen;
    app.zoom = 2.0;
    let screen = render(&mut app, None, 100, 30);
    assert_eq!(status_parts(&screen), ["FULLSCREEN", "a.jpg [1/1]", "200%"]);
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
    assert_eq!(omnibox_row(&screen).trim(), "Error: decode failed");
}

#[test]
fn fullscreen_uses_ansi_roles() {
    let mut app = app_with(&["a.jpg"]);
    app.mode = ViewMode::Fullscreen;
    let buffer = render_buffer(&mut app, None, 100, 30);
    assert_terminal_background(&buffer);
    assert_symbol(&buffer, (1, 1), " ", Color::Reset, Modifier::empty());
    assert_symbol(
        &buffer,
        (1, 28),
        "F",
        Color::LightGreen,
        Modifier::REVERSED | Modifier::BOLD,
    );

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
    assert_eq!(
        status_parts(&screen)[..2],
        ["FULLSCREEN", "\u{25b6} clip.mp4 00:00/00:00 [1/2]"],
        "{screen}"
    );
    assert_hides(&screen, "Space: pause");
    assert_eq!(
        app.image_rect,
        Rect::new(1, 1, 98, 26),
        "inside the main box, above the status row and the omnibox"
    );

    let screen = render(&mut app, Some("mpv not found"), 100, 30);
    assert_eq!(omnibox_row(&screen).trim(), "mpv not found");

    app.error = Some("no video stream".to_owned());
    let screen = render(&mut app, None, 100, 30);
    assert_eq!(omnibox_row(&screen).trim(), "Error: no video stream");
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
        Path::new("/bin/ffmpeg"),
        Path::new("/videos/clip.mp4"),
        (64, 48),
        &[],
        true,
    )
    .unwrap();
    decoder.feed(vec![0; 64 * 48 * 4]);
    assert!(player.poll());
    app.video = Some(player);
    let screen = render(&mut app, None, 100, 30);
    assert_eq!(
        status_parts(&screen)[..2],
        ["FULLSCREEN", "\u{25b6} clip.mp4 00:00/1:02:05 [1/2]"],
        "{screen}"
    );
    let position = crate::file_meta::whole_seconds(65.9);
    assert_eq!(
        tuilibr::time::media_length(position, tuilibr::time::LengthStyle::Padded),
        "01:05"
    );

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
    assert_eq!(
        omnibox_row(&screen).trim(),
        "Error: ffmpeg could not decode clip.mp4"
    );
}

use super::{display_path, shorten_path};
use std::path::{Path, PathBuf};

fn app_in(dir: &Path, names: &[&str]) -> crate::app::App {
    let mut app = app_with(names);
    app.current_dir = Some(dir.to_path_buf());
    app
}

#[test]
fn display_path_shortens_home() {
    let home = Path::new("/Users/me");
    assert_eq!(
        display_path(Path::new("/Users/me/Pictures/trip"), Some(home)),
        "~/Pictures/trip"
    );
    assert_eq!(display_path(Path::new("/Users/me"), Some(home)), "~");
}

#[test]
fn display_path_keeps_path_outside_home() {
    let home = Path::new("/Users/me");
    assert_eq!(
        display_path(Path::new("/Volumes/disk/photos"), Some(home)),
        "/Volumes/disk/photos"
    );
    assert_eq!(
        display_path(Path::new("/Users/meme/photos"), Some(home)),
        "/Users/meme/photos",
        "a folder name that starts like home is not home"
    );
    assert_eq!(display_path(Path::new("/tmp/a"), None), "/tmp/a");
}

#[test]
fn shorten_path_keeps_last_folder() {
    let path = "~/Pictures/phewr-test/12-live-photos";
    assert_eq!(shorten_path(path, 40).as_deref(), Some(path));
    assert_eq!(
        shorten_path(path, 30).as_deref(),
        Some("\u{2026}/phewr-test/12-live-photos")
    );
    assert_eq!(
        shorten_path(path, 20).as_deref(),
        Some("\u{2026}/12-live-photos")
    );
    assert_eq!(shorten_path(path, 5), None, "the last folder does not fit");
    assert_eq!(shorten_path("~", 0), None);
}

#[test]
fn gallery_row_starts_with_folder_path() {
    let mut app = app_in(Path::new("/Volumes/disk/trip"), &["a.jpg", "b.jpg"]);
    let screen = render(&mut app, None, 100, 30);
    assert_eq!(
        status_parts(&screen),
        ["NORMAL", "/Volumes/disk/trip", "a.jpg [1/2]"],
        "{screen}"
    );
}

#[test]
fn gallery_row_shows_home_as_tilde() {
    let mut app = app_in(Path::new("/Users/me/Pictures/trip"), &["a.jpg"]);
    app.home_dir = Some(PathBuf::from("/Users/me"));
    let screen = render(&mut app, None, 100, 30);
    assert_eq!(
        status_parts(&screen),
        ["NORMAL", "~/Pictures/trip", "a.jpg [1/1]"],
        "{screen}"
    );
}

#[test]
fn folder_switch_from_a_file_list_shows_the_path() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = app_in(Path::new("/Volumes/disk/trip"), &["a.jpg"]);
    app.file_list = true;
    let screen = render(&mut app, None, 100, 30);
    assert_hides(&screen, "/Volumes/disk/trip");

    app.switch_to_dir(temp.path());
    assert!(!app.file_list);
    let screen = render(&mut app, None, 100, 30);
    let canonical = crate::scanner::canonicalize_or_self(temp.path());
    assert_shows(&screen, &canonical.display().to_string());
}

#[test]
fn file_list_row_shows_no_path() {
    let mut app = app_in(Path::new("/Volumes/disk/trip"), &["a.jpg", "b.jpg"]);
    app.file_list = true;
    let screen = render(&mut app, None, 100, 30);
    assert_eq!(status_parts(&screen), ["NORMAL", "a.jpg [1/2]"], "{screen}");
}

#[test]
fn narrow_row_shortens_path_from_its_start() {
    let mut app = app_in(Path::new("/Volumes/disk/phewr-test/trip"), &["a.jpg"]);
    // Below 60 columns the mode block is ` N ` (3 columns), the name part is 13 columns.
    let screen = render(&mut app, None, 38, 30);
    assert_eq!(
        status_parts(&screen),
        ["N", "\u{2026}/phewr-test/trip", "a.jpg [1/1]"],
        "{screen}"
    );
    let screen = render(&mut app, None, 32, 30);
    assert_eq!(status_parts(&screen), ["N", "\u{2026}/trip", "a.jpg [1/1]"]);
    let screen = render(&mut app, None, 20, 30);
    assert_eq!(
        status_parts(&screen),
        ["N", "a.jpg [1/1]"],
        "no room for the path"
    );
}

#[test]
fn path_is_dim_and_name_keeps_its_style() {
    let mut app = app_in(Path::new("/Volumes/disk"), &["a.jpg"]);
    let buffer = render_buffer(&mut app, None, 60, 30);
    let path = cell_of(&buffer, "/Volumes");
    assert_eq!(path.modifier, crate::ui::STYLES.dim.add_modifier);
    assert_default_fg(cell_of(&buffer, "a.jpg"), Modifier::empty());
}

#[test]
fn fullscreen_row_starts_with_folder_path() {
    let mut app = app_in(Path::new("/Volumes/disk/trip"), &["a.jpg", "b.jpg"]);
    app.mode = ViewMode::Fullscreen;
    app.zoom = 2.0;
    let screen = render(&mut app, Some("a note"), 100, 30);
    assert_eq!(
        status_parts(&screen),
        ["FULLSCREEN", "/Volumes/disk/trip", "a.jpg [1/2]", "200%"],
        "{screen}"
    );
}

#[cfg(feature = "video")]
#[test]
fn video_row_starts_with_folder_path() {
    let mut app = app_in(Path::new("/Volumes/disk/trip"), &["clip.mp4", "b.jpg"]);
    app.mode = ViewMode::Video;
    let screen = render(&mut app, None, 100, 30);
    assert_eq!(
        status_parts(&screen)[..3],
        [
            "FULLSCREEN",
            "/Volumes/disk/trip",
            "\u{25b6} clip.mp4 00:00/00:00 [1/2]"
        ],
        "{screen}"
    );
}

fn assert_mode(buffer: &ratatui::buffer::Buffer, letter: &str, color: Color) {
    assert_symbol(
        buffer,
        (1, 28),
        letter,
        color,
        Modifier::REVERSED | Modifier::BOLD,
    );
}

#[test]
fn mode_names_each_view() {
    let mut app = app_with(&["a.jpg"]);
    let screen = render(&mut app, None, 100, 30);
    assert!(status_row(&screen).starts_with(" NORMAL  "), "{screen}");
    let buffer = render_buffer(&mut app, None, 100, 30);
    assert_mode(&buffer, "N", Color::Red);

    app.gallery.search_active = true;
    let screen = render(&mut app, None, 100, 30);
    assert!(status_row(&screen).starts_with(" SEARCH  "), "{screen}");
    let buffer = render_buffer(&mut app, None, 100, 30);
    assert_mode(&buffer, "S", Color::Blue);
    app.gallery.search_active = false;

    app.mode = ViewMode::Fullscreen;
    let screen = render(&mut app, None, 100, 30);
    assert!(status_row(&screen).starts_with(" FULLSCREEN  "), "{screen}");
    let buffer = render_buffer(&mut app, None, 100, 30);
    assert_mode(&buffer, "F", Color::LightGreen);

    app.help_visible = true;
    let buffer = render_buffer(&mut app, None, 100, 30);
    assert_mode(&buffer, "H", Color::LightCyan);
}

#[test]
fn mode_is_one_letter_on_a_narrow_screen() {
    let mut app = app_with(&["a.jpg"]);
    let screen = render(&mut app, None, 59, 30);
    assert_eq!(status_parts(&screen), ["N", "a.jpg [1/1]"], "{screen}");
    app.gallery.search_active = true;
    let screen = render(&mut app, None, 59, 30);
    assert_eq!(status_parts(&screen), ["/", "a.jpg [1/1]"], "{screen}");
    app.gallery.search_active = false;
    app.mode = ViewMode::Fullscreen;
    let screen = render(&mut app, None, 59, 30);
    assert_eq!(status_parts(&screen), ["F", "a.jpg [1/1]"], "{screen}");
    app.help_visible = true;
    let screen = render(&mut app, None, 59, 30);
    assert_eq!(status_parts(&screen), ["?", "a.jpg [1/1]"], "{screen}");
    let screen = render(&mut app, None, 60, 30);
    assert!(status_row(&screen).starts_with(" HELP  "), "{screen}");
}

#[test]
fn slideshow_shows_a_box_at_the_bottom_left() {
    let mut app = app_with(&["a.jpg"]);
    app.mode = ViewMode::Fullscreen;
    app.start_slideshow();
    let buffer = render_buffer(&mut app, None, 100, 30);
    let screen = screen_rows(&buffer).join("\n");
    // A slideshow has no bar and no omnibox: the box sits in the last row.
    let last_row = |screen: &str| screen.lines().last().unwrap_or_default().to_owned();
    assert!(last_row(&screen).starts_with(" SLIDESHOW "), "{screen}");
    assert_symbol(&buffer, (1, 29), "S", Color::Black, Modifier::BOLD);
    assert_eq!(buffer[(1, 29)].bg, Color::Gray, "the look of the badge");
    assert_eq!(
        app.image_rect,
        Rect::new(0, 0, 100, 30),
        "the box sits on the picture"
    );
    let screen = render(&mut app, None, 50, 30);
    assert!(last_row(&screen).starts_with(" S "), "{screen}");
}
