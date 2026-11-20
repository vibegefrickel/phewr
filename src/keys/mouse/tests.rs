//! The tests of the mouse in each view.
use crate::app::ViewMode;
use crate::app::mouse::DOUBLE_CLICK;
use crate::config::Config;
use crate::run_loop::LoopState;
use crate::test_support::{
    char_key, click, click_at, click_cell, draw_frame, fullscreen_app, gallery_app,
    gallery_app_with, mouse, mouse_at, mouse_cell, text, wait_until,
};
use crate::ui;
use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};
use std::fs;
use std::time::{Duration, Instant};

const LEFT: MouseEventKind = MouseEventKind::Down(MouseButton::Left);
const RIGHT: MouseEventKind = MouseEventKind::Down(MouseButton::Right);

#[test]
fn click_selects_and_double_click_opens() {
    let mut app = gallery_app(9);
    let start = Instant::now();

    assert!(!click_cell(&mut app, 4, start));
    assert_eq!(app.gallery.cursor, 4);
    assert_eq!(app.mode, ViewMode::Gallery);

    let late = start + DOUBLE_CLICK + Duration::from_millis(1);
    click_cell(&mut app, 4, late);
    assert_eq!(
        app.mode,
        ViewMode::Gallery,
        "a slow second click only selects"
    );

    click_cell(&mut app, 4, late + Duration::from_millis(100));
    assert_eq!(app.mode, ViewMode::Fullscreen);
    assert_eq!(app.current, 4);
    assert!(text(&app.graphics_out.take_bytes()).contains("a=d"));
}

#[test]
fn double_click_elsewhere_only_selects() {
    let mut app = gallery_app(9);
    let start = Instant::now();
    click_cell(&mut app, 1, start);
    click_cell(&mut app, 2, start + Duration::from_millis(50));
    assert_eq!(app.gallery.cursor, 2);
    assert_eq!(app.mode, ViewMode::Gallery);

    click(&mut app, (0, 0));
    assert_eq!(app.gallery.cursor, 2, "the gap at the edge");

    click_cell(&mut app, 4, start + Duration::from_millis(100));
    click_at(&mut app, (0, 0), start + Duration::from_millis(150));
    click_cell(&mut app, 4, start + Duration::from_millis(200));
    assert_eq!(
        app.mode,
        ViewMode::Gallery,
        "a gap between two clicks breaks the pair"
    );
    assert!(!mouse(&mut app, MouseEventKind::Moved, (5, 5)));
    assert!(!app.needs_render);
}

#[test]
fn click_ends_search_typing() {
    let mut app = gallery_app(9);
    char_key(&mut app, '/');
    char_key(&mut app, '0');
    wait_until(|| {
        app.poll_filter();
        !app.has_pending_work()
    });
    assert!(app.gallery.search_active);

    click_cell(&mut app, 0, Instant::now());
    assert!(!app.gallery.search_active);
    assert_eq!(app.gallery.search_query.text(), "0");
}

#[test]
fn double_click_walks_folders() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(root.join("sub").join("photo.png"), b"synthetic").unwrap();
    fs::write(root.join("sub").join("other.png"), b"synthetic").unwrap();
    let mut app = gallery_app(0);
    app.switch_to_dir(&root);
    wait_until(|| {
        app.refresh_from_scanner();
        !app.is_scanning()
    });
    assert_eq!(app.filenames, ["..", "sub"]);
    let start = Instant::now();

    click_cell(&mut app, 1, start);
    assert_eq!(app.mode, ViewMode::Gallery);
    click_cell(&mut app, 1, start + Duration::from_millis(10));
    assert_eq!(app.current_dir.as_deref(), Some(root.join("sub").as_path()));
    wait_until(|| {
        app.refresh_from_scanner();
        !app.is_scanning()
    });
    assert_eq!(app.mode, ViewMode::Gallery);
    assert_eq!(app.filenames, ["..", "other.png", "photo.png"]);

    let again = start + Duration::from_secs(1);
    click_cell(&mut app, 0, again);
    assert_eq!(app.gallery.cursor, 0);
    click_cell(&mut app, 0, again + Duration::from_millis(10));
    assert_eq!(app.current_dir.as_deref(), Some(root.as_path()));
    wait_until(|| {
        app.refresh_from_scanner();
        !app.is_scanning()
    });
    assert_eq!(app.current_dir.as_deref(), Some(root.as_path()));
    assert_eq!(app.gallery.cursor, 1, "the cursor is on sub");
}

