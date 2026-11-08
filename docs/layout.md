# Module map

This page holds the full module map of `src/`. `AGENTS.md` ("Layout") holds the short form. Read the page of the module in `docs/` for its design facts.

- `docs/`: the design notes by module. `video.md` holds the terminal player, the probe, the cubes,
  the animations, the grid loop frames and the mpv window. `images-and-colors.md` holds the
  decoders and the color conversion. `grid-and-kitty.md` holds the grid geometry, the Kitty
  output and the run loop waits. `app-state.md` holds the thumbnail jobs, the disk cache, the
  folder prefetch and the grid loop jobs. `docs/README.md` lists the pages.
- `main.rs`: the module list and `main`: the lookup of ffmpeg and ffprobe
  (`video::find_session_tools`), the CLI checks, the terminal
  session and the start of the run loop. `main` is a thin shell. `check_paths`,
  `apply_cli_overrides`, `probe_terminal` (the start probe and the cell size, `StartProbe`) and
  `prepare_app` hold the logic with tests.
- `cli.rs`: the clap arguments (`Cli`), the value parsers of `--threads` and `--interval`, the
  start directory and `write_cwd_file` (the folder of the grid for `--cwd-file`).
- `terminal.rs`: `TerminalSession` (raw mode, the alternate screen), the Kitty probe
  `query_terminal`, `query_cell_size_and_probe` (`CSI 16 t`, the OSC 10 foreground query, the
  Kitty probes and the DA1 query in one write, the DA1 answer ends the read) and the cell pixel
  size. `read_answer` is the poll
  loop of the tty read, over any fd.
- `run_loop.rs`: `LoopState` (the status note and, with the `video` feature, the `VideoState`),
  `run`, the background polls, `render_images` and `next_timeout`. The status note lives here, not
  in `App`. `handle_input` takes one input event. The tests are in `run_loop/tests.rs`.
- `keys/`: the key handlers. `mod.rs` holds `handle_event` and `handle_key`, the thin
  `match` that routes the overlays and the view modes. One file per view: `fullscreen.rs`,
  `gallery.rs` (with the search line), `video.rs` (feature `video`).
  `mouse.rs` holds the mouse handler per view.
- `kitty_output/`: the Kitty writes per frame. Each `render_*` has a `*_to` twin that writes to
  any `Write`, so that the tests read the escape sequences. `App::with_graphics_out` runs a write
  against the graphics output. `mod.rs` holds the fullscreen image and the video frame and
  re-exports the writes of the grid. `terminal_images.rs` holds `TerminalImages`,
  `TERMINAL_IMAGE_BUDGET`, `delete_image_to` and `trim_to_budget`. `grid.rs` holds the thumbnail
  writes (`emit_new_thumbnails_to`, `render_gallery_images`, `place_gallery_images_to`,
  `grid_placement`, `place_at`, `visible_image_ids`, `EMIT_PASS_BYTES`). `loops.rs` holds the
  grid loops (`play_grid_loops`, `transmit_animation_to`, `swap_loop_frames_to`).
  `grid_icons.rs` holds the SVG icons of the cells as Kitty images (`GridIcons`, `IconCell`,
  `sync_grid_icons`).
- `video_handoff/` (feature `video`): the start of the terminal player and of the mpv window,
  and the way back. `mod.rs` holds `VideoState` and `play_videos`. The ffprobe path lives in the
  decoder only (`Decoder::has_ffprobe`). `prestart/` holds the start of the player during a
  rest of the gallery cursor or of the fullscreen view on a Live still (`VideoPrestart` in `App::video_prestart`, `tick_prestart`, `take_prestart`,
  `prestart_timeout`). `window.rs` holds `poll_mpv_window`. `tests.rs` holds the start tests, and
  `tests/live.rs` the tests of the Live Photo motion.
- `player/` (feature `video`): the terminal player. `mod.rs` holds `Player` with its `Run` and
  `Repeat`, the pacing, `catch_up` (the restart ahead of the clock) and `end_of_run` (the repeat,
  the software fallback, the failure). `tests.rs` holds the player tests. `decoder/` holds the
  ffmpeg run: `mod.rs` the `Decoder` trait and the fallback order (`Pipeline::first`,
  `Pipeline::next`, `with_fallback`: an animation decodes in software only, also its first frame
  and its loops), `command/` the command line (one `build` with `Extras` and `Output`), and
  `process.rs` `SystemDecoder`, `FrameStream`, the frame pool and the reap. `probe/` holds
  `VideoInfo` (with `Samples` and `hardware`), the ffprobe run, the probe cache (`probe_peek` reads it without ffprobe) and `output_within`. `probe/parse/` holds the JSON
  and the pacing rates. `layout.rs` holds the frame box and the cell box. `tone_map/` holds the
  color cubes (the HDR tone mapping, the SDR primaries conversion and the cube files), and
  `tone_map/math.rs` holds the matrices, the curves and the tone map. `animation.rs` holds the
  header check for animated GIF, WebP, APNG and AVIS.
