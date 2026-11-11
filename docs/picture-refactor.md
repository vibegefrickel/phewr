# The shared picture pipeline: refactor plan

Status (2026-11-09): phases 1, 2, 3, 4 and 4b are done (`6b7482a`, `36ecdea`, `688dc4e`,
schnattr `8f45abc`, `455ac6f`). Step 2 of phase 4b is done too: schnattr runs the module of
`da45195` with `kitty-flush`, `kitty-events`, `kitty-layers` and `kitty-overlay`, moxcms 0.9.2 and
no `allow` on `mod pictures;`. Phase 5 is done (`caa87b3`, 2026-11-11): the self check of the
Kitty placements, and 0 lost cells in herdr after ten scrolls. The copies of phase 5 are in
schnattr and youtr. The module tests of `b6bf116` (2026-11-11) are not synced yet:
`just pictures-check` names five files in each copy. Phase 6 runs in the youtr session
(uncommitted on 2026-11-11).
Phase 0 is open: the maintainer measures the baseline in a real terminal and fills the herdr note.
This file in phewr is the master of the plan. schnattr keeps a copy of the draft of 2026-11-09.

phewr, schnattr and youtr each decode, shape and send pictures to the terminal. phewr does this
best: it is the fastest, it has the most formats, and it measured each step. This plan makes
phewr the owner of one picture pipeline. schnattr and youtr take it over, as youtr took over the
screen frame of schnattr (`docs/frame-refactor.md`).

The plan also fixes the pictures that go away under herdr. On 2026-11-09, the maintainer showed
that the fault is in herdr: in a plain Ghostty window, the pictures of schnattr stay.

## Goals

1. The same picture code in the three projects, with one owner: phewr.
2. Every picture shows, stays on screen, and comes back after a scroll, a resize, a focus change
   and an overlay. This applies in Ghostty, in Kitty, in WezTerm and inside herdr.
3. The time to the first picture, the bytes on the wire and the memory of the terminal are as low
   as phewr has them today, or lower.
4. A terminal without the Kitty graphics protocol shows text, and the app says so once.
5. Each project keeps its own data: the files of phewr, the attachments of schnattr, the
   downloads of youtr.

## The pipelines today

| Part | phewr | schnattr | youtr |
|---|---|---|---|
| Decode | `decode/` (6 300 lines): `Route::for_path`, libjpeg-turbo with DCT downscale, libheif, jxl-oxide, `sips` for RAW, PSD, PDF and SVG, embedded JPEG previews, color management to sRGB (moxcms), HDR tone mapping | `images/decode.rs`: the `image` crate, the EXIF orientation, HEIC through `qlmanage` with `sips` as the fallback | the `image` crate (JPEG, PNG, WebP) on downloaded thumbnails |
| Resize | `fast_image_resize` (SIMD), `rayon` | `image` `FilterType::Triangle` | `image` `FilterType::Triangle` |
| Shape | thumbnails padded to whole cells | `images/copies.rs`: the box copy, the 1 px inset, `round_corners`, the dimmed copy, tiles, the 1 024 px cap | `thumbs/load.rs`: the box copy, the inset, the corners, the dimmed copy |
| Jobs and caches | at most `MAX_THUMB_JOBS` decodes, grid order, a memory LRU of at most 512 MB that never evicts a visible cell, a disk cache of thumbnails, folder prefetch | at most 2 decodes, the newest first, an LRU of 48 entries with frame pins | an LRU of 256 small pictures |
| Kitty output | `encoder/` and `kitty_output/`: a probe, the transfer medium (`t=t` temp file local, `t=d` or PNG over SSH), `a=T`, at most 4 MB per pass, a budget of 128 MB, only changed placements, `N=1` for transient frames, the cell size through `CSI 16 t` | `frame/kitty.rs` (the shared frame engine): `t=d` only, all pictures in one frame, a placement for each picture on each frame, `place_under`, `maybe_stored`, a budget of 128 MB | the same frame engine |

## Why the pictures go away under herdr

herdr 0.9.5 keeps its own Kitty image store and relays pictures to the host terminal. The two
herdr notes in `~/Projects/dev` describe its paths:

- The native path takes `t=t` (temp file) transfers and passes them to the host. It allows one
  waiting transfer per client.
