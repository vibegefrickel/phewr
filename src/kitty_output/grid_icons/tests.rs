//! The tests of the grid icons: one image per icon, one placement per cell, and the deletes.
use super::{ICON_ID_BASE, first_id, image_ids, sync_grid_icons_to};
use crate::app::App;
use crate::icons::Icon;
use crate::kitty_output::tests::{grid_app, sharp_thumbnail};
use crate::test_support::kitty::FakeTerminal;
use crate::test_support::{draw_frame, folder_app, text};

const WHITE: [u8; 3] = [255, 255, 255];

/// Draw the grid of `app` and write the icon placements into `terminal`. Returns the bytes.
fn draw_and_sync(app: &mut App, terminal: &mut FakeTerminal) -> String {
    draw_and_sync_at(app, terminal, 30)
}

fn draw_and_sync_at(app: &mut App, terminal: &mut FakeTerminal, height: u16) -> String {
    draw_frame(app, 106, height);
    let mut out = Vec::new();
    sync_grid_icons_to(app, &mut out).unwrap();
    terminal.feed(&out);
    text(&out)
}

fn icon_app(count: usize) -> App {
    let mut app = grid_app(count);
    app.grid_icons.set_color(Some(WHITE));
    app
}

fn placements_of(terminal: &FakeTerminal, icon: Icon) -> Vec<u32> {
    let mut ids: Vec<u32> = terminal
        .placements
        .iter()
        .filter(|placement| image_ids(icon).contains(&placement.id))
        .filter_map(|placement| placement.number)
        .collect();
    ids.sort_unstable();
    ids
}

#[test]
fn one_image_serves_every_cell() {
    let mut app = icon_app(3);
    let mut terminal = FakeTerminal::new();

    let out = draw_and_sync(&mut app, &mut terminal);

    assert_eq!(out.matches("a=t,").count(), 1, "one transmit: {out:?}");
    assert_eq!(terminal.stored_ids(), vec![first_id(Icon::Loading)]);
    assert_eq!(placements_of(&terminal, Icon::Loading), vec![1, 2, 3]);
    assert_eq!(terminal.synchronized_depth, 0);
    assert!(terminal.black_cells.is_empty());
    let first = &terminal.placements[0];
    // 6 rows of 16 px give an icon of 2 rows. 32 px are 4 columns of 8 px.
    assert_eq!(first.cell_box, Some((4, 2)));
}

#[test]
fn unchanged_draw_writes_nothing() {
    let mut app = icon_app(3);
    let mut terminal = FakeTerminal::new();
    draw_and_sync(&mut app, &mut terminal);

    draw_frame(&mut app, 106, 30);
    super::sync_grid_icons(&mut app).unwrap();

    assert_eq!(
        text(&app.graphics_out.take_bytes()),
        "",
        "the sync skips a draw without a change"
    );
}

#[test]
fn thumbnail_removes_only_its_icon() {
    let mut app = icon_app(3);
    let mut terminal = FakeTerminal::new();
    draw_and_sync(&mut app, &mut terminal);

    app.thumb_cache.insert(1, sharp_thumbnail(160, 96));
    let out = draw_and_sync(&mut app, &mut terminal);

    assert!(out.contains("a=d,d=i,i="), "{out:?}");
    assert!(!out.contains("a=t,"), "the image stays: {out:?}");
    assert_eq!(placements_of(&terminal, Icon::Loading), vec![1, 3]);
}

