# Grid and Kitty output

This page holds the design facts of the grid geometry, the Kitty writes and the run loop waits.
AGENTS.md holds the rules and the layout of the files.

## Grid gaps and borders

The code is in `gallery/mod.rs` and `ui/grid.rs`.

- The cells of the grid have 1-cell gaps (`GAP`).
- A line of n cells takes n + 1 gaps: one between two neighbors and one at each edge.
- `update_grid` subtracts the gaps first. It gives the remainder to the first columns and rows.
- Every thumbnail is decoded for the base cell size. So a wider column only has a larger margin.

### Padding and placement

- The decode job pads every grid picture with transparent pixels to whole cells, at most the base
  cell box.
- `pad_for_grid` in `pictures/resize/padding.rs` gives a `GridPicture` with its cell count.
- The grid places the picture centered in its cell in whole cells. It uses `c=` and `r=` of that
  count and no `X=` and `Y=`.
- The content is centered to within half a cell.
- herdr relays a pixel offset as a stretched picture. This is the reason for the whole-cell
  placement.
- A rounded cell size cannot push a padded picture into the gap.

### Borders

- The selected cell gets a thick border (`BorderType::Thick`, `STYLES.cursor_cell`). It lies in
  the gaps around the cell and its label (`GalleryState::border_rect`).
- Each marked cell gets a plain yellow border (`STYLES.marked`).
- Neighbors share the gap line.
- The selected border is drawn last, so it wins.
- A border that the grid edge cuts stays open at that edge.
- No cell has a background fill.

## Cell shape

- `App::request_cell_shape` reads the headers of the first 64 files (`SHAPE_SAMPLE`) on a thread,
  once per scan. The files are the first files by name (the sorted first batch). The folder
  prefetch samples the same files.
- JPEG and PNG swap for the EXIF orientation. HEIC and AVIF read `ispe` and swap for `irot`.
- A RAW or PSD file gives the size of its embedded JPEG after the orientation.
- PDF, SVG, the other ImageIO formats and video count as 3:2.
- A header thread that ends without a result, or that does not start, gives the default shape
  3:2.
- The median snaps to one of 7 buckets: 9:16, 2:3, 3:4, 1:1, 4:3, 3:2 and 16:9.
- The column count includes 1 gap per column plus the edge gap.
- The first screen waits for the shape (2 ms to 4 ms).
- The shape stays until the next directory switch. So the columns never move while the user
  browses.

## Kitty output

The generic Kitty output is in `pictures::kitty`: the writer, the pixels, `EMIT_PASS_BYTES`, the
temp file medium and the storage budget. `pictures/kitty/placement.rs` holds `box_px`,
`centered`, `native_placement` and `filled_placement`. `pictures/kitty/query.rs` holds the start
probe and the cell size. The grid writes stay in `kitty_output/grid.rs`. The grid icons and the
loops stay in `kitty_output/`.

### The frame engine

`pictures/kitty/screen.rs` is the frame engine of the shared picture module (phase 2 of
`docs/picture-refactor.md`). It holds `Placements`, `Screen`, `Update`, `Placing` and `Layer`.

- The app puts the places of a frame into `Placements`: `place`, `place_under` (below the cell
  grounds, `kitty-layers`), `place_over` (a preview that keeps its place while its sharp picture
  waits), `keep` (stored, without a place) and `hide` (an overlay frame places no picture,
  `kitty-overlay`).
- One `Screen::write_frame` (or `flush`, `kitty-flush`) writes one synchronized update: the
  deletes, the removed placements, the transfers up to the byte cap in the order of the frame,
  the changed placements, and the trim of the budget.
- Each place has its own placement id. An unchanged place keeps its id, and a moved place takes a
  free id of its picture. The first place of a new picture goes out as one `a=T` with `p=`.
- `Placing::Changed` (the default) sends only a new or a moved placement. `Placing::EveryFrame`
  places every picture on each flush (`with_placing`, `kitty-placing`). youtr uses it until
  phase 5 of the plan.
