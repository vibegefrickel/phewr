//! The loops of the video and animated cells of the grid: the frames or the Kitty animation
//! of each loop, its placement and the swaps on the ticks.

use super::grid::{EMIT_PASS_BYTES, place_at, visible_image_ids};
use super::terminal_images::trim_to_budget;
use crate::app::App;
use crate::encoder;
use crate::encoder::kitty;
use crossterm::queue;
use std::io::{self, Write};
use std::time::Instant;

/// Place the frame of the loop of `img_idx` that shows at `tick`, when the grid shows its
/// loops and the terminal stores every frame. Returns `false` when the cell shows its still.
pub(super) fn place_loop_frame_to(
    app: &mut App,
    out: &mut impl Write,
    vis_idx: usize,
    img_idx: usize,
    tick: u64,
) -> io::Result<bool> {
    let shown = app.grid_loops_shown();
    let Some(cell_loop) = app.grid_loops.loops.get_mut(&img_idx) else {
        return Ok(false);
    };
    if !shown
        || !cell_loop
            .ids
            .iter()
            .all(|&id| app.terminal_images.contains(id))
    {
        return Ok(false);
    }
    cell_loop.start_at(tick);
    let id = cell_loop.id_at(tick);
    cell_loop.placed = Some(id);
    for &frame in &cell_loop.ids {
        app.terminal_images.touch(frame);
    }
    let cells = cell_loop.cells();
    place_at(app, out, vis_idx, cells, id)?;
    Ok(true)
}

/// Write the loops of the grid at `now`: the frames of the new loops, then the frames that
/// are due. A Kitty animation has no due frame, so after its start this writes nothing.
/// Returns `true` when a cell started its loop, so that the draw drops its marker.
pub(crate) fn play_grid_loops(app: &mut App, now: Instant) -> io::Result<bool> {
    if !app.loops_to_send() && !app.loop_frames_due(now) {
        return Ok(false);
    }
    let mut started = false;
    app.with_graphics_out(|app, out| {
        started = play_grid_loops_to(app, out, now)?;
        Ok(())
    })?;
    Ok(started)
}

/// Write the loops of the grid to `out`, in one synchronized update, so that the tests can read
/// the escape sequences. See [`play_grid_loops`].
pub(crate) fn play_grid_loops_to(
    app: &mut App,
    out: &mut impl Write,
    now: Instant,
) -> io::Result<bool> {
    use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};

    if !app.grid_loops_shown() {
        return Ok(false);
    }
    let tick = app.grid_loops.tick_at(now);
    queue!(out, BeginSynchronizedUpdate)?;
    let started = start_grid_loops_to(app, out, tick)?;
    swap_loop_frames_to(app, out, tick)?;
    queue!(out, EndSynchronizedUpdate)?;
    out.flush()?;
    Ok(started)
}

/// Send the frames of the visible loops that wait, in grid order, each frame one time without a
/// placement (`a=t`), or the whole loop as one Kitty animation, then put the current frame or
/// the animation in the place of the still. A pass sends up to
/// `EMIT_PASS_BYTES`, and the rest goes out in the next pass. A loop that does not fit the
/// terminal budget beside the images of the screen goes, and its cell keeps the still.
fn start_grid_loops_to(app: &mut App, out: &mut impl Write, tick: u64) -> io::Result<bool> {
    let mut kept = visible_image_ids(app);
    let mut kept_bytes: usize = kept
        .iter()
        .map(|&id| app.terminal_images.bytes_of(id))
        .sum();
    let mut written = 0;
    let mut started = false;
    let visible: Vec<(usize, usize)> = app.gallery.visible_items().collect();
    for (vis_idx, img_idx) in visible {
        let Some(cell_loop) = app.grid_loops.loops.get(&img_idx) else {
            continue;
        };
        if cell_loop.placed.is_some() {
            continue;
        }
        if !cell_loop
            .ids
            .iter()
            .all(|&id| app.terminal_images.contains(id))
        {
            if written >= EMIT_PASS_BYTES {
                continue;
            }
            let bytes = cell_loop.bytes();
            if kept_bytes + bytes > app.terminal_images.budget {
                app.grid_loops.drop_over_budget(img_idx);
                continue;
            }
            transmit_loop_to(app, out, img_idx)?;
            written += bytes;
            kept_bytes += bytes;
        }
        if let Some((_, still)) = app.thumb_cache.peek(img_idx) {
            kitty::delete_placements_to(out, still)?;
        }
        kept.extend(start_loop_to(app, out, vis_idx, img_idx, tick)?);
        started = true;
    }
    trim_to_budget(app, out, &kept)?;
    Ok(started)
}

