# AGENTS.md

This file is the entry point for each person and coding agent that works on viewr. Read it
first. Then read `TODO.md` for the open work.

## What this is

viewr is a terminal image and video gallery for the Kitty graphics protocol, written in Rust.
It is a fork of [rview](https://github.com/jrf/rview) by jrf (MIT). The fork had the name
imgrrr before 2026-10-17.

- The main goal: viewr is a fast viewer for all common image and video formats in the terminal,
  with full mouse support. Each change makes opening, scrolling or switching files faster, or it
  adds a common format. `TODO.md` holds the full goal and its scope.
- viewr is a viewer only. It does not edit, convert, write, move, rename or delete files. The
  remaining delete code (trash, permanent delete) goes after the open terminal tests. Do not add
  new file operations.
- The target is macOS on Apple Silicon, in Ghostty. viewr uses the Kitty graphics protocol, not the Kitty terminal. Nobody tests in the Kitty terminal.
- The maintainer is the only user today. Write user-facing texts for other people anyway.

The stack: ratatui 0.30, crossterm 0.29, clap, image 0.25, fast_image_resize, rayon, lru,
serde/toml, jxl-oxide, moxcms. The features `turbo` (libjpeg-turbo) and `heic` (libheif) link
native Homebrew libraries. The feature `video` links no library: it starts `ffmpeg`, `ffprobe` and
`mpv` from PATH when they exist, and reads ffprobe's JSON with serde_json. The feature `sips` runs
the macOS tool `/usr/bin/sips`. All four features are on by default.

## Build, test, run

```sh
just                 # = just check: fmt --check, clippy -D warnings (2 feature sets), nextest, rustdoc, cargo deny
just fix             # clippy --fix + cargo fmt
just test [FILTER]   # cargo nextest (falls back to cargo test)
just test-real [FILTER]  # also the ignored tests that decode real files (VIEWR_TEST_*)
just coverage        # line coverage summary (cargo-llvm-cov)
just coverage-html   # line coverage as HTML in target/llvm-cov/html
just bench [DIR]     # the speed measurements of src/bench.rs on VIEWR_TEST_DIR (default /tmp/viewr-bench)
just build | run | install | uninstall
just hooks           # install the pre-commit hook (fmt + clippy). `git commit -n` skips it once
just fmt | lint | doc | deny | audit | outdated   # the single steps
just dist            # arm64 binary in dist/ (dist profile, ad-hoc codesign, sha256)
just render          # dist + the filled formula and install.sh in dist/, publishes nothing
just release         # check + tag + Codeberg release + tap (maintainer only, see below)
```

Plain cargo works too. Homebrew's rustup is keg-only, so run
`export PATH="/opt/homebrew/opt/rustup/bin:$PATH"` first. The justfile does this itself.
The justfile also sets the `TURBOJPEG_*` variables, so turbojpeg-sys links Homebrew's
libjpeg-turbo statically and does not build it with cmake.

The repository pins the toolchain, the formatter and the lints:

- `rust-toolchain.toml`: stable, with rustfmt, clippy, rust-analyzer and llvm-tools. Edition 2024.
- `rustfmt.toml`: 2024 style, stable options only.
- `Cargo.toml`, the `[lints]` tables: clippy `pedantic` plus restriction lints. In
  `just check`, every warning is an error.
- `clippy.toml`: the thresholds.
- `deny.toml`: licenses, advisories, duplicate versions, git sources.
- `rust-analyzer.toml`: clippy as the checker of the editor, its own target directory.
- `.config/nextest.toml`: the timeout for a test that hangs.
- `.editorconfig`.

Files at run time:

- Config: `~/.config/viewr/config.toml`, or `$XDG_CONFIG_HOME/viewr/config.toml`. The sections
  are `[gallery]`, `[slideshow]`, `[info]` and `[video]`.
- State (the remembered zoom levels): `~/.local/state/viewr/state.toml`.
- Thumbnail cache: `~/Library/Caches/viewr/`, QOI files, pruned at 512 MB.

### Release (maintainer only)

CAUTION: Do not run `just release` or `just publish` unless the maintainer asks for it. These
recipes publish to Codeberg and push the Homebrew tap.

There is no CI. The maintainer builds and publishes each release from their own machine.

One-time setup:

1. Create the Codeberg repository `viewr` under `webgefrickel`.
2. Export `CODEBERG_TOKEN`.
3. Clone `homebrew-tap` next to this repository (`../homebrew-tap`).

For each release:

1. Change `version` in `Cargo.toml` and commit.
2. Run `just release`. It runs `just check`, builds the binary, tags, creates the Codeberg
   release with the asset and `install.sh`, and pushes the formula to the tap.

The release binary is arm64 only, because Homebrew's libheif is an arm64 library. The formula
depends on libheif, ffmpeg and mpv. `install.sh` stops if libheif or ffmpeg is missing. Before the
next release, make ffmpeg a recommendation in the formula and drop the `libavcodec` check from
`install.sh`: viewr runs without ffmpeg.

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
- **Each feature set builds.** `just lint` runs clippy with the default features and with
  `--no-default-features`. Code behind `#[cfg(feature = "...")]` must compile in both.
- **Terminal colors only.** The TUI uses the terminal default colors and the 16 ANSI colors from
  `styles.rs`. Do not add RGB values or 256-color indexes.
- **One formatter, one style.** Use rustfmt with `rustfmt.toml`. The hook runs
  `cargo fmt --check`.
- **Language:** code, comments and docs are in English.

### Tests

- Unit tests sit in a `#[cfg(test)] mod tests` at the end of each module.
- Tests that need a real file are `#[ignore = "..."]` and read `VIEWR_TEST_JPEG`,
  `VIEWR_TEST_HEIC`, `VIEWR_TEST_AVIF`, `VIEWR_TEST_JXL`, `VIEWR_TEST_RAW`, `VIEWR_TEST_PSD`,
  `VIEWR_TEST_HDR_VIDEO` or `VIEWR_TEST_SDR_VIDEO`.
- `tests/fixtures/color/` holds a Display P3 chart as JPEG, PNG, WebP, TIFF and HEIC with an ICC
  profile, as HEIC and AVIF with nclx, and as PNG with cICP and with gAMA and cHRM, plus an
  Adobe RGB JPEG and untagged sRGB files. `VIEWR_BENCH_COLORSPACE=srgb just bench` measures with
  the sRGB target.
- `tests/fixtures/png/` holds 10 PNG layouts (16 bit, palette, tRNS, gray, gray with alpha,
  RGBA, interlaced, EXIF orientation 6), and `make.sh` rebuilds them with magick and exiftool.
- `tests/fixtures/` holds 8 small animated and still files (GIF, WebP, APNG, AVIF), and
  `test_support::fixture(name)` finds them. `avifenc` makes a new AVIS fixture. The real ffmpeg
  test skips WebP without FFmpeg 9 (`webp_anim` decoder). `tests/fixtures/jxl/` holds 18 JPEG XL
  color charts and gradients, `ref/` their djxl renderings, and `make.sh` rebuilds both with
  cjxl, djxl, magick, exiftool and sips.
- A real ffmpeg playback test uses a virtual clock that jumps to each due frame. The wall-time
  twin is `#[ignore]` and runs in `just test-real`.
  `just test-real` runs them. A test without its variable skips with a note.
- The `sips` tests run `/usr/bin/sips` to write synthetic PSD and PDF files.
- The synthetic libheif tests encode with the aom (AVIF) and x265 (HEIC) encoder plugins of
  Homebrew's libheif. If a plugin is missing, `just check` fails.
- A test that writes files uses `tempfile::tempdir()` (`TempDir`) or `tempfile::NamedTempFile`.
  The directory goes away on drop, also when the test panics.
- The app builders, the key press helpers and the synthetic file writers that tests of more than
  one module share live in `test_support.rs`.
- A test app writes the Kitty sequences to a buffer (`GraphicsOutput::Buffer`). Read them with
  `app.graphics_out.take_bytes()`. A test that needs a config uses `App::new_with_config`, never
  `App::with_config`.
- A test never writes to the real state file or the real thumbnail cache. The test-only
  `App::new` points `app.state_file` and `app.disk_cache_dir` below `/dev/null`. A test that
  needs them sets them to temp paths.
- A test never sets `HOME` or `XDG_*`. Use the functions that take a path: `config::load_at`,
  `config::xdg_path`, `config::save_thumb_size(path, ..)`.
- A test waits for background work in a loop with a limit (for example 2 s in 10 ms steps), and
  never depends on the clock within a frame interval.
- A test calls `trash::delete` only with a missing path, so it never asks Finder.

## Layout

- `main.rs`: the module list and `main`: the lookup of ffmpeg and ffprobe
  (`video::find_session_tools`), the CLI checks, the terminal
  session and the start of the run loop.
- `cli.rs`: the clap arguments (`Cli`) and the start directory.
- `terminal.rs`: `TerminalSession` (raw mode, the alternate screen), the Kitty probe
  `query_terminal`, `query_cell_size_and_probe` (`CSI 16 t` and the Kitty probe in one write)
  and the cell pixel size.
- `run_loop.rs`: `LoopState` (the status note and, with the `video` feature, the `VideoState`), `run`, the background polls,
  `render_images` and `next_timeout`. The status note lives here, not in `App`.
- `keys/`: the key handlers. `mod.rs` holds `handle_event` and `handle_key`, the thin
  `match` that routes the overlays and the view modes. One file per view: `fullscreen.rs`,
  `gallery.rs` (with the search line), `video.rs` (feature `video`).
  `mouse.rs` holds the mouse handler per view.
- `kitty_output.rs`: the Kitty writes per frame: the fullscreen image, the video frame, the
  grid and the new thumbnails. Each `render_*` has a `*_to` twin that writes to any `Write`,
  so that the tests read the escape sequences. `App::with_graphics_out` runs a write against the
  graphics output.
- `video_handoff/` (feature `video`): the start of the terminal player and of the mpv window,
  and the way back. `mod.rs` holds `VideoState` and `play_videos`. `window.rs` holds
  `poll_mpv_window`.
- `player/` (feature `video`): the terminal player. `mod.rs` holds `Player` and the pacing,
  `decoder.rs` the ffmpeg command line and the `Decoder` trait, `probe.rs` `VideoInfo`,
  `layout.rs` the frame box and the cell box, `tone_map.rs` the HDR curve and the cube,
  `animation.rs` the header check for animated GIF, WebP, APNG and AVIS.
- `yazi.rs`: the reveal of the last file in yazi, and the PATH lookup `find_on_path`.
- `app/`: `App`, the whole view state. `mod.rs` holds the struct, its fields and its
  construction. One file per topic: `info_panel.rs`, `grid_zoom.rs` (the cell size and the
  state file), `slideshow.rs`, `scan.rs` (the scanner results, the sort and the filter),
  `cell_shape.rs`, `fullscreen.rs` (enter, leave, the steps and the decode), `zoom.rs` (zoom,
  pan and the full-resolution source), `delete.rs` (the marks and the delete question),
  `directory.rs` (the directory switch), `folder_prefetch.rs` (the decode of the
  first screen of the folder under the cursor into the disk cache), `mouse.rs` (the mouse state and the
  screen layout of the last draw), `folders.rs` (the folder entries of
  the grid, `Enter` into a folder, `go_to_parent`), `video.rs` (the terminal player state, the
  return after a video, the mpv window watch), `thumbnails.rs` (the jobs, the queue, the HEIC
  previews, the generations), `disk_cache.rs` (the thumbnail disk cache).
- `decode/`: the decoders and the decode routes. `Route::for_path` in `mod.rs` picks the
  route by extension, `decode_image_with_hint` runs it, `foreground` marks the decode that the
  user waits for. `raster.rs` (the `image` crate and the EXIF orientation), `turbo.rs`
  (libjpeg-turbo with DCT scaling), `libheif.rs` (HEIC, HEIF, AVIF, the embedded thumbnails
  and the core share `LibheifSlot`), `resize.rs` (`load_and_resize`, Catmull-Rom), `jxl.rs`
  (JPEG XL, ICC profiles through `moxcms`), `sips.rs` (camera RAW, PSD, PDF, SVG and other ImageIO formats), `tonemap.rs`
  (EXR and HDR to 8 bit), `heif_boxes.rs` (the safe ISOBMFF reader for `irot`, `imir` and
  `ispe`), `shape.rs` (the image dimensions from the file headers, for the cell shape),
  `embedded_jpeg.rs` (the safe reader for the JPEG inside camera RAW and PSD files), `color.rs`
  (the conversion into the color space of the terminal: `ColorTarget`, `SourceColor`, the
  transform cache, and the ICC and CICP readers for JPEG APP2, the PNG chunks and the TIFF
  tag). `decode_image` returns `Decoded` (the pixels and their `SourceColor`).
  `Decoded::fit_within` resizes, then converts. `decode_image_with_hint` converts at the decoded
  size.
- `ui.rs`: the ratatui drawing of each view, the help popup and the info panel. The help popup has
  two columns from 100 terminal columns on (`HELP_TWO_COLUMN_WIDTH`, rows in `help_gallery_rows`
  and `help_viewer_rows`), else one column that clips. The gallery, the
  fullscreen view and the video view have no frame. They use the full area minus 1 status row
  (`content_area`, `status_area`). The help popup and the info panel keep their
  borders.
- `gallery.rs`: the grid geometry, the cursor and the in-memory thumbnail cache.
  `update_grid` gives the remainder of the width and the height to the first columns and rows,
  so the grid reaches the edges. `set_cell_shape` picks the column count from the photo aspect
  and the pixel size of a terminal cell.
- `scanner.rs`: the background directory scan and the lists of supported extensions.
  `LIBHEIF_EXTENSIONS` is the one list for the libheif route. `subfolders` lists the subfolders
  of one folder argument, `..` first. `files_of` lists the files of one folder in the order of
  the scan, without a thread.
- `image_list.rs`: `SharedImageList`, the list that the scanner fills and the UI reads.
  `EntryKind`, `FolderEntry` and the folder prefix (`push_folders`, `folder_count`).
- `prefetch.rs`: decodes the neighbors of the fullscreen image ahead of time. Results carry a
  generation. `invalidate()` drops the jobs in flight and forgets the failed indices. Each result
  (`Prefetched`) keeps its decode box. `take_resized` fits the image into the requested box. If
  the decode box is smaller, it returns `None`.
- `search.rs`: fuzzy filename search (nucleo-matcher).
- `info.rs`: file data and EXIF for the info panel, and the EXIF orientation.
- `styles.rs`: `STYLES`, one style per role on the screen.
- `config.rs`: `config.toml` and the state file.
- `video.rs` (feature `video`): `Tools`, the session lookup of ffmpeg and ffprobe, the first-frame
  thumbnails and the video extensions.
- `mpv.rs` (feature `video`): the window arguments for mpv, the exit notes, the step exit codes
  and the `Runner` trait for the process start.
- `encoder/`: the Kitty graphics protocol (`KittyBackend`). `Transfer` picks the medium: direct,
  PNG (SSH) or a temp file (`t=t`). `detect_transfer` probes the terminal at start with `a=q`.
  `VIEWR_KITTY_TRANSFER=direct|png|file` skips the probe. `Pixels` is a view of RGB or RGBA
  pixels. `transmit` sends an RGBA image without a transparent pixel as RGB (`f=24`) through
  every medium. The check and the copy go in chunks of 4096 pixels and stop at the first
  transparent chunk, so a padded grid thumbnail stays RGBA at almost no cost.
- `test_support.rs` (test only): the app builders, the key press helpers and the synthetic
  file writers that the tests of more than one module share. `test_support::video` holds
  `FakeRunner`, `FakeDecoder` and the app builders of the video tests.
- `bench.rs` (test only): the ignored speed benchmarks. They drive the real loop steps without a
  terminal and read `VIEWR_TEST_DIR`. `start_to_the_first_grid_picture` measures the process
  start and the steps of `main`, `disk_cache_formats` the cache formats.

## Gotchas

- **Terminal player:** ffmpeg runs as a subprocess (`player/decoder.rs`, behind the `Decoder`
  trait), with `-hwaccel videotoolbox`, `-noautorotate`, `scale_vt` and `transpose_vt` on the
  GPU, `hflip,vflip` for 180 degrees, `hwdownload` as `p010le` (10 bit) or `nv12`, and
  `format=rgb48le,lut3d=...:interp=tetrahedral` for HLG and PQ. A GPU run without a frame runs
  again in software (`Pipeline::Software`). A video with a variable frame rate gets
  `-fps_mode cfr -r` too: the probe calls it VFR when `r_frame_rate` and `avg_frame_rate` differ
  by more than 1 %, and the output rate is `r_frame_rate` up to 60 fps, else the average rate
  capped at 60 fps (`pacing_rate`, `variable_rate_fps` in `player/probe.rs`). Without `-r`,
  ffmpeg writes raw frames at `r_frame_rate`, so the pacing uses the same rate. iPhone clips
  (30 and 29.99) stay constant-rate. ffmpeg writes `-pix_fmt rgb24` for a video and `rgba` for an
  animation, and `pixel_format` in `player/decoder.rs` is the one source for the frame layout
  (`player::Frame`). One frame per temp file, image id `u32::MAX - 1`
  (the grid ids count up from 1), `c=` / `r=` to the cell box, no `X=` / `Y=`. The frame box is
  the window in pixels, capped at 1920x1080 (`MAX_FRAME`). A resize moves the cell box. The
  frame size stays until the next ffmpeg start. The reader thread holds 2 frames
  (`FRAME_QUEUE`), so a pause blocks ffmpeg on the pipe. A frame more than one interval late
  drops only when a newer frame waits. Otherwise the clock moves to the frame, and a slow
  decode plays slower instead of frozen. The help popup holds the player and gets no frame. A
  run after a seek without a frame is the end of the clip, not an error. The run loop waits
  until the next frame is due, 10 ms while ffmpeg has no frame ready, 250 ms in a pause. Tests
  use `FakeDecoder` and never start a real ffmpeg or mpv.
- **Video player default:** `[video] player` defaults to `terminal`, and `builtin` maps to
  `terminal`. Without the temp file medium (SSH, a probe that picked `direct` or `png`) or
  without ffmpeg, a video opens in the mpv window with a note. mpv's own Kitty output is gone:
  it played a 4K HLG clip at 2.4 fps in Ghostty and cannot scale a small frame.
- **Video placeholder:** after the player or the mpv window, `App::video_return` selects the
  view. A video in `ViewMode::Fullscreen` is a placeholder (`shows_video_placeholder`).
  `load_if_needed`, `zoom_in` and `slideshow_tick` skip it, because the image decode cannot read
  a video. `p` on the placeholder opens the mpv window.
- **Animated images:** an animated GIF, WebP, APNG or AVIS plays in the terminal player like a
  video. The thumbnail job reads the animation flag into `App::animated`, and the grid marker
  (a dim `▶` in the label, or in the gap below the cell) reads only that flag, so the draw
  reads no file. A video gets the same marker once its thumbnail is in `thumb_cache`
  (`shows_play_marker` in `ui.rs`). The route (`App::plays_at`) reads the header of the one file after a key
  press. `App::is_video_at` still drives the placeholder and the loading icon. The probe reads
  at most 16 packets: the rate is the GCD of the frame delays, or the mean rate above 50 fps,
  and the length comes from the container. An AVIS file holds two video streams, and the probe
  maps the one with the most frames. ffmpeg gets no `-hwaccel` for an animation, because the
  AV1 decoder stops with it. Esc from an animation returns to its still frame in the fullscreen
  view. In a running slideshow, the terminal player holds an animation for at least the
  slideshow interval (`Player::play_at_least`). The mpv window has no such hold.
- **Slideshow and videos:** a running slideshow that is not paused plays a video one time. The
  terminal player reports `finished`, and the slideshow moves on. Outside a slideshow, the clip
  loops. In the mpv window, mpv gets `--loop-file=no`, `--keep-open=no`, `--idle=no`,
  `--loop-playlist=no` and `--input-commands` that bind `q`, `Q` and `CLOSE_WIN` to `quit 4`.
  Exit code 0 is the end of the file. Each other exit stops the slideshow on the placeholder.
  `--input-commands` needs mpv 0.38 or later. On macOS, the app quit (Cmd+Q) of a window mpv
  exits with 0, so a window slideshow continues.
- **mpv steps:** each mpv start gets `--input-commands` with `keybind n "quit 10"` and
  `keybind N "quit 11"` (`mpv::NEXT_FILE`, `mpv::PREVIOUS_FILE`). `App::step_from_video` keeps
  `video_return`, so a run of videos from the gallery ends in the gallery. A step goes from the
  file that mpv played, not from the gallery cursor.
- **mpv window:** viewr follows each mpv window until it exits (`App::poll_video_window`). The
  run loop polls every 100 ms (`WINDOW_POLL`) while a window plays. A step in viewr away from
  the file of the window forgets the window. The window gets `--fs --native-fs=no` and
  `--hwdec=auto-safe`. `--fs` overrides `fs=no` of a user `mpv.conf`.
- **HDR in the terminal:** `player::probe` reads the transfer curve. For HLG and PQ,
  `player::tone_map::tone_map_file` writes a 3D cube (`tonemap-hlg-v2.cube`,
  `tonemap-pq-v2.cube`) into the thumbnail cache directory, or into `$TMPDIR/viewr` if the cache
  path holds filter graph syntax. If the cube output changes, change the version in the file
  names. `SDR_WHITE_NITS = 500` matches the macOS SDR export on three iPhone clips. The PQ cube
  has no test on a real PQ file.
- **Entry kinds:** the first `App::folder_count` entries of `images` are folders, `..` first.
  The scanner pushes them before the first file batch, and the sort at the end of the scan sorts
  the files only. Every step of the fullscreen view starts at `App::first_file()`, the prefetch
  gets the first file index, and the thumbnail jobs, the marks, the delete, the cell shape and
  the info panel skip folders. A folder cell has no thumbnail and no Kitty image. An empty
  folder (no file, and no folder besides `..`) shows a note in the status row (`is_empty_folder`
  in `ui.rs`). There is no directory picker. A start with several paths lists no
  folders. A symlink to a folder stays out. One image next to folders opens the fullscreen view
  at once, as in a flat folder.
- **Video programs:** viewr links no FFmpeg library. `main` looks up `ffmpeg` and `ffprobe` one
  time, and the tests never do, so `session_first_frame` gives `NotFound` in a test. The probe
  (`player/probe.rs`) is one `ffprobe -of json` process of about 32 ms with a 2 s timeout
  (`output_within`). A probe cache keys each `VideoInfo` by path, mtime and length, so
  `Player::open` after a cold thumbnail starts no ffprobe. An animation reads 17 packets, an
  `.avif` file 34. The first frame runs the player GPU chain with `-frames:v 1`, with the
  rotation and the cube, and waits at most 5 s. The player maps the probed stream with
  `-map 0:N` for every video. Without the programs, a video cell keeps its `▶` icon, and
  `Enter` opens the mpv window with a note.
- **yazi reveal:** `reveal_in_yazi` in `yazi.rs` runs after `TerminalSession::restore`, only when
  `YAZI_ID` is set and `ya` is on PATH. It waits at most 1 s for `ya` (`YA_TIMEOUT`), so a yazi
  that does not answer cannot hold viewr. Tests pass a closure and never start `ya`.
- **Status notes:** `ui::draw` takes a `status` note from `LoopState` in `run_loop.rs`. `App` holds no
  status note. The note shows in the status row at the bottom. The next key press clears it.

- **The run loop polls every 10 ms while a background job runs** (`App::has_pending_work`:
  a decode, a prefetch, a thumbnail, a filter, the scan or thumbnails that wait for their emit),
  because no event wakes it, and every 1 ms during the first 20 ms of the header read for the
  cell shape (`SHAPE_POLL`, `SHAPE_POLL_WINDOW`). Otherwise it waits 250 ms (`next_timeout` in
  `run_loop.rs`).
- **Decodes share the cores.** `LibheifSlot` counts the running libheif decodes and gives each
  context a share of the tile threads. The fullscreen and zoom decodes run inside `foreground`
  and take all cores. At most 4 thumbnail decodes (`MAX_THUMB_JOBS`) and 2 prefetch decodes
  (`MAX_PREFETCH_JOBS`) run at once. The other cells wait in a queue in grid order, but
  `next_sharp_job` starts the cells that show nothing before the HEIC sharp jobs. A failed
  thumbnail decode sends `None` and frees its slot. The thumbnail and preview queues are
  `JobSlots` with an image index per job. A job whose index left the list starts nothing.
  `decode::spawn_foreground` runs a foreground decode on the thread pool and returns its
  receiver.
- **Kitty temp file medium:** the temp file medium wins only when the probe answer is `OK` and
  the terminal deleted the probe file. A terminal with another `TMPDIR` reads the file but keeps
  it, and viewr would leave a 17 MB file per frame. The file name contains
  `tty-graphics-protocol`, as the protocol requires. A terminal that answers the probe later
  than 300 ms sends the answer as key presses.
- **Kitty images are not ratatui widgets.** ratatui draws the frame and the borders. viewr then
  writes the pictures with Kitty escape sequences at the cell positions of the last draw. A
  popup must delete the pictures first (`graphics_delete_all`), because Kitty draws them above
  the text.
  A key handler deletes the pictures only before an overlay. A zoom or a resize calls
  `mark_dirty`, and the grid write deletes the old pictures itself. A delete without a later
  draw leaves black cells.
- **Grid gaps and borders:** the cells of the grid have 1-cell gaps (`GAP` in `gallery.rs`). A
  line of n cells takes n + 1 gaps: one between two neighbors and one at each edge.
  `update_grid` subtracts the gaps first and gives the remainder to the first columns and rows.
  The decode job pads every grid picture with transparent pixels to whole cells, at most the
  base cell box (`pad_for_grid` in `app/thumbnails.rs` gives a `GridPicture` with its cell
  count). The grid places it centered in its cell in whole cells, with `c=` and `r=` of that
  count and no `X=` / `Y=`, so the content is centered to within half a cell. herdr relays a
  pixel offset as a stretched picture, and a rounded cell size cannot push a padded picture
  into the gap. The selected cell gets a thick border
  (`BorderType::Thick`, `STYLES.cursor_cell`) in the gaps around the cell and its label
  (`GalleryState::border_rect`). Each marked cell gets a plain yellow border (`STYLES.marked`).
  Neighbors share the gap line. The selected border is drawn last, so it wins. A border that the
  grid edge cuts stays open at that edge. No cell has a background fill. Every thumbnail is
  decoded for the base cell size, so a wider column only has a larger margin.
- **Terminal storage:** `TerminalImages` in `kitty_output.rs` tracks the images that the
  terminal stores, with their bytes (4 bytes per pixel, also for a picture that went out as RGB,
  so the count is an upper bound). The grid deletes evicted and replaced ids with `a=d,d=I`
  and trims the oldest unshown images above `TERMINAL_IMAGE_BUDGET` (128 MB). Ghostty evicts
  unplaced images at 320 MB, and herdr stores each image twice. A trimmed cell sends its
  picture again with `a=T`. The in-memory cache holds `max(200, 2 * visible cells)` entries
  and at most `THUMB_CACHE_BYTE_LIMIT` (512 MB) of padded pixels. It never evicts a visible
  cell. The cache also holds the parked sharp thumbnails of the folders that the grid left
  (`ThumbnailCache::park`, `take_parked`), keyed by path and decode size, with the mtime and
  the length of the file. A changed file decodes again. Parked pictures count against the limit
  and go first. The grid writes run in a synchronized update.
- **Cell size:** `query_cell_pixel_size` asks with `CSI 16 t`, because TIOCGWINSZ includes the
  window padding of Ghostty (2258 px / 52 rows = 43.42 px). If the terminal answered at
  start, the run loop asks again after each resize. The read takes every byte of the tty, so
  a key typed in that millisecond is lost. At the start, the query goes out together with the
  Kitty probe, in one round trip. A terminal that does not answer one of them costs
  `PROBE_TIMEOUT` (300 ms) one time.
- **Mouse:** viewr enables only the mouse modes 1000, 1002 and 1006 (`terminal::enable_mouse`),
  never 1003, so a pointer move without a button does not wake the run loop. Do not use
  crossterm's `EnableMouseCapture`, because it turns on 1003. `[gallery] mouse = false` turns
  the capture off. The run loop releases the mouse while an mpv window plays
  (`sync_mouse_capture`), and `TerminalSession::restore` releases it on exit and on a panic.
  `ui::draw` records `app.layout`, and a mouse handler reads the regions through
  `ui::split_info`, `ui::content_area` and `ui::status_area`. A test that needs them calls
  `test_support::draw_frame` first. The double-click (400 ms) uses `handle_event_at` with a
  clock, so tests add a `Duration` to an `Instant` and never sleep. macOS can send Shift+wheel
  as a horizontal wheel, so `ScrollLeft` and `ScrollRight` change the grid size too.
- **Late probe answer:** `LateAnswerFilter` in `keys/mod.rs` drops the key events of a Kitty
  probe answer that comes after `PROBE_TIMEOUT`, from Alt+`_` to Alt+`\`.
- **Grid zoom:** the grid shows 2 to 12 rows (`MIN_VISIBLE_ROWS`, `MAX_VISIBLE_ROWS`). A 2x2
  grid is the largest thumbnail size. The fullscreen view is the larger view. The config key
  and the state file clamp to that range on load. A terminal too small for 2 rows keeps the
  rows that fit.
  The state file holds a global `visible_rows` fallback, a `zoom_counter` and a `[zoom]` table
  keyed by the canonical folder path. Each `[zoom."/path"]` entry holds `rows` and `order`. The
  plain form `"/path" = 3` also loads. `+` / `-` save the level for the folder and as the
  global fallback. `0` resets the folder and removes its entry. The table keeps the 200 most
  recent folders.
- **Loading icon:** a grid cell without a thumbnail shows a dim `◌` (image) or `▶` (video) at its
  center (`draw_loading_icon` in `ui.rs`, `App::is_video_at`). The icon is text in the ratatui
  frame, so the thumbnail that Kitty draws above it covers it. It needs no redraw and has no
  animation. A HEIC preview counts as a thumbnail.
- **Cell shape:** `App::request_cell_shape` reads the headers of the first `SHAPE_SAMPLE` (64)
  files on a thread, one time per scan (`decode/shape.rs`). JPEG and PNG swap for the EXIF
  orientation, HEIC and AVIF read `ispe` and swap for `irot`. A RAW or PSD file gives the size of its
  embedded JPEG after the orientation. PDF, SVG, the other ImageIO formats and video count as
  3:2. The median snaps to one of 7 buckets (9:16, 2:3, 3:4, 1:1, 4:3, 3:2, 16:9). The column
  count includes 1 gap per column plus the edge gap. The first
  screen waits for the shape (2 ms to 4 ms), and the shape stays until the next directory
  switch, so the columns never move while the user browses.
- **Resize filter:** every resize uses Catmull-Rom (`resize_options` in `decode/resize.rs`). It is one
  third faster than Lanczos3, and the difference is not visible.
- **Process start:** about 30 ms pass before `main`, most of it the dylib load of libheif and its
  codec libraries (a C program that links libheif needs 24.9 ms, with no library 4.6 ms).
  Foundation (the `trash` crate) costs about 3.5 ms. The `dist` profile and `target-cpu` give no
  measurable gain, so `just install` builds `--release`.
- **Disk cache format:** the cache files are QOI (`.qoi`): a hit reads and decodes in about
  0.75 ms instead of 1.7 ms for PNG, at the same size. Raw RGBA reads faster but takes 2.3 times
  the space. The prune removes the `.png` files of older versions at once.
- **Thumbnail cache key:** the key hashes `viewr-thumbnail-v11` (`THUMBNAIL_CACHE_VERSION`), the
  path, the size, the mtime, the target size and the color target. Change the version string when the thumbnail
  output changes. The folder prefetch writes the same key (`warm_disk_cache` calls
  `sharp_thumbnail`). The prune removes temp files of the cache that are older than 1 hour.
- **Folder prefetch:** after the cursor rests on a folder cell for `FOLDER_PREFETCH_DELAY`
  (300 ms) and `App::has_pending_work` is false, one thread decodes the first screen of that
  folder into the disk cache (`app/folder_prefetch.rs`). It computes what Enter computes, with
  the same functions (`scanner::files_of`, the shape sample, `sort_by_name`, `folder_zoom_rows`,
  `GalleryState::for_folder`), and takes the union of both cursor orders. It pauses before each
  file while the grid has work, so one running decode can overlap the grid. A cursor move, a
  folder change, a zoom, a resize, a mode change or the quit cancels it, and the next start
  waits for the old thread. `..` takes part only after a real cursor move. A folder that this
  session prefetched, or one with fewer than 2 files, does not start. Only the folder switch
  bench ticks the prefetch.
- **Image colors:** Ghostty on macOS draws into a Display P3 surface, and its image shader copies
  the pixels of a Kitty image without a conversion, also with `window-colorspace = srgb` (that
  setting converts only the text colors). So viewr converts each image into Display P3
  (`[terminal] colorspace`, default `display-p3`, or `srgb`). Each decoder reports the color of
  its pixels: ICC from JPEG APP2 (also over several segments), PNG iCCP, WebP ICCP, TIFF tag
  34675 and HEIF `prof`/`rICC`, CICP from HEIF `nclx` and PNG `cICP`, and PNG `gAMA`/`cHRM`.
  The conversion runs after the resize, as one cached `moxcms` transform with the rows in
  parallel. A source in the target space converts nothing, so iPhone photos (Display P3) cost
  nothing. A source that cannot convert (an unreadable or non-RGB ICC profile, unknown CICP
  codes) converts as sRGB. PQ and HLG sources keep their values. `main` sets the target, and
  tests keep sRGB. Videos, video thumbnails and the frames of animations in the terminal player
  do not convert yet.
- **JPEG XL colors:** jxl-oxide 0.12 converts ICC v2 samples wrong. A lossless file with an ICC
  profile renders in its own space, and `moxcms` converts it (16 bit for deeper files, in tasks
  of 64 rows). A transform error, XYB, the enum encodings and a PQ or HLG ICC profile stay with
  jxl-oxide. Nobody tone-maps a PQ or HLG ICC profile yet. The codestream orientation applies,
  the EXIF orientation does not.
- **`image` decoders:** the `image` crate has only the decoders that the scanner lists
  (`default-features = false`). If you add a format to the scanner, add the `image` feature
  too. The `image` feature `avif` is the encoder, not a decoder.
- **Embedded HEIC thumbnails:** a cold HEIC cell decodes in two phases. Phase 1 decodes the
  embedded thumbnail (iPhone: 320x240) and shows it as a preview (`start_preview_decode`, at most
  `MAX_PREVIEW_JOBS`). The thumbnail must have the same `irot` and `imir` as the primary image.
  `decode/heif_boxes.rs` reads them. The preview goes out at its own resolution, padded to the
  shape of the box, and the terminal scales it to the box of the sharp picture with `c=` / `r=`.
  Phase 2 is the sharp decode through `MAX_THUMB_JOBS`. The sharp jobs of libheif files wait
  until every preview is out (`sharp_job_waits`). Other formats start at once. A preview never
  goes into the disk cache, and a disk cache hit skips the preview. The sharp image gets a new
  Kitty id. `emit_new_thumbnails_to` deletes the preview image (`a=d,d=I`) before the transmit
  (`app.stale_image_ids`). The sharp decode still uses the embedded thumbnail when it is at least
  the fitted size of the primary image. Fullscreen, zoom and prefetch pass
  `EmbeddedThumbnail::Deny`.
- **libheif reads through a stream.** `open_heif_context` opens a file with
  `libheif_rs::StreamReader`. `HeifContext::read_from_file` copies the whole file first, which
  costs 30 ms to 40 ms per 24 MP photo.
- **The `sips` route:** sips writes an uncompressed TIFF with the Orientation tag into a temp
  file. The grid and the fullscreen view ask for a copy at the size of their box (`-Z`), and the
  zoom gets the full image. A relative path gets `./` in front, because sips reads `-x` as a
  function name. JPEG, PNG and HEIC never take this route. RAW and PSD try the embedded JPEG
  first (`decode/embedded_jpeg.rs`): the largest JPEG of the TIFF IFDs, the CR3 boxes, the RAF
  header or PSD resource 1036. It is the grid preview. A JPEG that covers the box is the sharp
  thumbnail and goes into the disk cache, so sips does not run for the cell. The fullscreen view
  and the prefetch also take a JPEG that covers their box. The zoom keeps sips. ORF has no
  reachable preview.
- **Embedded JPEG orientation:** the container orientation (IFD0 of a TIFF RAW, `CMT1` of a
  CR3) wins over the EXIF of the embedded JPEG. Without a container orientation, the EXIF of
  the JPEG applies (RAF). The turbo path, the `image` fallback and the cell shape use this one
  rule. The DCT scaling of a rotated JPEG compares against the turned box.

## Workflow for agents

1. Read `TODO.md` for the current list.
2. Run `just check` after each change. If clippy has suggestions, run `just fix` first.
3. Never commit, reset, rebase or stash. When the work is ready for a commit, stop. Report the
   changed files and a suggested commit message. The maintainer commits. The only exception:
   the orchestrator of the herdr-worktrees skill merges approved worker branches through its
   scripts.
