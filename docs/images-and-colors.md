# Images and colors

This page holds the design facts of the decoders (`src/decode/`) and of the color conversion.
AGENTS.md holds the rules and the layout of the files.

## Parts of `decode/`

- The tone map of float images (`tonemap.rs`) and of PQ and HLG stills (`hdr/`).
- The HEIF box reader (`heif_boxes.rs`).
- The resize (`resize.rs`).
- The color conversion (`color/`).
- The fullscreen image in the pixel layout of its decode (`screen/`).
- The folder shape from the file headers (`shape.rs`).

## Resize and pixel layouts

- Every resize uses Catmull-Rom. It costs two thirds of the time of Lanczos3 on a 24 MP frame.
- The layout of each step is the fastest one that `fast_image_resize` has for it. The code is
  `rgb_resize_is_fastest` in `decode/resize.rs`.
- Measurement of 2026-10-30, into a 3392x2052 box:

  | Layout | Scale 0.75 | Scale 1.5 |
  | --- | --- | --- |
  | RGBA without the alpha multiply | 17.6 ms | 9.3 ms |
  | RGB | 35.2 ms | 12.0 ms |

- From a scale of 1/2 down, RGB is as fast.
- The RGB copy of an opaque RGBA resize happens after the resize, at the size of the screen.
- The tests of `decode/screen/` compare the RGB layouts with the RGBA reference path. This is the
  path that viewr used before the RGB layouts.

## libheif

- `HeifContext::read_from_file` reads and copies the whole file first. This costs 30 ms to 40 ms
  for a 24 MP iPhone photo. `open_heif_context` reads through a stream instead.
- libheif writes RGB about 5 % faster than RGBA, with the same values.
- RGB is the fastest resize layout for the grid and for a 24 MP photo in the fullscreen box.
- The libheif default is 4 tile threads. With this default, 10 grid decodes ran 40 threads on
  10 cores, and each decode took twice as long. `libheif_tile_threads` shares the cores instead.
- The decoders keep the depth of an HDR file. libheif decodes 16-bit RGB.

### HEIF boxes

- `decode/heif_boxes.rs` reads `pitm` (the primary item), `iref` with its `thmb` references, and
  `iprp` with `ipco` (the properties) and `ipma` (the property indices of each item).
- The reader does not read `iinf`, because libheif already lists the thumbnail items.
- The `ispe` size needs no decode.
- A missing 90 or 270 degree rotation changes the shape of a thumbnail. A missing 180 degree
  rotation or mirror does not.

## Embedded JPEG (RAW and PSD)

- Most RAW formats are TIFF. An IFD holds the JPEG in one of three ways:
  - as `JPEGInterchangeFormat` and `JPEGInterchangeFormatLength` (tags 0x201 and 0x202),
  - as the one strip of a JPEG-compressed image (`StripOffsets`, `StripByteCounts` and
    `Compression` 6 or 7),
  - as the Panasonic `JpgFromRaw` tag (0x2e).
- The reader walks IFD0, its `SubIFDs` (0x14a) and the IFD chain. It takes the largest JPEG.
- The raw sensor data of a DNG or a CR2 is a lossless JPEG (SOF3). The SOF check rejects it.
- Canon CR3 is ISOBMFF. The `PRVW` and `THMB` boxes hold the JPEGs. `CMT1` holds the TIFF with
  the orientation.
- Fujifilm RAF has a fixed header with the JPEG offset and length.
- A Photoshop file carries its thumbnail in the image resource 1036 (JPEG RGB).
- sips takes 0.1 s to 1 s per RAW file. The embedded JPEG decodes in a few milliseconds, so the
  grid shows it without a wait.
- The files are `decode/embedded_jpeg/` (`mod.rs`, `tiff.rs`, `cr3.rs`, `psd.rs`).

## sips

- sips writes a TIFF. The `image` crate reads it in 8 or 16 bit.
- A TIFF is faster than a PNG, because sips does not compress it. A full 24 MP photo takes 0.16 s
  against 0.91 s.
- A PDF page and an SVG are vector graphics. sips draws them again at the requested size, so they
  fill the box.
- The zoom gets a large copy of a vector file (`VECTOR_ZOOM_SIDE`, 4096).
- A PDF page has a transparent background.

## Float images (EXR and Radiance HDR)

- These formats go through `decode::tonemap`, not through `decode::hdr`.
- A display-referred file keeps its samples in 0..=1. A plain conversion to 8 bit clamps each
  value above 1.0 to white. It also shows the dark parts too dark, because it does not apply the
  sRGB transfer curve.