/// Send each frame of the loop of `img_idx` that the terminal does not store, without a
/// placement, and record its bytes.
fn transmit_loop_to(app: &mut App, out: &mut impl Write, img_idx: usize) -> io::Result<()> {
    let Some(cell_loop) = app.grid_loops.loops.get(&img_idx) else {
        return Ok(());
    };
    if cell_loop.animation {
        return transmit_animation_to(app, out, img_idx);
    }
    for (frame, &id) in cell_loop.frames.iter().zip(&cell_loop.ids) {
        if app.terminal_images.contains(id) {
            continue;
        }
        app.graphics.transmit(
            out,
            (&frame.image).into(),
            &encoder::DisplayOptions {
                id: Some(id),
                cell_box: None,
                columns: None,
                transmit_only: true,
                transient: true,
            },
        )?;
        app.terminal_images.insert(id, frame.image.as_raw().len());
    }
    Ok(())
}

/// Send the loop of `img_idx` as one Kitty animation under its one id, without a placement:
/// the first frame as the root image (`a=t`), each further frame with its gap (`a=f`), then
/// the control that runs it in a loop forever (`a=a`). The terminal stores the bytes of all
/// frames under the id.
fn transmit_animation_to(app: &mut App, out: &mut impl Write, img_idx: usize) -> io::Result<()> {
    let Some(cell_loop) = app.grid_loops.loops.get(&img_idx) else {
        return Ok(());
    };
    let (Some(&id), Some(root)) = (cell_loop.ids.first(), cell_loop.frames.first()) else {
        return Ok(());
    };
    app.graphics.transmit(
        out,
        (&root.image).into(),
        &encoder::DisplayOptions {
            id: Some(id),
            cell_box: None,
            columns: None,
            transmit_only: true,
            transient: true,
        },
    )?;
    for (frame, picture) in cell_loop.frames.iter().enumerate().skip(1) {
        app.graphics
            .transmit_frame(out, (&picture.image).into(), id, cell_loop.gap_ms(frame))?;
    }
    kitty::animate_to(out, id, cell_loop.gap_ms(0))?;
    app.terminal_images.insert(id, cell_loop.bytes());
    Ok(())
}

/// Place the frame of the loop of `img_idx` at `tick` in the visible cell `vis_idx`, and start
/// the clock of the loop. Returns the ids of its frames.
fn start_loop_to(
    app: &mut App,
    out: &mut impl Write,
    vis_idx: usize,
    img_idx: usize,
    tick: u64,
) -> io::Result<Vec<u32>> {
    let Some(cell_loop) = app.grid_loops.loops.get_mut(&img_idx) else {
        return Ok(Vec::new());
    };
    cell_loop.start_at(tick);
    let id = cell_loop.id_at(tick);
    cell_loop.placed = Some(id);
    let (cells, ids) = (cell_loop.cells(), cell_loop.ids.clone());
    place_at(app, out, vis_idx, cells, id)?;
    Ok(ids)
}

