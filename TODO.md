# TODO

Main goal: **phewr is a speed-focused viewer for all common image and video formats in the terminal, with full mouse support.**

Scope:

- Speed comes first: every change must make opening, scrolling or switching files faster, or it must add a common image/video format.
- Full mouse support is a first-class input method, on par with the keyboard. Every view and overlay must be usable with the mouse.
- Viewer only: phewr does not trash, delete, move, rename or otherwise modify files.
- Target terminal: Ghostty on macOS Apple Silicon, using the Kitty graphics protocol.
- Measure speed changes with `just bench` on `PHEWR_TEST_DIR`.

State of the main goal (2026-11-08):

- Speed: the grid, the fullscreen view, the prefetch, the disk cache (QOI), the Kitty temp file medium and the player start ahead of Enter are in place. The open speed items are in "Next".
- Formats: JPEG, PNG, HEIC, AVIF, JPEG XL, camera RAW, PSD, animations, videos and Live Photos open. JPEG XL and camera RAW pass tests on real files.
- Videos: the terminal player is the default video path. `p` opens the mpv window with sound.
- Mouse: every view and overlay works with the mouse. The maintainer test is open.
- Health on 2026-11-08: `just check` passes with 946 tests (55 ignored tests need real files), clippy is clean, and `cargo deny` reports only duplicate versions.

## Now

Maintainer tests in Ghostty and in herdr: run the 14 steps in `~/Pictures/phewr-test/TESTS.md`. The folder holds a test set for each step (photos, colors, animations, videos, camera RAW from raw.pixls.us, every format, folders, placeholders and Live Photos). Steps 9 and 10 include the grid loops: every video and animated cell must loop after its still. Measured on 2026-10-30: 30 looping cells cost about 3 % of one core in total (phewr 0.2 %, Ghostty 1.7 %, herdr 1.3 %).

Open points of the test run:

- Plain Ghostty 1.4.0 must pick the animation path (`?` shows it). herdr 0.9.5 gets the ticks without the probe, and the ticks play there (maintainer, 2026-11-03).
- Make sure that `a=f` frames go through `t=t` in Ghostty, and that the start in herdr is not slower (the DA1 answer must end the read).
- The SVG grid icons (steps 3 and 8): the icons show in the terminal foreground at 45 % alpha. If herdr does not answer OSC 10, the octant icons stay. Make sure that both cases look right.
- Cmd+Q in the mpv window of a running slideshow must stop the slideshow on the placeholder (`--input-ipc-server`, merged 2026-11-03).

Test run of 2026-10-24 (plain Ghostty): steps 1 and 2 passed. The run stopped at step 3a, and the fixes from it are in (see Done). Continue at step 3 with the current build.

After the tests:

- RAW: the portrait RAW files in `08-raw` (except `IMG_1361.DNG`) are copies with a changed Orientation tag, so they test the container orientation, not a real portrait shot.

## Next

Order: speed first, then formats, then the first release. The first release is postponed to Q1 2027 at the earliest (decided 2026-11-03). Until then, the maintainer tests phewr and only pushes the source to Codeberg for the dotfiles install.

1. Speed
   - herdr is slower than plain Ghostty (maintainer, 2026-10-24). Measured on 2026-10-30 with Ghostty 1.4.0 and herdr 0.9.5 (write side, 1080p frames through `t=t`): plain Ghostty accepts 1047 fps RGB and 797 fps RGBA, a herdr pane 967 fps RGB and 369 fps RGBA (the native path waits for the host `OK` per frame), and a scroll retires the native path for the rest of the connection (668 and 730 fps after it). The RGBA fast path for phewr waits for the answer to the herdr issue: send `f=32`, so that the herdr `t=f` fast path applies. Measure it against the numbers above before you build it. If herdr scrolls without lag on Ghostty 1.4.0, drop the idea.
   - JPEG XL patch files (text, screenshots) still copy on one thread, because jxl-oxide has no public region API. The upstream draft needs four API items: re-export `ImageBuffer` and `Region`, public grid regions, `ExtraChannel::bit_depth()`, or a row-range `write_to_buffer`. With them, phewr drops the direct `jxl-render` dependency.
   - Loop job results (idea, 2026-11-08): a running loop job alone gives no 10 ms poll, so the first loop frame of a cold grid and the start of the folder prefetch can wait up to 250 ms for the idle timeout. A wake on the job result removes this wait without a 10 ms poll during the long ffmpeg runs. Measure it with `bench::live::grid_animation` before you build it.
   - AV1 on M1 and M2 has no VideoToolbox decoder and takes the software path after one failed GPU run. Untested.
   - Candidate, low priority: the shared memory medium `t=s` needs `shm_open` (unsafe or a crate). The temp file medium reaches the same payload size.
