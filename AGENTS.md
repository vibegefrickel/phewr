# AGENTS.md

This file is the entry point for each person and coding agent that works on viewr. Read it
first. Then read `TODO.md` for the open work.

## What this is

viewr is a terminal image and video gallery for the Kitty graphics protocol, written in Rust.
It is a fork of [rview](https://github.com/jrf/rview) by jrf (MIT). The fork had the name
imgrrr before 2026-10-17.

- viewr is a viewer only. It does not edit, convert or write images.
- The only file operations are trash, permanent delete, and (planned) move and rename.
- The target is macOS on Apple Silicon, in Ghostty, Kitty and WezTerm.
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
- State (the remembered thumbnail size): `~/.local/state/viewr/state.toml`.
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
  handler is a thin `match` that calls one function per view mode (`handle_key` in `main.rs`).
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
  `VIEWR_TEST_HEIC`, `VIEWR_TEST_AVIF`, `VIEWR_TEST_JXL`, `VIEWR_TEST_RAW` or `VIEWR_TEST_PSD`.
  `just test-real` runs them. A test without its variable skips with a note.
- The `sips` tests run `/usr/bin/sips` to write synthetic PSD and PDF files.
- The synthetic libheif tests encode with the aom (AVIF) and x265 (HEIC) encoder plugins of
  Homebrew's libheif. If a plugin is missing, `just check` fails.
- A test that writes files uses `tempfile::tempdir()` (`TempDir`) or `tempfile::NamedTempFile`.
  The directory goes away on drop, also when the test panics.
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

- `main.rs`: CLI (clap), terminal session, event loop, key handlers, Kitty output per frame.
- `app.rs`: `App`, the whole view state. Decoding (`decode_image_with_hint`, turbojpeg,
  libheif for HEIC/HEIF/AVIF), resizing, the disk thumbnail cache, slideshow, zoom and pan,
  delete.
- `decode/`: the decoders outside `app.rs`. `Route::for_path` picks the route by extension.
  `jxl.rs` (JPEG XL), `sips.rs` (camera RAW, PSD, PDF, SVG and other ImageIO formats),
  `tonemap.rs` (EXR and HDR to 8 bit), `heif_boxes.rs` (the safe ISOBMFF reader for `irot` and
  `imir`).
- `ui.rs`: the ratatui drawing of each view, the help popup and the pickers.
- `gallery.rs`: the grid geometry, the cursor and the in-memory thumbnail cache.
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
- `mpv.rs` (feature `video`): video playback in mpv. The PATH lookup, the argument lists, the
  exit notes and the `Runner` trait for the process start.
- `encoder/`: the Kitty graphics protocol (`KittyBackend`).

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
- **Status notes:** `ui::draw` takes a `status` note from `LoopState` in `main.rs`. `App` holds no
  status note. The next key press clears it.

- **Kitty images are not ratatui widgets.** ratatui draws the frame and the borders. viewr then
  writes the pictures with Kitty escape sequences at the cell positions of the last draw. A
  popup must delete the pictures first (`graphics_delete_all`), because Kitty draws them above
  the text.
- **FFmpeg versions:** `ffmpeg-next` must match the major version of Homebrew's FFmpeg.
  ffmpeg-next 8 does not build against FFmpeg 9. After a major update of Homebrew's FFmpeg, a
  release binary that links the old `libavcodec.<major>.dylib` does not start.
- **Thumbnail cache key:** the key hashes `viewr-thumbnail-v4`, the path, the size, the mtime
  and the target size. Change the version string when the thumbnail output changes.
- **`image` decoders:** the `image` crate has only the decoders that the scanner lists
  (`default-features = false`). If you add a format to the scanner, add the `image` feature
  too. The `image` feature `avif` is the encoder, not a decoder.
- **Embedded HEIC thumbnails:** a grid decode passes `EmbeddedThumbnail::Allow`. viewr uses the
  embedded thumbnail if it is at least the fitted size of the primary image and has the same
  `irot` and `imir` properties. `decode/heif_boxes.rs` reads them. Fullscreen, zoom and prefetch
  pass `EmbeddedThumbnail::Deny`.
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