- `Screen` holds `TerminalImages` (`App::screen`), the ids after a clear (`maybe_stored`) and
  the pictures to send again after a focus gain (`focus_gained`, `kitty-events`).
- A started `Update` always ends its synchronized update. Its `Drop` writes `ESC 8` and
  `END_SYNC` after an early error.
- phewr calls `write_frame`, `wipe`, `record`, `forget_placements`, `stores`, `bytes_of`,
  `budget` and `waiting`. Each other part of the interface has a cargo feature (phase 4b). An app
  turns on the features that it calls. phewr turns on none, and its tests build every part.

  | Feature | It gates |
  |---|---|
  | `kitty-flush` | `Screen::flush` |
  | `kitty-events` | `Screen::resized`, `cleared`, `focus_gained`, `delete_all`, the field `Screen::size`, `TerminalImages::ids` |
  | `kitty-layers` | `Placements::place_under` |
  | `kitty-overlay` | `Placements::hide`, `Screen::unplace_all` |
  | `kitty-placing` | `Screen::with_placing` |
  | `kitty-pass-bytes` | `Screen::with_pass_bytes` |
  | `kitty-store` | `Screen::lost`, `Screen::stored_bytes` |

  `Screen::with_self_check`, `Screen::answer`, `Screen::has_lost` and `pictures::kitty::answers`
  have no feature gate, because phewr calls them.

- `Loops`, `transmit_frame_to` and `animate_to` stay in `pictures::kitty`, because the usage hint
  `N=1` depends on the probe result. The loop logic of phewr stays in `kitty_output/loops.rs`.
  It writes the frames into the update of the pass and calls `Screen::record`.
- A lost frame of a visible loop gives the still and a new transfer of the loop. Nothing calls
  `Screen::lost` (`kitty-store`) today, and the trim keeps every frame of a visible loop. So
  this path has no trigger yet. `Screen::answer` takes the same path for a lost picture of the
  self check, and `Screen::lost` sets `has_lost` too.

#### Self check

