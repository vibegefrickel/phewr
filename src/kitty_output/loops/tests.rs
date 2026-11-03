//! The tests of the grid loops: the frames on the ticks and the Kitty animations.
use super::super::grid::place_gallery_images_to;
use super::{play_grid_loops, play_grid_loops_to};
use crate::app::App;
use crate::app::ViewMode;
use crate::app::thumbnails::{pad_for_grid, pad_loop_frame};
use crate::encoder::Transfer;
use crate::encoder::kitty;
use crate::encoder::{KittyBackend, Loops};
use crate::image_list::SharedImageList;
use crate::kitty_output::render_gallery_images;
use crate::test_support::char_key;
use crate::test_support::kitty::Animation;
use crate::test_support::kitty::FakeTerminal;
use crate::test_support::text;
use image::{Rgba, RgbaImage};
use ratatui::layout::Rect;
use std::path::PathBuf;
use std::time::Duration;

const BOX_PX: (u32, u32) = (160, 96);

const CELL_PX: (u32, u32) = (8, 16);

/// The time of a video frame on the loop clock: 4 ticks of 1/12 s.
const FRAME_TICKS: u64 = 4;

/// The RGBA bytes of a still and of a half-size loop frame of `clip_app`.
const STILL_BYTES: usize = 160 * 96 * 4;

const FRAME_BYTES: usize = 80 * 48 * 4;

const BEGIN_SYNC: &str = "\x1b[?2026h";

const END_SYNC: &str = "\x1b[?2026l";

/// A grid of `count` clips with sharp stills: 5 columns and 4 rows of 160x96 px cells.
fn clip_app(count: usize) -> App {
    let mut app = App::new(CELL_PX, SharedImageList::new());
    app.images = (0..count)
        .map(|index| PathBuf::from(format!("/clips/{index}.mp4")))
        .collect();
    app.filenames = (0..count).map(|index| format!("{index}.mp4")).collect();
    app.gallery.filtered_indices = (0..count).collect();
    app.gallery.zoom_rows = 4;
    app.gallery.update_grid(Rect::new(0, 0, 106, 29));
    app.scan_complete = true;
    for index in 0..count {
        let still = RgbaImage::from_pixel(160, 96, Rgba([9, 9, 9, 255]));
        app.thumb_cache
            .insert(index, pad_for_grid(still, BOX_PX, CELL_PX, true));
    }
    app
}

/// Give the cell `index` a loop of `count` half-size frames of 333 ms. Returns the frame ids.
fn add_loop(app: &mut App, index: usize, count: usize) -> Vec<u32> {
    let frames = (0..count)
        .map(|frame| {
            let red = u8::try_from(frame * 10).unwrap();
            let image = RgbaImage::from_pixel(80, 48, Rgba([red, 0, 0, 255]));
            pad_loop_frame(image, BOX_PX, BOX_PX, CELL_PX)
        })
        .collect();
    let delays = vec![Duration::from_millis(333); count];
    let loops = app.graphics.loops();
    app.grid_loops.insert(index, frames, &delays, loops);
    app.grid_loops.loops[&index].ids.clone()
}

fn place_stills(app: &mut App, terminal: &mut FakeTerminal) {
    let mut out = Vec::new();
    place_gallery_images_to(app, &mut out).unwrap();
    terminal.feed(&out);
}

/// Write the loops at `tick`. Returns the output and whether a loop started.
fn play_at(app: &mut App, tick: u64) -> (String, bool) {
    let mut out = Vec::new();
    let now = app.grid_loops.instant_of(tick);
    let started = play_grid_loops_to(app, &mut out, now).unwrap();
    (text(&out), started)
}

fn placed(terminal: &FakeTerminal, ids: &[u32]) -> Vec<u32> {
    terminal
        .placements
        .iter()
        .map(|placement| placement.id)
        .filter(|id| ids.contains(id))
        .collect()
}

