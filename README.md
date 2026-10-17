# viewr

viewr is a terminal image and video gallery for the [Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/), in the spirit of Phoenix Slides. It shows a directory as a thumbnail grid. It opens images fullscreen, runs slideshows, plays videos and shows EXIF data inside the terminal.

viewr runs in [Ghostty](https://ghostty.org/), [Kitty](https://sw.kovidgoyal.net/kitty/) and [WezTerm](https://wezfurlong.org/wezterm/).

viewr is a fork of [rview](https://github.com/jrf/rview) by jrf (MIT). The fork adds these features:

- **Resizable thumbnails:** `+` and `-` zoom the grid, and `0` resets it. viewr remembers the last size.
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
viewr -t ~/.config/themes/nord.toml        # theme file, or a catalog name
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

## Controls

### Gallery

| Key | Action |
|-----|--------|
| `h` `j` `k` `l`, arrows | Move |
| `g` `G`, `Home` `End` | First / last |
| `Ctrl-b` `Ctrl-f`, `PgUp` `PgDn` | Page up / down |
| `Enter` | Open fullscreen |
| `s` | Start a slideshow from the cursor |
| `+` `-` `0` | Larger / smaller / default thumbnails |
| `i` | Toggle the info panel |
| `/` | Search. `Enter` applies, `Esc` clears, `Backspace` deletes |
| `Space` | Toggle selection |
| `a` `A` | Select all / clear selection |
| `d` `D` | Trash / permanently delete, with confirm |
| `o` | Directory picker |
| `t` | Theme picker |
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
| `Esc` | Stop the slideshow, or back to the gallery |
| `q` | Quit |

### Video

viewr gives a video to mpv. All mpv keys work: `Space` pauses, `←` / `→` seek, `f` toggles fullscreen in window mode. `q` or `Ctrl-c` stops mpv. viewr then goes back to the view that opened the video:

- From the gallery (`Enter`), viewr shows the gallery with the cursor on the video.
- From fullscreen (`←` / `→`, `Home` / `End`), viewr stays on the video and shows "video: press Enter to play". `Enter` plays the video again, and `←` / `→` step on.

In a slideshow, a video plays one time and the slideshow continues. To stop the slideshow, quit mpv with `q` or `Ctrl-c`, or press `Esc` in the built-in player. With the window player, the slideshow waits until the window closes. Use `q` or the close button to stop it, because `Cmd-q` counts as the end of the clip. A paused slideshow plays the video in a loop.

In fullscreen, a status note (for example "clip.mp4 plays in an mpv window") replaces the key hints until the next key press.

The built-in player (`[video] player = "builtin"`) keeps its keys: `Space`, `←` / `→`, `h` / `l`, `Home` / `End`, `d` / `D`, `Esc`, `q`.

## Configuration

viewr reads `~/.config/viewr/config.toml`. If `XDG_CONFIG_HOME` is set, viewr reads `$XDG_CONFIG_HOME/viewr/config.toml`.

```toml
# Theme file or catalog entry, see "Themes".
theme = "~/.config/themes/nord.toml"
theme_catalog = "~/.config/themes/catalog.toml"

[gallery]
thumb_cols = 20      # thumbnail cell width in terminal cells, border included
thumb_rows = 8       # thumbnail cell height

[slideshow]
interval = 3.0       # seconds per slide
loop = true          # start over after the last image

[info]
show_on_start = false

[video]
# "terminal" (default): mpv plays in this terminal with --vo=kitty. Press q in mpv to go back.
# "window": mpv plays in a window of its own, and viewr keeps running.
# "builtin": the built-in FFmpeg player (10 fps, no sound).
player = "terminal"
```

If mpv is not on PATH, viewr uses the built-in player and shows a note.

When you zoom the grid with `+` / `-`, viewr writes the new size to `~/.local/state/viewr/state.toml`. On the next start, that size wins over `[gallery]`. `0` returns to the configured size and erases the remembered size.

viewr caches thumbnails in the OS cache directory: `~/Library/Caches/viewr` on macOS, `~/.cache/viewr` on Linux. The cache stays below 512 MB.

### Themes

viewr loads themes from the `theme` and `theme_catalog` keys above. `-t` sets the theme for one session. If the `-t` value is neither a theme file nor a catalog name, viewr stops and lists the catalog names. If the `theme` key names an unknown theme, viewr uses the first catalog theme. `t` shows a preview of each catalog theme. viewr does not write that choice back.

## Formats

**Images:** JPEG, PNG, GIF, WebP, BMP, TIFF, ICO, QOI, TGA.

**JPEG XL:** viewr decodes JPEG XL in Rust (jxl-oxide). It converts the colors to sRGB and applies the orientation of the file. An animated JPEG XL shows its first frame.

**EXR and HDR:** viewr maps the float values to 8 bit with an extended Reinhard tone map. The white point is the brightest value of the image.

**Camera RAW, PSD, JPEG 2000, PDF, SVG, ICNS, SGI, DDS and PICT** (feature `sips`, macOS only): viewr converts these files with the built-in macOS tool `sips`. The grid and the fullscreen view get a copy at the size of their box. The zoom gets the full image. A PDF shows its first page.

**HEIC, HEIF and AVIF** (feature `heic`): viewr decodes these files through libheif. Without `heic`, viewr does not open AVIF. For HEIC grid thumbnails, viewr uses the thumbnail that the file holds, if it is large enough and has the rotation and the mirror of the main image.

viewr applies the EXIF orientation of JPEG, TIFF, WebP and PNG files, so portrait photos show upright.

**Video** (feature `video`): MP4, MOV, MKV, AVI, WebM, M4V. mpv plays the video in a loop. Gallery thumbnails use the first frame, which FFmpeg decodes.

## Development

AGENTS.md holds the rules for the code and the workflow.

```bash
just              # = just check: fmt --check, clippy -D warnings, nextest, rustdoc, cargo deny
just fix          # clippy --fix + cargo fmt
just test         # cargo nextest (falls back to cargo test)
just coverage     # line coverage summary (cargo-llvm-cov)
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
