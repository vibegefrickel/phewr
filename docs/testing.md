# Tests in detail

This page holds the long form of the test rules in `AGENTS.md`: the variables of the real-file tests, the fixtures and their make scripts. Read `AGENTS.md` ("Tests") first.

- Unit tests sit in a `#[cfg(test)] mod tests` at the end of each module.
- Tests that need a real file are `#[ignore = "..."]` and read `PHEWR_TEST_RAW`,
  `PHEWR_TEST_ORF` or `PHEWR_TEST_LIVE_DIR`. The real-file tests of the decoders (JPEG, HEIC, AVIF,
  JXL, PSD) and of the moved video code are in tuilibr and read `TUILIBR_TEST_*`. The video tests
  read `TUILIBR_TEST_SDR_VIDEO` and `TUILIBR_TEST_HDR_VIDEO`. The Live tests and the Live benches stay in
  phewr.
  `real_flipped_copy_shows_upright` copies `PHEWR_TEST_RAW`
  (a RAF or ARW) into a temp dir and writes the orientations 2, 3 and 4 with exiftool.
  `PHEWR_TEST_LIVE_DIR` is the folder of real Live Photo files
  (`~/Pictures/phewr-test/12-live-photos/`, its `README.md` lists the cases, 15 file cells since
  the 8 real iPad pairs of 2026-11-18). `real_live_still_loops_its_motion` reads the loop of
  `IMG_1043.HEIC`, because the test grid shows 2 rows and the still has index 1.
  `real_identifiers_match_exiftool`, `real_folder_cases`, `real_mismatch_note`,
  `real_motion_plays_once`, `real_live_still_loops_its_motion` and
  `real_motion_length_rounds_to_2_8` read it, the last three with the real ffmpeg or ffprobe.
  `real_rest_starts_motion_ahead` copies the pairs into a temp folder, so its probe is cold.
  `FakeDecoder::cold` empties its probe cache, and `finish_prepare` ends its probe on a thread. `test_support::live` writes a JPEG still with the identifier in an Apple MakerNote
  and a movie with the identifier in `moov/meta`.
  `real_orf_preview_turns_like_develop` copies `PHEWR_TEST_ORF`, writes the IFD0 orientation 1
  and 6 with exiftool, and compares the preview with the sips develop.
  `TUILIBR_TEST_SDR_VIDEO` also feeds the first
  frame test of `tuilibr::pictures::motion::first_frame`. The bench `previews::raw_files` reads every
  RAW file of `PHEWR_TEST_RAW_DIR`.
- The picture module is `tuilibr::pictures`. Its tests, its fixtures and their `make.sh` files are
  in tuilibr (`tuilibr/docs/pictures.md`). The module tests read `TUILIBR_TEST_*`. The tests of
  phewr read `PHEWR_TEST_*`.
- The dev dependency on tuilibr has the feature `test-support`. `test_support` of phewr re-exports
  `tuilibr::pictures::test_support`. phewr has no fixture folder, and `test_support::fixture(name)` is gone.
  `test_support::module_fixture(name)` finds every fixture in tuilibr (`anim.*`, `video/`,
  `color/`, `png/`, `jxl/`, `hdr/` and `still.*`).
- `just test-lean` runs every test with the lean set (`--no-default-features --features
  video,sips`), without `heic`, `jxl` and `turbo`. `just check` runs it.
- `cargo nextest run --profile ci` runs every test without fail-fast and writes the junit report
  to `target/nextest/ci/junit.xml` (`.config/nextest.toml`). `just audit` checks the shell scripts
  that exist with shellcheck and shfmt. phewr has no shell script now.
- The folders `color/`, `png/`, `jxl/` and `hdr/` and the 4 stills `still.avif`, `still.gif`,
  `still.png` and `still.webp` are in tuilibr. `PHEWR_BENCH_COLORSPACE=srgb just bench` measures
  with the sRGB target.
- The fixtures `anim.*` (4 small animated files: GIF, WebP, APNG, AVIF) and `video/` (the HDR10, 12-bit and layout
  clips) and their `make.sh` files are in the fixture folder of tuilibr. Their tests
  (`real_hdr10_peak_sources`, `real_12_bit_clips_decode`, `real_layouts_download_on_gpu`) run in
  tuilibr. See `tuilibr/docs/pictures.md`, "The motion module".
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
  `test_support/`. The synthetic file writers live in `tuilibr::pictures::test_support`, and
  `test_support/` re-exports them. The fake Kitty terminal is `test_support::kitty`. The frame
  engine has its own fake writer tests in `tuilibr::pictures::kitty::screen::tests`: the bytes of each
  rule, the byte cap over several passes, the recovery after a clear and a focus gain, and both
  placement policies.
- `FakeTerminal` models the self check. For a placement with `q=1` of an image that it does not
  store, it writes an `ENOENT` answer into `FakeTerminal::answers`, the input of the app.
  `FakeTerminal::lose` drops an image and its placements without a delete, as herdr does when
  it resets its delivery. `tuilibr::pictures::test_support::key_events` gives the crossterm key events of
  input bytes: an `ESC` and a key is Alt and that key, and an upper case letter has Shift. It is
  a model of the crossterm parse. The tests of `AnswerKeys` (`tuilibr::pictures::kitty::answers::tests`)
  and of the run loop (`run_loop/tests.rs`) feed the answers through it.
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
