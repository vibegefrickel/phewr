# Images and colors

The code of this page lives in tuilibr since 2026-11-19. `tuilibr/docs/pictures.md` is the master.

This page holds the design facts of the decoders (`tuilibr::pictures::decode`) and of the color
conversion. AGENTS.md holds the rules and the layout of the files.

## Parts of `tuilibr::pictures::decode`

- The tone map of float images (`tonemap.rs`) and of PQ and HLG stills (`hdr/`).
- The HEIF box reader (`heif_boxes.rs`).
- The extension lists and `has_extension` (`extensions.rs`).
- The color conversion (`color/`).
- The fullscreen image in the pixel layout of its decode (`screen/`).
- `dimensions`, the size from the header (`mod.rs`).

The resize is in `tuilibr::pictures::resize`, and the folder shape from the file headers is in
`tuilibr::pictures::shape`.

## Resize and pixel layouts

- Every resize uses Catmull-Rom. It costs two thirds of the time of Lanczos3 on a 24 MP frame.
- The layout of each step is the fastest one that `fast_image_resize` has for it. The code is
  `rgb_resize_is_fastest` in `tuilibr::pictures::resize`.
- Measurement of 2026-10-30, into a 3392x2052 box:

  | Layout | Scale 0.75 | Scale 1.5 |
  | --- | --- | --- |
  | RGBA without the alpha multiply | 17.6 ms | 9.3 ms |
  | RGB | 35.2 ms | 12.0 ms |

- From a scale of 1/2 down, RGB is as fast.
- The RGB copy of an opaque RGBA resize happens after the resize, at the size of the screen.
- The tests of `tuilibr::pictures::decode::screen` compare the RGB layouts with the RGBA reference path.
  This is the path that phewr used before the RGB layouts.
- libjpeg-turbo decodes at the smallest DCT scale of 1/8, 1/4, 1/2 and 1 that covers the fit of
  the image into the box (`turbo::pick_scaling_factor`, the rule of `EmbeddedJpeg::covers`). The
  decode is never smaller than the fit. The RGB or RGBA choice then follows the decoded size.
- Decode time per scale, 12 MP iPhone JPEG, one thread, RGBA, median of 10 runs (2026-11-03):

  | Scale | 1/8 | 1/4 | 3/8 | 1/2 | 5/8 | 3/4 | 7/8 | 1/1 |
  | --- | --- | --- | --- | --- | --- | --- | --- | --- |
  | Time | 15 ms | 16 ms | 18 ms | 17 ms | 24 ms | 28 ms | 33 ms | 22 ms |

- Only the 8x8, 4x4 and 2x2 IDCTs have NEON code. The other eighths run in plain C, so phewr
  never picks them. On 24 iPhone JPEGs, the fit rule makes the cold grid about 8 % faster
  (139 ms to 128 ms). The fullscreen open and the prefetch do not change.

## libheif

- `HeifContext::read_from_file` reads and copies the whole file first. This costs 30 ms to 40 ms
  for a 24 MP iPhone photo. `open_heif_context` reads through a stream instead.
- libheif writes RGB about 5 % faster than RGBA, with the same values.
- RGB is the fastest resize layout for the grid and for a 24 MP photo in the fullscreen box.
- The libheif default is 4 tile threads. With this default, 10 grid decodes ran 40 threads on
  10 cores, and each decode took twice as long. `libheif_tile_threads` shares the cores instead.
- The decoders keep the depth of an HDR file. libheif decodes 16-bit RGB.

### HEIF boxes

- `tuilibr::pictures::decode::heif_boxes` reads `pitm` (the primary item), `iref` with its `thmb`
  references, and `iprp` with `ipco` (the properties) and `ipma` (the property indices of each
  item).
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
- An Olympus or OM System ORF holds no JPEG in its TIFF IFDs. Its preview sits in the maker
  note: tag 0x927C of the EXIF IFD, the maker note IFD, the `CameraSettings` sub-IFD (0x2020)
  and the pair `PreviewImageStart` (0x101) and `PreviewImageLength` (0x102).
  - The forms `OLYMPUS\0` and `OM SYSTEM\0` count the offsets from the maker note start. The
    old form `OLYMP\0` or `EPSON\0` counts them from the TIFF header.
  - `CameraSettings` can be a pointer or inline `undef` data. A `PreviewImageValid` (0x100) of 0
    gives no preview.
  - Without that preview, an ORF takes the TIFF IFD walk.
