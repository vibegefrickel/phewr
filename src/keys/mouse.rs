//! The mouse: the clicks, the wheel and the drag, routed like the keys. An overlay wins over the
//! view mode. A click on the status row opens the help, a click on the info panel closes it.

use crate::app::{App, Drag, ViewMode};
use crate::ui;
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use std::io;
use std::time::Instant;

/// The part of the fullscreen width at each side where a click steps to the neighbor.
const EDGE_SHARE: u16 = 6;

/// Route a mouse event: the overlays (help, delete question) win over the view mode. `now` is
/// the clock of the double-click.
pub(crate) fn handle_mouse(app: &mut App, mouse: MouseEvent, now: Instant) -> io::Result<bool> {
    let at = Position::new(mouse.column, mouse.row);
    if app.help_visible {
        if is_press(mouse.kind) {
            app.help_visible = false;
            app.needs_render = true;
        }
        return Ok(false);
    }
    if app.pending_delete.is_some() {
        // A click cancels the question. The mouse never deletes.
        if is_press(mouse.kind) {
            app.cancel_delete();
        }
        return Ok(false);
    }
    match app.mode {
        ViewMode::Gallery => gallery_mouse(app, mouse, at, now),
        ViewMode::Fullscreen => fullscreen_mouse(app, mouse, at),
        #[cfg(feature = "video")]
        ViewMode::Video => video_mouse(app, mouse, at),
    }
}

fn is_press(kind: MouseEventKind) -> bool {
    matches!(kind, MouseEventKind::Down(_))
}

/// The areas of the view in the last draw: the content, the status row and the info panel.
struct Regions {
    content: Rect,
    status: Rect,
    info: Option<Rect>,
}

fn regions(app: &App) -> Regions {
    let (main, info) = ui::split_info(app.layout.screen, app);
    Regions {
        content: ui::content_area(main),
        status: ui::status_area(main),
        info,
    }
}

/// A left click on the status row or the info panel, which every view shares. True when the
/// click was one of those.
fn shared_click(app: &mut App, regions: &Regions, at: Position) -> io::Result<bool> {
    if regions.status.contains(at) {
        app.graphics_delete_all()?;
        app.help_visible = true;
        app.needs_render = true;
        return Ok(true);
    }
    if regions.info.is_some_and(|info| info.contains(at)) {
        app.graphics_delete_all()?;
        app.toggle_info();
        return Ok(true);
    }
    Ok(false)
}

fn gallery_mouse(app: &mut App, mouse: MouseEvent, at: Position, now: Instant) -> io::Result<bool> {
    let regions = regions(app);
    let shift = mouse.modifiers.contains(KeyModifiers::SHIFT);
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if shared_click(app, &regions, at)? {
                app.mouse.last_click = None;
            } else {
                gallery_click(app, at, now)?;
            }
        }
        MouseEventKind::Down(MouseButton::Right) => {
            app.mouse.last_click = None;
            gallery_right_click(app, at)?;
        }
        MouseEventKind::ScrollUp if shift => app.thumb_zoom_in(),
        MouseEventKind::ScrollDown if shift => app.thumb_zoom_out(),
        // macOS can send Shift+wheel as a horizontal wheel without the SHIFT flag. The side
        // depends on the natural scrolling setting. A sideways trackpad swipe sends the same
        // events and also changes the size.
        MouseEventKind::ScrollRight => app.thumb_zoom_in(),
        MouseEventKind::ScrollLeft => app.thumb_zoom_out(),
        MouseEventKind::ScrollUp => scroll_grid(app, -1),
        MouseEventKind::ScrollDown => scroll_grid(app, 1),
        _ => {}
    }
    Ok(false)
}

/// The wheel scrolls the grid by one row. A scroll redraws the pictures.
fn scroll_grid(app: &mut App, rows: isize) {
    let before = app.gallery.scroll_offset;
    app.gallery.scroll_rows(rows);
    if app.gallery.scroll_offset != before {
        app.pre_decode_hovered();
        app.needs_render = true;
    }
}

/// A click puts the cursor on the cell. A double-click opens it: a folder switches the grid,
/// a file opens the fullscreen view or the player. A click on a gap breaks a double-click.
fn gallery_click(app: &mut App, at: Position, now: Instant) -> io::Result<()> {
    let Some(position) = select_cell(app, at) else {
        app.mouse.last_click = None;
        return Ok(());
    };
    if !app.mouse.double_click(position, now) {
        return Ok(());
    }
    if app.selected_is_folder() {
        app.enter_selected_folder();
    } else {
        app.graphics_delete_all()?;
        app.enter_fullscreen_selected();
    }
    Ok(())
}