#[test]
fn tick_swaps_placement_without_pixels() {
    let mut app = clip_app(3);
    let mut terminal = FakeTerminal::new();
    place_stills(&mut app, &mut terminal);
    let (_, still) = app.thumb_cache.peek(1).unwrap();
    let ids = add_loop(&mut app, 1, 3);
    assert!(app.loops_to_send());
    assert!(!app.loop_plays(1));

    let (first, started) = play_at(&mut app, 0);
    terminal.feed(first.as_bytes());
    assert!(started, "the draw drops the marker");
    assert_eq!(first.matches("\x1b_Ga=t,").count(), 3, "{first}");
    assert!(!first.contains("\x1b_Ga=T,"), "no frame places itself");
    assert!(first.contains(&format!("\x1b_Ga=d,d=i,i={still},")));
    assert!(first.starts_with(BEGIN_SYNC) && first.ends_with(END_SYNC));
    assert_eq!(placed(&terminal, &ids), vec![ids[0]]);
    assert!(placed(&terminal, &[still]).is_empty());
    assert!(
        terminal.stores(still),
        "the still stays for the next overlay"
    );
    assert!(app.loop_plays(1));
    assert!(!app.loops_to_send());

    let (tick, started) = play_at(&mut app, FRAME_TICKS);
    terminal.feed(tick.as_bytes());
    assert!(!started);
    assert!(!tick.contains("a=t,") && !tick.contains("a=T,"), "{tick}");
    assert!(
        tick.contains(&format!("\x1b_Ga=d,d=i,i={},", ids[0])),
        "{tick:?} {ids:?}"
    );
    assert!(tick.contains(&format!("\x1b_Ga=p,q=2,C=1,i={},c=20,r=6", ids[1])));
    assert!(
        tick.len() < 150,
        "{} bytes for one cell: {tick:?}",
        tick.len()
    );
    assert_eq!(placed(&terminal, &ids), vec![ids[1]]);
    assert!(terminal.black_cells.is_empty());

    // In the middle of a frame, nothing is due, and the pass writes nothing.
    let now = app.grid_loops.instant_of(FRAME_TICKS + 3);
    assert!(!app.loop_frames_due(now));
    assert!(!play_grid_loops(&mut app, now).unwrap());
    assert!(app.graphics_out.take_bytes().is_empty());

    // After the last frame, the loop starts again.
    let (again, _) = play_at(&mut app, 3 * FRAME_TICKS);
    assert!(again.contains(&format!("i={},c=20,r=6", ids[0])), "{again}");
}

fn animation_app(count: usize) -> App {
    let mut app = clip_app(count);
    app.graphics = KittyBackend::new(Transfer::Direct).with_loops(Loops::Animation);
    app
}

#[test]
fn animation_replaces_still_once() {
    let mut app = animation_app(3);
    let mut terminal = FakeTerminal::new();
    place_stills(&mut app, &mut terminal);
    let (_, still) = app.thumb_cache.peek(1).unwrap();
    let ids = add_loop(&mut app, 1, 3);
    assert_eq!(ids.len(), 1, "one id for the whole animation");
    let id = ids[0];
    assert!(app.loops_to_send());

    let (first, started) = play_at(&mut app, 0);
    terminal.feed(first.as_bytes());
    assert!(started, "the draw drops the marker");
    assert!(first.starts_with(BEGIN_SYNC) && first.ends_with(END_SYNC));
    assert_eq!(terminal.synchronized_depth, 0);
    assert_eq!(first.matches("\x1b_Ga=t,").count(), 1, "the root frame");
    assert_eq!(
        first
            .matches(&format!("\x1b_Ga=f,q=2,f=24,s=80,v=48,i={id},z=333,N=1,"))
            .count(),
        2,
        "{first}"
    );
    let root = first
        .find(&format!("\x1b_Ga=t,q=2,C=1,f=24,s=80,v=48,i={id},"))
        .unwrap();
    let frame = first.find("\x1b_Ga=f,").unwrap();
    let control = first
        .find(&format!(
            "\x1b_Ga=a,q=2,i={id},r=1,z=333\x1b\\\x1b_Ga=a,q=2,i={id},s=3,v=1"
        ))
        .unwrap();
    let place = first
        .find(&format!("\x1b_Ga=p,q=2,C=1,i={id},c=20,r=6"))
        .unwrap();
    let unplace = first.find(&format!("\x1b_Ga=d,d=i,i={still},")).unwrap();
    assert!(
        root < frame && frame < control && control < place && control < unplace,
        "the still shows until the whole animation is in the terminal"
    );
    assert_eq!(
        terminal.animation(id).cloned(),
        Some(Animation {
            gaps: vec![333; 3],
            state: Some(3),
            loops: Some(1),
        })
    );
    assert!(terminal.orphan_frames.is_empty());
    assert_eq!(placed(&terminal, &ids), vec![id]);
    assert!(placed(&terminal, &[still]).is_empty());
    assert_eq!(app.terminal_images.bytes_of(id), 3 * FRAME_BYTES);
    assert_eq!(terminal.stored_bytes(), 3 * STILL_BYTES + 3 * FRAME_BYTES);
    assert!(app.loop_plays(1));
    assert!(!app.loops_to_send());
}