#[test]
fn wheel_scrolls_and_shift_zooms() {
    let mut app = gallery_app(9);
    assert_eq!(app.gallery.visible_rows, 2);

    mouse(&mut app, MouseEventKind::ScrollDown, (10, 5));
    assert_eq!(app.gallery.scroll_offset, 1);
    assert_eq!(app.gallery.cursor, 3, "the cursor follows into the screen");
    assert!(app.needs_render);
    app.needs_render = false;
    mouse(&mut app, MouseEventKind::ScrollDown, (10, 5));
    assert_eq!(
        app.gallery.scroll_offset, 1,
        "the last row stays at the bottom"
    );
    assert!(!app.needs_render);
    mouse(&mut app, MouseEventKind::ScrollUp, (10, 5));
    assert_eq!(app.gallery.scroll_offset, 0);
    assert_eq!(app.gallery.cursor, 3, "still on the screen, so it stays");

    let rows = app.gallery.zoom_rows;
    let now = Instant::now();
    mouse_at(
        &mut app,
        MouseEventKind::ScrollDown,
        (10, 5),
        KeyModifiers::SHIFT,
        now,
    );
    assert_eq!(
        app.gallery.zoom_rows,
        rows + 1,
        "Shift+wheel down: smaller cells"
    );
    mouse_at(
        &mut app,
        MouseEventKind::ScrollUp,
        (10, 5),
        KeyModifiers::SHIFT,
        now,
    );
    assert_eq!(app.gallery.zoom_rows, rows);
    assert_eq!(app.gallery.scroll_offset, 0, "a zoom does not scroll");
}

#[test]
fn horizontal_scroll_in_the_grid_does_nothing() {
    let mut app = gallery_app(9);
    draw_frame(&mut app, 120, 24);
    let rows = app.gallery.zoom_rows;
    let offset = app.gallery.scroll_offset;
    for kind in [MouseEventKind::ScrollLeft, MouseEventKind::ScrollRight] {
        mouse(&mut app, kind, (10, 5));
        assert_eq!(app.gallery.zoom_rows, rows, "{kind:?} must not zoom");
        assert_eq!(
            app.gallery.scroll_offset, offset,
            "{kind:?} must not scroll"
        );
    }
}

#[test]
fn right_click_shows_info_panel() {
    let mut app = gallery_app(9);
    mouse(&mut app, RIGHT, (0, 0));
    assert_eq!(app.gallery.cursor, 0, "a right click on a gap does nothing");
    assert!(!app.info_visible);
    assert!(app.graphics_out.take_bytes().is_empty());

    mouse_cell(&mut app, RIGHT, 5);
    assert_eq!(app.gallery.cursor, 5);
    assert!(app.info_visible);
    assert!(text(&app.graphics_out.take_bytes()).contains("a=d"));
}

#[test]
fn right_click_on_the_cursor_cell_closes_info_panel() {
    let mut app = gallery_app(9);
    mouse_cell(&mut app, RIGHT, 5);
    assert!(app.info_visible);
    draw_frame(&mut app, 120, 24);
    app.graphics_out.take_bytes();

    mouse_cell(&mut app, RIGHT, 5);
    assert_eq!(app.gallery.cursor, 5);
    assert!(!app.info_visible);
    assert!(text(&app.graphics_out.take_bytes()).contains("a=d"));
}

#[test]
fn right_click_on_another_cell_closes_info_panel_without_moving_cursor() {
    let mut app = gallery_app(9);
    mouse_cell(&mut app, RIGHT, 5);
    draw_frame(&mut app, 120, 24);
    app.graphics_out.take_bytes();

    mouse_cell(&mut app, RIGHT, 2);
    assert_eq!(app.gallery.cursor, 5);
    assert!(!app.info_visible);
    assert!(text(&app.graphics_out.take_bytes()).contains("a=d"));
}

#[test]
fn right_click_on_a_gap_closes_info_panel_without_moving_cursor() {
    let mut app = gallery_app(9);
    mouse_cell(&mut app, RIGHT, 5);
    draw_frame(&mut app, 120, 24);
    app.graphics_out.take_bytes();

    mouse(&mut app, RIGHT, (0, 0));
    assert_eq!(app.gallery.cursor, 5);
    assert!(!app.info_visible);
    assert!(text(&app.graphics_out.take_bytes()).contains("a=d"));
}