- When the geometry of a placement changes while a transfer waits, herdr turns the native path
  off for the session. It resets its delivery cache and uploads every picture again
  (`herdr-issue-kitty-graphics-performance.md`, finding 2).
- herdr replays every placement on every client frame (finding 3).

schnattr sends every picture inline (`t=d`), sends all new pictures in one frame, and places every
picture again on each frame. A scroll then changes many placements while large inline transfers
wait. This is the case in which herdr resets its delivery and loses pictures. phewr avoids most of
this: it uses `t=t`, at most 4 MB per pass and only changed placements. The maintainer also runs
phewr outside herdr, in its own Ghostty window.

## Decisions

The maintainer decided these items on 2026-11-09:

1. **A copied module, not a crate.** phewr keeps the master copy in `src/pictures/`. A sync recipe
   in phewr (`just pictures-sync [target]`) copies it into schnattr and youtr, writes a `SOURCE`
   file with the sums, and refuses a copy with edits of its own, as `just frame-sync` does. The
   code must pass the union of the lint tables of the three projects. A sync into a project needs
   the explicit go of the session of that project.
2. **Built from source, no release.** There is no release, no package and no installer for any of
   the projects (decision of the maintainer, 2026-11-09). Each user clones a repository and runs
   `just install`. schnattr and phewr link the Homebrew `libheif` and `jpeg-turbo` at build time,
   so all decoders (`turbo`, `heic`, `jxl`, `sips`) run there. youtr decodes only JPEG, PNG and
   WebP thumbnails and links no native library: it takes the module with `heic`, `jxl` and
   `turbo` turned off.
3. **The frame engine moves.** `frame/kitty.rs` moves into `src/pictures/kitty/`, merged with the
   phewr output. The frame keeps only the `Pictures` trait and stays free of pictures.
4. **Only changed placements.** Only a new or a moved placement goes out, as in phewr. The resend
   after a focus gain stays. The "place every picture on each frame" fix of 2026-11-08 goes,
   because herdr replays placements itself. youtr keeps placing every picture on each frame until
   the self check of phase 5 works under herdr. If the self check does not work, youtr keeps that
   policy or takes the resend after a scroll stops.

## The module

```text
src/pictures/
  decode/    the routes (from phewr decode/): the image crate, turbo (feature), heic (feature),
             jxl (feature), sips and qlmanage (macOS), embedded previews, the EXIF orientation,
             color management to sRGB, HDR tone mapping
  resize/    fast_image_resize: fit, cover, exact, the whole-cell pad
  shape/     the box copy of a place: the 1 px inset, round_corners (10 % of the shorter side),
             the alpha dim (x 0.5), the tile cut, the long-side cap
  jobs/      the decode pool: a bound on parallel decodes, the order of the screen, a panic guard,
             the memory LRU with frame pins, an optional disk cache of thumbnails
  kitty/     the probe, the transfer medium, the terminal store with its budget, the placements
             with ids and a z index, the per-pass byte cap, the synchronized update, the deletes,
             the resend after a clear, the cell size
```

The module must build and pass its tests with `heic`, `jxl` and `turbo` turned off, because youtr
takes it that way. Then its `Cargo.toml` has no `libheif-rs`, `turbojpeg` or `jxl-oxide`. Each
project declares the dependencies of the module in its own `Cargo.toml`: `image`,
`fast_image_resize`, `moxcms`, `jxl-oxide`, `kamadak-exif`, `turbojpeg`, `libheif-rs`, `base64`,
`rayon` and `lru`, with the same versions as phewr. The sync makes sure that the versions agree.

The interface for an app:

- The app gives a source (a path, or bytes from a download) and a key.
- The app asks for a place: a key, the cells, and a shape (`Chat`, `Message`, `Tile`, `Fitted`,
  `Fullscreen`) with a shade (bright, dimmed) and a layer (above the text, under the cells).
- After the draw of ratatui, the app calls one flush. The flush writes the deletes, the
  transfers up to the byte cap, and the changed placements, in one synchronized update.
- Events from the app: resize, clear, focus gain, overlay on, quit.

What stays in each app: the stand-in picture of schnattr, the download of youtr, the folder scan,
the video and the loops of phewr, and each app's own keys for its pictures.

## The rules of the Kitty layer

These rules make the output fast and keep every picture on screen:

