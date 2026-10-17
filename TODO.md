# TODO

## Now

- Testing phase: tests for `app.rs` (58%) and `main.rs` (43%), then `scanner.rs` (71%), `config.rs` (75%) and `theme.rs` (84%). Total line coverage: 76%
- Use `tempfile` in the tests instead of hand-made directories below `std::env::temp_dir()`

## Next

- Slideshow: continue after mpv instead of a stop at the video (decision open)
- Next and previous video from inside mpv (for example mpv key bindings that run `quit <code>`)
- Built-in player (`video.rs`, `decode_loop`): a pause after the last packet stops the loop. After the resume, the next pass reads no packet, `got_frame` stays `false` and the thread returns at the end of the clip
- Built-in player (`video.rs`, `decode_loop`): each pause loses one packet. The `break` on `Pause` leaves the loop before `send_packet` for the packet that `input.packets()` already read
- Tests: `App::graphics_delete_all()` writes Kitty escape sequences to the real stdout. nextest captures them, but `cargo test -- --nocapture` deletes the images of the terminal. Give `App` a writer or a fake graphics backend
- Theme: `--theme nope` with a non-empty catalog selects the first catalog entry and shows no error (`load_themes_at` runs the fallback before the `strict` check)
- Theme: `load_themes_at` reads `~/.config/viewr/config.toml` and ignores `XDG_CONFIG_HOME`, but `config.rs` reads `$XDG_CONFIG_HOME/viewr/config.toml`
- Delete: after a delete in the grid, the cursor jumps to the first image (`update_filter` resets it). Keep the cursor near the deleted image
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
- Mouse-based pan and zoom in the fullscreen view

## Done

- Fork of rview, first as imgrrr, then renamed to viewr
- Tooling from schnattr: justfile, lint table, cargo deny, nextest, Codeberg release flow
- Configurable, zoomable thumbnail size (`+` / `-` / `0`, remembered in state file)
- Slideshow with interval, pause and loop (`s`, `Space`, `[` / `]`, `--slideshow`)
- Info panel with file data and EXIF (`i`, `--info`)
- HEIC / HEIF decoding through libheif
- Video playback in mpv (`[video] player`: terminal, window, builtin)
- Return to the view that opened a video, video placeholder in fullscreen
- Prefetch: generation counter against stale results, no retry loop after a failed decode
- AVIF through libheif, embedded HEIC grid thumbnails, EXIF orientation for WebP and PNG
- EXIF orientation applied to JPEG / TIFF
