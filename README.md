# phewr

phewr is a terminal image and video gallery for the [Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/), in the spirit of Phoenix Slides. It shows a directory as a thumbnail grid. It opens images fullscreen, runs slideshows, plays videos and shows EXIF data inside the terminal.

phewr draws through the Kitty graphics protocol. It is tested in [Ghostty](https://ghostty.org/).

phewr is a fork of [rview](https://github.com/jrf/rview) by jrf (MIT). The fork adds these features:

- **Zoomable grid:** `+` and `-` show fewer or more thumbnail rows, and `0` resets the zoom. phewr remembers the zoom level.
- **Slideshow:** `s` starts a slideshow from the current image. `Space` pauses it, and `[` / `]` change the interval. The slideshow loops by default.
- **Info panel:** `i` shows file data and EXIF next to the grid or the fullscreen image. EXIF data includes the camera, lens, exposure, aperture, ISO, focal length and GPS position.
- **HEIC / HEIF:** phewr decodes iPhone photos through libheif and applies their rotation.
- **Upright JPEGs:** phewr applies the EXIF orientation, so portrait photos show upright.
- **More formats:** JPEG XL, EXR, HDR, QOI and TGA. On macOS, phewr also opens camera RAW, PSD, PDF and SVG through `sips`.

## Install

There is no release yet. Build phewr from source (see "From source" below). The maintainer tests phewr until the end of 2026. The first release comes in Q1 2027 at the earliest. The Homebrew tap and the installer script below work only after that release.

phewr is for macOS on Apple Silicon. The binary links the Homebrew library libheif. phewr starts `ffmpeg`, `ffprobe` and `mpv` when they are installed: they are optional, and without them videos have no thumbnails and play only in the mpv window, or not at all.

### Homebrew (after the first release)

```bash
brew tap webgefrickel/tap https://codeberg.org/webgefrickel/homebrew-tap.git
brew install webgefrickel/tap/phewr
```

The formula installs libheif as a dependency. ffmpeg and mpv are recommended dependencies: `brew install` adds them by default, and `--without-ffmpeg` or `--without-mpv` leaves them out.

### Installer script (after the first release)

```bash
brew install libheif ffmpeg mpv   # ffmpeg and mpv are optional
curl -fsSL https://codeberg.org/webgefrickel/phewr/releases/latest/download/install.sh | sh
```

The script installs `phewr` into `~/.local/bin`. Set `PREFIX` to use a different prefix.

### From source

```bash
brew install rustup just jpeg-turbo libheif ffmpeg mpv pkgconf
just install
```

`just install` builds a release binary and copies it to `~/.local/bin/phewr`.

Feature flags:

| Feature | Default | Needs |
|---------|---------|-------|
| `turbo` | on | libjpeg-turbo, fast JPEG decoding with downscaling |
| `heic` | on | libheif >= 1.19 |
| `video` | on | plays MP4/MOV/MKV/AVI/WebM/M4V; needs `ffmpeg` and `ffprobe` 7 or later on PATH at run time, not at build time |
| `sips` | on | macOS `/usr/bin/sips`, opens camera RAW, PSD, PDF, SVG and other ImageIO formats |

```bash
cargo install --path . --no-default-features --features turbo   # without HEIC and video
```

## Usage

```bash
phewr                                      # current directory
phewr ~/Pictures
phewr image.png                            # its folder, with the cursor on image.png
phewr a.png b.png                          # only these files
phewr -s ~/Pictures/holiday                # start a slideshow at once
phewr -s -n 5 ~/Pictures/holiday           # ... with 5 seconds per slide
phewr -i ~/Pictures                        # open with the info panel
phewr -j 4 ~/Pictures                      # decode threads (default: all cores)
```

### From yazi

```toml
# ~/.config/yazi/keymap.toml
[[mgr.prepend_keymap]]
on   = [ "g", "v" ]
run  = "shell --block -- phewr %h"
desc = "Image grid of the hovered folder, or at the hovered file (phewr)"
```

On a folder, `g v` opens that folder. On a file, it opens the folder of the file with the cursor on the file. When phewr quits, yazi moves its cursor to the last file of phewr. phewr runs `ya emit reveal <path>` for this. If `YAZI_ID` is not set or `ya` is not on PATH, nothing happens.

Inside a multiplexer that relays Kitty graphics slowly, open phewr in a new Ghostty window instead. `YAZI_ID` goes along, so the reveal still works:

```toml
run  = 'shell -- open -na Ghostty.app --args -e sh -c "YAZI_ID=$YAZI_ID exec $HOME/.local/bin/phewr %h"'
```

## Controls

### Gallery

| Key | Action |
|-----|--------|
| `h` `j` `k` `l`, arrows | Move |
| `g` `G`, `Home` `End` | First / last |
| `Ctrl-b` `Ctrl-f`, `PgUp` `PgDn` | Page up / down |
| `Enter` | Open fullscreen, play a video, or open a folder |
| `Backspace` | Go to the parent folder. The way back takes the thumbnails from memory |
| `p` | Play the video under the cursor in the mpv window, with sound |
| `s` | Start a slideshow from the cursor |
| `+` `-` `0` | Larger / smaller / default thumbnails (2 to 12 rows, remembered per folder, `Enter` opens the larger view) |
| `n` | Show or hide filename labels |
| `i` | Toggle the info panel |
| `/` | Search. `Enter` applies, `Esc` clears, `Backspace` deletes |
| `Space` | Toggle selection |
| `a` `A` | Select all / clear selection |
| `q` | Quit |
| `?` | Full help |

The grid lists the subfolders of the folder first, with `..` for the parent. A folder cell shows a large folder icon and its name, and `..` shows two dots. Hidden folders and symlinks to folders stay out. Folders take no mark and no delete, and the fullscreen view and the slideshow skip them. The search matches folder names. phewr always starts in the grid, also in a folder with one image. When the cursor rests on a folder, phewr prepares the thumbnails of that folder in the background, so `Enter` shows them at once.

The grid fills the terminal. The cells have the shape of the photos in the folder: a folder of portrait photos gets narrow cells and more columns. The grid separates the thumbnails with 1-cell gaps. A thick blue border marks the thumbnail under the cursor. A thin yellow border marks each selected image (`Space`). A cell shows a large dim picture (image) or play triangle (video) while its thumbnail loads. The still of a video or an animated image carries a play triangle until its loop starts.

The grid also works under a multiplexer that relays Kitty graphics, such as herdr. The filename toggle and a font size change decode the thumbnails again.

There is no search bar and no frame. A status row at the bottom shows the name and the position of the selected entry, the folder and image count when the grid has folders, the search query and the match count while you search with `/`, the number of selected images and `scanning…` while the scan runs. At the right, it shows the facts of the selected file: the size in pixels and megapixels, the file size, the format and the date of the shot (the EXIF date, else the modified time). A video shows its length, its size, its frame rate and `SDR`, `HLG` or `PQ`. The status row shows no key hints: `?` opens the help. In the fullscreen view, the image uses the whole terminal above the status row, and the status row shows the same facts for the open file.

### Fullscreen

| Key | Action |
|-----|--------|
| `h` `j` / `k` `l`, `←` `→` | Previous / next image (`hjkl` pan when zoomed) |
| `Home` `End` | First / last |
| `+` `-` `0` | Zoom in, out, fit. `hjkl` pans when zoomed |
| `s` | Start / stop the slideshow |
| `Space` | Pause / resume the slideshow |
| `[` `]` | Shorter / longer interval (0.5 s steps) |
| `i` | Toggle the info panel |
| `Enter` | Play the video (on a video placeholder) |
| `p` | Play the video in the mpv window, with sound (on a video placeholder) |
| `q` | Back to the gallery. In a running slideshow, `q` stops it and goes back in one press |
| `Esc` | Stop the slideshow, or back to the gallery |

### Video

phewr plays a video in the terminal with its own player. ffmpeg from Homebrew decodes on the media engine of the Mac, scales the frame on the GPU and maps HLG and PQ (iPhone HDR, Dolby Vision 8.4, HDR10) to SDR with a tone mapping cube. Every video and animation converts into the color space of the terminal, like the still images. phewr sends each frame through the Kitty temp file medium, and the terminal scales it to the window. A 1080p video plays at 30 fps without dropped frames in Ghostty. Opaque images and video frames go to the terminal as RGB, with a quarter fewer bytes than RGBA. Videos with a variable frame rate, for example screen recordings and phone clips, play at their real speed. The terminal player has no sound, no pause and no seek: it plays like an animated image, and it always keeps real time. If the decode or the terminal is too slow, it drops frames and never plays in slow motion. `p` opens the mpv window for sound and all controls.

| Key | Action |
|-----|--------|
| `←` `→`, `h` `j` / `k` `l`, `N` `n` | Previous / next file |
| `Space` | Pause / resume a running slideshow |
| `Home` `End` | First / last file |
| `p` | Continue in the mpv window, with sound |
| `q` `Esc` | Back to the gallery |
| `?` | Help |

`q` or `Esc` in the player goes back to the gallery in one press, with the cursor on the video, also when a step in the fullscreen view opened the video. After the mpv window, phewr goes back to the view that opened the window. In the fullscreen view, the video then shows "video: Enter plays, p opens the mpv window".

`p` opens the video in a fullscreen mpv window with sound, from the grid, the fullscreen view and the player. The window gets `--fs --native-fs=no`, so macOS does not move it to its own Space, and `--hwdec=auto-safe`. mpv does its own HDR tone mapping on the GPU. All mpv keys work. In mpv, `n` plays the next file of the list and `N` (Shift-n) the previous file (mpv 0.38 or later). After the window closes, phewr returns to the view that opened it.

Over SSH, or when the Kitty probe found no temp file medium, a video opens in the mpv window, with a note. Without mpv, a note shows and the view stays. The terminal player needs `ffmpeg` on PATH.

In a slideshow, a video plays one time and the slideshow continues. Outside a slideshow, the video loops. In the mpv window, the slideshow waits until the window closes. Use `q` or the close button to stop it, because `Cmd-q` counts as the end of the clip. A paused slideshow plays the video in a loop.

A status note (for example "clip.mp4 plays in an mpv window") replaces the file facts until the next key press.

`q` goes back one step in every view. Only `q` in the grid quits phewr. An empty folder shows a note in the status row. `Backspace` goes up, and `q` quits.

### Mouse

phewr reads the mouse in every view. `[gallery] mouse = false` turns this off, and the terminal keeps its own mouse. Shift+click and Shift+drag still select text in Ghostty while phewr runs.

| Where | Mouse | Action |
|-------|-------|--------|
| Grid | Click / double-click | Select / open the image, video or folder |
| Grid | Right click | Info panel |
| Grid | Wheel / Shift+wheel | Scroll / change the thumbnail size (a sideways trackpad swipe also changes the size) |
| Fullscreen | Wheel | Zoom |
| Fullscreen | Drag | Pan the zoomed image |
| Fullscreen | Click near the left or right edge | Previous / next image |
| Fullscreen | Right click | Info panel |
| Player | Click beside the video or right click | Back |
| Every view | Click on the status row | Help |

A click closes the help and the info panel. While an mpv window plays, phewr releases the mouse.

## Configuration

phewr reads `~/.config/phewr/config.toml`. If `XDG_CONFIG_HOME` is set, phewr reads `$XDG_CONFIG_HOME/phewr/config.toml`.

```toml
[gallery]
mouse = true         # read the mouse; false keeps the terminal's own mouse
animate = true       # video and animated cells play a short loop in the grid; false keeps the still with its play triangle
loops = "auto"       # how the loops play: "auto" (the terminal decides), "animation" (Kitty animation frames, Ghostty 1.4.0 and later), "ticks" (phewr swaps the frames)
visible_rows = 4     # thumbnail rows in the grid (the zoom level, 2 to 12), the columns fill the width

[slideshow]
interval = 3.0       # seconds per slide
loop = true          # start over after the last image

[info]
show_on_start = false

[video]
# "terminal" (default): the terminal player of phewr, no sound. p opens the mpv window.
# "window": mpv plays every video in a fullscreen window of its own, with sound.
# "builtin" from older versions means "terminal".
player = "terminal"

[terminal]
# "display-p3" (default): the terminal shows image pixels as Display P3, as Ghostty on macOS does.
# "srgb": the terminal shows image pixels as sRGB.
colorspace = "display-p3"
```

The terminal player and the video thumbnails need `ffmpeg` and `ffprobe` on PATH. Without them, a video cell shows the large play icon, and a video opens in the mpv window. Without the temp file medium, a video also opens in the mpv window. Without mpv, a note shows.

When you zoom the grid with `+` / `-`, phewr remembers the size for the current folder and as the last size in `~/.local/state/phewr/state.toml`. A folder opens with its own size. A folder without one opens with the last size, or with `[gallery] visible_rows`. `0` returns the current folder to the configured size and erases its entry. phewr keeps the sizes of the last 200 folders.

phewr caches thumbnails in the OS cache directory: `~/Library/Caches/phewr` on macOS, `~/.cache/phewr` on Linux. The cache stays below 512 MB.

### Colors

phewr uses the default foreground and background of the terminal and the 16 ANSI colors. The color scheme of the terminal sets the look of phewr. phewr has no settings for these colors.

### Kitty transfer

At start, phewr asks the terminal whether it reads image data from a temp file. If the terminal answers yes, phewr writes each picture to a file in `$TMPDIR` and sends only the path. Otherwise the pixels go through the terminal as text. The last row of the help popup (`?`) shows the result, for example `Kitty transfer: temp file (t=t)`. The environment variable `PHEWR_KITTY_TRANSFER=direct|png|file` skips the question and forces one medium. Over SSH, phewr sends PNG.

In the same round trip, phewr asks the terminal whether it plays Kitty animations. If the terminal answers yes (Ghostty 1.4.0 and later), each grid loop goes to the terminal one time as one animation, and the terminal plays it. Otherwise phewr swaps the frames itself. `[gallery] loops` in the config, or the environment variable `PHEWR_KITTY_LOOPS=animation|ticks`, which wins over the config, fixes the path. In herdr, phewr swaps the frames itself, because herdr 0.9.5 answers the probe but plays no Kitty animation. `[gallery] loops` and `PHEWR_KITTY_LOOPS` win over this rule.

## Formats

**Images:** JPEG, PNG, GIF, WebP, BMP, TIFF, ICO, QOI, TGA.

**Colors:** phewr converts the colors of each image into the color space of the terminal. Ghostty shows image pixels as Display P3, so that is the default. phewr reads ICC profiles (JPEG, PNG, WebP, TIFF, HEIC, AVIF), the nclx box of HEIC and AVIF, and the cICP, gAMA and cHRM chunks of PNG. A camera JPEG in Adobe RGB without a profile carries an EXIF mark, and phewr reads it too. An image without color information is sRGB. iPhone photos are already Display P3 and need no conversion.

**Animated images:** animated GIF, WebP, APNG (`.png`, `.apng`) and AVIF image sequences (AVIS) play in the terminal player, in a loop. In the grid, every visible animated cell and video cell plays a short loop after its still thumbnail shows: a video shows 12 frames from the whole clip at 3 fps, an animation its own frames. `[gallery] animate = false` in the config turns the loops off, and the still keeps its play triangle. `Enter` plays the animation, and `p` opens it in the mpv window. In a slideshow, the terminal player plays an animation for at least the slideshow interval. Animated WebP needs FFmpeg 9 or later (the `webp_anim` decoder).

**JPEG XL:** phewr decodes JPEG XL in Rust (jxl-oxide). It converts the colors into the color space of the terminal, also from ICC profiles and wide gamut, tone-maps PQ and HLG to SDR, and applies the orientation of the file. An animated JPEG XL shows its first frame.

**EXR and HDR:** phewr maps the float values to 8 bit with an extended Reinhard tone map. The white point is the brightest value of the image.

**Camera RAW, PSD, JPEG 2000, PDF, SVG, ICNS, SGI, DDS and PICT** (feature `sips`, macOS only): phewr converts these files with the built-in macOS tool `sips`. The grid and the fullscreen view get a copy at the size of their box. The zoom gets the full image. A PDF shows its first page. Camera RAW and PSD files show the JPEG preview that they carry at once in the grid. If that preview is large enough, phewr uses it instead of `sips` for the grid and the fullscreen view.

**HEIC, HEIF and AVIF** (feature `heic`): phewr decodes these files through libheif. Without `heic`, phewr does not open AVIF. A cold HEIC grid shows the thumbnails that the files hold at once, then replaces them with sharp thumbnails. phewr uses such a thumbnail only if it has the rotation and the mirror of the main image.

phewr applies the EXIF orientation of JPEG, TIFF, WebP and PNG files, so portrait photos show upright.

**Video** (feature `video`): MP4, MOV, MKV, AVI, WebM, M4V. The terminal player plays the video in a loop. Gallery thumbnails use the first frame, which `ffmpeg` decodes with the rotation and the HDR tone mapping.

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
PHEWR_TEST_HEIC=~/Pictures/IMG_0001.HEIC \
PHEWR_TEST_JPEG=~/Pictures/IMG_0002.JPG \
just test-real          # a test without its variable skips with a note
just test-real heic     # only the tests that match the filter
```

## License

MIT, the same license as rview. See LICENSE.
