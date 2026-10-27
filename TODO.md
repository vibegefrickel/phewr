# TODO

Main goal: **viewr is a speed-focused viewer for all common image and video formats in the terminal, with full mouse support.**

Scope:

- Speed comes first: every change must make opening, scrolling or switching files faster, or it must add a common image/video format.
- Full mouse support is a first-class input method, on par with the keyboard. Every view and overlay must be usable with the mouse.
- Viewer only: viewr does not trash, delete, move, rename or otherwise modify files. After the terminal tests, remove the remaining delete code.
- Target terminal: Ghostty on macOS Apple Silicon, using the Kitty graphics protocol.
- Measure speed changes with `just bench` on `VIEWR_TEST_DIR`.

State of the main goal:

- Speed: the grid, the fullscreen view, the prefetch, the disk cache (QOI) and the Kitty temp file medium are in place. The open speed items are in "Next".
- Formats: JPEG, PNG, HEIC, AVIF, JPEG XL, camera RAW, PSD, animations and videos open. JPEG XL and camera RAW have no test on real files yet.
- Videos: the terminal player is the default video path. `p` opens the mpv window with sound.
- Mouse: every view and overlay works with the mouse. The maintainer test is open.

## Now

Maintainer tests in Ghostty: run the 13 steps in `~/Pictures/viewr-test/TESTS.md`. The folder holds a test set for each step (photos, colors, animations, videos, camera RAW from raw.pixls.us, every format, folders and placeholders).

Test run of 2026-10-24 (plain Ghostty, build 27dcfa2): steps 1 and 2 passed (medium `file`, the dim text is readable, no temp files after the quit, the warm grid fills at once, the file facts show). The run stopped at step 3a. Continue at step 3.

The four fixes from the test run are in (see Done).

After the tests:

- RAW: the portrait RAW files in `08-raw` (except `IMG_1361.DNG`) are copies with a changed Orientation tag, so they test the container orientation, not a real portrait shot.
- Then start the delete removal (Next, item 3).

## Next

Order: speed first, then formats, then the delete removal, then the first release.

1. Speed
   - herdr is much slower than plain Ghostty (maintainer, 2026-10-24). In plain Ghostty, the grid scrolls without lag at the normal zoom. In herdr, scrolling lags, and the terminal player played almost in slow motion (the player now keeps real time and drops frames instead).
     - Source research of herdr 0.9.5 (2026-10-25, `github.com/herdrdev/herdr`, tag v0.9.5). libghostty-vt in herdr parses each Kitty write and reads the temp file. herdr then copies the pixels to a `Vec`, hashes each new frame in full (SipHash), sends it over its server-to-client socket (bincode, at most 32 MiB) and writes it again for the host. All of this runs in the render path, at most every 16 ms (`MIN_RENDER_INTERVAL`).
     - The fast path to the host (`t=f` from a decoded file, `server/headless/native_graphics.rs`, `eligible`) takes RGBA (`f=32`) only. viewr sends opaque frames as RGB (`f=24`), so every video frame and every opaque picture takes the slower inline path: the client writes a new temp file per image (`client/frame_output.rs`), or base64 in 3072-byte chunks if its gates fail (`TERM_PROGRAM=ghostty`, no SSH or tmux, a probe).
     - The fast path waits for the host's `OK` before the next transfer. A geometry change (scroll, resize, overlay) while a transfer waits calls `retire_native_graphics_for_client`, which turns the fast path off for the rest of the client connection and empties the delivery cache, so everything uploads again inline.
     - herdr replays every placement on every client frame (`request_placement_replay` in `kitty_graphics/surface.rs`). A grid with many pictures costs placement work on each redraw.
     - herdr never re-encodes to PNG and never scales. The only switch is `terminal.kitty_graphics = false`.
     - Candidate fixes in viewr, each to measure in herdr against plain Ghostty: inside herdr (`HERDR_ENV=1`), cap the player frame at about 960x540 and about 15 fps (the maintainer accepts 10 fps), send RGBA instead of RGB so the fast path applies, send fewer `a=d` deletes between frames, and send no new image for an unchanged picture.
     - The herdr issue draft is `~/Projects/dev/herdr-issue-kitty-graphics-performance.md` (2026-10-25): the fast path for RGB, the permanent fallback after one scroll, and the placement replay on every frame. Fill in the numbers of its frame script in plain Ghostty and in herdr, then post it.
     - Decided 2026-10-25: measure first, build the viewr fixes later.
   - Player start: the first frame comes 170 ms to 305 ms after the open (measured 2026-10-27): the ffmpeg start, the VideoToolbox setup and, for HDR, the cube parse (33 points, about 42 ms). Candidates: start ffmpeg while the cursor rests on a video, or a smaller HDR cube.
   - Process start: libheif costs about 20 ms of dylib load before `main`. The delete removal takes `trash` and Foundation out (about 3.5 ms).
   - Keep the fullscreen image as RGB from the decode on and build the zoom crop as RGB. That removes the `to_rgba8` copy and the RGB step of each fullscreen write and each pan step (a type change of `App::loaded`, the prefetch and the zoom).
   - Lower thread priority (QoS class) for the folder prefetch and other background decodes, through a small crate, so that a running background decode never slows the visible grid. viewr's own code keeps its `unsafe` ban.
   - JPEG XL: the 8-bit copy is most of the decode. Measured 2026-10-25 on a 12 MP file: `render_frame` 65 ms (parallel), `ImageStream::write_to_buffer` 166 ms (one thread, an orientation and region lookup per sample). jxl-oxide 0.12.6 is the newest version, and its sample access is sealed (`FrameBufferSample`), so viewr cannot copy in parallel without a fork. `image_planar` copies on one thread too (about 120 ms). Next step: ask jxl-oxide upstream for a parallel or row-based `ImageStream`, or a public sample conversion.
   - ORF previews from the Olympus maker note, if a user asks.
   - Candidate, low priority: the shared memory medium `t=s` needs `shm_open` (unsafe or a crate). The temp file medium reaches the same payload size.