1. **Transfer medium.** A local terminal gets `t=t`: the pixels go into a temp file, and the
   terminal reads and deletes it. A transfer then is about 100 bytes on the pty. Over SSH, or when
   the probe fails, the transfer is `t=d` or a PNG (`f=100`). An environment variable overrides
   the choice, as `PHEWR_KITTY_TRANSFER` does today.
2. **Pixel format.** RGB (`f=24`) for an opaque picture, RGBA (`f=32`) only with transparent
   pixels.
3. **Byte cap per pass.** A pass writes at most 4 MB of new pixels, in the order of the screen,
   the cursor first. The next frame continues. A picture without its pixels in the terminal shows
   its placeholder text until it arrives.
4. **Placements.** One placement id per place. Only a new or a moved placement goes out. A
   placement that leaves the screen gets its delete. A z index below the cell grounds puts a
   picture under the cells (the more tile of schnattr).
5. **Terminal store.** The layer counts 4 bytes per pixel and stays under a budget of 128 MB. Above
   it, the oldest pictures off screen get `a=d,d=I`. A clear or a focus gain keeps their ids for a
   later delete (`maybe_stored`).
6. **Recovery.** After a resize, a clear, a focus gain or the end of an overlay, the next passes
   send the pictures on screen again, within the byte cap.
7. **Self check (phase 5).** The layer can place with `q=1`. Then the terminal answers an error
   for a picture that it does not hold (`ENOENT`). The layer reads the answer and sends that
   picture again. This needs a reader for the APC answers next to the key events, as the phewr
   probe has. Phase 5 makes sure that Ghostty and herdr send these answers.
8. **No pixel offsets.** No `X=` or `Y=`, because herdr stretches such a picture. A box copy has
   whole cells, padded with transparent pixels.
9. **Synchronized update.** One update around the cells of ratatui and the picture writes of a
   frame.

## Phases

Each phase is one herdr worker with the tier `heavy` and the automatic review, in the repository
that owns the change. Each phase is done when `just check` passes in each project that it touches.

### Phase 0: preparation (no worker)

1. The maintainer decides the four items of "Decisions".
2. Measure the baseline in each app, in plain Ghostty and in herdr:
   - the time from the start to the first picture on screen
   - the bytes on the pty for one screen of pictures
   - the memory of Ghostty and of herdr after one scroll through a large chat, folder or channel
   - the pictures that go away after ten scrolls up and down (schnattr: the family chat).
3. Write a herdr issue note, `herdr-issue-kitty-graphics-lost-pictures.md`, next to the two
   existing notes. It describes the lost pictures after a scroll with inline transfers, with a
   short Python script that reproduces it, as the performance note has.
4. Commit all trees: phewr, schnattr, youtr.

### Phase 1: the module in phewr (pure motion)

1. Make `src/pictures/` in phewr, with the layers of "The module".
2. Move `decode/`, the resize helpers, the thumbnail padding, the job pool, the disk cache, the
   memory cache and `encoder/` and `kitty_output/` into the module layers. The phewr app keeps its
   folder scan, grid, video, loops and keys.
3. Break the ties of `kitty_output` to the phewr `App`: the budget trim and the deletes take plain
   inputs. The module imports no app type of any project.
4. Add `just pictures-sync` and `just pictures-check` to phewr, modeled on the frame recipes of
   schnattr.
5. phewr uses the module. The benches of phewr give the same numbers within 5 %.

Done when `just check` and the benches of phewr pass, and no behavior changes.

Result (`6b7482a`, 2026-11-09):

- `src/pictures/` has 49 files and 13 716 lines in the five layers, and a test-only
  `test_support/`. `src/decode/` and `src/encoder/` are gone. The module imports no app module.
- `trim_to_budget` and `delete_image_to` take `&mut TerminalImages`, not the `App`.
- `just pictures-sync <target>` and `just pictures-check <target>` work. The worker tested them
  against a directory under `/tmp` only.
- `just check` passes with 952 tests. The benches stay within 5 % of `main` on a quiet machine.
  The pre-review measured "10 grid decodes in parallel" at 468 ms against 457 ms.

Open items for the next phases:

- Done in phase 3: the five gates on the phewr feature `video` are gone from the module.
- Done in phase 3: the feature `jxl` gates the JPEG XL route, and `jxl-oxide` and `jxl-render`
  are optional.
