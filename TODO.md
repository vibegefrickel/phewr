# TODO

## Now

- Testing phase: measure the coverage baseline (`just coverage`), then add tests for the weak modules
- Use `tempfile` in the tests instead of hand-made directories below `std::env::temp_dir()`

## Next

- After mpv, go back to the fullscreen view instead of the gallery (needs `app.rs`: remember the mode before the video)
- Next and previous video from inside mpv (for example mpv key bindings that run `quit <code>`)
- Prefetch: drop stale results after `invalidate()` (a generation counter like `thumb_generation`), and retry an index after a failed decode
- Make sure that the mpv hand-off works in Ghostty, Kitty and WezTerm (the worker tested it only in a herdr pane)
- Modern formats: JPEG XL (jxl-oxide), EXR/HDR/QOI/TGA in the scanner, Live Photo pairing, animated GIF/WebP/APNG/AVIS, RAW through ImageIO
- HEIC thumbnails: a full check of the 180° rotation and the mirror needs one `#[expect(unsafe_code)]` call into libheif (decision open)
- libheif path: scale with fast_image_resize instead of the nearest-neighbour scaling of libheif
- First release: create the Codeberg repository `viewr`, then `just render` and `just release`
- Subfolders as entries in the grid (navigate without the picker)
- Optional thumbnail labels for a denser grid
- Reveal the picked image back in yazi (`ya emit reveal`)

## Later

- Move / copy / rename files (culling workflow)
- Shuffle order for slideshows
- Persistent thumbnail size per directory

## Done

- Fork of rview, first as imgrrr, then renamed to viewr
- Tooling from schnattr: justfile, lint table, cargo deny, nextest, Codeberg release flow
- Configurable, zoomable thumbnail size (`+` / `-` / `0`, remembered in state file)
- Slideshow with interval, pause and loop (`s`, `Space`, `[` / `]`, `--slideshow`)
- Info panel with file data and EXIF (`i`, `--info`)
- HEIC / HEIF decoding through libheif
- Video playback in mpv (`[video] player`: terminal, window, builtin)
- AVIF through libheif, embedded HEIC grid thumbnails, EXIF orientation for WebP and PNG
- EXIF orientation applied to JPEG / TIFF