#[test]
fn rows_of_two_heights_keep_all_icons() {
    let mut app = folder_app(&["..", "a", "b", "c", "d", "e", "f", "g", "h", "i"], &[]);
    app.grid_icons.set_color(Some(WHITE));
    app.gallery.zoom_rows = 3;
    // The first grid rows get the spare rows. Find a height with two icon sizes.
    let height = (20..80)
        .find(|&height| {
            draw_frame(&mut app, 106, height);
            let rows: std::collections::HashSet<u16> = app
                .grid_icons
                .wanted
                .iter()
                .map(|cell| cell.area.height)
                .collect();
            rows.len() > 1
        })
        .expect("a height with two icon sizes");
    let mut terminal = FakeTerminal::new();

    draw_and_sync_at(&mut app, &mut terminal, height);
    draw_frame(&mut app, 106, height);

    assert_eq!(terminal.placements.len(), app.grid_icons.wanted.len());
    assert!(terminal.black_cells.is_empty());
    assert!(app.grid_icons.in_sync(), "the next draw writes nothing");
    let sizes: std::collections::HashSet<_> = app
        .grid_icons
        .wanted
        .iter()
        .map(|cell| (cell.icon, cell.area.height))
        .collect();
    assert_eq!(
        terminal.stored_ids().len(),
        sizes.len(),
        "one image per size"
    );
}

#[test]
fn delete_all_sends_the_image_again() {
    let mut app = icon_app(2);
    let mut terminal = FakeTerminal::new();
    draw_and_sync(&mut app, &mut terminal);

    let mut out = Vec::new();
    app.graphics_delete_all_to(&mut out).unwrap();
    terminal.feed(&out);
    let out = draw_and_sync(&mut app, &mut terminal);

    assert_eq!(out.matches("a=t,").count(), 1, "{out:?}");
    assert_eq!(placements_of(&terminal, Icon::Loading), vec![1, 2]);
    assert!(terminal.black_cells.is_empty());
}

#[test]
fn new_size_replaces_the_image() {
    let mut app = icon_app(2);
    let mut terminal = FakeTerminal::new();
    draw_and_sync(&mut app, &mut terminal);

    // 2 rows give cells of 13 rows and icons of 4 rows.
    app.gallery.zoom_rows = 2;
    let out = draw_and_sync(&mut app, &mut terminal);

    assert_eq!(out.matches("a=t,").count(), 1, "{out:?}");
    assert_eq!(placements_of(&terminal, Icon::Loading), vec![1, 2]);
    assert!(
        terminal
            .placements
            .iter()
            .all(|placement| placement.cell_box.is_some_and(|(_, rows)| rows > 2)),
        "{:?}",
        terminal.placements
    );
}

#[test]
fn help_and_other_views_take_the_icons_away() {
    let mut app = icon_app(2);
    let mut terminal = FakeTerminal::new();
    draw_and_sync(&mut app, &mut terminal);

    app.help_visible = true;
    draw_and_sync(&mut app, &mut terminal);
    assert!(terminal.placements.is_empty());

    app.help_visible = false;
    draw_and_sync(&mut app, &mut terminal);
    assert_eq!(placements_of(&terminal, Icon::Loading), vec![1, 2]);

    app.mode = crate::app::ViewMode::Fullscreen;
    draw_and_sync(&mut app, &mut terminal);
    assert!(terminal.placements.is_empty());
}

#[test]
fn folders_get_their_icons() {
    let mut app = folder_app(&["..", "a", "b"], &["x.jpg"]);
    app.grid_icons.set_color(Some(WHITE));
    let mut terminal = FakeTerminal::new();

    draw_and_sync(&mut app, &mut terminal);

    assert_eq!(placements_of(&terminal, Icon::Parent).len(), 1);
    assert_eq!(placements_of(&terminal, Icon::Folder).len(), 2);
    assert_eq!(placements_of(&terminal, Icon::Loading).len(), 1);
}

#[test]
fn without_color_icons_stay_text() {
    let mut app = grid_app(2);
    let mut terminal = FakeTerminal::new();

    let out = draw_and_sync(&mut app, &mut terminal);

    assert!(app.grid_icons.wanted.is_empty());
    assert!(!out.contains("\x1b_G"), "{out:?}");
}

#[test]
fn icon_ids_sit_below_the_loops() {
    let ids: Vec<u32> = Icon::ALL.into_iter().flat_map(image_ids).collect();
    assert_eq!(ids.len(), 16);
    assert!(ids.iter().all(|id| (ICON_ID_BASE..1 << 31).contains(id)));
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]), "no id twice");
}