- With `Screen::with_self_check(true)`, each `a=p` goes out with `q=1`, and a transfer keeps
  `q=2`. The terminal then answers only an error, for example
  `ESC _ G i=7,p=1;ENOENT: image not found ESC \` for a picture that it does not hold. Without
  the self check, a placement is `q=2` too. phewr turns the self check on by default, and
  `PHEWR_KITTY_SELF_CHECK=0` turns it off.
- The app hands each answer to `Screen::answer`. An `ENOENT` for a stored picture forgets the
  picture and its placements, and the method returns `true`. An answer for a picture that the
  screen no longer stores, and every other error, change nothing. `Screen::has_lost` is true
  from a loss until the next `write_frame` or `flush`, so the app writes a frame soon. That
  frame sends the pixels of the lost picture again.
- `pictures/kitty/answers.rs` holds two readers. `AnswerReader::read` finds the answers in the
  input bytes, gives the other bytes back in their order and keeps a split answer for the next
  read. It ignores an `OK` and counts each whole answer (`ends`). Above `MAX_ANSWER_BYTES`
  (1024), it gives an unterminated APC back as keys, so a broken answer cannot eat the keys
  forever. A lone `ESC` at the end of a read is a key, as crossterm reads it.
- phewr reads key events, not bytes, because crossterm owns the tty read. crossterm turns an
  answer into the keys Alt+`_`, `G`, `i`, `=`, ... and Alt+`\`. `AnswerKeys::read` rebuilds the
  bytes of each event for an `AnswerReader` and holds the events while an answer can still
  come. An event that cannot belong to an answer, for example an arrow, releases the held
  events. A `Resize`, `FocusGained` or `FocusLost` event passes through alone and keeps the
  hold, because crossterm makes it from a signal or from a sequence of its own, so it can land
  between the key events of one answer (found by a youtr worker, 2026-11-11). The hold ends
  after `ANSWER_WAIT` (50 ms) of quiet input (`release_stale`, `wait`).
  The events of a started answer that never ends go, and the other held events come back, for
  example a lone Alt+`_`.
- `KeyRead::ends` counts the whole Kitty answers of an event, `OK` included. `LateAnswerFilter`
  lowers its owed count by that number, so it never swallows an answer of the self check.
- A placement that the terminal drops without an `ENOENT` stays gone until its next move. herdr
  can do that. The fallback of phase 5, step 3 (a resend when a scroll stops) is open.

### Writers and pixels

- The rule that Kitty images are not ratatui widgets is in AGENTS.md.
- `Pixels::opaque_rgb` copies a 2736x2052 frame in about 1 ms with an array-store loop. This is
  four times faster than a `copy_from_slice` of 3 bytes per pixel.
- `transmit` sends an RGBA image without a transparent pixel as RGB (`f=24`). The check and the
  copy go in chunks of 4096 pixels and stop at the first transparent chunk. So a padded grid
  thumbnail stays RGBA at almost no cost.
- The payload cap is in [Video](video.md#payload-cap).

### Emit passes

- A pass writes the new thumbnails, the previews, the loop frames and the icons in grid order, as
  one frame of the engine in one synchronized update. It stops at `EMIT_PASS_BYTES`
  (`pictures/kitty/mod.rs`). One run-loop pass writes at most one grid pass. A waiting picture or
  icon brings the next pass at once (`GridPass::waiting`).
- A redraw (the return from fullscreen or video, the close of an overlay, a page scroll) writes
  only the changed placements and no `d=a`. It has no byte cap (`Update::uncapped`) and sends
  every missing visible picture in one update, as before the engine.
- 4 MB is about 9 sharp cells of the Retina grid of the bench. The count is RGBA bytes of the
  cache, not bytes on the wire.
- An opaque picture goes out as RGB (3/4 of the count). The direct medium adds base64 (4/3 of its
  data). The temp file medium writes one file per picture.
- One write of 36 cells takes about 11 ms.
- The rest goes out in the next pass.

### Temp file medium

- The rule for the choice of the medium is in `gotchas.md` ("Kitty temp file medium").
- A terminal with another `TMPDIR` reads the file but keeps it. Then phewr leaves a file per
  frame. The file holds raw pixels: about 17 MB for an RGB Retina still, and about 22 MB for an
  RGBA frame of the player or of a grid loop.
- The file name contains `tty-graphics-protocol`, as the protocol requires.
- A terminal that answers the probe later than 300 ms sends the answer as key presses
  (`LateAnswerFilter` in `keys/mod.rs`).
- Animation frames of the grid loops and of the terminal player go out as RGBA as they are
  (`Pixels::keep_alpha`). An opaque frame is `f=32`, and the temp file is 18 % larger.
- The direct and PNG media keep the RGB conversion, and so does a grid picture.

### Terminal storage

- `TerminalImages` (`pictures/kitty/terminal_images.rs`, inside `Screen`) tracks the images that
  the terminal stores, with their bytes. It counts 4 bytes per pixel, also for a picture that went out as RGB.
  So the count is an upper bound.
- The grid deletes evicted and replaced ids with `a=d,d=I`.
- The grid trims the oldest unshown images above `TERMINAL_IMAGE_BUDGET` (128 MB). The trim
  skips the ids that the visible cells show and goes on with the next older image. A trimmed
  cell sends its picture again with `a=T`.
- Ghostty evicts unplaced images at 320 MB. herdr stores each image twice.
- When the loops run as Kitty animations (`Loops::Animation`), every video frame and every loop
  frame carries the usage hint `N=1` (transient). Ghostty 1.4.0 then evicts them before the
  stills.
- With `Loops::Ticks`, no `N` key goes out, because the protocol does not say that a terminal
  ignores an unknown key.
- Stills and the fullscreen image carry no hint.
- The in-memory cache holds `max(200, 2 * visible cells)` entries and at most
  512 MB of padded pixels (`THUMB_CACHE_LIMITS` in `app/thumbnails/mod.rs`). It never evicts a visible cell.
- The cache also holds the parked sharp thumbnails of the folders that the grid left
  (`ThumbnailCache::park`, `take_parked`). The key is the path and the decode size, with the
  mtime and the length of the file. A changed file decodes again.
- Parked pictures count against the limit and go first.
- The grid writes run in a synchronized update.

### Grid icons

- The icons are the Lucide SVGs in `assets/icons/` (ISC license, `assets/icons/LICENSE`), with
  `stroke-width` 1.5 instead of 2. `include_str!` puts them into the binary.
- The start write (`pictures/kitty/query.rs`) asks for the terminal foreground with OSC 10,
  before the Kitty probes. The
  terminal answers in order, so the read needs no end condition of its own. The query ends with
  BEL, and an answer with `ESC \` does not count as a Kitty answer.
- `icons::render` draws an icon with resvg at the pixel size of the icon box. The pixels are the
  foreground color at 45 % of the SVG alpha (`ICON_ALPHA`). One render takes 0.11 ms at 144x129
  px and 0.25 ms at 400x344 px (release build). `GridIcons` keeps at most 64 renders in memory.
- Each icon has 4 fixed image ids below the loop ids (`ICON_ID_BASE`, `SLOTS`), one per pixel
  size. The first grid rows get the spare rows, so one grid can need 2 sizes of an icon. A new
  size takes a free id, else the id of a size that no visible cell needs. The placements of
  that id go first.
- The icons go out with the thumbnails in one frame of the engine. Their placement ids count per
  icon image (1, 2, 3), not per cell. The first place of an icon is an `a=T`.
- The draw records the icons that the cells need (`GridIcons::wanted`). When only the icons
  changed and no other pass is due, `sync_grid_icons` writes one grid pass. While a picture
  waits (`pending`), it writes nothing, because the next pass writes the icons too.
- The thumbnail padding is transparent. For this reason, a cell that gets its thumbnail loses its
  icon placement in the same pass.
- `d=a` and `d=A` remove the icon placements too. The engine knows the placements, and
  `GridIcons::forget_images` follows the deletes of the images.
- If the terminal does not answer OSC 10, or phewr sends no probe, the icons stay octant blocks
  (2x4 dots per cell) on a ratatui canvas. The thumbnail that Kitty draws covers them.
- A change of the terminal theme during a session does not change the icon color.

## Run loop waits

The code is `next_timeout` in `run_loop.rs`. The rules are in `gotchas.md` ("The run loop
polls"). This is the order of the decisions:

1. In the gallery, the wait is zero while emits are pending or `App::loops_to_send` is true.
2. The base wait depends on the state:
   - While a video player runs, it is the time to the next frame (`time_until_next_frame`).
     Background results then wait for the next frame.
   - Else it is 10 ms (`BUSY_POLL`) when `App::has_pending_work` is true or emits are pending.
     A running grid loop job counts.
   - Else it is 250 ms (`IDLE_POLL`).
3. The loop then takes the minimum with each of these waits:

| Wait | Constant | Condition |
| --- | --- | --- |
| 1 ms | `SHAPE_POLL` | The first 20 ms (`SHAPE_POLL_WINDOW`) of the cell shape read. |
| 2 ms | `CACHE_READ_POLL` | A disk cache read of the grid is on its way. |
| 2 ms | `FIRST_PICTURE_POLL` | The grid decodes and shows no picture yet. |
| 100 ms | `WINDOW_POLL` | An mpv window plays. |
| time left | none | The slideshow, the folder prefetch, the next grid loop frame and the prestart. |

- A playing grid loop in the Kitty animation path gives no wait (`grid_loop_timeout` skips it).
- No event wakes the run loop for a background job. This is the reason for the polls.
