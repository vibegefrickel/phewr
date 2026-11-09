# phewr

phewr is a terminal image and video gallery, in the spirit of Phoenix Slides. It shows a folder
as a thumbnail grid, opens images fullscreen, runs slideshows, plays videos and shows EXIF data,
all inside the terminal.

phewr draws through the [Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/).
It is tested in [Ghostty](https://ghostty.org/).

phewr is a fork of [rview](https://github.com/jrf/rview) by jrf. The fork adds a zoomable grid,
slideshows, an info panel with EXIF data, video playback, Live Photos, HEIC and many more formats.

**Status:** phewr is in development and has no release. The first release comes in Q1 2027 at the
earliest. To get phewr, clone the repository and build it (see "Install").

## Install

You need macOS on Apple Silicon, Rust (through rustup), just, libjpeg-turbo and libheif. ffmpeg
and mpv are optional. Without ffmpeg, videos have no thumbnails and play only in an mpv window.

```sh
brew install rustup just pkgconf jpeg-turbo libheif ffmpeg mpv
git clone https://codeberg.org/webgefrickel/phewr.git
cd phewr
just install
```

`just install` builds a release binary and copies it to `~/.local/bin/phewr`. Set `PREFIX` to
install into another prefix. `just uninstall` removes the binary again.

All features are on by default. To build without some of them, use cargo directly, for example
`cargo install --path . --no-default-features --features turbo`.

| Feature | Needs |
|---------|-------|
| `turbo` | libjpeg-turbo, for fast JPEG decoding |
| `heic` | libheif 1.19 or later, for HEIC, HEIF and AVIF |
| `video` | `ffmpeg` and `ffprobe` 7 or later on `PATH` at run time |
| `sips` | macOS `/usr/bin/sips`, for camera RAW, PSD, PDF, SVG and other formats |

## Usage

```sh
phewr                              # the current folder
phewr ~/Pictures
phewr image.png                    # its folder, with the cursor on image.png
phewr a.png b.png                  # only these files
phewr -s ~/Pictures/holiday        # start a slideshow at once
phewr -s -n 5 ~/Pictures/holiday   # ... with 5 seconds per slide
phewr -i ~/Pictures                # open with the info panel
phewr -j 4 ~/Pictures              # decode threads (default: all cores)
phewr --cwd-file /tmp/phewr-cwd    # write the folder of the grid into the file on exit
```

### Change the shell directory on exit

A program cannot change the directory of its shell. Like yazi, phewr writes the folder of the
grid into the file of `--cwd-file` when it exits, and a shell function does the `cd`:

```sh
# ~/.zshrc
p() {
  local tmp="$(mktemp -t "phewr-cwd.XXXXXX")"
  phewr "$@" --cwd-file="$tmp"
  if cwd="$(cat -- "$tmp")" && [ -n "$cwd" ] && [ "$cwd" != "$PWD" ]; then
    builtin cd -- "$cwd"
  fi
  rm -f -- "$tmp"
}
```

`p ~/Pictures` opens the grid. When you quit in a subfolder, the shell is in that subfolder.

### From yazi

```toml
# ~/.config/yazi/keymap.toml
[[mgr.prepend_keymap]]
on   = [ "g", "v" ]
run  = "shell --block -- phewr %h"
desc = "Image grid of the hovered folder, or at the hovered file (phewr)"
```

On a folder, `g v` opens that folder. On a file, it opens the folder of the file, with the cursor
on the file. When you quit phewr, yazi moves its cursor to the last file that you looked at.

If your multiplexer relays Kitty graphics slowly, open phewr in a new Ghostty window instead:

```toml
run  = 'shell -- open -na Ghostty.app --args -e sh -c "YAZI_ID=$YAZI_ID exec $HOME/.local/bin/phewr %h"'
```

### Marks to yazi

phewr is a viewer and changes no file. Mark files in the grid with `Space` or `a`, then quit:
phewr hands the marks to the yazi that started it, and yazi selects them. Then move, copy or
delete them in yazi. A marked Live Photo still brings its hidden `.mov`.

This needs a small yazi plugin. Save it as
`~/.config/yazi/plugins/phewr-select.yazi/main.lua` (yazi 26.9 or later):

```lua
return {
	entry = function(_, job)
		local list = job.args[1]
		local file = list and io.open(list, "r")
		if not file then
			return
		end
		for line in file:lines() do
			local url = Url(line)
			local cha = fs.cha(url)
			if cha then
				ya.emit("toggle", { File { url = url, cha = cha }, state = "on" })
			end
		end
		file:close()
		os.remove(list)
	end,
}
```

## Controls

`?` shows all keys. `q` goes back one step in every view. Only `q` in the grid quits phewr.

### Gallery

| Key | Action |
|-----|--------|
| `h` `j` `k` `l`, arrows | Move |
| `g` `G`, `Home` `End` | First / last |
| `Ctrl-b` `Ctrl-f`, `PgUp` `PgDn` | Page up / down |
| `Enter` | Open an image fullscreen, play a video, or open a folder |
| `Backspace` | Go to the parent folder |
| `p` | Play the video in an mpv window, with sound |
| `s` | Start a slideshow from the cursor |
| `+` `-` `0` | Larger / smaller / default thumbnails (2 to 12 rows) |
| `n` | Show or hide file names |
| `Tab` | Show or hide the info panel |
| `/` | Search. `Enter` applies, `Esc` clears. |
| `Space` | Mark or unmark (the marks go to yazi on quit) |
| `a` `A` | Mark all / clear the marks |
| `q` | Quit |

The grid shows the subfolders first, with `..` for the parent folder. The cell shape follows the
photos: a folder of portrait photos gets narrow cells. phewr remembers the zoom level of each
folder. Videos, animations and Live Photos play a short loop in the grid.

The status row at the bottom shows the selected file: its name, size in pixels, file size, format
and date. For a video, it shows the length, the frame rate and `SDR`, `HLG` or `PQ`.

### Fullscreen

| Key | Action |
|-----|--------|
| `h` `j` / `k` `l`, `←` `→` | Previous / next image (pan when zoomed) |
| `Home` `End` | First / last |
| `+` `-` `0` | Zoom in / out / fit |
| `s` | Start / stop the slideshow |
| `Space` | Pause / resume the slideshow |
| `[` `]` | Shorter / longer slideshow interval (0.5 s steps) |
| `Tab` | Show or hide the info panel |
| `Enter`, click in the middle | Play the video, the animation or the motion of a Live Photo |
| `p` | Play the video in an mpv window, with sound |
| `q` | Back to the gallery. In a slideshow, `q` stops it too. |
| `Esc` | Stop the slideshow, or go back to the gallery |

A slideshow uses the whole screen: it hides the status row and scales each picture to fill the
screen with its aspect. A portrait picture reaches the top and the bottom, a landscape picture
the left and the right. A small badge at the bottom right shows the play or pause state, the
interval and errors.

### Video

phewr plays videos in the terminal with its own player. The player has no sound, no pause and no
seek. It plays like an animated image and always keeps real time: if the decode or the terminal
is too slow, it drops frames. `p` opens the video in a fullscreen mpv window with sound and all
mpv controls. HDR videos (HLG, PQ, Dolby Vision 8.4, HDR10) play tone-mapped to SDR.

| Key | Action |
|-----|--------|
| `←` `→`, `h` `j` / `k` `l`, `N` `n` | Previous / next file |
| `Space` | Pause / resume a running slideshow |
| `Home` `End` | First / last file |
| `p` | Continue in the mpv window, with sound |
| `q` `Esc` | Back to the gallery |
| `?` | Help |

In the mpv window, `n` and `N` play the next and the previous file (mpv 0.38 or later). When the
window closes, phewr goes back to the view that opened it.

In a slideshow, a video plays once and the slideshow continues. Outside a slideshow, a video
loops. Over SSH, videos always open in the mpv window.

### Mouse

| Where | Mouse | Action |
|-------|-------|--------|
| Grid | Click / double-click | Select / open |
| Grid | Right click | Info panel |
| Grid | Wheel / Shift+wheel | Scroll / change the thumbnail size |
| Fullscreen | Wheel | Zoom |
| Fullscreen | Drag | Pan the zoomed image |
| Fullscreen | Click near the left or right edge | Previous / next image |
| Fullscreen | Right click | Info panel |
| Player | Click beside the video, or right click | Back |
| Every view | Click on the status row | Help |

To select text, hold Shift. `[gallery] mouse = false` turns the mouse off.

## Configuration

phewr reads `~/.config/phewr/config.toml`, or `$XDG_CONFIG_HOME/phewr/config.toml`. These are the
defaults:

```toml
[gallery]
mouse = true          # false leaves the mouse to the terminal
animate = true        # play short loops of videos, animations and Live Photos in the grid
live_photos = true    # hide the .mov of a Live Photo, Enter plays it
loops = "auto"        # "auto", "animation" (Kitty animation, Ghostty 1.4.0 or later) or "ticks"
visible_rows = 4      # thumbnail rows in the grid, 2 to 12

[slideshow]
interval = 3.0        # seconds per slide
loop = true           # start over after the last image

[info]
show_on_start = false

[video]
player = "terminal"   # "terminal" (no sound) or "window" (mpv with sound)

[terminal]
colorspace = "display-p3"   # "display-p3" (Ghostty on macOS) or "srgb"
```

phewr uses the colors of your terminal theme. It has no color settings.

| Path | Content |
|------|---------|
| `~/.local/state/phewr/state.toml` | the zoom level of the last 200 folders |
| `~/Library/Caches/phewr/` | the thumbnail cache, at most 512 MB |

At start, phewr asks the terminal how it accepts images. If the terminal reads temp files, phewr
sends only a file path, which is the fastest way. The last row of the help (`?`) shows the result.
`PHEWR_KITTY_TRANSFER=direct|png|file` forces one way, and `PHEWR_KITTY_LOOPS=animation|ticks`
forces the way that the grid loops play.

## Formats

- **Images:** JPEG, PNG, GIF, WebP, BMP, TIFF, ICO, QOI and TGA.
- **HEIC, HEIF and AVIF** (feature `heic`): through libheif.
- **JPEG XL:** in Rust, with wide gamut and HDR.
- **EXR and HDR:** tone-mapped to 8 bit.
- **Camera RAW, PSD, PDF, SVG, JPEG 2000, ICNS, SGI, DDS and PICT** (feature `sips`): through the
  macOS tool `sips`. RAW and PSD files show their embedded preview first. A PDF shows its first
  page.
- **Animated images:** GIF, WebP, APNG and AVIF sequences. Animated WebP needs FFmpeg 9 or later.
- **Live Photos:** phewr pairs an iPhone still with its `.mov`. The grid marks the still with
  `◎ Live` and loops the motion. `Enter` plays it once.
- **Video** (feature `video`): MP4, MOV, MKV, AVI, WebM and M4V.

phewr shows photos upright (EXIF orientation). It converts the colors of each image into the
color space of the terminal. It reads ICC profiles, and an image without color data counts as
sRGB.

## Development

```sh
brew install cargo-nextest cargo-deny cargo-llvm-cov
just          # format check, clippy, tests, rustdoc and cargo deny
just run      # build and start the gallery
just --list   # all recipes
```

Some tests decode real files from your library. A test without its variable skips:

```sh
PHEWR_TEST_HEIC=~/Pictures/IMG_0001.HEIC PHEWR_TEST_JPEG=~/Pictures/IMG_0002.JPG just test-real
```

`AGENTS.md` has the code layout, the rules and the workflow for contributors. `docs/` has the
design notes.

## AI assistance

phewr is written with the help of Claude by Anthropic, mainly the models Claude Opus and Claude
Fable. The maintainer reviews and tests each change.

## License

MIT. See `LICENSE`. phewr is a fork of [rview](https://github.com/jrf/rview) by jrf, which is MIT
too.
