# Benchmarks

The benches live in `src/bench/`, one file per group. A bench path is `bench::<file>::<name>`.
The benches use the picture code through `crate::pictures`.
`just bench [DIR] [FILTER]` runs them on `~/Pictures/phewr-test/01-mixed-big` by default. The
filter `bench::` finds all 26 benches. `just bench-live` runs only the group `bench::live`.

## The shared driver (`bench/mod.rs`)

- The benches include the wait of the run loop, because a sleep replaces `event::poll`.
- `measure_grid` leaves out the scan of the Kitty output. That scan takes milliseconds for a pass
  of 4 MB.
- `run_until` returns the longest pass. That is the time that the UI thread blocked.
- A cell counts as shown when its image is in the cache and the loop wrote the pending emits.
- `scanned_app` removes the videos. Then `next` never starts a player.
- `keep_files_only` removes the folders, the videos and the animations, because these open in
  the player.
- `Bench::drop` waits for the decodes. Then the cold numbers of the full `just bench` match a run
  of one bench.
- `grid_rect` and `fullscreen_rect` give the same box: the terminal above the status row.

## Start (`bench/start.rs`)

- `start_once` uses a `TestBackend`. The numbers do not hold the round trips of the cell size
  query and the Kitty probe. The probe time is the time of its temp file only.
- The process start runs the test binary. It links the same libraries as phewr.

## Cache formats (`bench/cache.rs`)

- PNG is the format of cache v10 and older. QOI is the format from v11 on.
- Raw RGBA is the lower bound of the read, at 2.3 times the bytes of QOI.
- LZ4 (`lz4_flex` 0.14, not a dependency) took 0.3 ms to 0.5 ms for the read and the decode, at
  76 % of the pixel bytes.

## Embedded previews (`bench/previews.rs`)

- Synthetic files stand in for a camera: a 6000x4000 preview in a TIFF and a 1620x1080 preview
  in a CR3. The folder adds its first file of each RAW or PSD extension.
- The bench measures the find, the decode for a grid cell and the decode for the fullscreen box.
  It compares each with sips.
- `raw_files` measures every RAW file of `PHEWR_TEST_RAW_DIR` with the real thumb box. It skips
  hidden files and the files that the scanner does not list.

## Decoders (`bench/decoders.rs`)

- `color_conversion` uses the first HEIC, JPEG and PNG of the folder. Each time is the median of
  5 runs.

## Folders (`bench/folders.rs`)

- `shape_from_headers` reads the first 64 files, as `App::request_cell_shape` does, and one file
  per extension.
- The shape bench measures the wait while the cold decodes of the old folder run. The first
  screen of the new folder waits for this read.
- `tree_with_subfolder` links the files, because the scan follows a linked file and the cache
  key takes the canonical path.
- Only `rest_until_prefetched` ticks the folder prefetch. No other bench starts one.
- `grid_during_prefetch` runs the prefetch in a second `App`. That prefetch never pauses for the
  grid, so the bench measures the worst case.
- Compare variants in one session, one after the other. The load of other builds moves the cold
  grid by up to 600 ms between sessions.

## Fullscreen (`bench/fullscreen.rs`)

- Each pan step builds a new crop. The write converts each crop once.

## Video (`bench/video.rs`)

- During the rest, the first-frame bench starts the player ahead of Enter, at the first pass
  after `PRESTART_DELAY`. The probe is in the cache, as after the thumbnail job.
- After the first frame, the clip plays for 3 s, for the frame rate and the dropped frames. Then
  the bench reports the pass that starts the player and the pass that drops it on a cursor move.
- `Passes::RunLoop` is the run loop of an idle grid: the background polls (the status row read of
  the file under the cursor) and `next_timeout`, 250 ms when nothing runs. The other case is a
  pass every 10 ms, as a wake-up at the end of the delay gives.
- `rest_on_the_first_video` draws no grid. Then no thumbnail job and no loop job runs, and the
  bench measures the warm grid case. `cache` replaces the cache directory, so the start writes
  its cube.

## Pixel layouts (`bench/layout_bench.rs`)

- Each step runs both layouts in turn. Then machine load hits both in the same way.

## Live Photos (`bench/live.rs`)

- `scan_cost` scans a folder of 10 000 empty files and a folder of 3334 triples (HEIC, MOV,
  `x.JPG`), each with `live_photos` on and off, and the real folder of `PHEWR_TEST_LIVE_DIR`. It
  reports the median of 11 scans for the first entry and for the end of the scan.
- The scan bench leaves out the file contents (the files are empty), the page cache of a cold
  start, the header read for the cell shape and the identifier read.
- With pairs on, the scan lists 6669 entries, not 10 002. So the share of the first entry is the
  number for the 5 % limit of the design. The share of the end of the scan is lower.
- `grid_animation` and `grid_ticks` measure the cold grid of a copy of `PHEWR_TEST_LIVE_DIR` with
  the real ffmpeg, until every visible Live cell plays its loop. The copy keeps the probe cache
  cold. They leave out the identifier read and the playback of the motion.
- Each bench prints the load average of the minute. Run them only below a load of 4.
- Result of 2026-11-04 (5 runs, load 3.02 to 3.41): the pairing takes 0.17 ms for 3334 pairs,
  3.2 % of the first entry, and about 0 ms without pairs. Every Live cell of the real folder
  loops after 1435 ms with Kitty animations and after 1595 ms with ticks.
- Result of 2026-11-09, after the 10 ms poll for loop jobs (medians of 5 runs, load 7 to 18): the
  first loop comes after 1016 ms (before: 1168 ms). Every Live cell loops after 1354 ms with Kitty
  animations (before: 1425 ms) and after 1365 ms with ticks (before: 1592 ms).

## Test helpers (`src/test_support/`, `src/pictures/test_support/`)

- The animated fixtures (`anim.gif`, `anim.webp`, `anim.png`, `anim.avif`) hold 3 frames of
  16x12 pixels. The stills hold one frame. `jxl/` holds the color charts of `jxl/make.sh`.
- The benches use the camera size 1620x1080 for `write_cr3_with_preview`, and
  `write_tiff_raw_with_preview` for a full-size preview. Both writers are in
  `pictures/test_support/files.rs`.
