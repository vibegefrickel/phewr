# TODO

## Now

- Batch 5 after the reset of the weekly Claude window (2026-10-21): formats and interaction items below

## Next

- Next and previous video from inside mpv (for example mpv key bindings that run `quit <code>`)
- Make sure that the mpv hand-off works in Ghostty, Kitty and WezTerm (the worker tested it only in a herdr pane)
- Modern formats: JPEG XL (jxl-oxide), EXR/HDR/QOI/TGA in the scanner, Live Photo pairing, animated GIF/WebP/APNG/AVIS, RAW through ImageIO
- HEIC thumbnails: a full check of the 180° rotation and the mirror needs one `#[expect(unsafe_code)]` call into libheif (decision open)
- First release: create the Codeberg repository `viewr`, then `just render` and `just release`
- Subfolders as entries in the grid (navigate without the picker)
- Optional thumbnail labels for a denser grid
- Reveal the picked image back in yazi (`ya emit reveal`)

## Later

- Move / copy / rename files (culling workflow)
- Shuffle order for slideshows
- Persistent thumbnail size per directory
- Mouse-based pan and zoom in the fullscreen view
- Window slideshow: read the `end-file` reason through the mpv IPC socket, so that Cmd+Q stops the slideshow

## Done

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