/// Swap the frame of each placed loop whose next frame is due at `tick`: delete the placement
/// of the old frame (`d=i`, the pixels stay) and place the new one by its id. A tick never
/// sends pixels.
fn swap_loop_frames_to(app: &mut App, out: &mut impl Write, tick: u64) -> io::Result<()> {
    let mut swaps = Vec::new();
    for (vis_idx, img_idx) in app.gallery.visible_items() {
        let Some(cell_loop) = app.grid_loops.loops.get(&img_idx) else {
            continue;
        };
        let Some(old) = cell_loop.placed else {
            continue;
        };
        let new = cell_loop.id_at(tick);
        if new != old {
            swaps.push((vis_idx, img_idx, old, new, cell_loop.cells()));
        }
    }
    for (vis_idx, img_idx, old, new, cells) in swaps {
        kitty::delete_placements_to(out, old)?;
        place_at(app, out, vis_idx, cells, new)?;
        if let Some(cell_loop) = app.grid_loops.loops.get_mut(&img_idx) {
            cell_loop.placed = Some(new);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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

    /// A grid of `count` clips with sharp stills: 5 columns and 4 rows of cells of 160x96 px.
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

    /// Give the cell `index` a loop of `count` half-size frames of 333 ms, each in its own
    /// color. Returns the Kitty ids of the frames.
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

    /// Place the stills of the grid, as the render of the grid does.
    fn place_stills(app: &mut App, terminal: &mut FakeTerminal) {
        let mut out = Vec::new();
        place_gallery_images_to(app, &mut out).unwrap();
        terminal.feed(&out);
    }

    /// Write the loops at `tick` of the loop clock. Returns the output and whether a loop
    /// started.
    fn play_at(app: &mut App, tick: u64) -> (String, bool) {
        let mut out = Vec::new();
        let now = app.grid_loops.instant_of(tick);
        let started = play_grid_loops_to(app, &mut out, now).unwrap();
        (text(&out), started)
    }

    /// The ids of `ids` that the terminal places.
    fn placed(terminal: &FakeTerminal, ids: &[u32]) -> Vec<u32> {
        terminal
            .placements
            .iter()
            .map(|placement| placement.id)
            .filter(|id| ids.contains(id))
            .collect()
    }

    #[test]
    fn a_loop_goes_out_once_and_each_tick_swaps_its_placement_without_pixels() {
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

    /// `clip_app` in a terminal that plays Kitty animations.
    fn animation_app(count: usize) -> App {
        let mut app = clip_app(count);
        app.graphics = KittyBackend::new(Transfer::Direct).with_loops(Loops::Animation);
        app
    }

    #[test]
    fn an_animation_goes_out_once_with_all_frames_and_then_replaces_the_still() {
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
    fn an_animation_needs_no_tick_and_the_run_loop_waits_idle() {
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
    fn an_animation_that_leaves_the_screen_deletes_its_one_image() {
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
    fn a_redraw_places_the_running_animation_again_and_a_delete_all_sends_it_again() {
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
    fn an_animation_above_the_terminal_budget_keeps_its_still() {
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
    fn the_help_popup_stops_an_animation_and_the_grid_sends_it_again_with_all_frames() {
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
    fn a_zoom_drops_the_animations_and_the_next_grid_write_deletes_all_images() {
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
    fn a_new_animation_for_the_same_cell_deletes_the_old_one_image() {
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
    fn the_run_loop_sleeps_until_the_next_frame_of_a_loop() {
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
    fn a_loop_above_the_terminal_budget_keeps_its_still() {
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
    fn a_loop_that_leaves_the_screen_deletes_its_frames_in_the_terminal() {
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

    /// The first Kitty command of each transmit in `output` (`a=T`, `a=t`, `a=f`), without its
    /// payload. A further chunk (`q=2` missing) carries no keys of the image.
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
    fn only_the_animation_path_marks_loop_frames_transient_and_a_still_never() {
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
    fn a_loop_frame_that_left_the_terminal_falls_back_to_the_still() {
        let mut app = clip_app(3);
        let mut terminal = FakeTerminal::new();
        place_stills(&mut app, &mut terminal);
        let (_, still) = app.thumb_cache.peek(1).unwrap();
        let ids = add_loop(&mut app, 1, 3);
        terminal.feed(play_at(&mut app, 0).0.as_bytes());
        assert_eq!(placed(&terminal, &ids), vec![ids[0]]);

        // The terminal drops one frame, and viewr forgets it, as after a trim.
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
    fn two_loops_due_in_one_tick_swap_in_one_update_in_grid_order() {
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
    fn a_redraw_of_the_grid_places_the_current_frame_instead_of_the_still() {
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
    fn the_help_popup_the_info_panel_and_the_fullscreen_view_stop_the_loops() {
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
    fn the_loops_play_on_while_the_delete_question_waits_in_the_status_row() {
        let mut app = clip_app(3);
        let mut terminal = FakeTerminal::new();
        place_stills(&mut app, &mut terminal);
        let ids = add_loop(&mut app, 1, 3);
        play_at(&mut app, 0);

        char_key(&mut app, 'd');
        assert!(app.pending_delete.is_some());
        assert!(app.loop_plays(1), "no marker beside a visible loop frame");
        let (tick, _) = play_at(&mut app, FRAME_TICKS);
        assert!(tick.contains(&format!("i={},c=20,r=6", ids[1])), "{tick}");
        char_key(&mut app, 'n');
        assert!(app.pending_delete.is_none());
    }

    #[test]
    fn a_zoom_drops_the_loops_and_the_next_grid_write_deletes_all_images() {
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
    fn without_animate_the_grid_shows_no_loop() {
        let mut app = clip_app(3);
        app.grid_loops.enabled = false;
        add_loop(&mut app, 1, 3);
        assert!(!app.loops_to_send());
        assert!(play_at(&mut app, 0).0.is_empty());
        assert!(!app.loop_plays(1));
    }
}