#[test]
fn animation_needs_no_tick() {
    let mut app = animation_app(3);
    let mut terminal = FakeTerminal::new();
    place_stills(&mut app, &mut terminal);
    add_loop(&mut app, 1, 3);
    terminal.feed(play_at(&mut app, 0).0.as_bytes());

    for tick in [1, FRAME_TICKS, 3 * FRAME_TICKS + 1] {
        let now = app.grid_loops.instant_of(tick);
        assert!(!app.loop_frames_due(now), "tick {tick}");
        assert_eq!(app.grid_loop_timeout(now), None, "tick {tick}");
        assert!(!play_grid_loops(&mut app, now).unwrap());
        assert!(app.graphics_out.take_bytes().is_empty(), "tick {tick}");
        let (out, started) = play_at(&mut app, tick);
        assert!(!started);
        assert_eq!(out, format!("{BEGIN_SYNC}{END_SYNC}"), "tick {tick}");
    }
    assert!(!app.loops_to_send());
    assert_eq!(
        crate::run_loop::next_timeout(&app, &[]),
        Duration::from_millis(250),
        "the idle wait"
    );
}

#[test]
fn animation_off_screen_deletes_image() {
    let mut app = animation_app(40);
    let mut terminal = FakeTerminal::new();
    place_stills(&mut app, &mut terminal);
    let ids = add_loop(&mut app, 0, 3);
    terminal.feed(play_at(&mut app, 0).0.as_bytes());
    assert!(terminal.animation(ids[0]).is_some());

    app.gallery.move_to_last();
    render_gallery_images(&mut app).unwrap();
    let out = app.graphics_out.take_bytes();
    terminal.feed(&out);
    let out = text(&out);
    assert_eq!(
        out.matches(&format!("\x1b_Ga=d,d=I,i={},", ids[0])).count(),
        1
    );
    assert!(!app.terminal_images.contains(ids[0]));
    assert!(!terminal.stores(ids[0]));
    assert!(app.grid_loops.loops.is_empty());
}

#[test]
fn redraw_places_running_animation() {
    let mut app = animation_app(3);
    let mut terminal = FakeTerminal::new();
    place_stills(&mut app, &mut terminal);
    let (_, still) = app.thumb_cache.peek(1).unwrap();
    let ids = add_loop(&mut app, 1, 3);
    terminal.feed(play_at(&mut app, 0).0.as_bytes());

    let mut out = Vec::new();
    place_gallery_images_to(&mut app, &mut out).unwrap();
    terminal.feed(&out);
    let out = text(&out);
    assert!(
        !out.contains("a=t,") && !out.contains("a=f,") && !out.contains("a=a,"),
        "{out}"
    );
    assert_eq!(placed(&terminal, &ids), ids, "placed by its id");
    assert!(placed(&terminal, &[still]).is_empty());
    assert_eq!(
        terminal.animation(ids[0]).and_then(|a| a.state),
        Some(3),
        "it runs on"
    );
    assert!(terminal.black_cells.is_empty());

    app.graphics_delete_all().unwrap();
    terminal.feed(&app.graphics_out.take_bytes());
    assert!(app.loops_to_send(), "the terminal forgot the animation");
    place_stills(&mut app, &mut terminal);
    let (again, started) = play_at(&mut app, FRAME_TICKS);
    terminal.feed(again.as_bytes());
    assert!(started);
    assert_eq!(again.matches("\x1b_Ga=f,q=2,").count(), 2, "{again}");
    assert_eq!(placed(&terminal, &ids), ids);
    assert!(terminal.animation(ids[0]).is_some());
}

#[test]
fn animation_over_budget_keeps_still() {
    let mut app = animation_app(3);
    let mut terminal = FakeTerminal::new();
    app.terminal_images.budget = 3 * STILL_BYTES + 2 * FRAME_BYTES;
    place_stills(&mut app, &mut terminal);
    add_loop(&mut app, 0, 3);

    let (out, started) = play_at(&mut app, 0);
    assert!(!started);
    assert!(!out.contains("a=t,") && !out.contains("a=f,"), "{out}");
    assert!(!app.loop_plays(0));
    assert!(
        app.grid_loops.loops.is_empty(),
        "the frames leave the memory too"
    );
    assert!(!app.loops_to_send(), "no pass waits for the cell");

    let ids = add_loop(&mut app, 1, 2);
    let (out, started) = play_at(&mut app, 0);
    terminal.feed(out.as_bytes());
    assert!(started, "an animation that fits plays");
    assert_eq!(placed(&terminal, &ids), ids);
    assert_eq!(terminal.animation(ids[0]).map(|a| a.gaps.len()), Some(2));
    assert_eq!(
        app.terminal_images.bytes(),
        3 * STILL_BYTES + 2 * FRAME_BYTES
    );
}

