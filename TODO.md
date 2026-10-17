# TODO

## Now

- Speed round (see below)

## Next

The main goal is speed for JPEG, HEIC and PNG.

- Speed round. Measure first on a real iPhone folder, then fix:
  - the time to the first thumbnail and to a full grid
  - the time to open an image in fullscreen
  - the prefetch hits for the next and the previous image
  - Give the gallery pre-decode a size hint (terminal size or the last fullscreen box). Today `take_resized` resizes a full-size pre-decode on the UI thread: 70 ms to 100 ms for a 24 MP HEIC.
  - HEIC fullscreen decode: 260 ms to 285 ms. libheif `decode` takes 188 ms, the resize and the copy take the rest.
- Kitty transfer: `src/encoder/kitty.rs` sends raw RGBA as base64 through the pty. A Retina fullscreen frame is about 32 MB of text.
  - Use the transfer medium `t=t` (temp file) or `t=s` (shared memory).
  - Make sure that Ghostty, Kitty, WezTerm and herdr panes support the medium. Keep the direct transfer as the fallback.
- mpv round: the playback in mpv is not stable yet. Collect the problems first.
  - Make sure that the mpv hand-off works in Ghostty, Kitty and WezTerm. The worker tested it only in a herdr pane.
  - Next and previous video from inside mpv (for example mpv key bindings that run `quit <code>`).
- Real files for the new formats. This machine has no JPEG XL and no camera RAW file.
  - Make sure that JPEG XL decodes correctly: sRGB, Display P3, PQ and an ICC profile (`VIEWR_TEST_JXL`).
  - Make sure that camera RAW decodes correctly, with a portrait photo (`VIEWR_TEST_RAW`).
  - Measure whether `sips -Z` on a RAW uses the embedded preview, and measure the `sips` times with a real RAW.
- Terminal colors: remove the hard-coded fallback theme (a Tokyo Night palette with the wrong background).
  - By default, viewr uses the background, the foreground and the ANSI colors of the terminal.
  - Ghostty runs cendre, so viewr follows cendre without a theme file.
  - Decide if the theme system stays or goes.
- Grid look:
  - By default, no borders. Only the selected image and the marked images get a border. The spacing between the thumbnails stays.
  - By default, no filenames. A key toggles the filenames.
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

## Done

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