2. Formats: real files that are still missing.
   - JPEG XL: a real HLG photo (Sony HLG `.HIF` is the one real source, the public files have no license), an iPhone 16 Pro ProRAW with JPEG XL, a plain camera or app `.jxl`, a P3 PQ file, and the saklistudio.com HDR samples (the server blocks scripts). Also open: the `moxcms` gamut edge and the XYB clip of jxl-oxide.
   - HEIC: a real file with an embedded thumbnail of another aspect.
   - Live Photos: a pair that a camera wrote as a pair (`docs/live-photos.md`, question 1).
3. Hidden tab (mode 2033): stop the loops, the player and the slideshow while the tab is hidden. Stopped on 2026-10-30: crossterm 0.27 to 0.29 never end the report sequence and lose all input after it. Continue when a crossterm release fixes it.
4. First release, postponed to Q1 2027 at the earliest: create the Codeberg repository `phewr`, then `just render` and `just release`. The dry run `just render` passed on 2026-11-03: a 3.3 MB arm64 tarball, and the formula passes `brew style` in a tap folder.
5. Dependencies: all direct dependencies are at their newest version (2026-11-03). `image` and `jxl-oxide` still pull moxcms 0.8, and ratatui pulls lru 0.18. If `moxcms_still_truncates_known_curves` fails after an update, remove `rounding_output_curve` and the test.
6. Upstream issue drafts in `~/Projects/dev/` (maintainer, not posted):
   - `herdr-issue-kitty-graphics-performance.md`: the three herdr findings with the numbers above.
   - `herdr-issue-kitty-graphics-offset.md`: herdr scales a relayed image to whole cells and applies `X=` / `Y=` twice.
   - `crossterm-issue-visibility-report.md`: the mode 2033 report sequence (item 3).
   - `jxl-oxide-issue-parallel-stream.md`: the four API items (item 1).
   - `moxcms-issue-gamma-table-round.md`: 0.9.1 `make_gamma_table_impl` (`src/gamma.rs:645`, line 654) lost `.round()`, so the identity transform sRGB to sRGB maps 200 to 199.

## Later

- Android Motion Photos (idea, not decided, 2026-11-03): Pixel and Samsung phones put the clip inside the JPEG or HEIC. phewr can give them the grid roll and the play on Enter of the Live Photos. immich-app/test-assets holds 5 real files in `formats/motionphoto/` (AGPL, local tests only).
- The marks (`Space`, `a`, `A`) stay in `app/marks.rs` without a use. A slideshow of the marked images is one option.

## Done