#[test]
fn status_row_click_toggles_help() {
    let mut app = gallery_app(9);
    draw_frame(&mut app, 120, 24);
    let status = ui::panes(app.layout.screen, &app).status;

    click(&mut app, (status.x + 3, status.y));
    assert!(app.help_visible);
    assert!(text(&app.graphics_out.take_bytes()).contains("a=d"));

    mouse(&mut app, MouseEventKind::ScrollDown, (10, 5));
    assert!(app.help_visible, "the wheel keeps the help");
    mouse(&mut app, RIGHT, (10, 5));
    assert!(!app.help_visible);
    assert!(app.needs_render);
}

/// A click on the omnibox does nothing in each view: no help, no step, no way back.
#[test]
fn omnibox_click_does_nothing() {
    let mut app = gallery_app(9);
    draw_frame(&mut app, 120, 24);
    let omnibox = ui::panes(app.layout.screen, &app).omnibox;
    assert_eq!(omnibox, ratatui::layout::Rect::new(0, 23, 120, 1));
    char_key(&mut app, '/');
    click(&mut app, (3, omnibox.y));
    assert!(!app.help_visible);
    assert!(app.gallery.search_active, "the search line keeps the keys");

    let mut app = fullscreen_app(3, 1);
    draw_frame(&mut app, 120, 24);
    click(&mut app, (3, omnibox.y));
    assert!(!app.help_visible);
    assert_eq!((app.mode, app.current), (ViewMode::Fullscreen, 1));
}

/// A click on the omnibox of the player is no click beside the frame.
#[cfg(feature = "video")]
#[test]
fn omnibox_click_keeps_the_player() {
    let mut app = crate::test_support::video::video_app();
    draw_frame(&mut app, 120, 24);
    let omnibox = ui::panes(app.layout.screen, &app).omnibox;
    click(&mut app, (3, omnibox.y));
    assert_eq!(app.mode, ViewMode::Video);
    assert!(!app.help_visible);
}

/// A click in the info pane does nothing, and a click in the main pane keeps its meaning in the
/// new rect of the pane.
#[test]
fn info_pane_click_does_nothing() {
    let mut app = gallery_app(9);
    app.info_visible = true;
    draw_frame(&mut app, 120, 24);
    let panes = ui::panes(app.layout.screen, &app);
    let info = panes.info.unwrap();
    assert!(
        info.right() < panes.main.x,
        "the info pane sits left of the grid"
    );
    app.graphics_out.take_bytes();

    click(&mut app, (info.x + 2, info.y + 2));
    assert!(app.info_visible);
    assert_eq!(app.gallery.cursor, 0);
    assert!(app.graphics_out.take_bytes().is_empty());

    let cell = crate::test_support::cell_center(&app, 1);
    assert!(panes.main.contains(cell.into()));
    click(&mut app, cell);
    assert_eq!(
        app.gallery.cursor, 1,
        "the grid moved right of the info pane"
    );
}

/// The narrow screen puts the info pane above the grid, and a click there does nothing.
#[test]
fn narrow_info_pane_sits_above_the_grid() {
    let mut app = gallery_app(9);
    app.info_visible = true;
    draw_frame(&mut app, 50, 30);
    let panes = ui::panes(app.layout.screen, &app);
    let info = panes.info.unwrap();
    assert!(info.bottom() < panes.main.y, "{panes:?}");
    click(&mut app, (info.x + 2, info.y + 1));
    assert!(app.info_visible);
    assert_eq!(app.gallery.cursor, 0);
}

#[test]
fn fullscreen_wheel_zooms_and_drag_pans() {
    let mut app = fullscreen_app(3, 1);
    draw_frame(&mut app, 120, 24);
    app.loaded = Some(image::RgbaImage::new(4000, 1000).into());
    app.needs_render = false;

    mouse(&mut app, MouseEventKind::ScrollUp, (60, 10));
    assert!(app.zoom > 1.0);
    app.needs_render = false;

    mouse(&mut app, LEFT, (60, 10));
    assert!(app.mouse.drag.is_some());
    mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), (50, 12));
    assert!(app.needs_render, "the drag pans");
    assert!(
        app.pan_position().0 > 0.5,
        "a drag to the left shows more of the right side"
    );
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), (115, 12));
    assert_eq!(app.current, 1, "a release after a drag is no edge click");
    assert!(app.mouse.drag.is_none());

    mouse(&mut app, MouseEventKind::ScrollDown, (60, 10));
    assert!((app.zoom - 1.0).abs() < f64::EPSILON);
}