2. Formats
   - HDR stills (PQ or HLG in AVIF, HEIC or ICC) have no tone map.
   - JPEG XL with a PQ or HLG ICC profile (rare) shows washed out: read the cicp tag and tone-map like the video cube.
   - Real files for the new formats. `~/Pictures/viewr-test/08-raw` holds camera RAW files. JPEG XL has only converted and synthetic files.
     - Make sure that JPEG XL decodes correctly: sRGB, Display P3, PQ and an ICC profile (`VIEWR_TEST_JXL`).
     - Make sure that camera RAW decodes correctly, with a portrait photo (`VIEWR_TEST_RAW`).
     - Measure whether `sips -Z` on a RAW uses the embedded preview, and measure the `sips` times with a real RAW.
   - The PQ cube has no test on a real PQ file (iPhone files are HLG). It maps with a 1000 nit peak. Candidate: read the mastering metadata of HDR10 files.
   - No real HEIC test file has an embedded thumbnail with another aspect than the primary image. Such a preview shows another margin than the sharp picture, not a stretch.
3. Delete removal, after the terminal tests in "Now"
   - viewr is a viewer only. Remove trash, permanent delete, the delete question, the `d` / `D` keys, their tests and docs, and the `trash` crate if nothing else needs it.
   - Keep the marks (`Space`, `a`, `A`) for a later use, for example a slideshow of the marked images.
