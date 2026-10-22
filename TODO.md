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

- Animation test (maintainer), in Ghostty: open an animated GIF, WebP and APNG from `~/Pictures` or the web, check the `▶` marker in the grid, the loop, `p`, and a slideshow with a short GIF.
- Mouse test (maintainer), in Ghostty: click, double-click and right click in the grid, wheel and Shift+wheel (does Shift+wheel-up zoom in, like `+`?), a sideways trackpad swipe, fullscreen wheel zoom, drag pan and edge clicks, player clicks and wheel, the picker, Shift+drag text selection. Watch the CPU while you move the mouse over a large grid.
- Real RAW test (maintainer): put a portrait DNG, CR2, CR3, NEF, ARW and RAF into a folder, run `VIEWR_TEST_RAW=<file> just test-real` and open the folder in the grid. The embedded previews have only synthetic tests.
- Terminal tests (maintainer), in Ghostty:
  - Batch 6: read the first status note (`Kitty transfer: ...`). After quitting, make sure that `ls $TMPDIR/tty-graphics-protocol-viewr-*` lists nothing. Check the dim text on cendre. If it is too dark, change the `dim` role in `src/styles.rs` to ANSI `Gray`. Then remove the start note (the three lines after `App::with_config` in `main`).
  - Batch 7: the mpv window (`p`, or `[video] player = "window"`) with an HDR clip, and `n` / `N` between videos and images in the window. The yazi reveal: start viewr with `g v` in yazi, quit, and make sure that the yazi cursor is on the last file.
  - Batch 8: a portrait folder gets narrow cells and more columns, a landscape folder wide cells.
  - After the terminal player merge: play HDR and SDR videos, seek, pause, `n` / `N`, `p`, and a slideshow with videos.
- VFR test (maintainer), in Ghostty: a 4K phone clip with a variable frame rate (r=60, avg=30) plays without stutter. If it stutters, take the average rate in `variable_rate_fps` when r_frame_rate is more than 1.5 times the average.
- RGB frames test (maintainer), in Ghostty and in herdr: after the `viewr-png-rgb` merge, opaque fullscreen images and player frames go out as `f=24`. Make sure that they look right, also a transparent PNG in fullscreen.
- Color test (maintainer), in Ghostty: after the P3 merge, sRGB images (screenshots, web images, Android photos) look less saturated than before, and iPhone photos look unchanged. Compare with `[terminal] colorspace = "srgb"`.
- Warm grid test (maintainer), in Ghostty and in herdr: a warm grid of 36 or more cells fills at once, without a stall or a flicker.

## Next

Order: speed first, then formats, then mouse support, then the delete removal, then the first release.

1. Speed
   - The benches leave background decodes running into the next bench. A wait for idle work at the end of each bench would make the cold numbers of the full `just bench` comparable.
   - The cold grid still waits `BUSY_POLL` (10 ms) for its first decode result.
   - Process start: libheif costs about 20 ms of dylib load before `main`. The delete removal takes `trash` and Foundation out (about 3.5 ms).
   - Keep the fullscreen image as RGB from the decode on and build the zoom crop as RGB. That removes the `to_rgba8` copy and the RGB step of each fullscreen write and each pan step (a type change of `App::loaded`, the prefetch and the zoom).
   - Lower thread priority (QoS class) for the folder prefetch and other background decodes, through a small crate, so that a running background decode never slows the visible grid. viewr's own code keeps its `unsafe` ban.
   - JPEG XL: copy the pixels in parallel (a 24 MP decode 300 ms to about 105 ms).
   - ORF previews from the Olympus maker note, if a user asks.
   - Candidate, low priority: the shared memory medium `t=s` needs `shm_open` (unsafe or a crate). The temp file medium reaches the same payload size.
2. Formats
   - An animated WebP with an ICCP chunk or an APNG with iCCP converts as sRGB in the player, because ffprobe reports no profile. The grid thumbnail reads the profile, so such a rare file jumps in color between grid and player.
   - Camera previews in Adobe RGB without an ICC profile (EXIF ColorSpace 0xFFFF with InteropIndex R03) show as sRGB. HDR stills (PQ or HLG in AVIF, HEIC or ICC) have no tone map.
   - Add an sRGB folder (like `/tmp/viewr-bench-srgb`) to the standard bench set, so the conversion cost shows.
   - JPEG XL with a PQ or HLG ICC profile (rare) shows washed out: read the cicp tag and tone-map like the video cube.
   - Real files for the new formats. This machine has no JPEG XL and no camera RAW file.
     - Make sure that JPEG XL decodes correctly: sRGB, Display P3, PQ and an ICC profile (`VIEWR_TEST_JXL`).
     - Make sure that camera RAW decodes correctly, with a portrait photo (`VIEWR_TEST_RAW`).
     - Measure whether `sips -Z` on a RAW uses the embedded preview, and measure the `sips` times with a real RAW.
   - The PQ cube has no test on a real PQ file (iPhone files are HLG). It maps with a 1000 nit peak. Candidate: read the mastering metadata of HDR10 files.
   - No real HEIC test file has an embedded thumbnail with another aspect than the primary image. Such a preview shows another margin than the sharp picture, not a stretch.
3. Mouse support: done (see Done).
4. Delete removal, after the terminal tests in "Now"
   - viewr is a viewer only. Remove trash, permanent delete, the delete question, the `d` / `D` keys, their tests and docs, and the `trash` crate if nothing else needs it.
   - Keep the marks (`Space`, `a`, `A`) for a later use, for example a slideshow of the marked images.
5. First release: make ffmpeg a recommendation in the formula and drop the `libavcodec` check from `install.sh`, because viewr runs without ffmpeg. Then create the Codeberg repository `viewr`, then `just render` and `just release`. The release binary does not bundle FFmpeg. FFmpeg stays a Homebrew dependency.

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