- Done in phase 3: each project sets the lint level of the module on its `mod pictures;` line, so
  a `pub(crate)` item without a caller does not fail clippy.
- The module uses these crates besides the list of "The module": `ratatui`, `crossterm`,
  `directories`, `rustix`, `tempfile` and `jxl-render`.
- Done in phase 3: the module takes its names from `CARGO_PKG_NAME`, and phewr keeps its names.
- Done in phase 4b: the module tests read `tests/fixtures/`, and `just pictures-sync` copies
  these fixtures into the copy.
- `pictures::kitty` still holds `Loops`, `transmit_frame` and the animation writes of
  `encoder/`. Phase 2 decides whether they belong to the app.

### Phase 2: one Kitty layer (phewr repository)

1. Merge the frame engine of schnattr into `pictures::kitty`: the placement ids per place, the z
   index under the cells, `hide`, `maybe_stored`, the resend after a focus gain.
2. Apply the rules of the Kitty layer above. Keep `N=1` for transient frames of phewr.
3. Tests with a fake writer: the bytes for each rule, the byte cap over several passes, the order
   of the screen, the recovery after a clear.

Done when `just check` passes in phewr, and phewr runs on the merged layer.

Result (`36ecdea`, 2026-11-09):

- `pictures/kitty/screen.rs` is the frame engine: `Placements` (`place`, `place_under`,
  `place_over`, `keep`, `hide`), `Screen`, `Update`, `Placing` (`Changed` by default,
  `EveryFrame` for youtr) and `Layer`. `docs/grid-and-kitty.md` describes it.
- The phewr grid writes one frame per pass through the engine. A redraw has no byte cap, as
  before. A preview keeps its place until its sharp picture shows.
- `Loops`, `transmit_frame_to` and `animate_to` stay in `pictures::kitty`, because `N=1` depends
  on the probe result.
- The pre-review found 2 issues: a second grid pass of the icons in one run-loop pass, and the
  loop benches outside 5 %. The worker fixed both. An interleaved A/B with 6 rounds at a load
  below 4 gave the cold loop benches 2 % to 3 % faster than `main`. The warm animation fill is
  bimodal in both trees.

Open items for phase 4 and phase 5:

- The engine report maps each part of `frame/kitty.rs` of schnattr: `Placements::place` and
  `place_under`, `hide`, `Screen::flush` for `update`, `resized`, `cleared`, `focus_gained`,
  `delete_all`, `Pixels`, `TERMINAL_IMAGE_BUDGET` for `TERMINAL_BUDGET`, and the sync constants.
- phewr keeps `d=A` on an overlay, a resize and the quit, and it has no focus reports (mode 1004).
  With `hide()` and focus reports, phewr gets the store back after an overlay and the resend of
  rule 6. This changes the behavior of phewr. Decided on 2026-11-09: the measurement in herdr at
  the start of phase 5 picks one of the two.
- Nothing calls `Screen::lost` yet. The self check of phase 5 is its first caller.

### Phase 3: the shapes and the caches (phewr repository)

1. Move the box copies of schnattr into `pictures::shape`: the inset, the corners, the dim, the
   tile cut, the cap. Use `fast_image_resize` for each resize.
2. The job pool takes a priority (the screen order, the cursor first) and a bound per app.
3. The memory cache counts bytes, not entries, and never evicts a picture on screen. The disk
   cache is optional per app.

Done when `just check` passes in phewr.

Result (`688dc4e`, 2026-11-09):

- `pictures::shape::copies` and `pictures::shape::corners` hold the box copies of schnattr with
  their tests. Each resize goes through `fast_image_resize` (Catmull-Rom). phewr calls no box copy.
- `ThumbJob`, `JobQueue` and `JobSlots` are generic over the key, and `Priority::on_screen` gives
  the order of the screen. The cell at the cursor decodes first (a decision of the maintainer).
- `ThumbnailCache::new` takes `CacheLimits { bytes, min_entries }` from the app. phewr passes
  `THUMB_CACHE_LIMITS` (512 MB, at least 200 entries), so its evictions stay the same.
- The new feature `jxl` gates the JPEG XL route. The module has no `video` gate.
  `just test-lean` runs every test without `heic`, `jxl` and `turbo`.
