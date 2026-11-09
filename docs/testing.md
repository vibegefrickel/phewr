# Tests in detail

This page holds the long form of the test rules in `AGENTS.md`: the variables of the real-file tests, the fixtures and their make scripts. Read `AGENTS.md` ("Tests") first.

- Unit tests sit in a `#[cfg(test)] mod tests` at the end of each module.
- Tests that need a real file are `#[ignore = "..."]` and read `PHEWR_TEST_JPEG`,
  `PHEWR_TEST_HEIC`, `PHEWR_TEST_AVIF`, `PHEWR_TEST_JXL`, `PHEWR_TEST_RAW`, `PHEWR_TEST_PSD`,
  `PHEWR_TEST_HDR_VIDEO` or `PHEWR_TEST_SDR_VIDEO`. `real_flipped_copy_shows_upright` copies `PHEWR_TEST_RAW`
  (a RAF or ARW) into a temp dir and writes the orientations 2, 3 and 4 with exiftool.
  `PHEWR_TEST_LIVE_DIR` is the folder of real Live Photo files
  (`~/Pictures/phewr-test/12-live-photos/`, its `README.md` lists the cases).
  `real_identifiers_match_exiftool`, `real_folder_cases`, `real_mismatch_note`,
  `real_motion_plays_once`, `real_live_still_loops_its_motion` and
  `real_motion_length_rounds_to_2_8` read it, the last three with the real ffmpeg or ffprobe.
  `real_rest_starts_motion_ahead` copies the pairs into a temp folder, so its probe is cold.
  `FakeDecoder::cold` empties its probe cache, and `finish_prepare` ends its probe on a thread. `test_support::live` writes a JPEG still with the identifier in an Apple MakerNote
  and a movie with the identifier in `moov/meta`.
  `real_orf_preview_turns_like_develop` copies `PHEWR_TEST_ORF`, writes the IFD0 orientation 1
  and 6 with exiftool, and compares the preview with the sips develop.
  `PHEWR_TEST_SDR_VIDEO` also feeds the first
  frame test `video::tests::real_clip_first_frame`. The bench `previews::raw_files` reads every
  RAW file of `PHEWR_TEST_RAW_DIR`. `real_jxl_matches_reference` compares `PHEWR_TEST_JXL` with
  `PHEWR_TEST_JXL_REF` (a djxl rendering, or the player cube for PQ and HLG) and fails above 4
  levels. `PHEWR_TEST_JXL_MAX` sets another limit, and `PHEWR_TEST_JXL_OUT` writes the decode.
  `bench::decoders::jpeg_xl` reads `PHEWR_TEST_JXL`, measures the decode and both copies, and
  names the path of the copy. `parallel_copy_matches_stream` compares the copy with the stream
  on every fixture.
- `tests/fixtures/color/` holds a Display P3 chart as JPEG, PNG, WebP, TIFF and HEIC with an ICC
  profile, as HEIC and AVIF with nclx, and as PNG with cICP and with gAMA and cHRM, plus an
  Adobe RGB JPEG and untagged sRGB files. `PHEWR_BENCH_COLORSPACE=srgb just bench` measures with
  the sRGB target.
- `tests/fixtures/png/` holds 10 PNG layouts (16 bit, palette, tRNS, gray, gray with alpha,
  RGBA, interlaced, EXIF orientation 6), and `make.sh` rebuilds them with magick and exiftool.
