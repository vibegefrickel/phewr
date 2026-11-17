# AGENTS.md

This file is the entry point for each person and coding agent that works on phewr. Read it
first. Then read `TODO.md` for the open work. The pages in `docs/` hold the design facts of the
modules. Read the page of the module that you change.

## What this is

phewr is a terminal image and video gallery for the Kitty graphics protocol, written in Rust.
It is a fork of [rview](https://github.com/jrf/rview) by jrf (MIT).

- The main goal: phewr is a fast viewer for all common image and video formats in the terminal,
  with full mouse support. Each change makes opening, scrolling or switching files faster, or it
  adds a common format. `TODO.md` holds the full goal and its scope.
- phewr is a viewer only. It does not edit, convert, write, move, rename or delete files. Do not
  add file operations.
- The target is macOS on Apple Silicon, in Ghostty. phewr uses the Kitty graphics protocol, not the
  Kitty terminal. Nobody tests in the Kitty terminal.
- The maintainer is the only user today. Write user-facing texts for other people anyway.
- phewr has no release. The first release comes in Q1 2027 at the earliest, and the maintainer
  plans no Homebrew release. Users clone the repository and build phewr with `just install`,
  and the dotfiles install does the same. There is no CI.

The stack: ratatui 0.30, crossterm 0.29, clap, image 0.25, fast_image_resize, rayon, lru,
serde/toml, jxl-oxide and jxl-render (both with exact pins), moxcms, resvg (without default
features, for the grid icons). The features `turbo` (libjpeg-turbo) and `heic` (libheif) link
native Homebrew libraries. The feature `jxl` makes jxl-oxide and jxl-render optional. The feature
`video` links no library: it starts `ffmpeg`, `ffprobe` and `mpv` from PATH when they exist, and
reads ffprobe's JSON with serde_json. The feature `sips` runs the macOS tool `/usr/bin/sips`. All
five features are on by default. Seven `kitty-*` features gate the parts of the frame engine that
phewr does not call. They are off by default.

## Build, test, run

```sh
just                 # = just check: fmt --check, clippy -D warnings (4 feature sets), check per feature, nextest, test-lean, rustdoc, cargo deny
just fix             # clippy --fix + cargo fmt
just test [FILTER]   # cargo nextest (falls back to cargo test)
just test-lean [FILTER]  # the tests without heic, jxl and turbo (the lean set: video,sips), with the kitty-* features
just test-real [FILTER]  # also the ignored tests that decode real files (PHEWR_TEST_*)
just coverage        # line coverage summary (cargo-llvm-cov)
just coverage-html   # line coverage as HTML in target/llvm-cov/html
just bench-live      # only the Live Photo benches (bench::live), on PHEWR_TEST_LIVE_DIR
just bench [DIR] [FILTER]  # the speed measurements of src/bench/ on PHEWR_TEST_DIR (default ~/Pictures/phewr-test/01-mixed-big), FILTER selects a bench (default bench::)
just build | run | install | uninstall
just hooks           # install the pre-commit hook (fmt + clippy). `git commit -n` skips it once
just fmt | lint | doc | deny | outdated   # the single steps
just pictures-sync TARGET   # copy src/pictures/ and the module fixtures to TARGET, write both SOURCE files, fmt + clippy
just pictures-check TARGET  # fail when the copy or its fixtures in TARGET drifted from the master copy
```

Plain cargo works too. Homebrew's rustup is keg-only, so run
`export PATH="/opt/homebrew/opt/rustup/bin:$PATH"` first. The justfile does this itself.
turbojpeg-sys finds the Homebrew libjpeg-turbo through pkgconf (feature `pkg-config`, no
`cmake`), so a plain `cargo build` needs no `TURBOJPEG_*` variables.

The repository pins the toolchain, the formatter and the lints:

- `rust-toolchain.toml`: stable, with rustfmt, clippy, rust-analyzer, rust-src and llvm-tools.
  Edition 2024.
- `rustfmt.toml`: 2024 style, stable options only.
- `Cargo.toml`, the `[lints]` tables: clippy `pedantic` plus restriction lints. In
  `just check`, every warning is an error.
- `clippy.toml`: the thresholds.
- `deny.toml`: licenses, advisories, duplicate versions, git sources.
- `rust-analyzer.toml`: clippy as the checker of the editor, its own target directory.
- `.config/nextest.toml`: the timeout for a test that hangs.
- `.editorconfig`.

Files at run time:

- Config: `~/.config/phewr/config.toml`, or `$XDG_CONFIG_HOME/phewr/config.toml`. The sections
  are `[gallery]`, `[slideshow]`, `[info]`, `[video]` and `[terminal]`.
- State (the remembered zoom levels): `~/.local/state/phewr/state.toml`.
- Thumbnail cache: `~/Library/Caches/phewr/`, QOI files and the `.loop` files of the grid loops,
  pruned at 512 MB.

## Rules

### Code style

- **Functions stay small.** The clippy limit is 150 lines and fewer than 7 arguments. A key
  handler is a thin `match` that calls one function per view mode (`handle_key` in `keys/mod.rs`).
- **Pure code motion stays pure.** A refactor does not change behavior. Features come
  separately.
- **Idiomatic, short Rust.** Use `let else`, `is_some_and`, iterators and small functions. Give
  each new function a one-line doc comment that says why, not what.
- **Lints are configuration, not attributes.** The lint set is in `Cargo.toml`. A local
  exception is `#[expect(lint, reason = "...")]`, never a bare `#[allow]`. If the exception is no
  longer necessary, `expect` fails the build.
- **No `unwrap` or `expect` outside tests.** clippy enforces this. A `Mutex` lock uses
  `unwrap_or_else(PoisonError::into_inner)`.
- **A dropped `Result` is a decision.** Each `let _ = ...` carries an
  `#[expect(clippy::let_underscore_must_use, reason = "...")]` that says why the error does not
  matter.
- **No output on stdout or stderr while the TUI runs.** A stray `eprintln!` breaks the screen.
  Only the CLI checks before the TUI starts print, behind an `#[expect(clippy::print_stderr)]`.
- **No `unsafe`.** The crate denies `unsafe_code`.
- **Each feature set builds.** `just lint` runs clippy with the default features, with the
  default features and the seven `kitty-*` features, with `--no-default-features` and with the
  lean set (`video,sips`). Then it runs `cargo check --all-targets` with `-D warnings` for each
  of `turbo`, `heic`, `jxl`, `video` and `sips` alone. It also runs that check for each `kitty-*`
  feature alone on top of the default features. Code behind `#[cfg(feature = "...")]` must
  compile in all of them.
- **Terminal colors only.** The TUI uses the terminal default colors and the 16 ANSI colors from
  `styles.rs`. Do not add RGB values or 256-color indexes. The grid icons use the foreground
  that the terminal reports (OSC 10).
- **One formatter, one style.** Use rustfmt with `rustfmt.toml`. The hook runs
  `cargo fmt --check`.
- **`src/pictures/` is the master copy of the shared picture module.** Change the module only
  here. The module imports no app module of phewr. It takes the app name from `CARGO_PKG_NAME`
  (`pictures::names`). The module holds no lint level for the whole module. Each project sets
  that lint level on its `mod pictures;` line (`main.rs`). An app turns on the `kitty-*`
  features that it calls. If an app leaves an item of "Dead items for an app"
  (`docs/picture-refactor.md`) without a caller, the app keeps `expect(dead_code)` on its
  `mod pictures;` line. A sync into another project needs the go of the session of that project.
  `docs/picture-refactor.md` holds the plan.
- **Language:** code, comments and docs are in English.

### Tests

`docs/testing.md` holds the long form: every `PHEWR_TEST_*` variable, every fixture folder and
its `make.sh`, and the real-file tests by name.

- Unit tests sit in a `#[cfg(test)] mod tests` at the end of each module. A long test module
  moves into `tests.rs` of the module directory (`pictures/decode/jxl.rs` and
  `pictures/decode/jxl/tests.rs`).
- A test that needs a real file is `#[ignore = "..."]` and reads a `PHEWR_TEST_*` variable.
  `just test-real` runs these tests. A test without its variable skips with a note.
  `just test-real` skips the benches (`--skip bench::`), because they set the global color target
  of the process.
- The fixtures live in `tests/fixtures/` (animations, `color/`, `png/`, `jxl/`, `hdr/`,
  `video/`). Each folder has a `make.sh` that rebuilds it. `test_support::fixture(name)` finds a
  file.
- A real ffmpeg playback test uses a virtual clock that jumps to each due frame. Its wall-time
  twin is `#[ignore]`.
- The `sips` tests run `/usr/bin/sips`. The synthetic libheif tests encode with the aom and x265
  plugins of Homebrew's libheif. If a plugin is missing, `just check` fails.
- `cargo nextest run --no-default-features` passes too. `just test-lean` runs every test with the
  lean set (`video,sips`) and the `kitty-*` features, and `just check` runs it.
- A test that writes files uses `tempfile::tempdir()` or `tempfile::NamedTempFile`.
- The helpers that the tests of more than one module share live in `test_support/`. The fake
  Kitty terminal is `test_support::kitty`.
- A test app writes the Kitty sequences to a buffer (`GraphicsOutput::Buffer`). Read them with
  `app.graphics_out.take_bytes()`. A test that needs a config uses `App::new_with_config`, never
  `App::with_config`.
- A test never writes to the real state file or the real thumbnail cache. The test-only
  `App::new` points `app.state_file` and `app.disk_cache_dir` below `/dev/null`.
- A test never sets `HOME` or `XDG_*`. Use the functions that take a path: `config::load_at`,
  `Paths::from_vars`, `config::save_visible_rows(path, ..)`.
- A test waits for background work in a loop with a limit (for example 2 s in 10 ms steps), and
  never depends on the clock within a frame interval.
- Tests never start a real ffmpeg, ffprobe or mpv outside the ignored real-file tests. Video
  tests use `FakeDecoder` and `FakeRunner` (`test_support/video.rs`).

## Layout

`docs/layout.md` holds the full module map with the main types and functions of each file. The
short form:

- `main.rs`: the module list and a thin `main`. `check_paths`, `apply_cli_overrides`,
  `probe_terminal` and `prepare_app` hold the logic with tests.
- `cli.rs`: the clap arguments, the start paths and `write_cwd_file` (`--cwd-file`).
- `terminal.rs`: `TerminalSession` and the mouse modes. The start probe and `read_answer` are in
  `pictures/kitty/query.rs`.
- `run_loop.rs`: `LoopState` (the status note, the `VideoState`), `run`, the background polls,
  `render_images` and `next_timeout`.
- `keys/`: the key handlers. `mod.rs` routes the overlays and the view modes and holds
  `LateAnswerFilter`. One file per view, and `mouse.rs`.
- `kitty_output/`: the Kitty writes of phewr per frame. Each `render_*` has a `*_to` twin that
  writes to any `Write`. `mod.rs` holds the fullscreen image and the video frame, `grid.rs` the
  thumbnails, `loops.rs` the grid loops, `grid_icons.rs` the SVG icons. The generic part is in
  `pictures::kitty`.
- `pictures/`: the shared picture module, the master copy for `just pictures-sync`. It imports
  no app module.
  - `decode/`: the decoders and the routes. `Route::for_path` picks the route by extension.
    `pictures::decode::dimensions` reads the size. `extensions.rs` holds the extension lists
    and `has_extension`. Submodules: `raster`, `turbo`, `libheif/`, `jxl`, `sips`, `embedded_jpeg/`,
    `heif_boxes`, `screen/`, `tonemap`, `hdr/` and `color/`.
  - `resize/`: the `fast_image_resize` helpers and `padding.rs` (the pad to whole cells).
  - `shape/`: the folder aspect from the headers and `DEFAULT_IMAGE_ASPECT`. `copies.rs` holds
    the box copies and `corners.rs` the rounded corners.
  - `jobs/`: `pool.rs` (`spawn_job` and the cache read pool), `queue.rs` (`JobQueue`,
    `JobSlots`, `Priority`), `thumbnail_cache.rs` (the memory cache with `CacheLimits`) and
    `disk_cache/` (`thumbnail_dir_below`, no default folder).
  - `names.rs`: `APP_NAME` from `CARGO_PKG_NAME` and the `<APP>_TEST_*` names.
  - `kitty/`: the Kitty graphics protocol (`KittyBackend`, `Transfer`, `Loops`, `Pixels`,
    `EMIT_PASS_BYTES`). `protocol/` holds the escape sequences, and `protocol/probe.rs` the start
    probes. `placement.rs` holds the placements, `terminal_images.rs` the storage budget and
    `query.rs` the start probe and the cell size. `answers.rs` holds the answers of the terminal
    for the self check: `AnswerReader` reads bytes, and `AnswerKeys` reads the key events of
    crossterm.
  - `test_support/`: the test file builders.
- `app/`: `App`, the whole view state, one file per topic: fullscreen, zoom, marks, folders,
  scan, slideshow, meta, mouse, the thumbnail jobs and their bounds (`thumbnails/`), the cache
  folder and the loop files of the disk cache (`disk_cache/`), the folder prefetch
  (`folder_prefetch/`) and the grid loops (`grid_loops/`).
- `ui/`: the ratatui drawing: `mod.rs` (the views and the info panel), `status.rs`, `grid.rs`,
  `help.rs`.
- `gallery/`: the grid geometry and the cursor.
- `scanner/`: the background scan, the video extensions and the Live Photo pairing (`live/`).
- `image_list.rs`: `SharedImageList`, the list that the scanner fills and the UI reads.
- `prefetch/`: the decode of the fullscreen neighbors.
- `search.rs`: the fuzzy filename search on one filter thread (`FilterWorker`).
- `info.rs`, `file_meta.rs`: the facts of the info panel and of the status row.
- `icons.rs`: the cell icons (resvg render, octant fallback) and the play stamp.
- `styles.rs`: `STYLES`, one style per role on the screen.
- `config/`: `config.toml` and the state file with its writer thread.
- `yazi.rs`: the reveal in yazi and the PATH lookup `find_on_path`.
- Feature `video`: `video/` (the tool lookup, the first-frame thumbnails), `player/` (the
  terminal player, the decoder, the probe, the tone map cubes), `video_handoff/` (the player
  start, the prestart, the mpv window), `grid_loop.rs` (the loop frames) and `mpv.rs`.
- `test_support/` and `bench/`: test only. `docs/benchmarks.md` holds what each bench measures.

## Gotchas

`docs/gotchas.md` holds the long form of each item, with the numbers and the dates of the
decisions. Read the long form before you change the code that an item names.

### Video

- **Terminal player:** ffmpeg runs as a subprocess with VideoToolbox and writes raw frames to a
  pipe, one frame per temp file under the image id `u32::MAX - 1` (`docs/video.md`, "Terminal
  player"). The GPU download format follows the frames of VideoToolbox. A stream above 10 bits
  starts in software (`VideoInfo::hardware`).
- **Player real time (rule):** the terminal player never plays in slow motion. A slow decode or a
  slow terminal drops frames. If the frames lag the clock by more than `MAX_LAG` (300 ms),
  `Player::catch_up` starts ffmpeg again ahead of the clock. The player has no pause and no seek.
  `p` opens the mpv window for all controls. The run loop waits until the next frame is due,
  10 ms while ffmpeg has no frame ready.
- **Video player default:** `[video] player` defaults to `terminal`, and `builtin` maps to
  `terminal`. Without the temp file medium or without ffmpeg, a video opens in the mpv window
  with a note.
- **Video placeholder:** `q`, `Esc` and the back click in the terminal player always go to the
  grid. A video in `ViewMode::Fullscreen` is a placeholder (`shows_video_placeholder`).
  `load_if_needed`, `zoom_in` and `slideshow_tick` skip it.
- **Animated images:** an animated GIF, WebP, APNG or AVIS plays in the terminal player like a
  video (`docs/video.md`, "Animated images").
- **Slideshow view:** a running or paused slideshow has no status row (`ui::status_rows`), so
  `image_rect` is the whole screen. The image goes out at its decoded size, and the terminal
  scales it to the fill of the box with `r=` or `c=` alone (`filled_placement` in
  `pictures/kitty/placement.rs`). Outside a slideshow, an image shows at its own size. A badge at
  the bottom right shows the play or pause glyph, the interval, the note or the error. The image and
  the video frame sit below the cell backgrounds (`BELOW_BACKGROUNDS`), so the badge covers them.
- **Slideshow and videos:** a running slideshow plays a video one time and moves on. Outside a
  slideshow, the clip loops. The mpv window of a slideshow reads the `end-file` reason through
  `--input-ipc-server`: `quit` or `stop` stops the slideshow (`mpv::exit_with_reason`).
- **mpv steps and window:** `n` and `N` in mpv exit with 10 and 11, and phewr steps from the file
  that mpv played. phewr polls each mpv window every 100 ms until it exits. The window gets
  `--fs --native-fs=no` and `--hwdec=auto-safe`. `--input-commands` needs mpv 0.38 or later.
  mpv's own Kitty output is gone: it played a 4K HLG clip at 2.4 fps in Ghostty.
- **HDR in the terminal:** for HLG and PQ, `player::tone_map` writes a 3D cube into the cache
  directory. `CUBE_VERSION` is the one version for all cube names: change it when the cube output
  changes. HDR keeps 33 points. A PQ video or still maps with its HDR10 peak, rounded up to the
  buckets 500, 1000, 2000, 4000 or 10000 nits (`pictures::decode::hdr::hdr10_peak`). The stills
  in `pictures/decode/hdr/` copy the math of the player cubes, and a test compares both copies.
  `SDR_WHITE_NITS` is 500, and HLG keeps 1000 nits.
- **Video programs:** phewr links no FFmpeg library. `main` looks up `ffmpeg` and `ffprobe` one
  time, and the tests never do. The probe cache keys each `VideoInfo` by path, mtime and length.
  A file whose GPU run failed starts in software for the rest of the session. Without the
  programs, a video cell keeps its large play icon, and Enter opens the mpv window with a note.
- **Start ahead of Enter:** after a rest of 200 ms (`PRESTART_DELAY`) on a video, an animation or
  a Live still in the fullscreen view, `video_handoff/prestart/` opens the player. The UI thread
  runs no ffprobe for a prestart. A cursor move, an overlay, the mpv window and a wait of 10 s
  (`PRESTART_LIFETIME`) drop the player.
- **Live Photos:** the folder scan hides the motion `.mov` of each Live still
  (`[gallery] live_photos`). The still loops its motion in the grid, and Enter plays it one time.
  The slideshow shows the still only. A HEIC wins the motion over a JPEG twin. A left click on
  the middle of the fullscreen picture plays like Enter. At the start of the motion, phewr reads
  both content identifiers on the UI thread. If they differ, the motion plays with a note.
  `[gallery] animate = false` gives no loop and no stamp. The design is in
  `docs/live-photos.md`.

### Folders, files and the status row

- **Entry kinds:** the first `App::folder_count` entries of `images` are folders, `..` first.
  Every step of the fullscreen view starts at `App::first_file()`. The thumbnail jobs, the marks,
  the cell shape and the info panel skip folders. Hidden files, hidden folders and symlinks to
  folders stay out. phewr always starts in the grid, and only `--slideshow` opens another view.
  One folder argument is one `read_dir` (`scanner::read_folder`), sorted by `name_key`. The grid
  lists a folder that phewr cannot read, and `Enter` shows the read error. For this reason, a
  listing of the home folder triggers no macOS privacy prompt.
- **One file argument:** `cli::start_paths` turns it into its folder and a cursor target. Several
  arguments stay a plain file list without folders and without Live pairs.
- **yazi reveal, marks and `--cwd-file`:** all run after `TerminalSession::restore`. The marks
  become the selection of the yazi that started phewr, through the plugin `phewr-select`. Each
  `ya` call gets at most 1 s (`YA_TIMEOUT`). Tests pass a closure and never start `ya`. `--cwd-file` writes
  `App::gallery_dir` without a newline.
- **Status notes:** the note lives in `LoopState`, not in `App`. The next key press or mouse
  button press clears it. A scan read error lives in `App::scan_error`.
- **Status row:** the row shows no key hints, only the facts of the selected file
  (`file_meta.rs`). A thread reads them, so a cursor move never waits. A status note, a
  question, an error, the search line and a row too narrow for both hide the facts.

### Run loop and threads

- **Run loop waits:** the loop polls every 10 ms while a background job runs
  (`App::has_pending_work`), and faster during the first picture and the cache reads. Otherwise
  it waits 250 ms. A grid loop job counts as a background job. The waits are in
  `docs/grid-and-kitty.md`.
- **Decodes share the cores:** `LibheifSlot` shares the tile threads. At most 4 thumbnail and 2
  prefetch decodes run at once, and a cache read never waits behind a cold decode
  (`docs/app-state.md`, "Thumbnail jobs").
- **Folder prefetch:** after a rest of 300 ms on a folder cell, one thread decodes the first
  screen of that folder into the disk cache with the same functions as Enter. It pauses while
  the grid has work. A cursor move, a folder change, a zoom, a resize, a mode change or the quit
  cancels it, and the next start waits for the old thread. A lower thread priority gave no gain
  (2026-11-03).
- **Grid zoom:** the grid shows 2 to 12 rows. A zoom step goes to the next zoom level
  (`GalleryState::zoom_levels`), not to the next row count. A level keeps the folder shape at a
  box width of at least `MIN_THUMB_COLS` (5), its box holds the large grid icon (5 rows or more),
  and every step changes the column count. The grid shows the largest level of at most the
  stored `zoom_rows`. A zoom or the info panel keeps the screen row of the cursor, and at the list edges the grid stays flush with the edge
  (`GalleryState::keep_cursor_place`). `+`, `-` and `0` do no file I/O on the UI thread: they
  queue a `StateChange` to the writer thread. A state file that does not parse stays as it is.
  The state file holds a global `visible_rows`, a `zoom_counter` and a `[zoom]` table keyed
  by the canonical folder path, with at most 200 folders. The drop of `App` flushes the queue.
  A zoom test uses `StateWriter::with_delay` and `flush`, not a poll loop.

### Kitty output and the terminal

- **Kitty images are not ratatui widgets.** ratatui draws the frame, then phewr writes the
  pictures at the cell positions of the last draw. A popup must delete the pictures first
  (`graphics_delete_all`), because Kitty draws them above the text. A zoom or a resize calls
  `mark_dirty`, and the grid write deletes the old pictures itself. A key handler deletes the
  pictures only before an overlay. A delete without a later draw leaves black cells.
- **Kitty emits:** a pass writes the new thumbnails in grid order, in one synchronized update, up
  to `EMIT_PASS_BYTES`.
- **Kitty temp file medium:** `t=t` wins only when the probe answer is `OK` and the terminal
  deleted the probe file. Otherwise a terminal with another `TMPDIR` keeps the files. The file
  name contains `tty-graphics-protocol`.
- **Grid gaps and borders:** the cells have 1-cell gaps, every picture is padded to whole cells
  and placed centered with `c=` and `r=`, and the borders live in the gaps.
- **Terminal storage:** `TerminalImages` tracks the images that the terminal stores, and the
  grid trims the oldest unshown images above `TERMINAL_IMAGE_BUDGET` (128 MB,
  `pictures/kitty/terminal_images.rs`).
- **Image ids:** the thumbnails count up from 1. The icons start at `ICON_ID_BASE`, the loops at
  `LOOP_ID_BASE` (`1 << 31`), and the video frame is `u32::MAX - 1`. The probes use 31 and 32.
- **Grid loops:** every visible video, animated and Live cell loops after its still, as a Kitty
  animation or through ticks (`Loops`). herdr gets the ticks without the animation probe:
  herdr 0.9.5 answers the probe itself but plays no Kitty animation (maintainer, 2026-11-03).
  `PHEWR_KITTY_LOOPS` and `[gallery] loops` win. `[gallery] animate = false` turns the loops off.
- **Cell icons:** a cell without a thumbnail shows a Lucide icon as a Kitty image in the terminal
  foreground (OSC 10). Without an OSC 10 answer, the icons are octant text. The design is in
  `docs/grid-and-kitty.md` ("Grid icons").
- **Cell size:** `CSI 16 t` gives the cell size, because TIOCGWINSZ includes the window padding
  of Ghostty. If the terminal answered at start, the run loop asks again after each resize. The
  read takes every byte of the tty, so a key typed in that millisecond is lost. A terminal that
  answers neither the probes nor DA1 costs `PROBE_TIMEOUT` (300 ms) one time.
- **Late probe answer:** the read ends when every owed answer came, or at the DA1 answer. Ghostty
  1.4 sends DA1 before the OSC 10 answer and the Kitty answers. So when an answer is missing at
  DA1, a second read waits at most `LATE_ANSWER_GRACE` (100 ms) for it. After a timeout of the whole round trip, `LateAnswerFilter` drops the key events of the late Kitty
  answers and of a late OSC 10 answer. `AnswerKeys` reads every Kitty answer first, also a late
  probe answer. Each one lowers the owed count of the filter (`kitty_answers_came`). The filter
  sees only the rest, in practice a late OSC 10 answer.
- **Self check:** the engine places with `q=1` (`Screen::with_self_check`). phewr turns it on,
  and `PHEWR_KITTY_SELF_CHECK=0` turns it off. A transfer keeps `q=2`, else each transfer gets an
  `OK`. `handle_input` runs `AnswerKeys` first, then `LateAnswerFilter`. An `ENOENT` goes to
  `Screen::answer`, and `sync_grid_icons` writes a grid pass while `Screen::has_lost` is true. A
  hold of key events ends after 50 ms of quiet input (`ANSWER_WAIT`). phewr drops the keys of a
  started answer that never ends.
- **Mouse:** phewr enables only the mouse modes 1000, 1002 and 1006, never 1003. Do not use
  crossterm's `EnableMouseCapture`, because it turns on 1003. A test that needs the screen
  regions calls `test_support::draw_frame` first. The double-click uses `handle_event_at` with a
  clock, so tests never sleep. `[gallery] mouse = false` turns the capture off. The run loop
  releases the mouse while an mpv window plays. A horizontal
  wheel does nothing in the grid, so a touchpad scroll never zooms by accident (2026-11-17).
- **Cell shape:** the shape comes from the headers of the first 64 files, once per scan, and
  stays until the next directory switch.

### Decoders and colors

- **Thumbnail cache key:** the key hashes `THUMBNAIL_CACHE_VERSION`, the path, the size, the
  mtime, the target size and the color target. The version is
  `concat!(env!("CARGO_PKG_NAME"), "-thumbnail-v22")`. When the thumbnail output changes,
  increase the `v22` part. If several workers change the output at the same time, each takes its
  own version, higher than main and than every branch in flight.
- **Thumbnail cache folder:** the thumbnails of phewr sit at the top of `~/Library/Caches/phewr`
  (`default_disk_cache_dir`). Do not move them. The prune refuses a folder that holds a
  subfolder, so phewr must keep no subfolder there.
- **Disk cache format:** QOI files. A hit reads and decodes in about 0.75 ms.
- **Resize and pixel layouts:** every resize uses Catmull-Rom and the fastest layout of
  `fast_image_resize`. Only a downscale to 1/2 or less resizes as RGB. An opaque image goes out
  as RGB. The grid thumbnails keep RGBA with their
  padding.
- **Image colors:** phewr converts every image into Display P3 (`[terminal] colorspace`),
  because Ghostty shows the pixels without a conversion. phewr needs moxcms 0.9.2 or later:
  0.9.1 truncated its output curves.
- **JPEG XL:** jxl-oxide 0.12 converts ICC v2 samples wrong, so `moxcms` converts lossless files
  with an ICC profile. jxl-oxide clips lossy HDR samples outside the sRGB gamut, and a test
  marks the limit. The sample copy reads the private layout of `jxl-render`, so
  `jxl-oxide = "=0.12.6"` and `jxl-render = "=0.12.4"` are exact pins. An update is a deliberate
  step: run `parallel_copy_matches_stream` and a byte comparison on real files.
- **`image` decoders:** the `image` crate has only the decoders that
  `pictures/decode/extensions.rs` lists. If you add a format to these lists, add the `image`
  feature too.
- **Embedded HEIC thumbnails:** a cold HEIC cell shows its embedded thumbnail first, then the
  sharp decode. The thumbnail must have the same `irot` and `imir` as the primary image. A
  preview never goes into the disk cache. The sharp jobs of libheif files wait until every
  preview is out (`sharp_job_waits`). The sharp image gets a new Kitty id, and the emit deletes
  the preview image first. Fullscreen, zoom and prefetch pass `EmbeddedThumbnail::Deny`.
- **libheif reads through a stream** (`libheif_rs::StreamReader`).
  `HeifContext::read_from_file` copies the whole file first, 30 ms to 40 ms per 24 MP photo.
- **The `sips` route:** camera RAW, PSD, PDF, SVG and other ImageIO formats. RAW and PSD try the
  embedded JPEG first (`pictures/decode/embedded_jpeg/`). A JPEG that covers the fit of the box is
  the thumbnail, and sips does not run. sips ignores the orientation of RAF and ARW, so phewr turns
  or flips the develop (`pictures::decode::sips::flip_gap`). A relative path gets `./` in front.
  If `IMAGE_EXTENSIONS` does not list the extension of a file, the file tries sips after a failed
  raster decode. A listed raster extension never tries sips. A sips run gets 10 s
  (`SIPS_TIMEOUT`), and then a kill.
- **Embedded JPEG orientation:** the container orientation wins over the EXIF of the embedded
  JPEG. libjpeg-turbo decodes at the smallest of 1/8, 1/4, 1/2 and 1 that covers the fit. The
  other eighths run without NEON and are slower than the full decode.
- **Process start:** about 30 ms pass before `main`, most of it the dylib load of libheif.
  `just install` builds `--release`. A profile with full LTO and stripped symbols gave no
  measurable gain.

## Workflow for agents

1. Read `TODO.md` for the current list.
2. Run `just check` after each change. If clippy has suggestions, run `just fix` first.
3. Never commit, reset, rebase or stash. When the work is ready for a commit, stop. Report the
   changed files and a suggested commit message. The maintainer commits. The only exception:
   the orchestrator of the herdr-worktrees skill merges approved worker branches through its
   scripts.