#[test]
fn edge_click_steps_unzoomed() {
    let mut app = fullscreen_app(4, 1);
    draw_frame(&mut app, 120, 24);

    click(&mut app, (5, 10));
    assert_eq!(app.current, 0);
    click(&mut app, (115, 10));
    assert_eq!(app.current, 1);
    click(&mut app, (60, 10));
    assert_eq!(app.current, 1);

    mouse(&mut app, MouseEventKind::ScrollUp, (60, 10));
    click(&mut app, (115, 10));
    assert_eq!(app.current, 1, "a zoomed view pans, it does not step");
}

#[test]
fn fullscreen_right_click_toggles_info() {
    let mut app = fullscreen_app(3, 1);
    mouse(&mut app, RIGHT, (10, 10));
    assert!(app.info_visible);
    assert!(text(&app.graphics_out.take_bytes()).contains("a=d"));
    mouse(&mut app, RIGHT, (10, 10));
    assert!(!app.info_visible);
}

#[cfg(feature = "video")]
#[test]
fn player_leaves_on_right_click_only() {
    use crate::player::Player;
    use crate::test_support::video::{clip_info, fake_decoder, shared, video_app};
    use std::path::Path;

    let mut app = video_app();
    draw_frame(&mut app, 120, 24);
    let content = ui::panes(app.layout.screen, &app).main;
    let mut info = clip_info();
    info.duration = 60.0;
    let decoder = fake_decoder(info);
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

    click(&mut app, (content.width / 2, content.height / 2));
    assert!(app.video.as_ref().unwrap().playing(), "no pause");
    mouse(&mut app, MouseEventKind::ScrollDown, (10, 10));
    mouse(&mut app, MouseEventKind::ScrollUp, (10, 10));
    assert_eq!(decoder.starts().len(), 1, "no seek");
    assert_eq!(app.mode, ViewMode::Video);
    assert!(app.video.is_some());

    mouse(&mut app, RIGHT, (10, 10));
    assert_eq!(app.mode, ViewMode::Gallery);
    assert!(app.video.is_none());
    assert!(text(&app.graphics_out.take_bytes()).contains("a=d"));
}

#[cfg(feature = "video")]
#[test]
fn player_status_row_click_opens_help() {
    use crate::test_support::video::video_app;

    let mut app = video_app();
    draw_frame(&mut app, 120, 24);
    let status = ui::panes(app.layout.screen, &app).status;
    click(&mut app, (3, status.y));
    assert_eq!(app.mode, ViewMode::Video);
    assert!(app.help_visible);
}

#[cfg(feature = "video")]
#[test]
fn click_beside_frame_goes_back() {
    use crate::player::Player;
    use crate::test_support::video::{clip_info, fake_decoder, shared, video_app};
    use std::path::Path;

    let mut app = video_app();
    draw_frame(&mut app, 120, 24);
    let decoder = fake_decoder(clip_info());
    let mut player = Player::open(
        shared(&decoder),
        Path::new("/bin/ffmpeg"),
        Path::new("/videos/clip.mp4"),
        (64, 48),
        &[],
        true,
    )
    .unwrap();
    // A portrait frame leaves wide bars at both sides.
    player.current_frame = crate::player::Frame::new((9, 16), false, vec![0; 9 * 16 * 3]);
    app.video = Some(player);
    let rect = app.image_rect;
    let middle = (rect.x + rect.width / 2, rect.y + rect.height / 2);

    click(&mut app, middle);
    assert!(
        app.video.as_ref().unwrap().playing(),
        "a click on the picture does nothing"
    );
    assert_eq!(app.mode, ViewMode::Video);
    click(&mut app, (rect.x + 1, middle.1));
    assert_eq!(app.mode, ViewMode::Gallery);
    assert!(app.video.is_none());
}

#[test]
fn mouse_off_never_captures() {
    let config = Config {
        mouse: false,
        ..Config::default()
    };
    let state = LoopState::new(&config);
    assert!(!state.mouse);
    let mut app = gallery_app_with(9, &config);
    click_cell(&mut app, 4, Instant::now());
    assert_eq!(app.gallery.cursor, 4, "the handler itself still works");
}