- `grid_loop.rs` (feature `video`): the frames of the grid loops: the ffmpeg runs and the frame
  choice (12 keyframes, the constant-rate run for a short clip, the 12 seeks for a long one, the
  caps of an animation).
- `app/grid_loops/`: the loops of the grid. `mod.rs` holds `GridLoops`, `LoopFrames` and the loop
  state of `App`. `cell.rs` holds `CellLoop` and the loop clock (`TICKS_PER_SECOND`,
  `START_ALIGN`, `tick_ends`). `jobs.rs` holds the jobs, their cancel flags and `ResultGuard`.
  `loop_source` gives the file of a loop: the motion of a Live Photo still, else the file itself.
- `yazi.rs`: the reveal of the last file in yazi, and the PATH lookup `find_on_path`.
- `app/`: `App`, the whole view state. `mod.rs` holds the struct, its fields, its construction
  and `box_px` (the pixel box of a rect). One file per topic: `info_panel.rs` (`view_file`, the
  file of the info panel and the status row), `grid_zoom.rs` (the cell size and the state file),
  `slideshow.rs`, `scan.rs` (the scanner results, the sort and the filter), `cell_shape.rs`,
  `fullscreen.rs` (enter, leave, the steps and the decode, `FullscreenDecode`, the image as a
  `ScreenImage`), `zoom.rs` (zoom,
  pan and the full-resolution source in `ZoomState`), `marks.rs` (the marks of the grid, the tests in `marks/tests.rs`), `directory.rs`
  (the directory switch), `folder_prefetch/` (the decode of the first screen of the folder under
  the cursor into the disk cache: the state and the rest of the cursor in `mod.rs`, the thread
  and the first screen in `worker.rs`), `mouse.rs` (the mouse state and the screen layout of the last draw),
  `meta.rs` (the facts of the file under the cursor, read on a thread), `folders.rs` (the folder
  entries of the grid, `is_parent_entry`, `Enter` into a folder, `go_to_parent`, the tests in
  `folders/tests.rs`), `video.rs` (the terminal player state, the return after a video, the mpv
  window watch), `thumbnails/` (`ThumbnailJobs` in `mod.rs`: the channel, the sharp, preview and
  cache read slots, the misses and the generation, and `ThumbImage`. `queue.rs` holds `JobQueue`
  and `JobSlots`, `padding.rs` the padding to whole cells (`pad_for_grid`, `pad_loop_frame`,
  `thumb_target_px`), `decode.rs` the work of one job (`warm_disk_cache`, the embedded preview,
  the animation flag)), `disk_cache/` (the thumbnail disk cache: `CacheEntry`, `LazyEntry` and
  `FileStamp` in `mod.rs`, the loop files in `loop_file.rs`, the temp file write and the prune in
  `write.rs`). `grid_loops/` is listed above.
- `decode/`: the decoders and the decode routes. `Route::for_path` in `mod.rs` picks the route by
  extension, `decode_image_with_hint` runs it, `foreground` marks the decode that the user waits
  for. `raster.rs` (the `image` crate, the EXIF orientation and `header_dimensions`, the size from
  the header for `info::dimensions` and `shape::shown_size`), `turbo.rs` (libjpeg-turbo with DCT
  scaling), `libheif/` (HEIC, HEIF, AVIF: `mod.rs` the decode and the core share `LibheifSlot`,
  `thumbnails.rs` the embedded thumbnails and `fitted_size` on `scaled_down_size`, `plane.rs` the
  interleaved plane), `resize.rs` (`load_and_resize`, Catmull-Rom, `Layout`,
  `rgb_resize_is_fastest` and `resize_region`, a resize of a region through a view), `screen/`
  (`ScreenImage` with `Rgb`, `Rgba` and `Opaque`, and `load_for_screen`, the fullscreen decode),
  `jxl.rs` (JPEG XL, ICC profiles through `moxcms`, the sample copy on all cores
  `render_samples`), `sips.rs` (camera RAW, PSD, PDF, SVG and
  other ImageIO formats), `tonemap.rs` (EXR and HDR to 8 bit), `hdr/` (the tone map of PQ and HLG
  stills: `mod.rs` the math of the HDR cube of the player, `primaries.rs` the primaries and the
  matrices), `heif_boxes.rs` (the safe ISOBMFF reader for `irot`, `imir`, `ispe`, `clli` and `mdcv`), `shape.rs`
  (the image dimensions from the file headers, for the cell shape), `embedded_jpeg/` (the safe
  reader for the JPEG inside camera RAW and PSD files: `mod.rs` with RAF and `decode_reference`,
  `tiff.rs`, `cr3.rs`, `orf.rs`,
  `psd.rs`), `color/` (the conversion into the color space of the terminal: `mod.rs` with
  `ColorTarget`, `SourceColor` and `convert`, `transform.rs` the transform cache, `jpeg.rs` the
  JPEG segment walk `jpeg_segments`, the APP2 ICC profile and the Adobe RGB mark, `tags.rs` the
  PNG chunks, the TIFF tag and the `cicp` tag of an ICC profile). `decode_image` returns
  `Decoded` (the pixels and their `SourceColor`). `Decoded::fit_within` resizes, then converts.
  `decode_image_with_hint` converts at the decoded size. A module directory keeps its long test
  module in `tests.rs`.