- `pictures::names` takes the names from `CARGO_PKG_NAME`. In phewr, each value is byte-for-byte
  the same, so the disk cache of the maintainer stays valid.
- The disk cache has no default folder. The app gives the folder, and `thumbnail_dir_below` gives
  `<root>/thumbnails`. The prune refuses a folder that holds a subfolder.
- `just check` passes with 1013 tests, and with 940 tests in the lean set. An interleaved A/B
  against a copy of `main` showed no regression in the bench.

Open items for phase 4:

- The interface for schnattr:
  - `pictures::shape::copies` (`Kind`, `Bitmap`, `boxed`, `dimmed`, `capped`) replaces
    `images/copies.rs`. `BoxCopy` stays in schnattr.
  - `pictures::shape::corners` (`round_corners`, `CORNER_RADIUS`) replaces `images/corners.rs`.
    `round_corners` takes and returns an `RgbaImage`.
  - `pictures::jobs::queue` (`ThumbJob`, `JobQueue`, `JobSlots`, `Priority`) and
    `pictures::jobs::pool` (`spawn_job`, `ThumbPhase`) replace the decode workers. schnattr keeps
    its bound (`MAX_DECODES = 2`) through `JobSlots::new`.
  - `pictures::jobs::thumbnail_cache` replaces the `LruCache` of schnattr, with
    `CacheLimits { bytes, min_entries: None }`.
  - The disk cache (`jobs::disk_cache`) is optional. An app that uses it passes
    `thumbnail_dir_below(<its cache root>)`.
- Each project sets the lint level of the module on its `mod pictures;` line. The module holds no
  lint level for the whole module.
- The `sips` feature must exist in each copy, because the module gates on it.
- The value of the memory cache is `GridPicture`, not `Bitmap`. If schnattr caches `Bitmap`
  copies, phase 4 must make the value generic.
- schnattr passes `thumbnail_dir_below(root)` and never its cache root, because that root holds
  `media/` and `gallery/`.
- Decision 1 holds the names. If the module becomes a workspace crate, `names.rs` must take a
  value from the app. In a workspace crate, `CARGO_PKG_NAME` is the name of that crate.

### Phase 4: schnattr takes the module (schnattr repository)

1. After the go of the schnattr session, phewr runs `just pictures-sync ../schnattr`. Add the
   dependencies of the module to the `Cargo.toml` of schnattr.
2. Replace `images/decode.rs`, `images/copies.rs`, `images/corners.rs` and the decode workers with
   the module. Keep `images/stand_in.rs` and the keys of the cache (`avatar_key`, the attachment
   ids).
3. The frame keeps the `Pictures` trait. `frame/kitty.rs` goes: the frame calls the module through
   the trait. Sync the frame to youtr after the go of the youtr session.
4. Make sure that `just build` links the Homebrew libheif and libjpeg-turbo (`otool -L`).

Done when `just check` passes, and the render tests show the same text.

Result (schnattr `8f45abc`, 2026-11-09):

- schnattr holds the copy of `61c5256` with all decoders in `src/pictures/`. Its own decode, box
  copies, corners, decode workers and `frame/kitty.rs` are gone. `images/stand_in.rs` stays.
- schnattr keeps its own LRU of box copies in `src/tui/images.rs`, not `ThumbnailCache`. The LRU
  holds the pending and the failed decodes, the file stamp of each avatar and the avatar pin.
  schnattr uses no disk cache.
- schnattr uses `Placing::Changed` and the temp file medium `t=t`. The first flush of a full
  screen sends 3 133 bytes instead of 2.5 MB. An unchanged frame sends 0 bytes instead of 713.
- The live check in herdr and in Ghostty passed, with pane switches and tab switches.
- Two interim steps stay until step 2 of phase 4b: the pin `moxcms = "=0.9.1"`, and
  `allow(unfulfilled_lint_expectations)` on `mod pictures;`.
- The sync of phase 4 copied no fixtures. The module fixtures went into schnattr by hand.

### Phase 4b: a feature per part of the engine interface (phewr repository)