4. Grid loops (decided 2026-10-23, one heavy worker, spawn on request)
   - Every visible video and animated cell plays a loop in the grid, always, not only under the cursor. The loop replaces the `▶` marker.
   - Video: 12 keyframes spread over the whole clip, about 3 fps, a 4 s loop. Extract them with `-skip_frame nokey` (measured: 0.11 s to 0.15 s per 4K clip). For a long clip, seek to 12 points instead.
   - Animated GIF, WebP, APNG and AVIS: their own frames and delays, with a cap on the frame count.
   - Ghostty has no Kitty animation frames (Ghostty #5255). viewr drives the loop: each frame goes to the terminal once as its own image, then each tick deletes the old placement and places the next frame (about 100 bytes per cell), in one synchronized update.
   - Count the frames against `TERMINAL_IMAGE_BUDGET` (128 MB, herdr stores each image twice). Only visible cells loop. Above the budget, a cell keeps its still thumbnail.
   - Keep the frames in the disk cache. The run loop wakes at the next due frame, not on a fixed poll.
   - Config key `[gallery] animate = false` turns the loops off. Measure the CPU of viewr and Ghostty with 30 looping cells, also in herdr.
5. First release: make ffmpeg a recommendation in the formula and drop the `libavcodec` check from `install.sh`, because viewr runs without ffmpeg. Then create the Codeberg repository `viewr`, then `just render` and `just release`. The release binary does not bundle FFmpeg. FFmpeg stays a Homebrew dependency.

## Later

- Live Photo pairing: an iPhone still (HEIC or JPEG) and a `.mov` with the same base name share an Apple content identifier. The identifier lives in the EXIF MakerNote of the still and in the QuickTime metadata of the movie.
- Window slideshow: read the `end-file` reason through the mpv IPC socket, so that Cmd+Q stops the slideshow
  - Start mpv with `--input-ipc-server=<unix-socket>` and read newline-delimited JSON events.
  - The `end-file` event has a `reason` field with values `eof`, `stop`, `quit`, `error`, `redirect` or `unknown`. Natural end-of-file reports `eof`. Cmd+Q or the app-menu quit reports `quit` or `stop`.
  - In `Playback::Once` for a slideshow, treat `reason == "eof"` as "continue" and every other reason as "stop slideshow". Today both EOF and Cmd+Q exit with code 0.
  - Open the socket in a thread, parse events, and send the final reason back through the existing `Runner::spawn` channel or a new one. Remove the socket file before start.
  - Low priority: the terminal player is the slideshow video path now.


## Done

- Player measured without a terminal (2026-10-27, `just bench` per clip of `07-video`): every clip decodes at 160 fps to 500 fps at 1080p and plays in real time with 0 drops. The first frame comes 170 ms to 305 ms after the open (ffmpeg start, VideoToolbox, the HDR cube). In plain Ghostty, the player has no bottleneck.
- VFR bursts: a base rate above 1.5 times the average plays at the average, at least 24 fps. A phone clip at 60 with an average of 30 plays at 30 fps instead of 60 duplicated frames. A screen recording at 25 with an average of 7.7 plays at 24 fps.
- An animated WebP or APNG with an ICC profile or a cICP chunk plays in its own colors: the probe reads the profile and maps sRGB, Display P3 and BT.2020 colorants to the cube primaries. Other profiles still play as sRGB.
- Camera JPEGs in Adobe RGB without an ICC profile (EXIF `ColorSpace` 0xFFFF with the interoperability index `R03`) convert as Adobe RGB, in the turbojpeg and the `image` path. Thumbnail cache key v13.
- No sRGB bench folder needed: `color_conversion_cost` already measures both directions. A grid cell converts in 0.14 ms to 0.21 ms, a fullscreen image in 0.65 ms to 1.69 ms, a 24 MP sRGB frame into Display P3 in 7 ms (2026-10-25). That is under 1 % of a cold grid decode.
- The run loop polls every 2 ms until the first picture of the grid shows (`FIRST_PICTURE_POLL`): the cold first picture after 14 ms to 17 ms instead of 16 ms to 18 ms.
- Each bench waits for its background work to end. In the full `just bench`, the cold grid shows all cells after 126 ms instead of 431 ms, the same as the grid bench alone (118 ms to 129 ms).
- In the fullscreen view and the player, `h` and `j` step to the previous file, `k` and `l` to the next. Zoomed in, `hjkl` still pan.
- viewr always starts in the grid, also in a folder with one image (`~/Pictures`). It never opens the fullscreen view by itself. Only `--slideshow` starts a slideshow.
- The terminal player keeps real time (rule of 2026-10-24): the clock never moves to a late frame, a lag over 300 ms starts ffmpeg again ahead of the clock, and a slow terminal drops frames. A clip of 6 s with a terminal that needs 3 intervals per frame ends after at most 6.6 s instead of 18 s. The real clips play at their length with 0 drops.
- The player has no pause and no seek: `h` / `l`, the arrows and `n` / `N` step to the previous and next file, Space pauses a running slideshow, the wheel and a click on the video do nothing, `p` opens the mpv window.
- Grid icons at 30 % of the cell height, a picture for images, two dots without a name for `..`. The help popup hides the icons and the play markers too.
- The start note is gone. The last row of the help popup names the Kitty medium.
- Large octant icons in the grid (loading image, video, folder, `..`). The status row shows the facts of the selected file at the right (size, megapixels, file size, format, date of the shot, and length, frame rate and HDR curve of a video) and no key hints.
- One file argument opens its folder with the cursor on the file. yazi `g v` runs `viewr %h`, so it opens the hovered folder or the folder at the hovered file.
- Warm grid: disk cache reads beside the decode slots and batched Kitty emits. All 36 cells show after 17 ms instead of 127 ms, the cold grid is not slower.
- Videos, video thumbnails and animations convert into the terminal color space: a 17-point SDR cube by primaries (BT.709, BT.601, P3, DCI-P3, BT.2020), the HDR cube writes P3, SDR thumbnails convert in Rust. Real time stays at 30 and 60 fps without drops.
- Disk cache in QOI (a hit 2.2 times faster), the cell size query and the Kitty probe in one round trip, the first warm picture after about 20 ms instead of about 28 ms. The `dist` profile gives no gain, so `just install` stays on `--release`.
- `ffmpeg-next` dropped: viewr starts `ffmpeg` / `ffprobe` from PATH when installed, the binary links no FFmpeg library, video thumbnails take about 250 ms instead of 0.8 to 2.4 s and get the rotation and the HDR tone mapping, start 30 ms to 21 ms.
- Image colors: every image converts into Display P3, because Ghostty shows image pixels as P3. sRGB images were too saturated before. `[terminal] colorspace` switches to sRGB.
- Opaque images and video frames go out as RGB (`f=24`): 25 % fewer bytes per frame, the player decode 30 to 50 % faster. PNG decoders measured: `zune-png` and `png` are not faster than the `image` route, so the PNG item is closed.
- The directory picker is gone: subfolders, `Backspace`, the mouse and a path argument cover navigation. An empty folder shows a note.
- Folder prefetch: after 300 ms on a folder, Enter shows every cell sharp after about 140 ms instead of about 2 s.
- JPEG XL colors: ICC v2 profiles convert through `moxcms` (Adobe RGB was wrong), 18 color chart fixtures, all within ΔE00 0.36 of djxl.
- Videos with a variable frame rate play at the right speed (a screen recording played 3.2 times too slow).
- Two-column help popup (36 rows from 100 columns on), the play marker for videos too.
- Animated GIF, WebP, APNG and AVIS play in the terminal player, with a `▶` marker in the grid. The animation flag comes from the thumbnail job, so the draw reads no file.
- Full mouse support in every view and overlay: modes 1000, 1002 and 1006 only (no 1003), `[gallery] mouse`, the capture is released while an mpv window plays.
- Speed batch: embedded JPEG previews for camera RAW (TIFF-based, CR3, RAF) and PSD, padding to whole cells only (a cold grid sends 44.1 MB instead of 57.3 MB), parked thumbnails for the way back (all cells sharp after about 25 ms), thumbnail cache key v7.
- Terminal video player (route 3) as the default video path: ffmpeg with videotoolbox, `scale_vt` and the HDR cube, frames through the temp file medium with `c=` / `r=`, 1080p at 30 fps without drops. `p` opens the mpv window with sound. The mpv `--vo=kitty` mode and the old 10 fps player are gone.
- Subfolders as grid entries: `..`, `Enter` into a folder, `Backspace` to the parent.
- Zoom level per folder: the state file keeps up to 200 folders, a folder without an entry uses the last level.
- `q` goes back one step in every view and quits only in the grid. In a running slideshow, `q` leaves in one press. In the built-in player during a slideshow, `q` goes to the fullscreen placeholder, like `Esc`.
- Zoom fix: a `+`, `-` or `0` at a limit no longer deletes all pictures (black cells), `q` in the fullscreen view goes back to the grid.
- Kitty output fixes (batch 10): thumbnails padded to whole cells, no `X=` / `Y=` (fixes the herdr overlap and the squeezed landscapes), `d=I` for evicted images and a 128 MB terminal budget (fixes the black cells), a 512 MB in-memory cache limit, `CSI 16 t` for the cell size, the filename toggle decodes again, late probe answers are dropped, empty cells decode first, the HEIC preview goes out at native size, grid zoom 2 to 12 rows.
- Refactor (batch 9): `main.rs` and `app.rs` split into 30 modules, then the repeating patterns removed.
- Video in a fullscreen mpv window by default, grid gaps with line borders, loading icons, no frames and no search bar (batch 8b).
- Grid fill (batch 8): no border reserve, a 4 px padding with the Kitty keys `X=` / `Y=`, a background fill for the selected and the marked cells, the remainder columns and rows reach the edges, the cell shape from the median aspect of the first 64 file headers (7 buckets), Catmull-Rom for every resize (24 MP resize 54 ms to 36 ms), thumbnail cache key v5.
- mpv round (batch 7): `--hwdec=auto-copy-safe` and a scale filter in the terminal (4K HDR clip 34 fps to 97 fps), a tone mapping LUT for HLG and PQ in the terminal (8.4 dB to 30.5 dB against the macOS SDR export), `n` / `N` step to the next and previous file from inside mpv, the mpv window shows its exit code, quiet FFmpeg log, the yazi reveal on quit.
- Two-phase HEIC grid thumbnails (batch 7): the embedded thumbnail shows at once, the sharp decode replaces it. libheif reads through a buffered stream. Cold grid of 40 HEIC files: first image 250 ms to 13 ms, all 24 cells shown after 125 ms, all cells sharp 1882 ms to 1461 ms.
- Speed round (batch 6): the run loop polls every 10 ms while a decode, a prefetch, a filter or the scan runs. The gallery pre-decode gets the fullscreen box as size hint (resize on the UI thread 52 ms to 72 ms, now 0 ms). libheif decodes share the cores, the fullscreen decode takes all of them. At most 4 thumbnail and 2 prefetch decodes run at once, the prefetch follows the step direction and keeps the shown image. On the test folder: HEIC open 525 ms to about 210 ms, cold grid first thumbnail 1013 ms to about 100 ms, warm grid 565 ms to 90 ms.
- Kitty transfer medium: a temp file (`t=t`) when the terminal answers the probe with `OK` and deletes the probe file, PNG over SSH, direct otherwise. `VIEWR_KITTY_TRANSFER=direct|png|file` skips the probe. One fullscreen frame: 29 MB of text, now 199 bytes and a 21.7 MB file.
- Benchmarks: `src/bench.rs` holds ignored benchmarks on `VIEWR_TEST_DIR`, `just bench [DIR]` runs them.
- Theme system removed: `src/theme.rs`, the theme catalog, the `t` picker and the `-t` flag are gone. viewr uses the terminal default colors and the 16 ANSI colors (`src/styles.rs`). The config keys `theme` and `theme_catalog` have no effect, an old config still loads.
- Grid look: a border only around the selected and the marked cells, no spacer line, centered thumbnails, filenames hidden by default (`n` toggles them)
- Grid zoom: `+`/`-` change the visible rows (default 4), the columns follow the terminal width (minimum 2), the state file saves the zoom level as `[gallery] visible_rows`
- Fullscreen fix: a gallery pre-decode without a hint showed HEIC, JPEG and PNG at full size
- JPEG XL (jxl-oxide, sRGB), EXR and HDR (tone map), QOI, TGA
- Camera RAW, PSD, JPEG 2000, PDF, SVG and other ImageIO formats through `sips` (TIFF output)
- HEIC grid thumbnails: a safe ISOBMFF reader compares `irot` and `imir` with the primary image
- No unsafe code: the `sips` route and the box reader replace ImageIO and libheif-sys
- Fork of rview, first as imgrrr, then renamed to viewr
- Tooling from schnattr: justfile, lint table, cargo deny, nextest, Codeberg release flow
- Configurable, zoomable thumbnail size (`+` / `-` / `0`, remembered in state file)
- Slideshow with interval, pause and loop (`s`, `Space`, `[` / `]`, `--slideshow`)
- Info panel with file data and EXIF (`i`, `--info`)
- HEIC / HEIF decoding through libheif
- Video playback in mpv (`[video] player`: terminal, window, builtin)
- Built-in player: a pause no longer drops a packet or stops the loop
- Theme: `-t` with an unknown name stops with an error, the theme follows `XDG_CONFIG_HOME`
- Slideshow plays each video one time and continues (terminal, window, built-in)
- No test writes Kitty sequences to the real stdout, all tests use `tempfile`
- Delete in the grid keeps the cursor in place
- libheif path scales with fast_image_resize (thumbnail cache key v3)
- Testing phase: 39% to 94% line coverage, `tempfile`, no test writes real user files
- Return to the view that opened a video, video placeholder in fullscreen
- Prefetch: generation counter against stale results, no retry loop after a failed decode
- AVIF through libheif, embedded HEIC grid thumbnails, EXIF orientation for WebP and PNG
- EXIF orientation applied to JPEG / TIFF