- `ui/`: the ratatui drawing of each view, the help popup and the info panel. `mod.rs` holds
  `draw`, `split_info`, `content_area`, `status_area`, the info panel, the fullscreen and the video
  view. `status.rs` holds the status row of each view (`draw_status_row`, `error_line`,
  `gallery_status_line`, `fullscreen_status_line`, `video_status_line`, `is_empty_folder`).
  `grid.rs` holds the grid cells: the borders, the labels (`label_text`, the column width through
  `unicode-width`) and the icons (`draw_cell_icon`). `help.rs` holds the help popup: two columns
  from 100 terminal columns on (`HELP_TWO_COLUMN_WIDTH`, the rows in `HELP_GALLERY_ROWS` and
  `HELP_FULLSCREEN_ROWS`), else one column that clips. The gallery, the fullscreen view and the
  video view have no frame. They use the full area minus 1 status row (`content_area`,
  `status_area`). The help popup and the info panel keep their borders.
- `gallery/`: `mod.rs` holds the grid geometry, the cursor and the two geometry helpers `box_px`
  (the pixel box of a rect) and `centered` (a box centered in a rect). `thumbnail_cache.rs` holds
  the in-memory thumbnail cache (`ThumbnailCache`, `GridPicture`, `ParkKey`) with the parked
  pictures.
  `update_grid` gives the remainder of the width and the height to the first columns and rows,
  so the grid reaches the edges. `set_cell_shape` picks the column count from the photo aspect
  and the pixel size of a terminal cell.
- `scanner/`: the background directory scan (`mod.rs`) and the lists of supported extensions
  (`extensions.rs`). `LIBHEIF_EXTENSIONS` is the one list for the libheif route. `has_extension(path, list)` is the
  extension test of the scanner and the decoders, without an allocation. `name_key` is the sort
  key of the batches and the folders (the lowercase name, then the path). `read_folder` is the one
  `read_dir` of a folder argument for its folders and its files. `folder_listing` gives the
  subfolders (`..` first) and the sorted files of one folder from one read, without a thread and
  without the errors, for the folder prefetch. `subfolders` and `files_of` are its test-only
  halves. `live/` holds the Live Photo pairs: `pair` takes the motion `.mov` of each HEIC or
  JPEG still out of the sorted files of one folder (the 3 names before each `.mov`, HEIF before
  JPEG), `LivePhotos` is the side table still -> motion, and `live/identifier.rs` (feature
  `video`) reads the content identifiers of a pair.
- `image_list.rs`: `SharedImageList`, the list that the scanner fills and the UI reads.
  `EntryKind`, `FolderEntry` and the folder prefix (`push_folders`, `folder_count`).
  `batch_since` gives the new entries and the list length from one lock, so the scan poll loses
  no batch.
- `prefetch/`: decodes the neighbors of the fullscreen image ahead of time (`mod.rs`, the tests in
  `tests.rs`). A job sends a
  `JobResult` with its generation. `invalidate()` drops the jobs in flight and forgets the failed
  indices. Each result (`Prefetched`) keeps its decode box. `take_resized` fits the image into
  the requested box (`Prefetched::fit_within`). If the decode box is smaller, it returns `None`.
  `Prefetcher::running` counts the jobs on the rayon pool, also the jobs of an old generation, and
  `MAX_PREFETCH_JOBS` limits that count. The decode is a field (`DecodeFn`), so a test can block it.