- A JPEG covers a box when its fit into the box needs no upscale (`EmbeddedJpeg::covers`). The
  grid, the fullscreen view and the prefetch use this one rule.
- A Photoshop file carries its thumbnail in the image resource 1036 (JPEG RGB).
- sips takes 0.1 s to 1 s per RAW file. The embedded JPEG decodes in a few milliseconds, so the
  grid shows it without a wait.
- The files are `tuilibr::pictures::decode::embedded_jpeg` (`mod.rs`, `tiff.rs`, `cr3.rs`, `orf.rs`,
  `psd.rs`).
- `P5048182.ORF` (OM-1): the 3200x2400 preview is found in 1 ms. The grid decodes it in 17 ms
  to 23 ms, against 1.3 s to 1.6 s for `sips -Z 624`. The fullscreen view (box 3392x2052)
  decodes it in 31 ms to 73 ms, against 2.2 s to 7.5 s for sips, under the load of other
  workers.

## sips

- sips writes a TIFF. The `image` crate reads it in 8 or 16 bit.
- A TIFF is faster than a PNG, because sips does not compress it. A full 24 MP photo takes 0.16 s
  against 0.91 s.
- A PDF page and an SVG are vector graphics. sips draws them again at the requested size, so they
  fill the box.
- The zoom gets a large copy of a vector file (`VECTOR_ZOOM_SIDE`, 4096).
- A PDF page has a transparent background.
- `sips -Z` reads no embedded preview. A pixel compare on 2026-11-03 put the `-Z 512` output
  closer to a scaled full develop than to the embedded JPEG for Canon CR3, Nikon NEF and Fujifilm
  RAF. CR3 develops 40 % faster with `-Z` (527 ms against 904 ms). NEF and RAF take the time of
  the full develop.
- The embedded JPEG route is 15 to 300 times faster than sips for a grid cell. The real files of
  `08-raw` (`previews::raw_files`, release build):

| Make | Embedded JPEG | Grid decode | Fullscreen decode | sips grid decode |
| --- | --- | --- | --- | --- |
| CR2 | 6720x4480 | 20 ms to 22 ms | 38 ms to 43 ms | 484 ms to 659 ms |
| RAF | 4416x2944 | 35 ms to 45 ms | 43 ms to 56 ms | 1153 ms to 1249 ms |
| NEF | 5568x3712 | 28 ms to 43 ms | 40 ms to 58 ms | 779 ms to 954 ms |
| DNG | 4032x3024 | 51 ms | 83 ms | 590 ms |
| CR3 | 1620x1080 | 4 ms to 9 ms | 4 ms to 6 ms | 669 ms to 1106 ms |
| RW2 | 1920x1440 | 5 ms | 6 ms | 1786 ms |
| ARW | 4608x3072 | 19 ms | 27 ms | 521 ms |

- sips ignores the orientation of RAF and ARW files. Four real portrait files of raw.pixls.us
  (Fujifilm X-A3 and X-H2, Sony DSLR-A450 and SLT-A58) showed it on macOS 26.7.2. phewr turns a
  develop that keeps the unturned aspect (`sips::corrected`). The turn costs about 30 ms for a
  fullscreen develop and 196 ms for a 7728x5152 zoom develop.
- sips develops `P1000475.RW2` (Panasonic) all black. A RAW develop with only zero color samples
  gives way to the embedded JPEG at the box. The check stops at the first sample that is not 0
  (28 µs on a 7728x5152 develop).
- The aspect cannot show an orientation of 2, 3 or 4 (a 180 degree turn or a mirror). For RAF and
  ARW, `sips::flip_gap` compares 32x32 gray pictures of the develop, as it is and oriented, with a
  small reference: the IFD1 JPEG of an ARW or the EXIF thumbnail of the RAF preview
  (`embedded_jpeg::decode_reference`). The large preview takes 31 ms to 63 ms to decode, the
  reference 0.5 ms to 1.5 ms with the compare. The oriented develop wins when it is closer by more
  than `FLIP_MARGIN` (2.0 gray levels). The smallest real gap of the copies of `DSCF0118.RAF`,
  `DSCF0119.RAF`, `DSCF0120.RAF` and `apsc_compressed_lossless.ARW` with the orientations 2, 3 and
  4 is 10.52 levels. A tie or a file without a reference keeps the develop. Thumbnail cache v18.

