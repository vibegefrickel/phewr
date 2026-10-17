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
serde/toml. The features `turbo` (libjpeg-turbo), `heic` (libheif) and `video` (FFmpeg) link
native Homebrew libraries. All three features are on by default.

## Build, test, run

```sh
just                 # = just check: fmt --check, clippy -D warnings (2 feature sets), nextest, rustdoc, cargo deny
just fix             # clippy --fix + cargo fmt
just test [FILTER]   # cargo nextest (falls back to cargo test)
just test-real       # also the ignored tests that decode VIEWR_TEST_JPEG and VIEWR_TEST_HEIC
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

- Config: `~/.config/viewr/config.toml`, or `$XDG_CONFIG_HOME/viewr/config.toml`.
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
- **One formatter, one style.** Use rustfmt with `rustfmt.toml`. The hook runs
  `cargo fmt --check`.
- **Language:** code, comments and docs are in English.

### Tests

- Unit tests sit in a `#[cfg(test)] mod tests` at the end of each module.
- Tests that need a real photo are `#[ignore = "..."]` and read `VIEWR_TEST_JPEG` or
  `VIEWR_TEST_HEIC`. `just test-real` runs them.
- A test that writes files works in its own directory below `std::env::temp_dir()`, with the
  process id in the name, and removes it at the end.

## Layout

- `main.rs`: CLI (clap), terminal session, event loop, key handlers, Kitty output per frame.
- `app.rs`: `App`, the whole view state. Decoding (`decode_image_with_hint`, turbojpeg,
  libheif), resizing, the disk thumbnail cache, slideshow, zoom and pan, delete.
- `ui.rs`: the ratatui drawing of each view, the help popup and the pickers.
- `gallery.rs`: the grid geometry, the cursor and the in-memory thumbnail cache.
- `scanner.rs`: the background directory scan and the list of supported extensions.
- `image_list.rs`: `SharedImageList`, the list that the scanner fills and the UI reads.
- `prefetch.rs`: decodes the neighbors of the fullscreen image ahead of time.
- `picker.rs`: the directory picker. `search.rs`: fuzzy filename search (nucleo-matcher).
- `info.rs`: file data and EXIF for the info panel, and the EXIF orientation.
- `theme.rs`: themes, the theme catalog and the shared theme file.
- `config.rs`: `config.toml` and the state file.
- `video.rs` (feature `video`): FFmpeg playback in a thread, and first-frame thumbnails.
- `encoder/`: the Kitty graphics protocol (`KittyBackend`).

## Gotchas

- **Kitty images are not ratatui widgets.** ratatui draws the frame and the borders. viewr then
  writes the pictures with Kitty escape sequences at the cell positions of the last draw. A
  popup must delete the pictures first (`graphics_delete_all`), because Kitty draws them above
  the text.
- **FFmpeg versions:** `ffmpeg-next` must match the major version of Homebrew's FFmpeg.
  ffmpeg-next 8 does not build against FFmpeg 9. After a major update of Homebrew's FFmpeg, a
  release binary that links the old `libavcodec.<major>.dylib` does not start.
- **Thumbnail cache key:** the key hashes `viewr-thumbnail-v1`, the path, the size, the mtime
  and the target size. Change the version string when the thumbnail output changes.

## Workflow for agents

1. Read `TODO.md` for the current list.
2. Run `just check` after each change. If clippy has suggestions, run `just fix` first.
3. Never commit, reset, rebase or stash. When the work is ready for a commit, stop. Report the
   changed files and a suggested commit message. The maintainer commits. The only exception:
   the orchestrator of the herdr-worktrees skill merges approved worker branches through its
   scripts.