- The operator is `c` for `w = 1`. For `w = 4`, linear 0.18 (middle gray) becomes 109 and 1.0
  becomes 193.

## Color conversion

The code is in `decode/color/`.

### Why viewr converts

- Ghostty on macOS draws into a Display P3 surface.
- The image shader of Ghostty copies the pixels of a Kitty image without a conversion. This holds
  also with `window-colorspace = srgb`, because that setting converts only the text colors.
- So Ghostty shows each image pixel as Display P3.
- viewr converts every image into Display P3. The key is `[terminal] colorspace`. The default is
  `display-p3`. The value `srgb` is for a terminal that shows the pixels as sRGB.
- `main` sets the target. The tests keep sRGB.

### Sources

Each decoder reports the color of its pixels.

- ICC profile: JPEG APP2 (also over several segments), PNG `iCCP`, WebP `ICCP`, TIFF tag 34675
  and HEIF `prof` or `rICC`.
- CICP: HEIF `nclx` and PNG `cICP`.
- PNG `gAMA` and `cHRM`.
- The Adobe RGB mark of a camera JPEG without a profile: EXIF `ColorSpace` 0xFFFF with the
  interoperability index `R03` (`color::jpeg_color`).
- An image without these data is sRGB.

### Conversion

- The conversion runs after the resize. It runs in the layout of the image (RGB through
  `color::convert_rgb`, or RGBA).
- It is one cached `moxcms` transform with the rows in parallel.
- A source in the target space converts nothing. So iPhone photos (Display P3) cost nothing.
- A source that cannot convert converts as sRGB. Examples: an unreadable or non-RGB ICC profile
  and unknown CICP codes.
- A PQ or HLG source (CICP transfer 16 or 18, also from the `cicp` tag of an ICC profile) gets
  the tone map of `decode::hdr`, because `moxcms` does not tone-map.
- The "as is" column of the table of `fixtures_match_colorsync` is what viewr sent before the
  color conversion existed.

### Player cubes

- The terminal player converts with a `lut3d` cube (`player/tone_map/mod.rs`).
- An SDR video or animation gets a 17-point cube from its `color_primaries` into the target.
  `Primaries::of_name` knows these primaries: BT.709, SMPTE 170M, BT.470 BG, Display P3, DCI-P3
  (with a Bradford adaptation) and BT.2020.
- An untagged stream and GIF count as BT.709 with the sRGB curve.
- ffprobe reports no profile for WebP and APNG. The probe reads their ICC profile or cICP chunk
  (`animation::embedded_primaries`).
- A profile with the colorants of sRGB, Display P3 or BT.2020 maps to its primaries
  (`color::icc_known_space`). Any other profile counts as BT.709.
- A source in the target space gets no cube.
- The SDR cube runs in 8 bit (`rgb24`, `rgba`). Its values hold half a level, because `lut3d`
  truncates to 8 bit. A real ffmpeg test fails if `lut3d` starts to round.
- At 1080p, the SDR cube costs 1.4 ms to 2 ms per frame.
- The sizes and the costs of the cubes are in [Video](video.md#color-cubes).

## HDR stills

The code is in `decode/hdr/`.

- The cube of the player samples the map at 33 points per channel, and `lut3d` interpolates
  between them. A still runs the map per pixel, with one table for the signal curve and one for
  the output curve.
- The decoders keep the depth of an HDR file:
  - libheif decodes 16-bit RGB.
  - `jxl-oxide` writes its 16-bit stream.
  - A 16-bit PNG stays 16-bit.
- `Decoded` resizes the 16-bit image and maps the small image, with the rows in parallel.
- An 8-bit HDR image maps in `color::convert`. An example is the embedded thumbnail of an HDR
  HEIC file.
- The source is a CICP transfer of 16 (PQ) or 18 (HLG). It comes from one of these places:
  - a HEIF `nclx` box,
  - a PNG `cICP` chunk,
  - the `cicp` tag of an ICC profile (`SourceColor::icc`),
  - the enum encoding of a JPEG XL file.
- The primaries are BT.2020, Display P3, DCI-P3 or BT.709. DCI-P3 uses the Bradford adaptation of
  its white point, as in the SDR cubes of the player.
- Other or unknown primaries count as BT.2020, the primaries of BT.2100.
- A test compares the still map with each point of the player cubes. A change of one copy fails
  the test.

## JPEG XL

- The facts of the ICC handling and of the XYB limit are in AGENTS.md (gotcha "JPEG XL colors").
- A PQ or HLG file keeps its 16-bit signal for the tone map of `decode/hdr/`.
