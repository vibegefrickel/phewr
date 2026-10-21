# TODO

## Now

- Terminal tests after batch 6 (maintainer): start viewr in Ghostty. Read the first status note (`Kitty transfer: ...`). Step through HEIC files. Then make sure that `ls $TMPDIR/tty-graphics-protocol-viewr-*` lists nothing. `VIEWR_KITTY_TRANSFER=direct` is the escape hatch. Check the `DarkGray` borders and the dim text on cendre. If the dim text is too dark, change the `dim` role in `src/styles.rs` to ANSI `Gray`.
- Terminal tests after batch 7 (maintainer), in Ghostty:
  - The cold HEIC grid: the previews come first, then the sharp thumbnails replace them without a jump or a leftover image.
  - mpv in the terminal: the speed and the HDR colors of an iPhone HDR MOV, and `n` / `N` between videos and images.
  - mpv in a window (`[video] player = "window"`) with an HDR clip. The worker could not measure the window mode.
  - The yazi reveal: start viewr with `g v` in yazi, quit, and make sure that the yazi cursor is on the last file. Nobody has proven that `ya emit reveal` reaches a yazi that blocks on `shell --block`.
- Terminal tests after batch 8 (maintainer), in Ghostty:
  - The grid: no empty lines between the thumbnails, the grid reaches the right and the bottom edge, the blue fill under the cursor and the yellow fill of marked images. The Kitty keys `X=` and `Y=` center each picture to the pixel. If a terminal ignores them, the pictures sit in the top left corner of their cells.
  - A transparent PNG under the cursor: the blue fill shows through the transparent pixels. If that looks wrong, put each thumbnail on an opaque background, or fill only a frame around the picture.
  - A portrait folder gets narrow cells and more columns, a landscape folder wide cells.
- Terminal tests after batch 10 (maintainer), in Ghostty and in herdr: `just install`, then `viewr /tmp/viewr-bench`. No picture overlaps a border or the gap row, landscapes keep their shape at every zoom level from 2 to 12 rows, no black cells after a long scroll up and down, the HEIC previews come first and the empty JPG and MOV cells fill early.

## Next

- Follow-ups after the Kitty output fixes (batch 10):
  - The Kitty bytes of a cold grid rose by 28 percent under the direct transfer (44.9 MB to 57.3 MB, 36 cells), because the padding to the full box doubles a landscape picture in a portrait box. A follow-up can pad to whole cells only and keep the no-offset rule, with the content centered to within half a cell. With the temp file medium in local Ghostty, the cost is only a larger file write.
  - No real HEIC test file has an embedded thumbnail with another aspect than the primary image. Such a preview shows another margin than the sharp picture, not a stretch.

The main goal is perceived speed for JPEG, HEIC and PNG.

- Speed round, open items after batch 6 (numbers from `just bench` on 40 HEIC, 24 MP, 10 cores):
  - Dropped: a JPEG preview from the EXIF thumbnail. iPhone JPEGs carry 160x120, which is too blurry for a grid cell.
  - Candidate: the shared memory medium `t=s` needs `shm_open` (unsafe or a crate). The temp file medium reaches the same payload size, so this is low priority.
  - The start status note with the transfer medium stays until the terminal tests are done (decided 2026-10-21). Then remove the three lines after `App::with_config` in `main`.
- mpv follow-ups after batch 7:
  - Dropped: `--vo-kitty-use-shm=yes`. Ghostty shows no picture with it (tested 2026-10-21).
  - The PQ LUT has no test on a real PQ file (iPhone files are HLG). It maps with a 1000 nit peak. Candidate: read the mastering metadata of HDR10 files.
  - Candidate: rescale when the terminal gets larger during playback. Today the scale size comes from the terminal size at the start of mpv.
- Real files for the new formats. This machine has no JPEG XL and no camera RAW file.
  - Make sure that JPEG XL decodes correctly: sRGB, Display P3, PQ and an ICC profile (`VIEWR_TEST_JXL`).
  - Make sure that camera RAW decodes correctly, with a portrait photo (`VIEWR_TEST_RAW`).
  - Measure whether `sips -Z` on a RAW uses the embedded preview, and measure the `sips` times with a real RAW.
- First release: create the Codeberg repository `viewr`, then `just render` and `just release`. The release binary does not bundle FFmpeg. FFmpeg stays a Homebrew dependency.
- Subfolders as entries in the grid (navigate without the picker)

## Later

- Animated GIF, WebP, APNG and AVIS through mpv, Live Photo pairing
- Move and rename files (culling workflow, no copy)
- Shuffle order for slideshows
- Persistent thumbnail size per directory
- Mouse-based pan and zoom in the fullscreen view
- Window slideshow: read the `end-file` reason through the mpv IPC socket, so that Cmd+Q stops the slideshow

- Moving thumbnails: the selected grid cell plays a short loop for GIF, WebP, APNG, AVIF and video files when the cell is active
  - Extract 12 frames with FFmpeg for each animated file. Space the frames evenly across the video duration. You can also sample the first 4 seconds.
  - Loop the 12 frames over 4 seconds. Set the gap between frames to 333 ms.
  - Use the Kitty animation protocol for terminal-driven looping. Upload frames with `a=f`. Start the loop with `a=a,s=3,v=1`.
  - Test the animation support in Ghostty. Fall back to a static thumbnail when the terminal does not support animation.
  - Decode the frames in a background thread. Cache the frame data so the UI can replay the loop without redecoding.
  - Stop the animation and remove the image when the cursor leaves the cell.
  - Add a config key to disable moving thumbnails.

## Done

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