## Float images (EXR and Radiance HDR)

- These formats go through `tuilibr::pictures::decode::tonemap`, not through `tuilibr::pictures::decode::hdr`.
- A display-referred file keeps its samples in 0..=1. A plain conversion to 8 bit clamps each
  value above 1.0 to white. It also shows the dark parts too dark, because it does not apply the
  sRGB transfer curve.
- The operator is `c` for `w = 1`. For `w = 4`, linear 0.18 (middle gray) becomes 109 and 1.0
  becomes 193.

## Color conversion

The code is in `tuilibr::pictures::decode::color`.

### Why phewr converts

- Ghostty on macOS draws into a Display P3 surface.
- The image shader of Ghostty copies the pixels of a Kitty image without a conversion. This holds
  also with `window-colorspace = srgb`, because that setting converts only the text colors.
- So Ghostty shows each image pixel as Display P3.
- phewr converts every image into Display P3. The key is `[terminal] colorspace`. The default is
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
- moxcms 0.9.1 truncated the output table of a known curve: `make_gamma_table_impl` in its
  `src/gamma.rs` lost the `.round()` of 0.8.1, and half of all 8-bit values came out one level
  dark. From 2026-11-03 to 2026-11-09, `rounding_output_curve` wrote the target curve with 7
  parameters and without CICP as a workaround. moxcms 0.9.2 rounds again, so the workaround and
  its canary test are gone. Against the workaround, 0.07 % of the 8-bit values of a color grid
  move by one level (thumbnail cache v22).
- `gray_levels_keep_their_value` sends all 256 gray levels through the production transforms.
- A source in the target space converts nothing. So iPhone photos (Display P3) cost nothing.
- A source that cannot convert converts as sRGB. Examples: an unreadable or non-RGB ICC profile
  and unknown CICP codes.
- A PQ or HLG source (CICP transfer 16 or 18, also from the `cicp` tag of an ICC profile) gets
  the tone map of `tuilibr::pictures::decode::hdr`, because `moxcms` does not tone-map.
- The "as is" column of the table of `fixtures_match_colorsync` is what phewr sent before the
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
  (`player::tone_map::icc_known_space`). Any other profile counts as BT.709.
- A source in the target space gets no cube.
- The SDR cube runs in 8 bit (`rgb24`, `rgba`). Its values hold half a level, because `lut3d`
  truncates to 8 bit. A real ffmpeg test fails if `lut3d` starts to round.