Decision of the maintainer, 2026-11-09. The parts of `pictures::kitty::screen` that phewr does not
call get one cargo feature each: `flush`, the events (`resized`, `cleared`, `focus_gained`,
`delete_all`), `with_placing`, `with_pass_bytes`, `lost`, `hide` and `place_under`. The module
gates these parts, and their `expect(dead_code)` goes.

1. Add the features to the module and to the `Cargo.toml` of phewr. phewr turns none of them on,
   and its behavior does not change.
2. After the go of the schnattr session, run `just pictures-sync ../schnattr`. schnattr turns on
   the features that it calls. This sync also brings the master after `61c5256`: moxcms 0.9.2
   without the rounding workaround, and `THUMBNAIL_CACHE_VERSION` v22. schnattr moves its pin
   from `moxcms = "=0.9.1"` to `"0.9.2"`.

Phase 4b comes after the merge of phase 4, because phase 4 must not edit the copy in schnattr.

Done when `just check` passes in phewr and in schnattr, and no `expect(dead_code)` of the engine
is left in the module.

Result of step 1 (`455ac6f`, 2026-11-09). Step 2 is open.

- Seven cargo features gate the parts of the engine that phewr does not call. phewr turns none
  of them on. Each gate is `cfg(any(test, feature = "..."))`, so the tests build every part.

  | Feature | It gates |
  |---|---|
  | `kitty-flush` | `Screen::flush` |
  | `kitty-events` | `Screen::resized`, `cleared`, `focus_gained`, `delete_all`, the field `Screen::size`, `TerminalImages::ids` |
  | `kitty-layers` | `Placements::place_under` |
  | `kitty-overlay` | `Placements::hide`, `Screen::unplace_all` |
  | `kitty-placing` | `Screen::with_placing` |
  | `kitty-pass-bytes` | `Screen::with_pass_bytes` |
  | `kitty-store` | `Screen::lost`, `Screen::stored_bytes` |

- schnattr calls `flush`, `resized`, `cleared`, `focus_gained`, `delete_all`, `place_under` and
  `hide`. youtr needs `kitty-placing` for `Placing::EveryFrame`. phewr calls `lost` and
  `stored_bytes` only in its tests.
- Dead items for an app: if an app leaves one of these items without a caller, the app keeps
  `expect(dead_code)` on its `mod pictures;` line.
  - `Screen::forget_placements` and `protocol::clear_placements_to`. Without `kitty-overlay`, the
    module does not call them. phewr calls them only with `video`.
  - The box copies and the corners (`shape::copies`, `shape::corners`) and `thumbnail_dir_below`.
  - Each gated part of a `kitty-*` feature that the app turns on but does not call.
- The fixture sync: `just pictures-sync` and `just pictures-check` also cover the module
  fixtures (`pictures_fixtures` in the justfile). The sync copies `color/`, `hdr/`, `jxl/` and
  `png/` in full (107 files with each `make.sh` and `ref/`) and the 4 `still.*` files. It writes
  `tests/fixtures/SOURCE`. The animations, the root `make.sh`, `video/` and `.DS_Store` stay out.
  If an entry leaves the list, the next sync deletes its files and its empty folders. The other
  files in `tests/fixtures/` of the target stay.
- Two changes of the master:
  - The route fallback: a file with an extension that `IMAGE_EXTENSIONS` does not list tries
    `sips` after a failed raster decode. A listed raster extension never tries `sips`.
  - The `sips` limit: a `sips` run gets 10 s (`SIPS_TIMEOUT`), and then a kill.
- `just lint` runs clippy with all seven features, and `cargo check` with each feature alone on
  top of the default features. `just test-lean` runs with all seven features.
- One `dead_code` expectation of the module stays: `is_hdr` in `pictures/decode/hdr/mod.rs`.
  Only the libheif and the JPEG XL routes call `is_hdr`. phewr needs the expectation for the
  lean set and for `--no-default-features`. It is not part of the engine interface.

Open items:

- `pictures::decode::dimensions` has no `sips` fallback. A HEIC file with the extension `.img`
  decodes, but its shape and its size facts stay empty.
- In step 2, schnattr also deletes its own `sips` limit (`images/converter.rs`), because the
  master has `SIPS_TIMEOUT` now.

### Phase 5: herdr hardening (phewr repository, then schnattr and youtr)

