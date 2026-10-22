# TODO

Main goal: **viewr is a speed-focused viewer for all common image and video formats in the terminal, with full mouse support.**

Scope:

- Speed comes first: every change must make opening, scrolling or switching files faster, or it must add a common image/video format.
- Full mouse support is a first-class input method, on par with the keyboard. Every view and overlay must be usable with the mouse.
- Viewer only: viewr does not trash, delete, move, rename or otherwise modify files. Remove the remaining delete code once the terminal tests are done.
- Target terminal: Ghostty on macOS Apple Silicon, using the Kitty graphics protocol.
- Measure speed changes with `just bench` on `VIEWR_TEST_DIR`.

Implications for the current structure:

- The built-in terminal video player (Route 3) becomes the default video path, because mpv `--vo=kitty` is too slow in Ghostty (4K HLG plays at 2.4 fps). mpv window mode can stay as an option.
- HEIC two-phase thumbnails, prefetch, disk cache and the temp-file Kitty medium are the right architecture. Extend the preview pattern to other formats that have embedded thumbnails, and keep the caches hot.
- Format support must be verified on real files: JPEG XL, camera RAW and animated image formats are the biggest gaps.
- Mouse support is not optional polish: grid selection, scrolling, zoom, pan, fullscreen stepping and overlay dismissal all need mouse handlers.
- Window-slideshow IPC and moving thumbnails improve UX but are lower priority than decode/render speed, format coverage and core mouse interactions.

## Now

- In progress: `viewr-terminal-player`, the built-in terminal video player (route 3). Decided on 2026-10-21: it is the default video path. ffmpeg runs as a subprocess with videotoolbox, `scale_vt` and the HLG/PQ cube, each frame goes through the temp file medium with `c=` / `r=`, at most 1920x1080. No sound: `p` opens the video in the fullscreen mpv window with sound. `[video] player` keeps `terminal` (default) and `window`. The mpv `--vo=kitty` mode and the old 10 fps player go away. Without the temp file medium, videos open in the mpv window. No delete keys in the player.
  - Measured in Ghostty (200x47 cells, 4K HLG `IMG_0639.MOV`): untimed 960x540 305 fps, 1280x720 212 fps, 1920x1080 107 fps. Real time 30 fps with 0 dropped frames at 1280x720 and 1920x1080. No flicker, colors close to the mpv window. `direct` (base64) is unusable. The tool is in `research/route3/` (git ignores it).
- In review: `viewr-subfolders`, folders as grid entries (`..`, `Enter`, `Backspace`). Merge it after the approval.
- Terminal tests (maintainer), in Ghostty:
  - Batch 6: read the first status note (`Kitty transfer: ...`). After quitting, make sure that `ls $TMPDIR/tty-graphics-protocol-viewr-*` lists nothing. Check the dim text on cendre. If it is too dark, change the `dim` role in `src/styles.rs` to ANSI `Gray`. Then remove the start note (the three lines after `App::with_config` in `main`).
  - Batch 7: the mpv window (`p`, or `[video] player = "window"`) with an HDR clip, and `n` / `N` between videos and images in the window. The yazi reveal: start viewr with `g v` in yazi, quit, and make sure that the yazi cursor is on the last file.
  - Batch 8: a portrait folder gets narrow cells and more columns, a landscape folder wide cells.
  - After the terminal player merge: play HDR and SDR videos, seek, pause, `n` / `N`, `p`, and a slideshow with videos.

## Next

Order: speed first, then formats, then mouse support, then the delete removal, then the first release.

1. Speed
   - Extend the two-phase preview to other formats with an embedded preview: camera RAW (the embedded JPEG preview, also for `sips -Z`), PSD (the embedded thumbnail), and any other format of the `sips` route that carries one. Measure with `just bench` on real files.
   - The Kitty bytes of a cold grid rose by 28 percent under the direct transfer (44.9 MB to 57.3 MB, 36 cells), because the padding to the full box doubles a landscape picture in a portrait box. Pad to whole cells only and keep the no-offset rule, with the content centered to within half a cell. With the temp file medium in local Ghostty, the cost is only a larger file write.
   - Keep the caches hot: make sure that the prefetch and the disk cache also serve a folder switch through the grid (subfolders) and the way back with `Backspace`.
   - Candidate, low priority: the shared memory medium `t=s` needs `shm_open` (unsafe or a crate). The temp file medium reaches the same payload size.
2. Formats
   - Animated GIF, WebP, APNG and AVIS through the new terminal player. Live Photo pairing stays under Later.
     - mpv plays animated GIF, WebP and APNG natively through FFmpeg. AVIS is the AVIF image-sequence brand (`ftyp` brand `avis`) and usually has the same `.avif` extension.
     - In the grid, keep the static first-frame thumbnail. Open the animated file in the terminal player on Enter, and in the mpv window with `p`.
   - Real files for the new formats. This machine has no JPEG XL and no camera RAW file.
     - Make sure that JPEG XL decodes correctly: sRGB, Display P3, PQ and an ICC profile (`VIEWR_TEST_JXL`).
     - Make sure that camera RAW decodes correctly, with a portrait photo (`VIEWR_TEST_RAW`).
     - Measure whether `sips -Z` on a RAW uses the embedded preview, and measure the `sips` times with a real RAW.
   - The PQ cube has no test on a real PQ file (iPhone files are HLG). It maps with a 1000 nit peak. Candidate: read the mastering metadata of HDR10 files.
   - No real HEIC test file has an embedded thumbnail with another aspect than the primary image. Such a preview shows another margin than the sharp picture, not a stretch.
