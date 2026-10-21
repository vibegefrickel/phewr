# viewr

viewr is a terminal image and video gallery for the [Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/), in the spirit of Phoenix Slides. It shows a directory as a thumbnail grid. It opens images fullscreen, runs slideshows, plays videos and shows EXIF data inside the terminal.

viewr draws through the Kitty graphics protocol. It is tested in [Ghostty](https://ghostty.org/).

viewr is a fork of [rview](https://github.com/jrf/rview) by jrf (MIT). The fork adds these features:

- **Zoomable grid:** `+` and `-` show fewer or more thumbnail rows, and `0` resets the zoom. viewr remembers the zoom level.
- **Slideshow:** `s` starts a slideshow from the current image. `Space` pauses it, and `[` / `]` change the interval. The slideshow loops by default.
- **Info panel:** `i` shows file data and EXIF next to the grid or the fullscreen image. EXIF data includes the camera, lens, exposure, aperture, ISO, focal length and GPS position.
- **HEIC / HEIF:** viewr decodes iPhone photos through libheif and applies their rotation.
- **Upright JPEGs:** viewr applies the EXIF orientation, so portrait photos show upright.
- **More formats:** JPEG XL, EXR, HDR, QOI and TGA. On macOS, viewr also opens camera RAW, PSD, PDF and SVG through `sips`.

## Install

viewr is for macOS on Apple Silicon. The binary links the Homebrew libraries libheif and FFmpeg.

### Homebrew

```bash
brew tap webgefrickel/tap https://codeberg.org/webgefrickel/homebrew-tap.git
brew install webgefrickel/tap/viewr
```

The formula installs libheif, ffmpeg and mpv as dependencies.

### Installer script

```bash
brew install libheif ffmpeg mpv
curl -fsSL https://codeberg.org/webgefrickel/viewr/releases/latest/download/install.sh | sh
```

The script installs `viewr` into `~/.local/bin`. Set `PREFIX` to use a different prefix.

### From source

```bash
brew install rustup just jpeg-turbo libheif ffmpeg mpv pkgconf
just install
```

`just install` builds a release binary and copies it to `~/.local/bin/viewr`.

Feature flags:

| Feature | Default | Needs |
|---------|---------|-------|
| `turbo` | on | libjpeg-turbo, fast JPEG decoding with downscaling |
| `heic` | on | libheif >= 1.19 |
| `video` | on | FFmpeg >= 7, plays MP4/MOV/MKV/AVI/WebM/M4V |
| `sips` | on | macOS `/usr/bin/sips`, opens camera RAW, PSD, PDF, SVG and other ImageIO formats |

```bash
cargo install --path . --no-default-features --features turbo   # without HEIC and video
```

## Usage

```bash
viewr                                      # current directory
viewr ~/Pictures
viewr image.png                            # single file opens fullscreen
viewr -s ~/Pictures/holiday                # start a slideshow at once
viewr -s -n 5 ~/Pictures/holiday           # ... with 5 seconds per slide
viewr -i ~/Pictures                        # open with the info panel
viewr -j 4 ~/Pictures                      # decode threads (default: all cores)
```

### From yazi

```toml
# ~/.config/yazi/keymap.toml
[[mgr.prepend_keymap]]
on   = [ "g", "v" ]
run  = "shell --block -- viewr ."
desc = "Image grid (viewr)"
```

When viewr quits, yazi moves its cursor to the last file of viewr. viewr runs `ya emit reveal <path>` for this. If `YAZI_ID` is not set or `ya` is not on PATH, nothing happens.

## Controls

### Gallery

| Key | Action |
|-----|--------|
| `h` `j` `k` `l`, arrows | Move |
| `g` `G`, `Home` `End` | First / last |
| `Ctrl-b` `Ctrl-f`, `PgUp` `PgDn` | Page up / down |
| `Enter` | Open fullscreen |
| `s` | Start a slideshow from the cursor |
| `+` `-` `0` | Larger / smaller / default thumbnails (2 to 12 rows, `Enter` opens the larger view) |
| `n` | Show or hide filename labels |
| `i` | Toggle the info panel |
| `/` | Search. `Enter` applies, `Esc` clears, `Backspace` deletes |
| `Space` | Toggle selection |
| `a` `A` | Select all / clear selection |

The grid fills the terminal. The cells have the shape of the photos in the folder: a folder of portrait photos gets narrow cells and more columns. The grid separates the thumbnails with 1-cell gaps. A thick blue border marks the thumbnail under the cursor. A thin yellow border marks each selected image (`Space`). A cell shows a dim ◌ (image) or ▶ (video) while its thumbnail loads.

The grid also works under a multiplexer that relays Kitty graphics, such as herdr. The filename toggle and a font size change decode the thumbnails again.

There is no search bar and no frame. A status row at the bottom shows the name and the position of the selected image, the search query and the match count while you search with `/`, the number of selected images, `scanning…` while the scan runs, and the key hints. In the fullscreen view, the image uses the whole terminal above the status row, and the filename shows in the status row.
| `d` `D` | Trash / permanently delete, with confirm |
| `o` | Directory picker |
| `q` `Esc` | Quit |
| `?` | Full help |

### Fullscreen

| Key | Action |
|-----|--------|
| `h` `l`, arrows | Previous / next image |
| `Home` `End` | First / last |
| `+` `-` `0` | Zoom in, out, fit. `hjkl` pans when zoomed |
| `s` | Start / stop the slideshow |
| `Space` | Pause / resume the slideshow |
| `[` `]` | Shorter / longer interval (0.5 s steps) |
| `i` | Toggle the info panel |
| `d` `D` | Trash / permanently delete, with confirm |
| `Enter` | Play the video again (on a video placeholder) |
| `q` | Back to the gallery. In a running slideshow, `q` stops it and goes back in one press |
| `Esc` | Stop the slideshow, or back to the gallery |

### Video

viewr gives a video to mpv. By default, mpv plays it in a fullscreen window, with the same picture and HDR as the mpv app. All mpv keys work: `Space` pauses, `←` / `→` seek, `f` leaves or enters fullscreen. `q` or `Ctrl-c` stops mpv. viewr then goes back to the view that opened the video:

- From the gallery (`Enter`), viewr shows the gallery with the cursor on the video.
- From fullscreen (`←` / `→`, `Home` / `End`), viewr stays on the video and shows "video: press Enter to play". `Enter` plays the video again, and `←` / `→` step on.

In mpv, `n` plays the next file of the list and `N` (Shift-n) the previous file. viewr starts mpv at once for a video and shows an image in fullscreen. These keys need mpv 0.38 or later.

The window gets `--fs --native-fs=no`, so macOS does not move it to its own Space. With the terminal player (`[video] player = "terminal"`), viewr starts mpv with `--vo=kitty --profile=sw-fast --hwdec=auto-copy-safe` and a scale filter that fits each frame to the terminal before the rotation. A 4K iPhone video plays about three times faster with this filter. The Kitty output of mpv cannot tone map HDR video. For HLG and PQ videos (iPhone HDR, Dolby Vision 8.4, HDR10), viewr writes a tone mapping LUT into the cache directory, and mpv applies it. The colors are close to the SDR export of macOS. The mpv window gets `--hwdec=auto-safe` and does its own HDR tone mapping on the GPU. The filters of your `mpv.conf` stay active.

In a slideshow, a video plays one time and the slideshow continues. With the window player (the default), the slideshow waits until the window closes. Use `q` or the close button to stop it, because `Cmd-q` counts as the end of the clip. With the terminal player, quit mpv with `q` or `Ctrl-c` to stop the slideshow. In the built-in player, press `Esc`. A paused slideshow plays the video in a loop.

In fullscreen, a status note (for example "clip.mp4 plays in an mpv window") replaces the key hints until the next key press.

The built-in player (`[video] player = "builtin"`) keeps its keys: `Space`, `←` / `→`, `h` / `l`, `Home` / `End`, `d` / `D`. `q` and `Esc` go back to the view that opened the video.

`q` goes back one step in every view. Only `q` in the grid quits viewr. In the directory picker, `q` closes the picker. If the folder has no photos yet, `q` in the picker quits viewr, because there is no view to go back to.

## Configuration

viewr reads `~/.config/viewr/config.toml`. If `XDG_CONFIG_HOME` is set, viewr reads `$XDG_CONFIG_HOME/viewr/config.toml`.

```toml
[gallery]
visible_rows = 4     # thumbnail rows in the grid (the zoom level, 2 to 12), the columns fill the width

[slideshow]
interval = 3.0       # seconds per slide
loop = true          # start over after the last image

[info]
show_on_start = false

[video]
# "window" (default): mpv plays in a fullscreen window of its own, and viewr keeps running.
# "terminal": mpv plays in this terminal with --vo=kitty. Slow in Ghostty, use it over SSH. Press q in mpv to go back.
# "builtin": the built-in FFmpeg player (10 fps, no sound).
player = "window"
```

If mpv is not on PATH, viewr uses the built-in player and shows a note.

When you zoom the grid with `+` / `-`, viewr writes the new size to `~/.local/state/viewr/state.toml`. On the next start, that size wins over `[gallery]`. `0` returns to the configured size and erases the remembered size.

viewr caches thumbnails in the OS cache directory: `~/Library/Caches/viewr` on macOS, `~/.cache/viewr` on Linux. The cache stays below 512 MB.

### Colors

viewr uses the default foreground and background of the terminal and the 16 ANSI colors. The color scheme of the terminal sets the look of viewr. viewr has no color settings.

### Kitty transfer

At start, viewr asks the terminal whether it reads image data from a temp file. If the terminal answers yes, viewr writes each picture to a file in `$TMPDIR` and sends only the path. Otherwise the pixels go through the terminal as text. The first status note shows the result, for example `Kitty transfer: temp file (t=t)`. The environment variable `VIEWR_KITTY_TRANSFER=direct|png|file` skips the question and forces one medium. Over SSH, viewr sends PNG.

## Formats

**Images:** JPEG, PNG, GIF, WebP, BMP, TIFF, ICO, QOI, TGA.

**JPEG XL:** viewr decodes JPEG XL in Rust (jxl-oxide). It converts the colors to sRGB and applies the orientation of the file. An animated JPEG XL shows its first frame.

**EXR and HDR:** viewr maps the float values to 8 bit with an extended Reinhard tone map. The white point is the brightest value of the image.

**Camera RAW, PSD, JPEG 2000, PDF, SVG, ICNS, SGI, DDS and PICT** (feature `sips`, macOS only): viewr converts these files with the built-in macOS tool `sips`. The grid and the fullscreen view get a copy at the size of their box. The zoom gets the full image. A PDF shows its first page.

**HEIC, HEIF and AVIF** (feature `heic`): viewr decodes these files through libheif. Without `heic`, viewr does not open AVIF. A cold HEIC grid shows the thumbnails that the files hold at once, then replaces them with sharp thumbnails. viewr uses such a thumbnail only if it has the rotation and the mirror of the main image.

viewr applies the EXIF orientation of JPEG, TIFF, WebP and PNG files, so portrait photos show upright.

**Video** (feature `video`): MP4, MOV, MKV, AVI, WebM, M4V. mpv plays the video in a loop. Gallery thumbnails use the first frame, which FFmpeg decodes.

## Development

AGENTS.md holds the rules for the code and the workflow.

```bash
just              # = just check: fmt --check, clippy -D warnings, nextest, rustdoc, cargo deny
just fix          # clippy --fix + cargo fmt
just test         # cargo nextest (falls back to cargo test)
just coverage     # line coverage summary (cargo-llvm-cov)
just bench DIR    # speed measurements on a folder of your photos (ignored tests)
just run          # release build, then the gallery
just hooks        # install the pre-commit hook (fmt + clippy)

# decode real files from your library
VIEWR_TEST_HEIC=~/Pictures/IMG_0001.HEIC \
VIEWR_TEST_JPEG=~/Pictures/IMG_0002.JPG \
just test-real          # a test without its variable skips with a note
just test-real heic     # only the tests that match the filter
```

## License

MIT, the same license as rview. See LICENSE.