/// A right click on a cell shows its info panel. The panel stays open and follows the cursor.
fn gallery_right_click(app: &mut App, at: Position) -> io::Result<()> {
    if select_cell(app, at).is_some() && !app.info_visible {
        app.graphics_delete_all()?;
        app.toggle_info();
    }
    Ok(())
}

/// Put the cursor on the cell under `at`. A click ends the typing of a search and keeps the
/// filter, like Enter. The position in the filtered list, or `None` on a gap.
fn select_cell(app: &mut App, at: Position) -> Option<usize> {
    let position = app.gallery.cell_at(at.x, at.y)?;
    app.gallery.search_active = false;
    app.gallery.cursor = position;
    app.gallery.ensure_cursor_visible();
    app.pre_decode_hovered();
    Some(position)
}

fn fullscreen_mouse(app: &mut App, mouse: MouseEvent, at: Position) -> io::Result<bool> {
    let regions = regions(app);
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if !shared_click(app, &regions, at)? {
                app.mouse.drag = Some(Drag {
                    last: at,
                    moved: false,
                });
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => drag_pan(app, at),
        MouseEventKind::Up(MouseButton::Left) => {
            let Some(drag) = app.mouse.drag.take() else {
                return Ok(false);
            };
            if !drag.moved && app.zoom <= 1.0 && regions.content.contains(at) {
                edge_step(app, regions.content, at);
            }
        }
        MouseEventKind::Down(MouseButton::Right) => {
            app.graphics_delete_all()?;
            app.toggle_info();
        }
        MouseEventKind::ScrollUp => app.zoom_in(),
        MouseEventKind::ScrollDown => app.zoom_out(),
        _ => {}
    }
    Ok(false)
}

/// Move the zoomed image with the mouse: the picture follows the pointer, one cell of drag
/// moves it by one cell. The pan fraction is the drag in pixels over the pan range.
fn drag_pan(app: &mut App, at: Position) {
    let Some(drag) = app.mouse.drag.as_mut() else {
        return;
    };
    if drag.last == at {
        return;
    }
    let dx = f64::from(at.x) - f64::from(drag.last.x);
    let dy = f64::from(at.y) - f64::from(drag.last.y);
    drag.last = at;
    drag.moved = true;
    let (range_x, range_y) = app.pan_range_px();
    let fx = share(-dx * f64::from(app.cell_px.0), range_x);
    let fy = share(-dy * f64::from(app.cell_px.1), range_y);
    app.pan_fraction(fx, fy);
}

/// A move in pixels as a share of the pan range. No range, no pan.
fn share(pixels: f64, range: f64) -> f64 {
    if range > 0.0 { pixels / range } else { 0.0 }
}

/// A click near the left edge steps back, near the right edge forward. The middle does nothing.
fn edge_step(app: &mut App, content: Rect, at: Position) {
    let edge = (content.width / EDGE_SHARE).max(1);
    if at.x < content.x + edge {
        app.prev();
    } else if at.x >= content.right().saturating_sub(edge) {
        app.next();
    } else {
        return;
    }
    app.slideshow_touch();
}

/// The player: a click on the picture does nothing, a click beside it or a right click goes
/// back like `q`. The wheel does nothing: the player has no seek, and a trackpad swipe would
/// start ffmpeg again for each step. The status row and the info panel act as in the other
/// views.
#[cfg(feature = "video")]
fn video_mouse(app: &mut App, mouse: MouseEvent, at: Position) -> io::Result<bool> {
    use super::video::leave_player;

    let regions = regions(app);
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) if shared_click(app, &regions, at)? => {}
        MouseEventKind::Down(MouseButton::Left) if video_picture(app).contains(at) => {}
        MouseEventKind::Down(MouseButton::Left | MouseButton::Right) => leave_player(app)?,
        _ => {}
    }
    Ok(false)
}

/// The cells of the video frame, as `render_video_frame_to` places it. Before the first frame,
/// the whole box counts as the picture, so an early click does not leave.
#[cfg(feature = "video")]
fn video_picture(app: &App) -> Rect {
    let frame = app
        .video
        .as_ref()
        .and_then(|video| video.current_frame.as_ref())
        .map(crate::player::Frame::dimensions);
    let Some(frame) = frame else {
        return app.image_rect;
    };
    crate::player::cell_box(frame, app.image_rect, app.cell_px)
}