- `tests/fixtures/` holds 8 small animated and still files (GIF, WebP, APNG, AVIF), and
  `test_support::fixture(name)` finds them. `make.sh` rebuilds them from FFmpeg `testsrc` with
  magick, avifenc, ffmpeg and img2webp, and its optional argument selects the output folder. The
  AVIF settings `-q 30 -s 6` reproduce the committed pixels. The real ffmpeg
  test skips WebP without FFmpeg 9 (`webp_anim` decoder). `tests/fixtures/jxl/` holds 19 JPEG XL
  color charts and gradients, `ref/` their djxl renderings, and `make.sh` rebuilds both with
  cjxl, djxl, magick, exiftool and sips. `jxl/ref/` holds no `pq.png`: djxl maps PQ to a 255-nit
  display, and phewr maps like the player. `tests/fixtures/hdr/` holds a PQ and an HLG chart with
  BT.2020 primaries as 10-bit AVIF and HEIC (nclx), JPEG XL (lossless and lossy, and `pq-icc.jxl`)
  and 16-bit PNG (`cICP`), `ref/` their renderings through the HDR cube of the player (ffmpeg
  `lut3d`), and `make.sh` rebuilds them. `pq-4000.avif`, `pq-4000.heic` and `pq-4000-8bit.heic`
  carry a `clli` box of 4000 nits, `pq-4000.png` and `pq-icc-4000.png` a `cLLI` chunk, and
  `pq-4000.jxl` an intensity target of 4000 nits, all with `ref/pq4000-*.png`. `pq-4000-thumb.heic` is
  `pq-4000.heic` with an embedded thumbnail of 32x16 pixels, for the peak of the preview.
  `tests/fixtures/video/` holds two HDR10 clips (64x64, 3 frames, 10-bit HEVC, PQ) and `make.sh`:
  `hdr10-container.mp4` carries `mdcv` (1000 nits) and `clli` (MaxCLL 4000 nits) in its sample
  entry, and `hdr10-sei.mp4` carries MaxCLL 2000 nits only in the SEI.
  `player::probe::tests::real_hdr10_peak_sources` probes both with the real ffprobe, also with
  the stream probe alone. `pq12-hevc.mp4` (12-bit HEVC, PQ) and `hlg12-prores.mov` (12-bit
  ProRes 4444, HLG) show the two routes of a stream above 10 bits:
  `player::decoder::tests::real_12_bit_clips_decode` decodes the HEVC clip in software and the
  ProRes clip on the GPU. `real_layouts_download_on_gpu` encodes 4:2:2, 4:4:4 and alpha clips
  in a temp folder and runs each download format through the GPU chain, also turned.
  `cargo nextest run --no-default-features` passes too.
  `just test-real` skips the benches (`--skip bench::`), because they set the global color target
  of the process.
- A real ffmpeg playback test uses a virtual clock that jumps to each due frame. The wall-time
  twin is `#[ignore]` and runs in `just test-real`.
  `just test-real` runs them. A test without its variable skips with a note.
- The `sips` tests run `/usr/bin/sips` to write synthetic PSD and PDF files.
- The synthetic libheif tests encode with the aom (AVIF) and x265 (HEIC) encoder plugins of
  Homebrew's libheif. If a plugin is missing, `just check` fails.
- A test that writes files uses `tempfile::tempdir()` (`TempDir`) or `tempfile::NamedTempFile`.
  The directory goes away on drop, also when the test panics.
- The app builders and the key press helpers that tests of more than one module share live in
  `test_support/`. The synthetic file writers live in `pictures/test_support/`, and
  `test_support/` re-exports them. The fake Kitty terminal is `test_support::kitty`.
- A test app writes the Kitty sequences to a buffer (`GraphicsOutput::Buffer`). Read them with
  `app.graphics_out.take_bytes()`. A test that needs a config uses `App::new_with_config`, never
  `App::with_config`.
- A test never writes to the real state file or the real thumbnail cache. The test-only
  `App::new` points `app.state_file` and `app.disk_cache_dir` below `/dev/null`. A test that
  needs them sets them to temp paths.
- A test never sets `HOME` or `XDG_*`. Use the functions that take a path: `config::load_at`,
  `Paths::from_vars`, `config::save_visible_rows(path, ..)`.
- A test waits for background work in a loop with a limit (for example 2 s in 10 ms steps), and
  never depends on the clock within a frame interval.
