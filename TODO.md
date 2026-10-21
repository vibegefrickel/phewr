# TODO

## Now

- Terminal tests after batch 6 (maintainer): start viewr in Ghostty, Kitty and WezTerm. Read the first status note (`Kitty transfer: ...`). Step through HEIC files. Then make sure that `ls $TMPDIR/tty-graphics-protocol-viewr-*` lists nothing. `VIEWR_KITTY_TRANSFER=direct` is the escape hatch. Check the `DarkGray` borders and the dim text on cendre. If the dim text is too dark, change the `dim` role in `src/styles.rs` to ANSI `Gray`.
- mpv round (see below)

## Next

The main goal is perceived speed for JPEG, HEIC and PNG.

- Speed round, open items after batch 6 (numbers from `just bench` on 40 HEIC, 24 MP, 10 cores):
  - The cold HEIC grid is bound by the HEVC decode: about 100 ms per image with all cores, 1.9 s for 24 cells. Candidate: show the embedded 320x240 iPhone thumbnail at once as a preview, then replace it with the sharp decode (two-phase thumbnail job).
  - Candidate: the shared memory medium `t=s` needs `shm_open` (unsafe or a crate). The temp file medium reaches the same payload size, so this is low priority.
  - Decide on the start status note with the transfer medium. It helps the terminal tests. Removal: the three lines after `App::with_config` in `main`.
  - Decide on Catmull-Rom for the resizes: about 40 percent less resize time on a 24 MP frame, no visible difference at Retina density. `resize_options` in `src/app.rs` and the thumbnail cache key are the two places. `just bench` has the measurement.
- mpv round: the playback in mpv is not stable yet. Collect the problems first.
  - Make sure that the mpv hand-off works in Ghostty, Kitty and WezTerm. The worker tested it only in a herdr pane.
  - Next and previous video from inside mpv (for example mpv key bindings that run `quit <code>`).
  - iPhone HDR MOV files are oversaturated and slow with `--vo=kitty`. Dolby Vision/HLG tone-mapping causes the brightness problem.
  - Test `tone-mapping=bt.2446a` and `target-colorspace-hint=no` for iPhone HDR video.
  - Test `hwdec=videotoolbox`, `profile=sw-fast` and `framedrop=vo` to improve performance.
  - Document the mpv options that viewr passes for HDR files and for the Kitty output driver.
- Real files for the new formats. This machine has no JPEG XL and no camera RAW file.
  - Make sure that JPEG XL decodes correctly: sRGB, Display P3, PQ and an ICC profile (`VIEWR_TEST_JXL`).
  - Make sure that camera RAW decodes correctly, with a portrait photo (`VIEWR_TEST_RAW`).
  - Measure whether `sips -Z` on a RAW uses the embedded preview, and measure the `sips` times with a real RAW.
- Horizontal flow in the grid (justified rows): all thumbnails in a row have the same height. Each width follows the aspect ratio.
  - The layout needs the dimensions before the decode. Read them from the file header.
  - The cursor keys up and down need a new rule, because the columns do not line up.
- First release: create the Codeberg repository `viewr`, then `just render` and `just release`. The release binary does not bundle FFmpeg. FFmpeg stays a Homebrew dependency.
- Subfolders as entries in the grid (navigate without the picker)
- Reveal the picked image back in yazi (`ya emit reveal`)

## Later

- Animated GIF, WebP, APNG and AVIS through mpv, Live Photo pairing
- Move and rename files (culling workflow, no copy)
- Shuffle order for slideshows
- Persistent thumbnail size per directory
- Mouse-based pan and zoom in the fullscreen view
- Window slideshow: read the `end-file` reason through the mpv IPC socket, so that Cmd+Q stops the slideshow

- Chafa preview variant: a fast image and video preview mode that uses the chafa character-art backend instead of the Kitty graphics protocol
  - Goal: give a speed-optimized preview for large galleries and slow formats
  - Chafa output works in any true-color terminal. This is useful for preview on terminals that do not support the Kitty protocol.
  - The Kitty decode and transfer times are measured (`just bench`, speed round in Done). Write the chafa plan from these numbers.
  - Use chafa to convert each image and each video frame into terminal symbols and colors.
  - Start with still images. Add video frames after still images work.
  - Add a config key and a CLI flag to pick the backend. Make Kitty the default. Keep Kitty as the fallback.
  - Decide between the `chafa` binary and the chafa library. Build a prototype before you change the encoder.
  - Add a `ChafaBackend` in `encoder/` next to `KittyBackend`. Make the UI show the text output as a ratatui widget.
  - Cache the chafa output for thumbnails so the UI does not regenerate it each frame.
  - For video, extract frames with FFmpeg at a low rate. Run chafa on each frame. Do not use mpv for this mode.
  - Test the backend with synthetic images and videos. Write the output to a buffer, not to the real terminal.
  - Use chafa as a fast preview for HDR iPhone video. Chafa can avoid the mpv tone-mapping and Kitty transfer overhead.
  - Test `ffmpeg -f rawvideo -pix_fmt rgb24` piped into `chafa -f kitty` for iPhone MOV files.
  - Compare the speed and the color quality of chafa against mpv `--vo=kitty` on iPhone HDR video.

- Moving thumbnails: the selected grid cell plays a short loop for GIF, WebP, APNG, AVIF and video files when the cell is active
  - Extract 12 frames with FFmpeg for each animated file. Space the frames evenly across the video duration. You can also sample the first 4 seconds.
  - Loop the 12 frames over 4 seconds. Set the gap between frames to 333 ms.
  - Use the Kitty animation protocol for terminal-driven looping. Upload frames with `a=f`. Start the loop with `a=a,s=3,v=1`.
  - Test the animation support in Ghostty, Kitty and WezTerm. Fall back to a static thumbnail when the terminal does not support animation.
  - For the chafa backend, convert each frame with chafa and cycle the output client-side.
  - Decode the frames in a background thread. Cache the frame data so the UI can replay the loop without redecoding.
  - Stop the animation and remove the image when the cursor leaves the cell.
  - Add a config key to disable moving thumbnails.

## Done

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