#[cfg(test)]
mod tests {
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
    fn a_click_selects_the_cell_and_a_double_click_opens_it() {
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
    fn a_double_click_on_another_cell_only_selects_and_a_gap_does_nothing() {
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
    fn a_click_ends_the_search_typing_and_keeps_the_filter() {
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
        assert_eq!(app.gallery.search_query, "0");
    }

    #[test]
    fn a_double_click_walks_into_a_folder_and_back_through_the_parent_entry() {
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
    fn the_wheel_scrolls_one_row_and_shift_or_sideways_zooms_the_grid() {
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
        mouse(&mut app, MouseEventKind::ScrollLeft, (10, 5));
        assert_eq!(app.gallery.zoom_rows, rows + 1);
        mouse(&mut app, MouseEventKind::ScrollRight, (10, 5));
        assert_eq!(app.gallery.zoom_rows, rows);
        assert_eq!(app.gallery.scroll_offset, 0, "a zoom does not scroll");
    }

    #[test]
    fn a_right_click_shows_the_info_panel_of_the_cell_and_keeps_it_open() {
        let mut app = gallery_app(9);
        mouse_cell(&mut app, RIGHT, 5);
        assert_eq!(app.gallery.cursor, 5);
        assert!(app.info_visible);
        assert!(text(&app.graphics_out.take_bytes()).contains("a=d"));

        mouse_cell(&mut app, RIGHT, 2);
        assert_eq!(app.gallery.cursor, 2);
        assert!(app.info_visible);
        assert!(app.graphics_out.take_bytes().is_empty());

        mouse(&mut app, RIGHT, (0, 0));
        assert_eq!(app.gallery.cursor, 2, "a right click on a gap does nothing");
    }

    #[test]
    fn a_click_on_the_status_row_opens_the_help_and_any_click_closes_it() {
        let mut app = gallery_app(9);
        draw_frame(&mut app, 120, 24);
        let status = ui::status_area(app.layout.screen);

        click(&mut app, (status.x + 3, status.y));
        assert!(app.help_visible);
        assert!(text(&app.graphics_out.take_bytes()).contains("a=d"));

        mouse(&mut app, MouseEventKind::ScrollDown, (10, 5));
        assert!(app.help_visible, "the wheel keeps the help");
        mouse(&mut app, RIGHT, (10, 5));
        assert!(!app.help_visible);
        assert!(app.needs_render);
    }

    #[test]
    fn a_click_on_the_info_panel_closes_it() {
        let mut app = gallery_app(9);
        app.info_visible = true;
        draw_frame(&mut app, 120, 24);
        let (_, info) = ui::split_info(app.layout.screen, &app);
        let info = info.unwrap();

        click(&mut app, (info.x + 2, info.y + 2));
        assert!(!app.info_visible);
        assert!(text(&app.graphics_out.take_bytes()).contains("a=d"));
    }

    #[test]
    fn a_click_cancels_the_delete_question_and_never_deletes() {
        let mut app = fullscreen_app(3, 1);
        char_key(&mut app, 'd');
        assert!(app.pending_delete.is_some());

        mouse(&mut app, MouseEventKind::ScrollUp, (10, 5));
        assert!(app.pending_delete.is_some(), "the wheel keeps the question");
        click(&mut app, (10, 5));
        assert!(app.pending_delete.is_none());
        assert_eq!(app.images.len(), 3);
        assert_eq!(app.mode, ViewMode::Fullscreen);
    }

    #[test]
    fn the_fullscreen_wheel_zooms_and_a_drag_pans() {
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
    fn an_edge_click_steps_and_the_middle_or_a_zoomed_view_does_not() {
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
    fn a_right_click_in_the_fullscreen_view_toggles_the_info_panel() {
        let mut app = fullscreen_app(3, 1);
        mouse(&mut app, RIGHT, (10, 10));
        assert!(app.info_visible);
        assert!(text(&app.graphics_out.take_bytes()).contains("a=d"));
        mouse(&mut app, RIGHT, (10, 10));
        assert!(!app.info_visible);
    }

    #[cfg(feature = "video")]
    #[test]
    fn the_player_ignores_a_click_and_the_wheel_and_leaves_on_a_right_click() {
        use crate::player::Player;
        use crate::test_support::video::{clip_info, fake_decoder, shared, video_app};
        use std::path::Path;

        let mut app = video_app();
        draw_frame(&mut app, 120, 24);
        let content = ui::content_area(app.layout.screen);
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
    fn a_click_on_the_status_row_of_the_player_opens_the_help() {
        use crate::test_support::video::video_app;

        let mut app = video_app();
        draw_frame(&mut app, 120, 24);
        let status = ui::status_area(app.layout.screen);
        click(&mut app, (3, status.y));
        assert_eq!(app.mode, ViewMode::Video);
        assert!(app.help_visible);
    }

    #[cfg(feature = "video")]
    #[test]
    fn a_click_beside_the_video_frame_goes_back_like_q() {
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
    fn without_the_mouse_key_the_loop_never_captures() {
        let config = Config {
            mouse: false,
            ..Config::default()
        };
        let state = LoopState::new(&config);
        assert!(!state.mouse);
        assert!(!state.mouse_captured);
        let mut app = gallery_app_with(9, &config);
        click_cell(&mut app, 4, Instant::now());
        assert_eq!(app.gallery.cursor, 4, "the handler itself still works");
    }
}