- `search.rs`: fuzzy filename search (nucleo-matcher). `FilterWorker` (`App::filter_worker`) is one
  long-lived filter thread with one `Matcher`. It takes the queries through a channel, holds the
  names as an `Arc`, and runs only the newest query.
- `info.rs`: file data and EXIF for the info panel, and the EXIF orientation.
- `file_meta.rs`: the facts of one file for the status row (`FileMeta`, `describe`).
- `icons.rs`: the large icons of the grid cells: `render` (the Lucide SVGs of `assets/icons/`
  through resvg), the octant fallback and the play stamp of the stills.
- `styles.rs`: `STYLES`, one style per role on the screen.
- `config/`: `mod.rs` holds `config.toml`. `Paths` (`from_env`, `from_vars`) holds the two file
  paths, `load(&Paths)` reads the config, `load_at` takes the paths for the tests. `state.rs`
  holds the state file: the zoom table, the writer thread (`StateWriter`, `StateChange`) and the
  save through a temp file and a rename. `save_visible_rows` and `clear_folder_zoom` are
  test-only.
- `video/` (feature `video`, `mod.rs` and `tests.rs`): `Tools`, the session lookup of ffmpeg and ffprobe, `cube_dirs` (the
  places of the cube files) and `session_decoder` (ffmpeg with a decoder that has ffprobe), the
  first-frame thumbnails and the video extensions.
- `mpv.rs` (feature `video`): the window arguments for mpv, the exit notes, the step exit codes
  and the `Runner` trait for the process start.
- `encoder/`: the Kitty graphics protocol (`KittyBackend` in `kitty/mod.rs`, the types in
  `mod.rs`). `kitty/probe.rs` holds the start probes (`detect_backend`, `chosen_loops`,
  `describe`, `describe_loops`, `is_ssh`).
  `Transfer` picks the medium: direct,
  PNG (SSH) or a temp file (`t=t`). `Loops` picks the path of the grid loops: Kitty animations or
  the ticks. `detect_backend` probes the terminal at start with `a=q` and with one animation
  frame, and `chosen_loops` reads `PHEWR_KITTY_LOOPS` and the config key.
  `PHEWR_KITTY_TRANSFER=direct|png|file` skips the transfer probe. `Pixels` is a view of RGB or
  RGBA pixels. `transmit` sends an RGBA image without a transparent pixel as RGB (`f=24`) through
  every medium. The check and the copy go in chunks of 4096 pixels and stop at the first
  transparent chunk, so a padded grid thumbnail stays RGBA at almost no cost. `KittyBackend` has
  `transmit` and `transmit_frame`. The writers without state are free functions of
  `encoder::kitty` (`delete_all_to`, `delete_image_to`, `place_by_id_to`, `animate_to`), and `apc`
  writes each command frame (`ESC _ G ... ESC \`).
- `test_support/` (test only): the helpers that the tests of more than one module share.
  `mod.rs` holds the app builders and the waits, `input.rs` the key and mouse helpers, `files.rs`
  the JPEG, TIFF RAW, CR3, RAF and PSD writers (feature `sips` on macOS), `heif.rs` the HEIF
  writers (feature `heic`, `write_heif_sized` with a thumbnail of any size), `kitty.rs` the fake terminal and `video.rs` `FakeRunner`,
  `FakeDecoder` and the app builders of the video tests (feature `video`), `live.rs` (feature
  `video`) the synthetic Live Photo pairs, `scanned_app` and `real_live_dir`.
- `bench/` (test only): the ignored speed benchmarks, one file per group. A bench path is
  `bench::<file>::<name>`. They drive the real loop steps without a terminal and read
  `PHEWR_TEST_DIR`. `mod.rs` holds the shared driver (`Bench`, `settle`, `measure_grid`).
  `start::first_grid_picture` measures the process start and the steps of `main`,
  `cache::formats` the cache formats, `live::scan_cost` the scan with and without the
  Live Photo pairing, `live::grid_animation` and `live::grid_ticks` the cold grid of a copy of
  `PHEWR_TEST_LIVE_DIR` until every Live cell loops, `layout_bench::layouts_against_rgba` the pixel layouts of
  the fullscreen path (the A/B table of the RGB and RGBA resizes). `folders::grid_during_prefetch`
  runs the cold grid while a folder prefetch of `PHEWR_TEST_PREFETCH_DIR` runs. `measure_grid`
  stops the clock at the end of the pass. Each bench lets its background work end before the next
  one starts (the `Drop` of `Bench`, `settle`), so the numbers of the full `just bench` match a
  run of one bench. `docs/benchmarks.md` holds what each bench leaves out.