3. Mouse support
   - Full mouse support: pan and zoom in the fullscreen view, grid scroll, and cell selection
     - crossterm 0.29 emits `Event::Mouse(MouseEvent)` with `MouseEventKind::{Down, Up, Drag, Moved, ScrollDown, ScrollUp, ScrollLeft, ScrollRight}`. Each event carries `(column, row)` in cells and `KeyModifiers`. Enable capture with `EnableMouseCapture` in `TerminalSession` and disable it on restore.
     - ratatui 0.30 supports mouse events; see `examples/apps/custom-widget` for click/hover handling.
     - Ghostty still lets the user select text with Shift+click or Shift+drag while mouse capture is on. Document this in the help popup so users know they can copy filenames.
     - Grid interactions:
       - Left click inside a cell: move the cursor to that image.
       - Double-click a cell: enter fullscreen on that image.
       - Scroll wheel: scroll the grid by one row per notch. Hold Shift to zoom (`+`/`-` equivalent) so the wheel stays natural when not shifted.
       - Right click: open the info panel for the clicked image.
     - Fullscreen and zoom interactions:
       - Scroll wheel: zoom in/out, like `+`/`-`.
       - Drag with left button: pan the image, like the arrow keys.
       - Click near the left/right edge: step to the previous/next image.
     - Overlay interactions:
       - Click outside a popup (help, delete question, picker, info panel): dismiss it, like `Esc` or `q`.
       - Click an item in the picker or a button in the delete question: activate it.
     - Implementation points:
       - Extend `keys::handle_event` to match `Event::Mouse` and dispatch to `handle_mouse` based on `app.mode`.
       - Translate mouse `(column, row)` to a grid cell with `gallery.cell_at(column, row)`; precompute cell rectangles during `update_grid`.
       - Track drag state in `App`: last mouse position, button held, drag threshold in pixels or cells.
       - Keep mouse handling non-blocking: a mouse event sets state and marks the app dirty, just like a key press.
     - Add a config key `[gallery] mouse = true|false` (default `true`). When `false`, viewr never enables mouse capture, and the terminal keeps its normal selection and scrolling.
4. Delete removal, after the terminal tests in "Now"
   - viewr is a viewer only. Remove trash, permanent delete, the delete question, the `d` / `D` keys, their tests and docs, and the `trash` crate if nothing else needs it.
   - Keep the marks (`Space`, `a`, `A`) for a later use, for example a slideshow of the marked images.
5. First release: create the Codeberg repository `viewr`, then `just render` and `just release`. The release binary does not bundle FFmpeg. FFmpeg stays a Homebrew dependency.

## Later

- Live Photo pairing: an iPhone still (HEIC or JPEG) and a `.mov` with the same base name share an Apple content identifier. The identifier lives in the EXIF MakerNote of the still and in the QuickTime metadata of the movie.
- Window slideshow: read the `end-file` reason through the mpv IPC socket, so that Cmd+Q stops the slideshow
  - Start mpv with `--input-ipc-server=<unix-socket>` and read newline-delimited JSON events.
  - The `end-file` event has a `reason` field with values `eof`, `stop`, `quit`, `error`, `redirect` or `unknown`. Natural end-of-file reports `eof`; Cmd+Q or the app-menu quit reports `quit` or `stop`.
  - In `Playback::Once` for a slideshow, treat `reason == "eof"` as "continue" and every other reason as "stop slideshow". Today both EOF and Cmd+Q exit with code 0.
  - Open the socket in a thread, parse events, and send the final reason back through the existing `Runner::spawn` channel or a new one. Remove the socket file before start.
  - Low priority: the terminal player is the slideshow video path now.


- Moving thumbnails: the selected grid cell plays a short loop for GIF, WebP, APNG, AVIF and video files when the cell is active
  - Extract 12 frames with FFmpeg for each animated file. Space the frames evenly across the video duration. You can also sample the first 4 seconds.
  - Loop the 12 frames over 4 seconds. Set the gap between frames to 333 ms.
  - Use the Kitty animation protocol for terminal-driven looping. Upload frames with `a=f`. Start the loop with `a=a,s=3,v=1`.
  - Query the terminal for animation support first. Ghostty does not support Kitty animation frames yet (Ghostty #5255), so the fallback to a static thumbnail is required on Ghostty.
  - Decode the frames in a background thread. Cache the frame data so the UI can replay the loop without redecoding.
  - Stop the animation and remove the image when the cursor leaves the cell.
  - Add a config key to disable moving thumbnails.
  - Lowest priority: it is visual polish, not decode/render speed or format coverage.


## Done

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