Phase 5 starts after phase 4 and phase 4b (decision of the maintainer, 2026-11-09). At its start,
the measurement in herdr also decides for phewr: `d=A` on an overlay and a resize as today, or
`hide()` with focus reports (mode 1004).

1. Build the self check of rule 7, behind a setting, off by default.
2. Measure in herdr: does herdr pass the `ENOENT` answers of the host, or answer itself? If herdr
   answers itself, the self check uses the answer of herdr.
3. If the self check works, turn it on by default. If it does not work, the layer sends the
   pictures on screen again when a scroll stops (300 ms without a change of the placements),
   inside herdr only (`HERDR_ENV` or `TERM_PROGRAM=herdr`).

Result (`caa87b3`, 2026-11-11):

- The measurement before the change: phewr lost 2 to 5 cells in a herdr pane after ten fast
  scrolls, at each window size and zoom level, and 0 in plain Ghostty. schnattr lost 0 in both.
  A probe showed that herdr passes the `ENOENT` answer of the terminal through
  (`herdr-issue-kitty-graphics-lost-pictures.md`, "Actual result").
- With the self check on, the engine places with `q=1`. `Screen::with_self_check(bool)` turns it
  on (off by default in the module). `Screen::answer(&Answer)` forgets a lost picture, and the
  next flush sends it again. `Screen::has_lost()` says that a loss waits.
- `pictures::kitty::answers` reads the answers: `AnswerReader` from raw bytes, `AnswerKeys` from
  the key events of crossterm, which turns an answer into Alt+`_`, `G`, ... `AnswerKeys` holds
  the events at most `ANSWER_WAIT` (50 ms) and drops a started answer without its end.
- phewr turns the self check on by default. `PHEWR_KITTY_SELF_CHECK=0` turns it off.
  `LateAnswerFilter` counts whole Kitty answers, so a self-check answer is never swallowed.
- phewr keeps `d=A` on an overlay, a resize and the quit (the decision of the overlay question).
- The bench stays within 5 %, except two loop values of 1 and 2.5 ms in the bimodal bench.
- The measurement after the merge (2026-11-11): ten fast scrolls in a herdr pane, 0 cells
  missing. The scroll in herdr is slower than in plain Ghostty, as the herdr performance note
  describes. Step 3 (the resend when a scroll stops) is not needed. Phase 5 is done.

Done when the "lost pictures" count of phase 0 is 0 in herdr for each app.

### Phase 6: youtr takes the module (youtr repository)

Phase 6 comes after phase 5, so the placement policy of youtr follows the result of the self check
(decision 4).

1. After the go of the youtr session, phewr runs `just pictures-sync ../youtr`. youtr turns
   `heic`, `jxl` and `turbo` off (decision 2). The download of
   thumbnails stays in youtr. The decode, the shape and the Kitty layer come from the module.
2. Remove the youtr copies of the box copy, the corners and the dim.

Done when `just check` passes in youtr.

### Phase 7: the test matrix

Each app, in each terminal, with each scenario:

- Terminals: Ghostty, Ghostty inside herdr, Kitty, WezTerm, Terminal.app (text only), SSH to the
  same Mac.
- Scenarios: the first start, ten scrolls up and down, a resize, a focus change to another app and
  back, the help overlay, a chat or folder with more than 500 pictures, the quit.
- Measures: the numbers of phase 0. The plan is done when no picture goes away, and each measure
  is equal to or better than the baseline.

The maintainer runs the scenarios that need a real terminal. The workers write the rest as tests.

## Risks

- **Three copies of a large module.** The `SOURCE` check of each copy and the refusal of the sync
  keep the copies equal. The module must pass three lint tables.
- **herdr changes.** A new herdr version can fix or change the delivery. The self check of phase 5
  makes the apps independent of the herdr behavior.
- **The temp file medium.** The terminal must reach the temp directory. Inside a sandboxed
  terminal or over SSH, the probe picks `t=d`.

## Notes for the sessions that run the plan

- phewr owns the module. A change of the module starts in the phewr repository, then the sync
  brings it to the other projects after their go.
- The frame stays with schnattr as its master copy. A frame sync into youtr needs the explicit go
  of the youtr session.
- youtr names no sibling project in its code and docs.
- youtr links no native library (decision 2).
- Each worker runs with `--review auto`. The orchestrator lists each finding of the pre-review and
  each answer of the worker in the merge report.