#[test]
fn help_popup_restarts_animation() {
    let mut app = animation_app(3);
    let mut terminal = FakeTerminal::new();
    place_stills(&mut app, &mut terminal);
    let ids = add_loop(&mut app, 1, 3);
    terminal.feed(play_at(&mut app, 0).0.as_bytes());

    char_key(&mut app, '?');
    let out = app.graphics_out.take_bytes();
    terminal.feed(&out);
    assert!(text(&out).contains("\x1b_Ga=d,d=A,"));
    assert!(
        !terminal.stores(ids[0]),
        "the terminal forgot the animation"
    );
    assert!(!app.loop_plays(1));
    assert!(!app.loops_to_send());
    assert!(
        play_at(&mut app, FRAME_TICKS).0.is_empty(),
        "nothing above the popup"
    );

    char_key(&mut app, 'x');
    place_stills(&mut app, &mut terminal);
    let (again, started) = play_at(&mut app, 2 * FRAME_TICKS);
    terminal.feed(again.as_bytes());
    assert!(started);
    assert_eq!(again.matches("\x1b_Ga=t,").count(), 1, "{again}");
    assert_eq!(again.matches("\x1b_Ga=f,q=2,").count(), 2, "{again}");
    assert!(again.contains(&format!("\x1b_Ga=a,q=2,i={},s=3,v=1", ids[0])));
    assert_eq!(placed(&terminal, &ids), ids);
    assert_eq!(terminal.animation(ids[0]).map(|a| a.gaps.len()), Some(3));
}

#[test]
fn zoom_drops_animations() {
    let mut app = animation_app(3);
    let mut terminal = FakeTerminal::new();
    place_stills(&mut app, &mut terminal);
    let ids = add_loop(&mut app, 1, 3);
    terminal.feed(play_at(&mut app, 0).0.as_bytes());

    char_key(&mut app, '+');
    assert!(app.grid_loops.loops.is_empty());
    let mut out = Vec::new();
    place_gallery_images_to(&mut app, &mut out).unwrap();
    terminal.feed(&out);
    assert!(text(&out).contains("\x1b_Ga=d,d=A,"));
    assert!(!terminal.stores(ids[0]));
}

#[test]
fn new_animation_deletes_old_image() {
    let mut app = animation_app(3);
    let mut terminal = FakeTerminal::new();
    place_stills(&mut app, &mut terminal);
    let old = add_loop(&mut app, 1, 3);
    terminal.feed(play_at(&mut app, 0).0.as_bytes());

    let new = add_loop(&mut app, 1, 2);
    assert_ne!(old, new);
    assert_eq!(
        app.grid_loops.stale_ids, old,
        "one id for the old animation"
    );
    let mut out = Vec::new();
    place_gallery_images_to(&mut app, &mut out).unwrap();
    terminal.feed(&out);
    let out = text(&out);
    assert_eq!(
        out.matches(&format!("\x1b_Ga=d,d=I,i={},", old[0])).count(),
        1
    );
    assert!(!terminal.stores(old[0]));

    terminal.feed(play_at(&mut app, FRAME_TICKS).0.as_bytes());
    assert_eq!(placed(&terminal, &new), new);
    assert_eq!(terminal.animation(new[0]).map(|a| a.gaps.len()), Some(2));
    assert!(terminal.black_cells.is_empty());
}

#[test]
fn run_loop_sleeps_until_next_frame() {
    let mut app = clip_app(3);
    assert_eq!(app.grid_loop_timeout(app.grid_loops.instant_of(0)), None);
    add_loop(&mut app, 1, 3);
    play_at(&mut app, 0);
    let timeout = app.grid_loop_timeout(app.grid_loops.instant_of(2));
    let frame = app.grid_loops.instant_of(FRAME_TICKS) - app.grid_loops.instant_of(2);
    assert_eq!(timeout, Some(frame));
    assert!(crate::run_loop::next_timeout(&app, &[]) <= Duration::from_millis(334));

    app.grid_loops.loops.clear();
    assert_eq!(
        crate::run_loop::next_timeout(&app, &[]),
        Duration::from_millis(250),
        "without a loop, the idle wait stays"
    );
}