- At 1080p, the SDR cube costs 1.4 ms to 2 ms per frame.
- The sizes and the costs of the cubes are in [Video](video.md#color-cubes).

## HDR stills

The code is in `tuilibr::pictures::decode::hdr`.

- A PQ still maps with `Decoded::peak`. libheif fills it from `clli` or `mdcv` (deep and 8-bit
  files), a PNG from `cLLI` or `mDCV` (the draft spellings `cLLi` and `mDCv` count too), and a
  JPEG XL from `intensity_target`. The 500 bucket has no roll-off, for content at or below SDR
  white. The 8-bit HDR path maps through `color::convert`.
- cjxl writes an intensity target of 10000 nits into each PQ file. phewr counts 255 and 10000 as
  unset. A JPEG XL that is mastered at 10000 nits maps with the 1000 nit bucket and clips
  its highlights above about 1000 nits.
- The libavif test stills: `colors_hdr_rec2020.avif` (MaxCLL 470, bucket 500) gets brighter, its
  mean level goes from 151.6 to 172.2. `seine_hdr_rec2020.avif` (MaxCLL 703, bucket 1000) stays
  the same.

- The cube of the player samples the map at 33 points per channel, and `lut3d` interpolates
  between them. A still runs the map per pixel, with one table for the signal curve and one for
  the output curve.
- The decoders keep the depth of an HDR file:
  - libheif decodes 16-bit RGB.
  - `jxl-oxide` writes its 16-bit stream.
  - A 16-bit PNG stays 16-bit.
- `Decoded` resizes the 16-bit image and maps the small image, with the rows in parallel.
- An 8-bit HDR image maps in `color::convert`. An example is the embedded thumbnail of an HDR
  HEIC file. A PQ thumbnail maps with the peak of the primary image (`HeifItems::primary_peak`),
  so it matches the sharp picture.
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

- The facts of the ICC handling and of the XYB limit are in `gotchas.md` ("JPEG XL colors").
- A PQ or HLG file keeps its 16-bit signal for the tone map of `tuilibr::pictures::decode::hdr`.
- On 2026-11-03, 40 public files (27 of the libjxl conformance set, 13 from jpegxl.info,
  convertico.com and libjxl/testdata) went through `real_jxl_matches_reference`. 34 match their
  reference within 4 levels. The six others:
  - `cmyk_layers`: phewr refuses CMYK on purpose.
  - `grayscale_jpeg`: djxl writes the gray ICC profile into an RGB PNG. Against the lcms
    conversion of the restored JPEG, phewr is within 4 levels.
  - `spot`: `moxcms` clips red at the sRGB gamut edge of a ProPhoto-like profile, where lcms keeps
    it (up to 32 levels on 144 of 240 000 pixels).
  - `test_pattern-PQ`: 6 levels against the player cube.
  - `sunrise-PQ` and `sunset-hdr-pq` (lossy XYB PQ): the XYB clip of jxl-oxide, up to 120 levels.
- jxl-oxide has no scaled decode, so the grid and the fullscreen box take the time of the full
  decode. The numbers 199 ms for 11 MP (ICC, lossy), 129 ms for 8.3 MP and 88 ms for 5.2 MP are
  from before the sample copy on all cores.
- Sample copy on all cores (`5ae9309`): `ImageStream::write_to_buffer` copies on one thread, with
  an orientation and a region lookup per sample. phewr copies the grids of
  `Render::color_channels` and `extra_channels` in tasks of rows on rayon. A lossless file
  renders as integer grids (I16 for 8 bit, I32 for 16 bit), a lossy file as f32 grids. The
  output is the same byte for byte on 72 files. Release build, median of 5 runs or more:

  | File | Route | Copy, stream | Copy, all cores | Decode before | Decode after |
  | --- | --- | ---: | ---: | ---: | ---: |
  | `09-formats/format.jxl` 6.8 MP lossy | enum | 106.8 ms | 3.9 ms | 158.3 ms | 66.2 ms |
  | 12 MP lossy | enum | 183.0 ms | 5.3 ms | 254.8 ms | 127.1 ms |
  | 24 MP lossy | enum | 545.0 ms | 14.4 ms | 676.3 ms | 330.0 ms |
  | 12 MP lossy, orientation 6 | enum | 291.2 ms | 22.4 ms | 399.0 ms | 163.7 ms |
  | 12 MP lossy with alpha | enum | 377.4 ms | 11.3 ms | 431.4 ms | 256.1 ms |
  | 12 MP lossless P3 ICC | ICC 8 bit | 178.8 ms | 4.8 ms | 398.5 ms | 300.0 ms |
  | 24 MP lossless P3 ICC | ICC 8 bit | 368.6 ms | 9.0 ms | 1147.0 ms | 498.9 ms |
  | 12 MP lossless 16-bit ICC | ICC 16 bit | 239.2 ms | 11.7 ms | 751.1 ms | 699.2 ms |
  | 12 MP lossy PQ | HDR | 228.8 ms | 9.2 ms | 400.2 ms | 193.1 ms |

- The fast path needs the keyframe as frame 0, no frame offset, no crop and no spot color.
  Patches, layers, a frame offset, a crop and spot colors take the stream. cjxl writes patches
  for text and screenshots, so these files still copy on one thread. The render is now the
  largest cost: about 70 ms for 12 MP lossy and 280 ms for 12 MP lossless under load.
- The update rule of the exact pins of `jxl-oxide` and `jxl-render` is in `gotchas.md` ("JPEG
  XL sample copy").
- Tested on 2026-11-03: two linear sRGB Lumix photos (within 1 level), a Samsung DNG 1.7 with
  JPEG XL tiles (through sips), and three lossy PQ photos (the XYB clip, max 104 to 166 levels).
- Not tested: a real HLG photo (Sony HLG `.HIF`), an iPhone 16 Pro ProRAW with JPEG XL, a plain
  camera or app `.jxl`, a P3 PQ file.