- Cleanup of 2026-11-08: `AGENTS.md` went from 804 to about 390 lines. The long forms moved word for word into `docs/gotchas.md`, `docs/layout.md` and `docs/testing.md`. The test modules of `decode/jxl.rs`, `decode/sips.rs` and `player/probe/mod.rs` moved into `tests.rs` files. `REVIEW-2026-10-28.md` is removed. A late OSC 10 answer after a probe timeout no longer becomes key presses (`LateAnswerFilter`).
- SVG grid icons (2026-11-08): the cells without a thumbnail show the Lucide icons `image`, `play`, `folder` and `folder-up` as Kitty images in the terminal foreground (OSC 10). Without an OSC 10 answer, the octant icons stay.
- `--cwd-file <PATH>` (2026-11-05): phewr writes the folder of the grid into the file on exit, like yazi. A `p()` wrapper in the shell does the `cd` (README, "Change the shell directory on exit").
- `Tab` toggles the info panel, not `i` (2026-11-04, `7e76b6c`).
- Live Photos (2026-11-03 and 2026-11-04): the pairing with the grid roll (`4e2d22b`, choices B and C), the motion length in the status row (`7a62965`), the motion prestart in the fullscreen view (`b82f164`, 0.1 ms instead of about 123 ms after Enter) and the bench group `bench::live` (`bd2a3d2`). On an idle machine, the pairing costs 3.2 % of the first scan entry with 3334 pairs, and every Live cell of the real folder loops after 1435 ms.
- DCT scale for the fit (2026-11-03, `52c9222`, cache v21): libjpeg-turbo decodes at the smallest NEON scale (1/8, 1/4, 1/2, 1) that covers the fit. The cold grid of 24 iPhone JPEGs is about 8 % faster. The odd eighths run in plain C and are slower than the full decode.
- ORF previews from the Olympus maker note (2026-11-03, `6912776`, cache v20), with the fit rule of `covers`: every RAW fullscreen view takes a preview that covers the fit (ORF: 31 ms to 73 ms instead of 2.2 s to 7.5 s).
- Video start without VideoToolbox (2026-11-03, `141a2ad`): after one failed GPU run, later starts of the file go straight to software for the session (VP8: 110 ms, then 32 ms).
- JPEG XL sample copy on all cores (2026-11-03, `5ae9309`): the decode is 1.3 to 2.3 times faster, with the exact pins `jxl-oxide =0.12.6` and `jxl-render =0.12.4`.
- Real files (2026-11-03): 34 of 40 public JPEG XL files match their reference within 4 levels, and gray PQ takes the HDR path. 6 more real JPEG XL files are in `~/Downloads/phewr-jxl-real/` (`REPORT.md` holds the sources and results). All 22 camera RAW files of `08-raw` and four real portrait RAF and ARW files pass (`df7ba6d`, `3c8027b`, the flip compare in `ace8d40`, cache v18).
- HDR10 peak (2026-11-03): a PQ video, HEIF, PNG or JPEG XL maps with the peak of its HDR metadata (`18cc453`). The probe reads the peak from the container (`af61a4b`), and the embedded HEIC preview maps with the peak of the primary image (`b34d7e6`).
- HEIC preview of another aspect (2026-11-03, `a972574`): two synthetic tests prove other margins and no stretch.
- Delete removal (2026-11-03, `5da64fa`): the keys `d` and `D`, the delete question and the `trash` crate are gone. The process start did not change (50 runs of `phewr --version`: median 15.2 ms before, 15.4 ms after).
- Dependencies current (2026-11-03, `2d8dd99`): lru 0.19 and moxcms 0.9.1, with the workaround `rounding_output_curve`.
- Small leftovers of the cleanup review (2026-11-03, `df7ba6d`): `batch_since` replaces `entries_since`, the loop ids skip the live ids after a wrap, an empty image writes nothing, and `real_clip_first_frame` tests the first frame on a real clip.
- Lower thread priority for the folder prefetch (2026-10-31 and 2026-11-03): no gain. A utility QoS class and a rayon pool of its own with 2 threads gave no gain beyond the spread of `folders::grid_during_prefetch`. With one libheif tile thread per prefetch decode, the grid was 80 ms (4 %) faster, inside the spread, and the prefetch of a 35-cell HEIC folder took 12.2 s instead of 3.9 s.
- Native Kitty animations for the grid loops (merged 2026-10-30, `9d84897`, the follow-ups in `e0a9e62`). herdr 0.9.5 gets the ticks.
- Player start (2026-10-31, `6008ca7`): the player starts while the cursor rests on a video. After a rest of 500 ms or more, the first frame comes 1 ms to 2 ms after Enter (about 240 ms without a rest). A measurement dropped the 17-point HDR cube (colors move by up to 39 levels). Decisions of 2026-11-03: a waiting player ends after 10 s, because a waiting ffmpeg on a 4K HLG clip holds about 450 MB. `Esc` does nothing in the grid, only `q` quits.
- HDR stills (2026-10-31, `263e48d`, cache v14): PQ and HLG in AVIF, HEIC, JPEG XL (also with a PQ or HLG ICC profile) and 16-bit PNG map like the player cube, with fixtures in `tests/fixtures/hdr/`.
- The release formula recommends ffmpeg and mpv, and `install.sh` only notes when they are missing (2026-10-27). The release binary does not bundle FFmpeg.
- Review of 2026-10-28 (worked 2026-10-30 to 2026-11-03): 26 commits through 12 workers. Every item is done or listed above. The plan file `REVIEW-2026-10-28.md` is in the git history (removed 2026-11-08).
- No small `▶` marker anymore.
- Grid loops (2026-10-27): every visible video and animated cell plays a loop of its frames, in place of the `▶` marker. Decisions: a short clip with fewer than 12 keyframes decodes 12 frames at a constant rate, 2 decode threads per loop run, new stills cancel the loop runs, the prune removes loop files before stills, `[gallery] animate = false` turns the loops off.
- One `q` leaves the player for the grid, also after a step from the fullscreen view onto a video (before, the first `q` showed the fullscreen placeholder).
- Player measured without a terminal (2026-10-27, `just bench` per clip of `07-video`): every clip decodes at 160 fps to 500 fps at 1080p and plays in real time with 0 drops.
- VFR bursts: a base rate above 1.5 times the average plays at the average, at least 24 fps.
- An animated WebP or APNG with an ICC profile or a cICP chunk plays in its own colors: the probe reads the profile and maps sRGB, Display P3 and BT.2020 colorants to the cube primaries.
- Camera JPEGs in Adobe RGB without an ICC profile (EXIF `ColorSpace` 0xFFFF with the interoperability index `R03`) convert as Adobe RGB, in the turbojpeg and the `image` path.
- No sRGB bench folder needed: `color_conversion_cost` already measures both directions.
- The run loop polls every 2 ms until the first picture of the grid shows (`FIRST_PICTURE_POLL`): the cold first picture after 14 ms to 17 ms instead of 16 ms to 18 ms.
- Each bench waits for its background work to end.
- In the fullscreen view and the player, `h` and `j` step to the previous file, `k` and `l` to the next.
- phewr always starts in the grid, also in a folder with one image (`~/Pictures`).
- The terminal player keeps real time (rule of 2026-10-24): the clock never moves to a late frame, a lag over 300 ms starts ffmpeg again ahead of the clock, and a slow terminal drops frames.
- The player has no pause and no seek: `h` / `l`, the arrows and `n` / `N` step to the previous and next file, Space pauses a running slideshow, the wheel and a click on the video do nothing, `p` opens the mpv window.
- Grid icons at 30 % of the cell height, a picture for images, two dots without a name for `..`.
- The start note is gone.
- Large octant icons in the grid (loading image, video, folder, `..`).
- One file argument opens its folder with the cursor on the file. yazi `g v` runs `phewr %h`, so it opens the hovered folder or the folder at the hovered file.
- Warm grid: disk cache reads beside the decode slots and batched Kitty emits.
- Videos, video thumbnails and animations convert into the terminal color space: a 17-point SDR cube by primaries (BT.709, BT.601, P3, DCI-P3, BT.2020), the HDR cube writes P3, SDR thumbnails convert in Rust. Real time stays at 30 and 60 fps without drops.
- Disk cache in QOI (a hit 2.2 times faster), the cell size query and the Kitty probe in one round trip, the first warm picture after about 20 ms instead of about 28 ms. The `dist` profile gives no gain, so `just install` stays on `--release`.
- `ffmpeg-next` dropped: phewr starts `ffmpeg` / `ffprobe` from PATH when installed, the binary links no FFmpeg library, video thumbnails take about 250 ms instead of 0.8 to 2.4 s and get the rotation and the HDR tone mapping, start 30 ms to 21 ms.
- Image colors: every image converts into Display P3, because Ghostty shows image pixels as P3. sRGB images were too saturated before.
- Opaque images and video frames go out as RGB (`f=24`): 25 % fewer bytes per frame, the player decode 30 to 50 % faster.
- The directory picker is gone: subfolders, `Backspace`, the mouse and a path argument cover navigation.
- Folder prefetch: after 300 ms on a folder, Enter shows every cell sharp after about 140 ms instead of about 2 s.
- JPEG XL colors: ICC v2 profiles convert through `moxcms` (Adobe RGB was wrong), 18 color chart fixtures, all within ΔE00 0.36 of djxl.
- Videos with a variable frame rate play at the right speed (a screen recording played 3.2 times too slow).
- Two-column help popup (36 rows from 100 columns on), the play marker for videos too.
- Animated GIF, WebP, APNG and AVIS play in the terminal player, with a `▶` marker in the grid.
- Full mouse support in every view and overlay: modes 1000, 1002 and 1006 only (no 1003), `[gallery] mouse`, the capture is released while an mpv window plays.
- Speed batch: embedded JPEG previews for camera RAW (TIFF-based, CR3, RAF) and PSD, padding to whole cells only (a cold grid sends 44.1 MB instead of 57.3 MB), parked thumbnails for the way back (all cells sharp after about 25 ms), thumbnail cache key v7.
- Terminal video player (route 3) as the default video path: ffmpeg with videotoolbox, `scale_vt` and the HDR cube, frames through the temp file medium with `c=` / `r=`, 1080p at 30 fps without drops.
- Subfolders as grid entries: `..`, `Enter` into a folder, `Backspace` to the parent.
- Zoom level per folder: the state file keeps up to 200 folders, a folder without an entry uses the last level.
- `q` goes back one step in every view and quits only in the grid.
- Zoom fix: a `+`, `-` or `0` at a limit no longer deletes all pictures (black cells), `q` in the fullscreen view goes back to the grid.
- Kitty output fixes (batch 10): thumbnails padded to whole cells, no `X=` / `Y=` (fixes the herdr overlap and the squeezed landscapes), `d=I` for evicted images and a 128 MB terminal budget (fixes the black cells), a 512 MB in-memory cache limit, `CSI 16 t` for the cell size, the filename toggle decodes again, late probe answers are dropped, empty cells decode first, the HEIC preview goes out at native size, grid zoom 2 to 12 rows.
- Refactor (batch 9): `main.rs` and `app.rs` split into 30 modules, then the repeating patterns removed.
- Video in a fullscreen mpv window by default, grid gaps with line borders, loading icons, no frames and no search bar (batch 8b).
- Grid fill (batch 8): no border reserve, a 4 px padding with the Kitty keys `X=` / `Y=`, a background fill for the selected and the marked cells, the remainder columns and rows reach the edges, the cell shape from the median aspect of the first 64 file headers (7 buckets), Catmull-Rom for every resize (24 MP resize 54 ms to 36 ms), thumbnail cache key v5.
- mpv round (batch 7): `--hwdec=auto-copy-safe` and a scale filter in the terminal (4K HDR clip 34 fps to 97 fps), a tone mapping LUT for HLG and PQ in the terminal (8.4 dB to 30.5 dB against the macOS SDR export), `n` / `N` step to the next and previous file from inside mpv, the mpv window shows its exit code, quiet FFmpeg log, the yazi reveal on quit.
- Two-phase HEIC grid thumbnails (batch 7): the embedded thumbnail shows at once, the sharp decode replaces it. libheif reads through a buffered stream.
- Speed round (batch 6): the run loop polls every 10 ms while a decode, a prefetch, a filter or the scan runs.
- Kitty transfer medium: a temp file (`t=t`) when the terminal answers the probe with `OK` and deletes the probe file, PNG over SSH, direct otherwise.
- Benchmarks: `src/bench/` holds ignored benchmarks on `PHEWR_TEST_DIR`, `just bench [DIR]` runs them.
- Theme system removed: `src/theme.rs`, the theme catalog, the `t` picker and the `-t` flag are gone. phewr uses the terminal default colors and the 16 ANSI colors (`src/styles.rs`).
- Grid look: a border only around the selected and the marked cells, no spacer line, centered thumbnails, filenames hidden by default (`n` toggles them)
- Grid zoom: `+`/`-` change the visible rows (default 4), the columns follow the terminal width (minimum 2), the state file saves the zoom level as `[gallery] visible_rows`
- Fullscreen fix: a gallery pre-decode without a hint showed HEIC, JPEG and PNG at full size
- JPEG XL (jxl-oxide, sRGB), EXR and HDR (tone map), QOI, TGA
- Camera RAW, PSD, JPEG 2000, PDF, SVG and other ImageIO formats through `sips` (TIFF output)
- HEIC grid thumbnails: a safe ISOBMFF reader compares `irot` and `imir` with the primary image
- No unsafe code: the `sips` route and the box reader replace ImageIO and libheif-sys
- Fork of rview
- Tooling from schnattr: justfile, lint table, cargo deny, nextest, Codeberg release flow
- Configurable, zoomable thumbnail size (`+` / `-` / `0`, remembered in state file)
- Slideshow with interval, pause and loop (`s`, `Space`, `[` / `]`, `--slideshow`)
- Info panel with file data and EXIF (`Tab`, `--info`)
- HEIC / HEIF decoding through libheif
- Video playback in mpv (`[video] player`: terminal, window, builtin)
- Built-in player: a pause no longer drops a packet or stops the loop
- Theme: `-t` with an unknown name stops with an error, the theme follows `XDG_CONFIG_HOME`
- Slideshow plays each video one time and continues (terminal, window, built-in)
- No test writes Kitty sequences to the real stdout, all tests use `tempfile`
- Delete in the grid keeps the cursor in place
- libheif path scales with fast_image_resize (thumbnail cache key v3)
- Testing phase: 39% to 94% line coverage, `tempfile`, no test writes real user files
- Return to the view that opened a video, video placeholder in fullscreen
- Prefetch: generation counter against stale results, no retry loop after a failed decode
- AVIF through libheif, embedded HEIC grid thumbnails, EXIF orientation for WebP and PNG
- EXIF orientation applied to JPEG / TIFF