#[test]
fn loop_over_budget_keeps_still() {
    let mut app = clip_app(3);
    let mut terminal = FakeTerminal::new();
    app.terminal_images.budget = 3 * STILL_BYTES + 2 * FRAME_BYTES;
    place_stills(&mut app, &mut terminal);
    add_loop(&mut app, 0, 3);

    let (out, started) = play_at(&mut app, 0);
    assert!(!started);
    assert!(!out.contains("a=t,"), "{out}");
    assert!(!app.loop_plays(0));
    assert!(
        app.grid_loops.loops.is_empty(),
        "the frames leave the memory too"
    );
    assert!(!app.loops_to_send(), "no pass waits for the cell");

    let ids = add_loop(&mut app, 1, 2);
    let (out, started) = play_at(&mut app, 0);
    terminal.feed(out.as_bytes());
    assert!(started, "a loop that fits plays");
    assert_eq!(placed(&terminal, &ids), vec![ids[0]]);
    assert!(app.terminal_images.bytes() <= app.terminal_images.budget);
}

#[test]
fn loop_off_screen_deletes_frames() {
    let mut app = clip_app(40);
    let mut terminal = FakeTerminal::new();
    place_stills(&mut app, &mut terminal);
    let ids = add_loop(&mut app, 0, 3);
    play_at(&mut app, 0);
    assert!(ids.iter().all(|&id| app.terminal_images.contains(id)));

    app.gallery.move_to_last();
    render_gallery_images(&mut app).unwrap();
    let out = text(&app.graphics_out.take_bytes());
    for id in &ids {
        assert!(out.contains(&format!("\x1b_Ga=d,d=I,i={id},")), "{id}");
        assert!(!app.terminal_images.contains(*id));
    }
    assert!(app.grid_loops.loops.is_empty());
}

/// The first command of each transmit in `output`, without its payload.
fn transmits(output: &str) -> Vec<&str> {
    output
        .split("\x1b_G")
        .skip(1)
        .map(|command| command.split(';').next().unwrap_or_default())
        .filter(|keys| keys.contains("q=2"))
        .filter(|keys| ["a=T,", "a=t,", "a=f,"].iter().any(|a| keys.starts_with(a)))
        .collect()
}

#[test]
fn only_animation_frames_are_transient() {
    for (mut app, hinted) in [(clip_app(3), false), (animation_app(3), true)] {
        let mut terminal = FakeTerminal::new();
        let mut stills = Vec::new();
        place_gallery_images_to(&mut app, &mut stills).unwrap();
        let stills = text(&stills);
        assert!(!transmits(&stills).is_empty());
        assert!(!stills.contains("N="), "a still is not transient: {stills}");
        terminal.feed(stills.as_bytes());

        add_loop(&mut app, 1, 3);
        let (first, _) = play_at(&mut app, 0);

        let frames = transmits(&first);
        assert!(frames.len() >= 3, "{first}");
        for keys in frames {
            let has_hint = keys.split(',').any(|key| key == "N=1");
            assert_eq!(has_hint, hinted, "{keys}");
        }
    }
}

#[test]
fn lost_frame_falls_back_to_still() {
    let mut app = clip_app(3);
    let mut terminal = FakeTerminal::new();
    place_stills(&mut app, &mut terminal);
    let (_, still) = app.thumb_cache.peek(1).unwrap();
    let ids = add_loop(&mut app, 1, 3);
    terminal.feed(play_at(&mut app, 0).0.as_bytes());
    assert_eq!(placed(&terminal, &ids), vec![ids[0]]);

    // The terminal drops one frame, and phewr forgets it, as after a trim.
    let mut out = Vec::new();
    kitty::delete_image_to(&mut out, ids[2]).unwrap();
    terminal.feed(&out);
    app.terminal_images.remove(ids[2]);
    place_stills(&mut app, &mut terminal);

    assert!(
        placed(&terminal, &ids).is_empty(),
        "no frame of a broken loop"
    );
    assert_eq!(placed(&terminal, &[still]), vec![still], "the still shows");
    assert!(terminal.black_cells.is_empty());
}

