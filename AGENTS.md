# AGENTS.md

This file is the entry point for each person and coding agent that works on viewr. Read it
first. Then read `TODO.md` for the open work.

## What this is

viewr is a terminal image and video gallery for the Kitty graphics protocol, written in Rust.
It is a fork of [rview](https://github.com/jrf/rview) by jrf (MIT). The fork had the name
imgrrr before 2026-10-17.

- viewr is a viewer only. It does not edit, convert or write images.
- The only file operations are trash, permanent delete, and (planned) move and rename.
- The target is macOS on Apple Silicon, in Ghostty. viewr uses the Kitty graphics protocol, not the Kitty terminal. Nobody tests in the Kitty terminal.
- The maintainer is the only user today. Write user-facing texts for other people anyway.

The stack: ratatui 0.30, crossterm 0.29, clap, image 0.25, fast_image_resize, rayon, lru,
serde/toml, jxl-oxide. The features `turbo` (libjpeg-turbo), `heic` (libheif) and `video` (FFmpeg)
link native Homebrew libraries. The feature `sips` runs the macOS tool `/usr/bin/sips`. All four
features are on by default. The release binary does not bundle FFmpeg. FFmpeg stays a Homebrew
dependency.

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
- Thumbnail cache: `~/Library/Caches/viewr/`, pruned at 512 MB.

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

The release binary is arm64 only, because Homebrew's libheif and FFmpeg are arm64 libraries.
The formula depends on libheif, ffmpeg and mpv. `install.sh` stops if libheif or ffmpeg is
missing.

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

- `main.rs`: the module list and `main`: the FFmpeg init, the CLI checks, the terminal
  session and the start of the run loop.
- `cli.rs`: the clap arguments (`Cli`) and the start directory.
- `terminal.rs`: `TerminalSession` (raw mode, the alternate screen), the Kitty probe
  `query_terminal` and the cell pixel size.
- `run_loop.rs`: `LoopState` (the status note and, with the `video` feature, the `VideoState`), `run`, the background polls,
  `render_images` and `next_timeout`. The status note lives here, not in `App`.
- `keys/`: the key handlers. `mod.rs` holds `handle_event` and `handle_key`, the thin
  `match` that routes the overlays and the view modes. One file per view: `fullscreen.rs`,
  `gallery.rs` (with the search line), `picker.rs`, `video.rs` (feature `video`).
- `kitty_output.rs`: the Kitty writes per frame: the fullscreen image, the video frame, the
  grid and the new thumbnails. Each `render_*` has a `*_to` twin that writes to any `Write`,
  so that the tests read the escape sequences. `App::with_graphics_out` runs a write against the
  graphics output.
- `video_handoff/` (feature `video`): the hand-off to mpv and the steps with `n` and `N`.
  `mod.rs` holds `VideoState` (the mpv player and the fallback note), the hand-off and the way
  back. `terminal.rs` holds `TerminalHandoff` and `play_in_terminal`. `window.rs` holds
  `poll_mpv_window`.
- `yazi.rs`: the reveal of the last file in yazi, and the PATH lookup `find_on_path`.
- `app/`: `App`, the whole view state. `mod.rs` holds the struct, its fields and its
  construction. One file per topic: `info_panel.rs`, `grid_zoom.rs` (the cell size and the
  state file), `slideshow.rs`, `scan.rs` (the scanner results, the sort and the filter),
  `cell_shape.rs`, `fullscreen.rs` (enter, leave, the steps and the decode), `zoom.rs` (zoom,
  pan and the full-resolution source), `delete.rs` (the marks and the delete question),
  `directory.rs` (the picker and the directory switch), `video.rs` (the built-in player, the
  return after a video, the mpv window watch), `thumbnails.rs` (the jobs, the queue, the HEIC
  previews, the generations), `disk_cache.rs` (the thumbnail disk cache).
- `decode/`: the decoders and the decode routes. `Route::for_path` in `mod.rs` picks the
  route by extension, `decode_image_with_hint` runs it, `foreground` marks the decode that the
  user waits for. `raster.rs` (the `image` crate and the EXIF orientation), `turbo.rs`
  (libjpeg-turbo with DCT scaling), `libheif.rs` (HEIC, HEIF, AVIF, the embedded thumbnails
  and the core share `LibheifSlot`), `resize.rs` (`load_and_resize`, Catmull-Rom), `jxl.rs`
  (JPEG XL), `sips.rs` (camera RAW, PSD, PDF, SVG and other ImageIO formats), `tonemap.rs`
  (EXR and HDR to 8 bit), `heif_boxes.rs` (the safe ISOBMFF reader for `irot`, `imir` and
  `ispe`), `shape.rs` (the image dimensions from the file headers, for the cell shape).
- `ui.rs`: the ratatui drawing of each view, the help popup and the pickers. The gallery, the
  fullscreen view and the video view have no frame. They use the full area minus 1 status row
  (`content_area`, `status_area`). The help popup, the pickers and the info panel keep their
  borders.
- `gallery.rs`: the grid geometry, the cursor and the in-memory thumbnail cache.
  `update_grid` gives the remainder of the width and the height to the first columns and rows,
  so the grid reaches the edges. `set_cell_shape` picks the column count from the photo aspect
  and the pixel size of a terminal cell.
- `scanner.rs`: the background directory scan and the lists of supported extensions.
  `LIBHEIF_EXTENSIONS` is the one list for the libheif route.
- `image_list.rs`: `SharedImageList`, the list that the scanner fills and the UI reads.
- `prefetch.rs`: decodes the neighbors of the fullscreen image ahead of time. Results carry a
  generation. `invalidate()` drops the jobs in flight and forgets the failed indices. Each result
  (`Prefetched`) keeps its decode box. `take_resized` fits the image into the requested box. If
  the decode box is smaller, it returns `None`.
- `picker.rs`: the directory picker. `search.rs`: fuzzy filename search (nucleo-matcher).
- `info.rs`: file data and EXIF for the info panel, and the EXIF orientation.
- `styles.rs`: `STYLES`, one style per role on the screen.
- `config.rs`: `config.toml` and the state file.
- `video.rs` (feature `video`): the built-in FFmpeg player in a thread, and first-frame
  thumbnails.
- `mpv.rs` (feature `video`): the argument lists for mpv, the exit notes, the step exit codes,
  the HDR probe and tone mapping LUT, and the `Runner` trait for the process start.
- `encoder/`: the Kitty graphics protocol (`KittyBackend`). `Transfer` picks the medium: direct,
  PNG (SSH) or a temp file (`t=t`). `detect_transfer` probes the terminal at start with `a=q`.
  `VIEWR_KITTY_TRANSFER=direct|png|file` skips the probe.
- `test_support.rs` (test only): the app builders, the key press helpers and the synthetic
  file writers that the tests of more than one module share. `test_support::video` holds
  `FakeTerminal`, `FakeRunner` and the app builders of the mpv tests.
- `bench.rs` (test only): the ignored speed benchmarks. They drive the real loop steps without a
  terminal and read `VIEWR_TEST_DIR`.

## Gotchas

- **The mpv hand-off keeps raw mode and the alternate screen.** viewr deletes the Kitty images,
  runs mpv with `--vo-kitty-alt-screen=no` and waits. Raw mode keeps ISIG off, so Ctrl+C, Ctrl+\
  and Ctrl+Z reach mpv as keys and do not stop viewr. After mpv, viewr drops the queued input,
  deletes the images and clears the screen. Tests use a fake `mpv::Runner` and a fake
  `TerminalHandoff`. Never start a real mpv in a test.
- **Video placeholder:** after mpv, and after Esc in the built-in player, `App::video_return`
  selects the view. A video in `ViewMode::Fullscreen` is a placeholder
  (`shows_video_placeholder`). `load_if_needed`, `zoom_in` and `slideshow_tick` skip it, because
  the image decode cannot read a video.
- **Built-in player pause:** a pause blocks the decoder thread before the next packet
  (`follow_commands` in `video.rs`). The decoder tests drive `decode_loop` through a frame
  channel without a buffer (`sync_channel(0)`), so no assertion depends on the clock.
- **Slideshow and videos:** a running slideshow that is not paused plays a video one time. mpv
  gets `--loop-file=no`, `--keep-open=no`, `--idle=no`, `--loop-playlist=no` and
  `--input-commands` that bind `q`, `Q` (and `CLOSE_WIN` in a window) to `quit 4`. Exit code 0
  is the end of the file. Each other exit stops the slideshow on the placeholder. Esc in the
  built-in player also stops it. `--input-commands` needs mpv 0.38 or later. On macOS, the app
  quit (Cmd+Q) of a window mpv exits with 0, so a window slideshow continues.
- **mpv steps:** each mpv start gets `--input-commands` with `keybind n "quit 10"` and
  `keybind N "quit 11"` (`mpv::NEXT_FILE`, `mpv::PREVIOUS_FILE`). `hand_off_videos` in
  `video_handoff/mod.rs` starts the next video at once, without a draw. `App::step_from_video` keeps
  `video_return`, so a run of videos from the gallery ends in the gallery. A step goes from the
  file that mpv played, not from the gallery cursor.
- **Video player default:** `[video] player` defaults to `window` (`VideoPlayer::default`).
  mpv's Kitty output is too slow in Ghostty: a 4K HLG clip plays at 2.4 fps, Ghostty ignores
  `--vo-kitty-use-shm`, and a smaller `--vo-kitty-width` fills only a corner, because mpv's
  Kitty output does not scale. The window gets `--fs --native-fs=no`, which overrides `fs=no` of
  a user `mpv.conf`.
- **mpv window:** viewr follows each mpv window until it exits (`App::poll_video_window`). The
  run loop polls every 100 ms (`WINDOW_POLL`) while a window plays. A step in viewr away from
  the file of the window forgets the window.
- **mpv speed:** the terminal start gets `--hwdec=auto-copy-safe` and a scale filter that fits
  each frame to the terminal before the rotation. The size comes from the terminal size at the
  start of mpv, so a larger terminal during playback gives a soft picture. The window start gets
  `--hwdec=auto-safe`. The filters go through `--vf-pre`, so the filters of a user `mpv.conf`
  stay.
- **HDR in the terminal:** `--tone-mapping` and `--target-colorspace-hint` have no effect on
  `--vo=kitty`, because only the GPU outputs read them. `mpv::hdr_of` reads the transfer curve
  with FFmpeg before each terminal start. For HLG and PQ, viewr writes a 3D LUT
  (`tonemap-hlg-v2.cube`, `tonemap-pq-v2.cube`) into the thumbnail cache directory, or into
  `$TMPDIR/viewr` if the cache path holds filter graph syntax. If the LUT output changes, change
  the version in the file names. `SDR_WHITE_NITS = 500` matches the macOS SDR export on three
  iPhone clips. The PQ LUT has no test on a real PQ file.
- **FFmpeg log:** `main` sets the FFmpeg log level to `Quiet` directly after `ffmpeg::init()`, so
  no libav warning reaches the screen of the TUI.
- **yazi reveal:** `reveal_in_yazi` in `yazi.rs` runs after `TerminalSession::restore`, only when
  `YAZI_ID` is set and `ya` is on PATH. It waits at most 1 s for `ya` (`YA_TIMEOUT`), so a yazi
  that does not answer cannot hold viewr. Tests pass a closure and never start `ya`.
- **Status notes:** `ui::draw` takes a `status` note from `LoopState` in `run_loop.rs`. `App` holds no
  status note. The note shows in the status row at the bottom. The next key press clears it.

- **The run loop polls every 10 ms while a background job runs** (`App::has_pending_work`:
  a decode, a prefetch, a thumbnail, a filter, the scan or thumbnails that wait for their emit),
  because no event wakes it. Otherwise it waits 250 ms (`next_timeout` in `run_loop.rs`).
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
  it, and viewr would leave a 22 MB file per frame. The file name contains
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
  The decode job pads every grid picture with transparent pixels to the pixel size of the base
  cell (`pad_for_grid` in `app/thumbnails.rs`), with the content centered inside. The picture
  starts at a whole cell and goes out with `c=` and `r=` of the base cell and no `X=` / `Y=`.
  herdr relays a pixel offset as a stretched picture, and a rounded cell size cannot push a
  padded picture into the gap. The selected cell gets a thick border
  (`BorderType::Thick`, `STYLES.cursor_cell`) in the gaps around the cell and its label
  (`GalleryState::border_rect`). Each marked cell gets a plain yellow border (`STYLES.marked`).
  Neighbors share the gap line. The selected border is drawn last, so it wins. A border that the
  grid edge cuts stays open at that edge. No cell has a background fill. Every thumbnail is
  decoded for the base cell size, so a wider column only has a larger margin.
- **Terminal storage:** `TerminalImages` in `kitty_output.rs` tracks the images that the
  terminal stores, with their bytes. The grid deletes evicted and replaced ids with `a=d,d=I`
  and trims the oldest unshown images above `TERMINAL_IMAGE_BUDGET` (128 MB). Ghostty evicts
  unplaced images at 320 MB, and herdr stores each image twice. A trimmed cell sends its
  picture again with `a=T`. The in-memory cache holds `max(200, 2 * visible cells)` entries
  and at most `THUMB_CACHE_BYTE_LIMIT` (512 MB) of padded pixels. It never evicts a visible
  cell. The grid writes run in a synchronized update.
- **Cell size:** `query_cell_pixel_size` asks with `CSI 16 t`, because TIOCGWINSZ includes the
  window padding of Ghostty (2258 px / 52 rows = 43.42 px). If the terminal answered at
  start, the run loop asks again after each resize. The read takes every byte of the tty, so
  a key typed in that millisecond is lost.
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
  orientation, HEIC and AVIF read `ispe` and swap for `irot`. RAW, PSD, PDF and video count as
  3:2. The median snaps to one of 7 buckets (9:16, 2:3, 3:4, 1:1, 4:3, 3:2, 16:9). The column
  count includes 1 gap per column plus the edge gap. The first
  screen waits for the shape (2 ms to 4 ms), and the shape stays until the next directory
  switch, so the columns never move while the user browses.
- **Resize filter:** every resize uses Catmull-Rom (`resize_options` in `decode/resize.rs`). It is one
  third faster than Lanczos3, and the difference is not visible.
- **FFmpeg versions:** `ffmpeg-next` must match the major version of Homebrew's FFmpeg.
  ffmpeg-next 8 does not build against FFmpeg 9. After a major update of Homebrew's FFmpeg, a
  release binary that links the old `libavcodec.<major>.dylib` does not start.
- **Thumbnail cache key:** the key hashes `viewr-thumbnail-v6` (`THUMBNAIL_CACHE_VERSION`), the
  path, the size, the mtime and the target size. Change the version string when the thumbnail output changes.
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
  function name. JPEG, PNG and HEIC never take this route.

## Workflow for agents

1. Read `TODO.md` for the current list.
2. Run `just check` after each change. If clippy has suggestions, run `just fix` first.
3. Never commit, reset, rebase or stash. When the work is ready for a commit, stop. Report the
   changed files and a suggested commit message. The maintainer commits. The only exception:
   the orchestrator of the herdr-worktrees skill merges approved worker branches through its
   scripts.
