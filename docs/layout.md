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
  `apply_cli_overrides`, the start query (`tuilibr::terminal::start::query`, `Start`, `query_with`
  in the tests), `delete_pictures`, `install_panic_exit` and `prepare_app` hold the logic with
  tests.
- `cli.rs`: the clap arguments (`Cli`), the value parsers of `--threads` and `--interval`, the
  start directory and `write_cwd_file` (the folder of the grid for `--cwd-file`).
- The terminal session (raw mode, the alternate screen, the mouse modes), the start query and the
  late answer filter are `tuilibr::terminal` since 2026-11-19 (`Session`, `Modes`,
  `enable_mouse`, `disable_mouse`, `release_terminal_to`, `install_panic_hook`, `start::query`,
  `answers::LateAnswerFilter`). phewr has no `terminal.rs`.
- `run_loop/mod.rs`: `LoopState` (the status note, `answers` (the `LateAnswerFilter` of tuilibr),
  `cell_size_from_terminal` and, with the `video` feature, the `VideoState`), `run`, the
  background polls, `render_images` and `next_timeout`. The status note lives here, not in `App`.
  `handle_input` takes one input event: the filter returns `Filtered { answers, events }`, each
  answer goes to `Screen::answer`, then `handle_events` calls `handle_app_event` for the events (the status note, the cell size after a resize, `handle_event`).
  `release_held` ends a hold of key events after `ANSWER_WAIT`, and the poll of `run` wakes for
  it. The tests are in `run_loop/tests.rs`.
- `keys/`: the key handlers. `mod.rs` holds `handle_event` and `handle_key`, the thin
  `match` that routes the overlays and the view modes. One file per view: `fullscreen.rs`,
  `gallery.rs` (with the search line), `video.rs` (feature `video`).
  `mouse/` holds the mouse handler per view.
- `kitty_output/`: the Kitty writes of phewr per frame (the generic part is `tuilibr::pictures::kitty`). Each `render_*` has a `*_to` twin that writes to
  any `Write`, so that the tests read the escape sequences. `App::with_graphics_out` runs a write
  against the graphics output. `mod.rs` holds the fullscreen image and the video frame and
  re-exports the writes of the grid. `grid/` holds the grid pass (`write_grid_to`, `GridPass`,
  `grid_frame`) and the thumbnail writes (`emit_new_thumbnails_to`, `render_gallery_images`,
  `place_gallery_images_to`, `grid_placement`, `visible_image_ids`). `loops/` holds the grid
  loops (`play_grid_loops`, `send_grid_loops_to`, `place_loop_frame`, `transmit_animation_to`).
  `grid_icons/` holds the SVG icons of the cells as Kitty images (`GridIcons`, `IconCell`,
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
- `app/`: `App`, the whole view state. `mod.rs` holds the struct, its fields and its
  construction. One file per topic: `info_panel.rs` (`view_file`, the
  file of the info panel and the status row), `grid_zoom.rs` (the cell size and the state file),
  `slideshow.rs`, `scan.rs` (the scanner results, the sort and the filter), `cell_shape.rs`,
  `fullscreen.rs` (enter, leave, the steps and the decode, `FullscreenDecode`, the image as a
  `ScreenImage`), `zoom.rs` (zoom,
  pan and the full-resolution source in `ZoomState`), `marks/` (the marks of the grid, the tests in `marks/tests.rs`), `directory.rs`
  (the directory switch), `folder_prefetch/` (the decode of the first screen of the folder under
  the cursor into the disk cache: the state and the rest of the cursor in `mod.rs`, the thread
  and the first screen in `worker.rs`), `mouse.rs` (the mouse state and the screen layout of the last draw),
  `meta.rs` (the facts of the file under the cursor, read on a thread), `folders/` (the folder
  entries of the grid, `is_parent_entry`, `Enter` into a folder, `go_to_parent`, the tests in
  `folders/tests.rs`), `video.rs` (the terminal player state, the return after a video, the mpv
  window watch), `thumbnails/` (`ThumbnailJobs` in `mod.rs`: the channel, the sharp, preview and
  cache read slots, the misses and the generation, and `ThumbImage`. `mod.rs` also holds the job
  bounds `MAX_THUMB_JOBS`, `MAX_PREVIEW_JOBS` and `MAX_CACHE_READS`, and `THUMB_CACHE_LIMITS`.
  `decode.rs` holds the work of one job (`warm_disk_cache`, the embedded preview, the animation
  flag)), `disk_cache/` (`default_disk_cache_dir` in `mod.rs`, the cache folder of phewr, and the
  loop files in `loop_file.rs`). `grid_loops/` is listed above.
- `tuilibr::pictures`: the shared picture module (decoders, resize, shape, jobs, the Kitty layer,
  the test builders). It lives in tuilibr since 2026-11-19. `tuilibr/docs/pictures.md` holds its module tree. This page lists only the files of phewr.
- `ui/`: the ratatui drawing of each view, the help popup and the info pane. `mod.rs` holds
  `draw`, `panes` (`Panes { info, main, status, omnibox }`, the one source of the areas for the draw, the
  mouse and the run loop: `frame::split_boxed` and `pane_box` in the maximal layout, `frame::split`
  and `divider` in the minimal one, `dim_pane` on the info pane), `content_area`, `status_area`, the
  info pane, the fullscreen and the video view. `omnibox/` fills the `OmniboxView` of the frame (the
  search line with the `TextField` of `GalleryState::search_query`, the error, the note). The run
  loop draws in `draw_pass` (the draw, `render_images`, the second draw, the cursor reset). `App::pane_layout` holds the layout of the session,
  `Config::layout` and `Config::layout_note` the config value and its start note. `status/` fills the `BarView` of each view for `tuilibr::frame::draw_bar`
  (`draw_status_row`, `BarParts`, `gallery_bar`, `fullscreen_bar`, `video_bar`, `folder_span`,
  `shorten_path`, `display_path`, `is_empty_folder`).
  `grid/` holds the grid cells: the borders, the folder names (`folder_name_text`, the column width
  through `unicode-width`) and the icons (`draw_cell_icon`). `help.rs` fills the `HelpView` for
  `tuilibr::frame::draw_help` (`GALLERY_COLUMN`, `FULLSCREEN_SECTIONS`, `kitty_rows`,
  `draw_help_popup`): the sections Gallery, Search, Fullscreen / video player, Mouse and Kitty
  graphics (the transfer medium and the loop path), two columns from 102 terminal columns on,
  else one column. The frame draws the box with rounded corners, the padding and the title. The gallery, the fullscreen view and the
  video view have no frame. They use the full area minus 1 status row (`content_area`,
  `status_area`). The help popup and the info panel keep their borders.
- `gallery/`: `mod.rs` holds the grid geometry and the cursor.
  `update_grid` gives the remainder of the width and the height to the first columns and rows,
  so the grid reaches the edges. `set_cell_shape` picks the column count from the photo aspect
  and the pixel size of a terminal cell.
- `scanner/`: the background directory scan (`mod.rs`) and the test of supported files
  (`extensions.rs`: `VIDEO_EXTENSIONS` and `is_supported`, on the lists of
  `tuilibr::pictures::decode::extensions`). `name_key` is the sort
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
- `test_support/` (test only): the helpers that the tests of more than one module share.
  `mod.rs` holds the app builders and the waits and re-exports `tuilibr::pictures::test_support` (feature `test-support`) and holds `module_fixture`.
  `input.rs` holds the key and mouse helpers, `kitty.rs` the fake terminal (`FakeTerminal`, with
  the `ENOENT` answers of the self check in `answers` and `lose`) and `video.rs`
  `FakeRunner`,
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