#[test]
fn due_loops_swap_in_grid_order() {
    let mut app = clip_app(3);
    let mut terminal = FakeTerminal::new();
    place_stills(&mut app, &mut terminal);
    let first = add_loop(&mut app, 0, 3);
    let second = add_loop(&mut app, 2, 3);
    terminal.feed(play_at(&mut app, 0).0.as_bytes());

    let (tick, started) = play_at(&mut app, FRAME_TICKS);
    terminal.feed(tick.as_bytes());

    assert!(!started);
    assert_eq!(tick.matches(BEGIN_SYNC).count(), 1, "{tick:?}");
    assert_eq!(tick.matches(END_SYNC).count(), 1, "{tick:?}");
    assert!(tick.starts_with(BEGIN_SYNC) && tick.ends_with(END_SYNC));
    assert_eq!(terminal.synchronized_depth, 0);
    let position = |keys: String| {
        tick.find(&keys)
            .unwrap_or_else(|| panic!("{keys}: {tick:?}"))
    };
    let old_first = position(format!("\x1b_Ga=d,d=i,i={},", first[0]));
    let new_first = position(format!("\x1b_Ga=p,q=2,C=1,i={},", first[1]));
    let old_second = position(format!("\x1b_Ga=d,d=i,i={},", second[0]));
    let new_second = position(format!("\x1b_Ga=p,q=2,C=1,i={},", second[1]));
    assert!(
        old_first < new_first && new_first < old_second && old_second < new_second,
        "grid order: {tick:?}"
    );
    assert_eq!(placed(&terminal, &first), vec![first[1]]);
    assert_eq!(placed(&terminal, &second), vec![second[1]]);
}

#[test]
fn redraw_places_current_frame() {
    let mut app = clip_app(3);
    let mut terminal = FakeTerminal::new();
    place_stills(&mut app, &mut terminal);
    let (_, still) = app.thumb_cache.peek(1).unwrap();
    let ids = add_loop(&mut app, 1, 3);
    terminal.feed(play_at(&mut app, 0).0.as_bytes());

    place_stills(&mut app, &mut terminal);
    assert_eq!(placed(&terminal, &ids).len(), 1);
    assert!(placed(&terminal, &[still]).is_empty());
    assert!(app.loop_plays(1));
    assert!(terminal.black_cells.is_empty());
}

#[test]
fn overlays_and_fullscreen_stop_loops() {
    let mut app = clip_app(3);
    let mut terminal = FakeTerminal::new();
    place_stills(&mut app, &mut terminal);
    add_loop(&mut app, 1, 3);
    play_at(&mut app, 0);

    char_key(&mut app, '?');
    assert!(text(&app.graphics_out.take_bytes()).contains("\x1b_Ga=d,d=A,"));
    assert!(!app.loop_plays(1));
    assert!(!app.loops_to_send());
    assert_eq!(app.grid_loop_timeout(app.grid_loops.instant_of(1)), None);
    assert!(
        play_at(&mut app, FRAME_TICKS).0.is_empty(),
        "no frame above the popup"
    );

    // The grid comes back with its stills, and the next pass sends the frames again.
    char_key(&mut app, 'x');
    assert!(!app.help_visible);
    place_stills(&mut app, &mut terminal);
    let (again, started) = play_at(&mut app, 2 * FRAME_TICKS);
    assert!(started);
    assert_eq!(again.matches("\x1b_Ga=t,").count(), 3);

    char_key(&mut app, 'i');
    assert!(app.info_visible);
    place_stills(&mut app, &mut terminal);
    assert!(!app.loop_plays(1));
    assert!(play_at(&mut app, 3 * FRAME_TICKS).0.is_empty());
    char_key(&mut app, 'i');

    app.mode = ViewMode::Fullscreen;
    assert!(!app.grid_loops_shown());
    assert!(play_at(&mut app, 4 * FRAME_TICKS).0.is_empty());
}

#[test]
fn zoom_drops_loops() {
    let mut app = clip_app(3);
    let mut terminal = FakeTerminal::new();
    place_stills(&mut app, &mut terminal);
    add_loop(&mut app, 1, 3);
    play_at(&mut app, 0);

    char_key(&mut app, '+');
    assert!(app.grid_loops.loops.is_empty());
    let mut out = Vec::new();
    place_gallery_images_to(&mut app, &mut out).unwrap();
    assert!(text(&out).contains("\x1b_Ga=d,d=A,"));
}

#[test]
fn animate_off_shows_no_loop() {
    let mut app = clip_app(3);
    app.grid_loops.enabled = false;
    add_loop(&mut app, 1, 3);
    assert!(!app.loops_to_send());
    assert!(play_at(&mut app, 0).0.is_empty());
    assert!(!app.loop_plays(1));
}
