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
- `pad_for_grid` in `app/thumbnails/padding.rs` gives a `GridPicture` with its cell count.
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
- The median snaps to one of 7 buckets: 9:16, 2:3, 3:4, 1:1, 4:3, 3:2 and 16:9.
- The column count includes 1 gap per column plus the edge gap.
- The first screen waits for the shape (2 ms to 4 ms).
- The shape stays until the next directory switch. So the columns never move while the user
  browses.

## Kitty output

### Writers and pixels

- The rule that Kitty images are not ratatui widgets is in AGENTS.md.
- `Pixels::opaque_rgb` copies a 2736x2052 frame in about 1 ms with an array-store loop. This is
  four times faster than a `copy_from_slice` of 3 bytes per pixel.
- `transmit` sends an RGBA image without a transparent pixel as RGB (`f=24`). The check and the
  copy go in chunks of 4096 pixels and stop at the first transparent chunk. So a padded grid
  thumbnail stays RGBA at almost no cost.
- The payload cap is in [Video](video.md#payload-cap).

### Emit passes

- A pass writes the new thumbnails in grid order, in one synchronized update. It stops at
  `EMIT_PASS_BYTES` (`kitty_output/grid.rs`).
- 4 MB is about 9 sharp cells of the Retina grid of the bench. The count is RGBA bytes of the
  cache, not bytes on the wire.
- An opaque picture goes out as RGB (3/4 of the count). The direct medium adds base64 (4/3 of its
  data). The temp file medium writes one file per picture.
- One write of 36 cells takes about 11 ms.
- The rest goes out in the next pass.

### Temp file medium

- The rule for the choice of the medium is in AGENTS.md (gotcha "Kitty temp file medium").
- A terminal with another `TMPDIR` reads the file but keeps it. Then viewr leaves a file per
  frame. A fullscreen frame is about 17 MB as RGB and about 22 MB as RGBA.
- The file name contains `tty-graphics-protocol`, as the protocol requires.
- A terminal that answers the probe later than 300 ms sends the answer as key presses
  (`LateAnswerFilter` in `keys/mod.rs`).
- Animation frames of the grid loops and of the terminal player go out as RGBA as they are
  (`Pixels::keep_alpha`). An opaque frame is `f=32`, and the temp file is 18 % larger.
- The direct and PNG media keep the RGB conversion, and so does a grid picture.

### Terminal storage

- `TerminalImages` (`kitty_output/terminal_images.rs`) tracks the images that the terminal
  stores, with their bytes. It counts 4 bytes per pixel, also for a picture that went out as RGB.
  So the count is an upper bound.
- The grid deletes evicted and replaced ids with `a=d,d=I`.
- The grid trims the oldest unshown images above `TERMINAL_IMAGE_BUDGET` (128 MB). A trimmed
  cell sends its picture again with `a=T`.
- Ghostty evicts unplaced images at 320 MB. herdr stores each image twice.
- When the loops run as Kitty animations (`Loops::Animation`), every video frame and every loop
  frame carries the usage hint `N=1` (transient). Ghostty 1.4.0 then evicts them before the
  stills.
- With `Loops::Ticks`, no `N` key goes out, because the protocol does not say that a terminal
  ignores an unknown key.
- Stills and the fullscreen image carry no hint.
- The in-memory cache holds `max(200, 2 * visible cells)` entries and at most
  `THUMB_CACHE_BYTE_LIMIT` (512 MB) of padded pixels. It never evicts a visible cell.
- The cache also holds the parked sharp thumbnails of the folders that the grid left
  (`ThumbnailCache::park`, `take_parked`). The key is the path and the decode size, with the
  mtime and the length of the file. A changed file decodes again.
- Parked pictures count against the limit and go first.
- The grid writes run in a synchronized update.

## Run loop waits

The code is `next_timeout` in `run_loop.rs`. The rules are in AGENTS.md (gotcha "The run loop
polls"). This is the order of the decisions:

1. In the gallery, the wait is zero while emits are pending or `App::loops_to_send` is true.
2. The base wait depends on the state:
   - While a video player runs, it is the time to the next frame (`time_until_next_frame`).
   - Else it is 10 ms (`BUSY_POLL`) when `App::has_pending_work_but_loops` is true or emits are
     pending. A running loop job alone gives no 10 ms poll.
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
