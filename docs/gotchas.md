# Gotchas in detail

This page holds the long form of each gotcha in `AGENTS.md`. Each item names the code, the measured numbers and the decisions with their dates. Read the item before you change the code that it names.

- **Terminal player:** ffmpeg runs as a subprocess with VideoToolbox and writes raw frames to a
  pipe, one frame per temp file under the image id `u32::MAX - 1`. The command line, the rates,
  the frame pool and the payload cap are in `docs/video.md` ("Terminal player"). The GPU
  download format follows the frames of VideoToolbox (`download_format`): the chroma layout and
  the depth pick `nv12`, `p010le`, `nv16`, `p210le`, `nv24`, `p410le`, `p416le` or `ayuv64le`.
  ffmpeg 9 gives VideoToolbox no HEVC or VP9 stream above 10 bits, so such a stream starts in
  software (`VideoInfo::hardware`). The software chain keeps its 12 bits up to the cube.
- **Player real time (rule):** the terminal player never plays in slow motion. A 6 s clip
  plays for 6 s, and a slow decode or a slow terminal drops frames. The clock of a run never
  moves to a late frame. A frame more than one interval late drops when a newer frame waits,
  else it shows late. If the frames lag the clock by more than `MAX_LAG` (300 ms),
  `Player::catch_up` starts ffmpeg again `RESTART_LEAD` (0.3 s) ahead of the clock, and the new
  run keeps the clock. A run whose clock passed the length of the clip while its frames lag
  ends at once. A restart without a frame is the end of the clip, not an error. The player has
  no pause and no seek: it plays like an animated image, and `p` opens the mpv window for all
  controls. The help popup holds the player and gets no frame. The run loop waits until the
  next frame is due, 10 ms while ffmpeg has no frame ready. Tests use `FakeDecoder` and never
  start a real ffmpeg or mpv.
- **Video player default:** `[video] player` defaults to `terminal`, and `builtin` maps to
  `terminal`. Without the temp file medium (SSH, a probe that picked `direct` or `png`) or
  without ffmpeg, a video opens in the mpv window with a note. mpv's own Kitty output is gone:
  it played a 4K HLG clip at 2.4 fps in Ghostty and cannot scale a small frame.
- **Video placeholder:** `q`, `Esc` and the back click in the terminal player always go to the
  grid (`leave_player` calls `App::exit_video`), because the player has no controls, and the
  placeholder is one more step back without a use. After the mpv window, `App::video_return` selects
  the view. A video in `ViewMode::Fullscreen` is a placeholder (`shows_video_placeholder`).
  `load_if_needed`, `zoom_in` and `slideshow_tick` skip it, because the image decode cannot read
  a video. `p` on the placeholder opens the mpv window.
- **Animated images:** an animated GIF, WebP, APNG or AVIS plays in the terminal player like a
  video, and its still carries a play triangle until its loop starts. The probe, the routes and
  the stamp are in `docs/video.md` ("Animated images" and "Probe").
- **Slideshow view (2026-11-09):** a running or paused slideshow has no status row
  (`ui::status_rows` gives 0 outside the grid), so `image_rect` covers the whole screen. A badge
  at the bottom right of `image_rect` (`draw_slideshow_badge` in `ui/status.rs`) shows the play
  or pause glyph and the interval, with the status note in front. An error replaces the badge
  text on a red background. The badge is always on (maintainer, 2026-11-09). Kitty draws a
  picture above the text, so the still and the video frame of a slideshow go out with
  `z=-1073741825` (`BELOW_BACKGROUNDS`): Ghostty draws them below every cell with a background
  color, and the cells with the default background show the picture. The badge has a background
  color (`STYLES.badge`, black on ANSI gray, like the active tab of herdr). The image goes out at its decoded size and
  the terminal scales it to the fill of `image_rect` (`filled_placement` in
  `kitty_output/mod.rs`): a portrait image gets `r=` with the rows of the box, a landscape image
  `c=` with the columns, and the terminal takes the other side from the aspect, so the picture
  never stretches. The decode does not upscale: the terminal scale costs no payload and no CPU.
  The start of a slideshow changes `image_rect` by one row, so the current image decodes again
  one time. Outside a slideshow, an image shows at its own size, centered.
- **Slideshow and videos:** a running slideshow that is not paused plays a video one time. The
  terminal player stops playing (`playing()` is false), and the slideshow moves on. Outside a slideshow, the clip
  loops. In the mpv window, mpv gets `--loop-file=no`, `--keep-open=no`, `--idle=no`,
  `--loop-playlist=no` and `--input-commands` that bind `q`, `Q` and `CLOSE_WIN` to `quit 4`.
  Exit code 0 is the end of the file. Each other exit stops the slideshow on the placeholder.
  `--input-commands` needs mpv 0.38 or later. mpv exits with 0 also on the app quit (Cmd+Q). For
  this reason, a slideshow window starts mpv with `--input-ipc-server` and a socket of its own,
  and a thread reads the `end-file` reason. Exit code 0 with the reason `quit` or `stop` stops the
  slideshow (`mpv::exit_with_reason`). The reason `eof`, any other reason, no reason and no socket
  continue it. The real Cmd+Q is not tested yet.
- **mpv steps:** each mpv start gets `--input-commands` with `keybind n "quit 10"` and
  `keybind N "quit 11"` (`mpv::NEXT_FILE`, `mpv::PREVIOUS_FILE`). `App::step_from_video` keeps
  `video_return`, so a run of videos from the gallery ends in the gallery. A step goes from the
  file that mpv played, not from the gallery cursor.
- **mpv window:** phewr follows each mpv window until it exits (`App::poll_video_window`). The
  run loop polls every 100 ms (`WINDOW_POLL`) while a window plays. A step in phewr away from
  the file of the window forgets the window. The window gets `--fs --native-fs=no` and
  `--hwdec=auto-safe`. `--fs` overrides `fs=no` of a user `mpv.conf`.
- **HDR in the terminal:** `player::probe` reads the transfer curve. For HLG and PQ,
  `player::tone_map::cube_file` writes a 3D cube into the thumbnail cache directory, or into
  `$TMPDIR/phewr` if the cache path holds filter graph syntax. `Cube::for_video` picks the cube, and
  the decoder calls it through `cube_for(info, target, dirs)`. The names are
  `tonemap-hlg-{p3,srgb}-v4.cube`, `tonemap-pq<nits>-{p3,srgb}-v4.cube` and, for SDR,
  `color-<primaries>-<target>-v4.cube`. `CUBE_VERSION` is the one version for all cube names: change
  it when the cube output changes. The tone mapping writes the target primaries. ffmpeg parses a
  cube at each start: 33 points take about 42 ms, 17 points about 16 ms. A 17-point HDR cube
  saves about 20 ms per start but moves colors by up to 39 levels (mean error 0.84 to 1.09 levels
  against 0.51 to 0.56), so HDR keeps 33 points. `SDR_WHITE_NITS = 500` matches the macOS SDR
  export on three iPhone clips. A PQ video or still maps with the peak of its HDR10 metadata:
  `MaxCLL` from 100 to 10000 nits, else the mastering display maximum, else 1000 nits
  (`pictures::decode::hdr::hdr10_peak`). The peak goes to the smallest bucket at or above it: 500, 1000,
  2000, 4000 or 10000 nits (`Peak::bucket`). The 500 bucket is SDR white and has no roll-off.
  HLG keeps 1000 nits. The probe reads `max_content` and `max_luminance` of the stream side
  data. A stream can carry HDR10 only in the SEI, without `mdcv` and `clli` in its sample entry.
  Then a second ffprobe reads the first frame,
  once per file (1.2 times the stream probe at 1080p, up to 1.7 times at 4K). A HEIF still reads
  `clli` and `mdcv` of its primary item (`HeifItems::primary_peak`) into `Decoded::peak`. The
  Apple HDR10 example clip (MaxCLL 1375) is the real test file. A PQ PNG reads
  `cLLI`, else `mDCV`, also with a PQ `iCCP` profile. A PQ JPEG XL reads `intensity_target`. Its
  values 255 (the header default) and 10000 (cjxl writes it for each PQ file) count as unset, so
  a JPEG XL really mastered at 10000 nits maps with 1000 nits. An 8-bit PQ HEIF reads `clli` and
  `mdcv` too, and the 8-bit path maps through `color::convert`. A PQ or HLG still
  (AVIF, HEIC, JPEG XL, 16-bit PNG with `cICP`) maps with a copy of the same math in
  `pictures/decode/hdr/`, per pixel and without a cube, after the resize: the primaries are BT.2020,
  Display P3, DCI-P3 (with the Bradford adaptation) or BT.709, and libheif decodes such a file in
  16 bit. A test compares the still map with each point of the player cubes, so a change of one
  copy fails it.
- **Live Photos:** one folder scan hides the motion `.mov` of each Live Photo still
  (`scanner::spawn_with`, `[gallery] live_photos`, on by default). The pass runs on the sorted
  names before the first entry and reads no file. `SharedImageList::set_live_photos` comes
  before the folders, and `App::live` takes the table with the first entry. A Live still carries
  `◎ Live` in the gap above its cell, and its status facts start with `Live 2.8 s` (the motion length, one
  decimal). The meta thread reads the length from the probe cache, else a second thread runs one
  ffprobe that does not count as pending work. Without a length, the row shows `Live`, and a
  failed probe gets no retry in the session. In the grid, it
  loops its motion like a video cell (`App::loop_source`) and carries the play stamp while the
  loops are on. `[gallery] animate = false` gives no loop and no stamp (maintainer,
  2026-11-03). Enter or a left click on the middle of the fullscreen picture plays the motion
  one time (`App::enter_motion_at`, `App::video_path`), and `current` stays on the still. The
  same click plays a video placeholder and an animation, like Enter. The end of the motion, `q`,
  `Esc` and a right click go back to the still. `p` opens the motion in the mpv window. At the
  start of the motion, phewr reads both content identifiers on the UI thread (a `moov` above
  1 MB counts as no Live Photo). If they differ, the motion plays with a note. The slideshow
  shows the still only. A HEIC wins the motion over a JPEG twin, and the JPEG stays a plain cell.
  Several path arguments pair nothing. One file argument that names a hidden `.mov` puts the
  cursor on its still. The folder prefetch pairs with the same config. The design and the
  measurements are in `docs/live-photos.md`.
- **Entry kinds:** the first `App::folder_count` entries of `images` are folders, `..` first.
  A folder scan lists no motion of a Live Photo still.
  The scanner pushes them before the first file batch, and the sort at the end of the scan sorts
  the files only. Every step of the fullscreen view starts at `App::first_file()`, the prefetch
  gets the first file index, and the thumbnail jobs, the marks, the cell shape and
  the info panel skip folders. A folder cell has no thumbnail and no Kitty image. An empty
  folder (no file, and no folder besides `..`) shows a note in the status row (`is_empty_folder`
  in `ui/status.rs`). There is no directory picker. A start with several paths lists no
  folders. A symlink to a folder stays out, and so do hidden folders and hidden files (`.x.jpg`,
  the AppleDouble `._x.jpg`). One folder argument is one `read_dir`
  (`scanner::read_folder`): the files come out sorted by `name_key`, so the first batch is in the
  final order and the end of the scan keeps the list. The first batch waits for the read and the
  sort of the whole folder (6.3 ms for 10 000 files). The grid lists a folder that phewr cannot
  read, and `Enter` shows the read error, so a listing of the home folder opens no subfolder and
  triggers no macOS privacy prompt. phewr always starts in the grid, also with one
  image: it never opens the fullscreen view or a slideshow by itself (only `--slideshow` does).
- **Video programs:** phewr links no FFmpeg library. `main` looks up `ffmpeg` and `ffprobe` one
  time, and the tests never do, so `session_first_frame` gives `NotFound` in a test. The probe
  (`player/probe/`) is one `ffprobe -of json` process of about 32 ms with a 2 s timeout
  (`output_within`). A probe cache keys each `VideoInfo` by path, mtime and length, so
  `Player::open` after a cold thumbnail starts no ffprobe. A second caller for the same path
  waits for the running ffprobe and starts none. An animation reads 17 packets, an
  `.avif` file 34. The first frame runs the player GPU chain with `-frames:v 1`, with the
  rotation and, for HLG and PQ, the tone mapping cube, and waits at most 5 s. An SDR first frame
  converts after the decode with `color::convert` (`convert_first_frame` in `video/mod.rs`), so it
  pays no cube parse. The player maps the probed stream with
  `-map 0:N` for every video. A file whose GPU run gave no frame, and whose software run gave
  one, starts in software for the rest of the session (`probe::remember_software` clears
  `hardware` in its probe cache entry). A changed mtime or length drops the entry. A cancel, a
  timeout and a software failure set no flag, and the flag is not on disk. An open player keeps
  its pipeline. A VP8 WebM is the real case: the first start takes about 110 ms, the second
  32 ms. Without the programs, a video cell keeps its large play icon, and
  `Enter` opens the mpv window with a note. When the gallery cursor rests on a video or an
  animated image for 200 ms (`PRESTART_DELAY`) and `App::has_pending_work` is false,
  `video_handoff/prestart/` opens its terminal player ahead of Enter. Enter takes it
  (`Player::begin_at`), and the clip plays in real time from Enter. In the fullscreen view of a Live
  Photo still, the same rest opens the player of its motion as a run that plays once, and Enter
  or the click on the middle takes it. The UI thread runs no ffprobe for a prestart: without an
  entry in the probe cache, a thread probes the file and writes its cube
  (`Decoder::prepare_on_thread`), and the start waits for it. After a motion played, its still
  starts no second player until a step to another file. A cursor move, a step to another file, a
  new frame box, an overlay, a slideshow, the mpv window and a wait of 10 s
  (`PRESTART_LIFETIME`) drop the player. The timings and the reasons are in `docs/video.md`
  ("Start ahead of Enter").
- **One file argument:** `cli::start_paths` turns one file argument into its canonical folder
  and a cursor target (`App::place_cursor_after_scan`, the `return_to` of `Backspace`). Several
  arguments stay a plain file list. yazi starts phewr with the hovered path (`phewr %h`).
- **yazi reveal:** `reveal_in_yazi` in `yazi.rs` runs after `TerminalSession::restore`, only when
  `YAZI_ID` is set and `ya` is on PATH. It waits at most 1 s for `ya` (`YA_TIMEOUT`), so a yazi
  that does not answer cannot hold phewr. Tests pass a closure and never start `ya`.
- **Marks to yazi (2026-11-09):** after the reveal, `select_in_yazi` writes the marks
  (`App::marked_paths`) into `phewr-marks-<pid>.txt` in the temp folder, one absolute path per
  line, and runs `ya emit plugin phewr-select <list>`. The yazi plugin (README, "Marks to yazi")
  selects each path with `toggle` and a `File` object, deletes the list and moves no cursor.
  yazi 26.9 `toggle` takes no path from the command line, only a `File` from Lua, so a plugin is
  necessary. A marked Live still brings its hidden `.mov`. A folder change clears the marks, so
  the pairs of the folder at quit cover every mark. A name with a line break stays out of the
  list. Without the plugin in yazi, nothing happens, and the list stays in the temp folder.
- **Shell directory on exit:** `--cwd-file <PATH>` writes `App::gallery_dir` into the file after
  `TerminalSession::restore`, without a newline, like `yazi --cwd-file`. A shell function reads
  it and does the `cd` (README, "Change the shell directory on exit"). A failed run writes
  nothing, and a failed write is the exit error of phewr.
- **Status notes:** `ui::draw` takes a `status` note from `LoopState` in `run_loop.rs`. `App` holds
  no status note. The note shows in the status row at the bottom. The next key press or mouse
  button press clears it (`clears_status`), a key release or repeat does not. A read error of the
  scan lives in `App::scan_error`, not in `App::error`. The status row shows it after `error`. It
  never hides the video placeholder, it survives the first decode, and a key press or a folder
  switch clears it.
- **Status row:** the row shows no key hints. At the right, it shows the facts of the selected
  file (`file_meta.rs`): the size, the file size, the format, the date and, for a video, the
  length, the frame rate and the HDR curve. A thread reads them (`app/meta.rs`, one read at a
  time, 256 entries), so a cursor move never waits. A status note, a question, an error, the
  search line and a row too narrow for both hide the facts. A video takes its facts from the
  probe cache of the thumbnail job.

- **The run loop polls every 10 ms while a background job runs** (`App::has_pending_work`:
  a decode, a prefetch, a thumbnail, a grid loop, a filter, the scan or thumbnails that wait for
  their emit),
  because no event wakes it, and every 1 ms during the first 20 ms of the header read for the
  cell shape (`SHAPE_POLL`, `SHAPE_POLL_WINDOW`), every 2 ms while a cache read runs or waits
  (`CACHE_READ_POLL`), and every 2 ms while the grid decodes and shows no picture yet
  (`FIRST_PICTURE_POLL`, `App::waits_for_the_first_picture`), and until the next frame of a
  playing grid loop (`App::grid_loop_timeout`). There is no wait while the grid has emits left
  from the last pass, or while new loop frames wait for their write (`App::loops_to_send`).
  A grid loop job gives the 10 ms poll too. From 2026-10-30 to 2026-11-09 it did not, and its
  frames waited for the 250 ms wait: the first loop of a cold Live grid then came about 150 ms
  later (`bench::live::grid_animation`, 1168 ms against 1016 ms). A 50 ms poll gave only half of
  the gain. Otherwise it waits 250 ms (`next_timeout` in `run_loop.rs`).
- **Decodes share the cores.** `LibheifSlot` shares the tile threads, at most 4 thumbnail and 2
  prefetch decodes run at once, and a cache read never waits behind a cold decode. The queues,
  the slots, the cache key and the header read are in `docs/app-state.md` ("Thumbnail jobs").
- **Kitty emits:** a pass writes the new thumbnails in grid order, in one synchronized update, up
  to `EMIT_PASS_BYTES`. The rest goes out in the next pass. The numbers are in
  `docs/grid-and-kitty.md` ("Emit passes").
- **Kitty temp file medium:** the temp file medium wins only when the probe answer is `OK` and
  the terminal deleted the probe file. Otherwise a terminal with another `TMPDIR` keeps the
  files. The file name contains `tty-graphics-protocol`, as the protocol requires. The frame
  layouts per medium are in `docs/grid-and-kitty.md` ("Temp file medium").
- **Kitty images are not ratatui widgets.** ratatui draws the frame and the borders. phewr then
  writes the pictures with Kitty escape sequences at the cell positions of the last draw. A
  popup must delete the pictures first (`graphics_delete_all`), because Kitty draws them above
  the text.
  A key handler deletes the pictures only before an overlay. A zoom or a resize calls
  `mark_dirty`, and the grid write deletes the old pictures itself. A delete without a later
  draw leaves black cells.
- **Grid gaps and borders:** the cells have 1-cell gaps, every picture is padded to whole cells
  and placed centered with `c=` and `r=`, and the borders live in the gaps. The geometry, the
  padding and the borders are in `docs/grid-and-kitty.md` ("Grid gaps and borders") and in the
  module doc of `gallery/mod.rs`.
- **Terminal storage:** `TerminalImages` in `pictures/kitty/terminal_images.rs` tracks the images that the
  terminal stores. The grid deletes evicted and replaced ids and trims the oldest unshown images
  above `TERMINAL_IMAGE_BUDGET` (128 MB). The trim skips the shown ids and goes on with the next
  older image. The in-memory cache never evicts a visible cell. The
  hints, the limits and the parked pictures are in `docs/grid-and-kitty.md` ("Terminal storage").
- **Cell size:** `query_cell_pixel_size` asks with `CSI 16 t`, because TIOCGWINSZ includes the
  window padding of Ghostty (2258 px / 52 rows = 43.42 px). If the terminal answered at
  start, the run loop asks again after each resize. The read takes every byte of the tty, so
  a key typed in that millisecond is lost. After a resize, that read can also take an `ENOENT`
  of the last grid pass. This costs nothing: the resize sets `graphics_storage_dirty`, and the
  next grid write sends `d=A` and every picture again. At the start, the query goes out
  together with the Kitty probes and the DA1 query, in one round trip. A terminal that answers
  neither the probes nor DA1 costs `PROBE_TIMEOUT` (300 ms) one time. `PHEWR_KITTY_LOOPS` or
  `[gallery] loops` skips the animation probe. SSH and `PHEWR_KITTY_TRANSFER` skip only the
  transfer probe.
- **Mouse:** phewr enables only the mouse modes 1000, 1002 and 1006 (`terminal::enable_mouse`),
  never 1003, so a pointer move without a button does not wake the run loop. Do not use
  crossterm's `EnableMouseCapture`, because it turns on 1003. `[gallery] mouse = false` turns
  the capture off. The run loop releases the mouse while an mpv window plays
  (`sync_mouse_capture`), and `TerminalSession::restore` releases it on exit and on a panic.
  `ui::draw` records `app.layout`, and a mouse handler reads the regions through
  `ui::split_info`, `ui::content_area` and `ui::status_area`. A test that needs them calls
  `test_support::draw_frame` first. The double-click (400 ms) uses `handle_event_at` with a
  clock, so tests add a `Duration` to an `Instant` and never sleep. macOS can send Shift+wheel
  as a horizontal wheel, so `ScrollLeft` and `ScrollRight` change the grid size too.
- **Late probe answer:** the start write holds the cell size query, the OSC 10 foreground query
  (`ESC ] 10 ; ? BEL`), the `a=q` transfer probe,
  the animation probe and the DA1 query `ESC [ c`. The animation probe is a silent 1x1 image
  (`a=t,q=2,i=32`), one frame `a=f,i=32` that the terminal answers, and `a=d,d=I,i=32`. The read
  ends at the DA1 answer, or when every owed answer came. After either, no Kitty answer can come
  late. `LateAnswerFilter` in `keys/mod.rs` is armed only after a timeout of the whole round
  trip. It drops the key events of each owed late answer: a Kitty answer from Alt+`_` to
  Alt+`\`, and the OSC 10 answer from Alt+`]` to Ctrl+`g` (BEL) or Alt+`\`. A missing OSC 10
  answer counts as owed after a timeout. A late DA1 answer is no key press: crossterm keeps it
  as an internal event. Since the self check (2026-11-11), `AnswerKeys` reads every Kitty answer
  before the filter, also a late probe answer. Each whole Kitty answer lowers the owed count of
  the filter (`LateAnswerFilter::kitty_answers_came`), so the filter never swallows an answer of
  the self check. A terminal answers in order, so the late probe answers come before the answers
  of the self check. The filter sees only the rest, in practice a late OSC 10 answer.
- **Self check (2026-11-11):** when the self check is on, the engine places with `q=1`
  (`Screen::with_self_check`, the argument `errors` of `place_as_to`). The terminal then answers
  an error for a placement of a picture that it does not hold, `ENOENT: image not found`. A
  transfer keeps `q=2`, else each transfer gets an `OK`. phewr turns the self check on by
  default, and `PHEWR_KITTY_SELF_CHECK=0` turns it off (`SELF_CHECK_OVERRIDE` in `main.rs`).
  crossterm reads an answer as key events (Alt+`_`, `G`, `i`, `=`, ... and Alt+`\`).
  `AnswerKeys` (`pictures/kitty/answers.rs`) rebuilds the bytes from the events and holds the
  events of a possible answer for at most 50 ms (`ANSWER_WAIT`). The run loop wakes at the end
  of the hold (`release_held`). phewr drops the events of a started answer that never ends,
  because their letters run the actions of their keys. `handle_input` runs `AnswerKeys` first,
  then `LateAnswerFilter`. An `ENOENT` goes to `Screen::answer`, and the screen forgets the
  picture and its placements. While `Screen::has_lost` is true, `sync_grid_icons` writes a grid
  pass, and that pass sends the lost picture again. The measurement of 2026-11-11: after ten
  scrolls in a herdr pane, phewr lost 2 to 5 cells before the self check, schnattr 0 and plain
  Ghostty 0. A placement that herdr drops without an `ENOENT` stays gone until its next move.
- **Grid zoom:** the grid shows 2 to 12 rows (`MIN_VISIBLE_ROWS`, `MAX_VISIBLE_ROWS`). A 2x2
  grid is the largest thumbnail size. The fullscreen view is the larger view. The config key
  and the state file clamp to that range on load. A terminal too small for 2 rows keeps the
  rows that fit. A change of the rows or the columns (a zoom, the info panel, a resize, the
  cell shape) keeps the place of the cursor row on the screen: the top row stays at the top, the
  bottom row at the bottom, and a row between maps its middle. The scroll then stays inside the
  list, so at its start or its end the screen stays full. Until 2026-11-09 the grid kept the
  scroll row: after a zoom out at the end of a list, only the last row showed. The place of the
  first change holds while the cursor and the scroll stay (`KeptPlace`), so a zoom out and back
  in returns to the same screen.
  A state file that does not parse stays as it is: `+`, `-` and `0` do not save over it, and each
  save goes through a temp file and a rename.
  The state file holds a global `visible_rows` fallback, a `zoom_counter` and a `[zoom]` table
  keyed by the canonical folder path. Each `[zoom."/path"]` entry holds `rows` and `order`. The
  plain form `"/path" = 3` also loads. `+` / `-` save the level for the folder and as the
  global fallback. `0` resets the folder and removes its entry. The table keeps the 200 most
  recent folders.
  `+`, `-` and `0` do no file I/O on the UI thread. They queue a `StateChange` to the writer
  thread of `App` (`state_writer`). The thread waits 100 ms for more changes, applies them in
  order to one read of the file, and writes once. `StateWriter` keeps the changes that are not
  written yet, and `folder_zoom_rows` applies them over the file, so a fast folder switch sees
  the new level. `0` writes only if the file holds an entry for the folder. The drop of `App`
  flushes the queue. A panic ends the process with `exit(101)` and loses the changes of the
  last 100 ms. A zoom test uses `StateWriter::with_delay` and `flush`, not a poll loop.
- **Grid loops:** every visible video, animated and Live Photo cell loops after its still, as a Kitty
  animation or through ticks (`Loops`), and `[gallery] animate = false` turns them off. The
  frames are in `docs/video.md` ("Grid loop frames"). The two paths, the ids, the jobs and the
  loop cache are in `docs/app-state.md` ("Grid loops"). In herdr (`HERDR_ENV` or
  `TERM_PROGRAM=herdr`), the loops take the ticks without the animation probe
  (`pictures::kitty::protocol::probe::chosen_loops`). herdr 0.9.5 keeps its own Kitty image store with
  animation frames and answers the probe itself, but it plays no Kitty animation. The ticks
  play there (maintainer, 2026-11-03). `PHEWR_KITTY_LOOPS` and `[gallery] loops` win over herdr.
- **Cell icons:** a grid cell without a thumbnail shows a large dim Lucide icon: `image` for a
  picture, `play` for a video, `folder` for a folder and `folder-up` for `..`, without a name
  (`icons.rs`, `draw_cell_icon` in `ui/grid.rs`). The icon box is about 30 % of the cell height
  and square in pixels (`App::cell_px`). A cell that cannot hold 2 rows, or an icon that the grid
  edge cuts, gets the one-cell symbol (`Icon::symbol`). The help popup hides the icons, as it
  hides the pictures. A HEIC preview counts as a thumbnail. The design is in
  `docs/grid-and-kitty.md` ("Grid icons").
- **Cell shape:** `App::request_cell_shape` reads the headers of the first `SHAPE_SAMPLE` (64) files
  on a thread, once per scan (`pictures/shape/mod.rs`). The shape stays until the next directory switch,
  so the columns never move while the user browses. The buckets and the per-format rules are in
  `docs/grid-and-kitty.md` ("Cell shape").
- **Resize filter:** every resize uses Catmull-Rom (`resize_options` in `pictures/resize/mod.rs`). It is
  one third faster than Lanczos3, and the difference is not visible.
- **Pixel layouts:** every resize picks the fastest layout of `fast_image_resize`
  (`resize::Layout`, `rgb_resize_is_fastest`). An opaque RGBA image (`Decoded::opaque`,
  `ScreenImage::Opaque`) resizes without the alpha multiply, the fastest at every scale, with the
  same pixels. Only a downscale to 1/2 or less resizes as RGB, and libjpeg-turbo and libheif
  decode in the layout of the resize that follows. An image with a transparent pixel keeps the
  alpha multiply. The fullscreen image and the zoom crop reach the Kitty write as RGB when they
  are opaque (`ScreenImage::Rgb`), so the write does no scan and no copy, and a zoom crop is a
  view of the source. `App::loaded`, the prefetch and the zoom carry a `ScreenImage`. The grid
  thumbnails keep their RGBA with the padding, with the same pixels as before (a test compares 46
  fixtures), so the cache version did not change.
- **Process start:** about 30 ms pass before `main`, most of it the dylib load of libheif and its
  codec libraries (a C program that links libheif needs 24.9 ms, with no library 4.6 ms).
  The removal of the `trash` crate took Foundation out of the binary, with no measurable change of
  the start time (50 runs of `phewr --version`: median 15.2 ms before, 15.4 ms after). The former `dist`
  profile (full LTO, stripped symbols) and `target-cpu` gave no measurable gain, so `just install`
  builds `--release`.
- **Disk cache format:** the cache files are QOI (`.qoi`): a hit reads and decodes in about
  0.75 ms instead of 1.7 ms for PNG, at the same size. Raw RGBA reads faster but takes 2.3 times
  the space. The prune removes the `.png` files of older versions at once.
- **Thumbnail cache key:** the key hashes `THUMBNAIL_CACHE_VERSION`, the path, the size, the
  mtime, the target size and the color target. The version is
  `concat!(env!("CARGO_PKG_NAME"), "-thumbnail-v22")`, so phewr hashes `phewr-thumbnail-v22` as
  before (v21 since the DCT scale follows the fit). When the thumbnail output changes, increase
  the `v21` part. If several workers change the thumbnail output at the same time, each takes its
  own version, higher than main and than every branch in flight. `thumbnail_entry` in
  `pictures/jobs/disk_cache/mod.rs` computes the key, and `CacheEntry` holds the file and the
  stamp (the length and the mtime). A job computes it at most one time, and the folder prefetch
  one time per file (`warm_disk_cache` and `sharp_thumbnail` call `decode_into_cache` with the same key). A test
  rebuilds the hash from its inputs, so a change of the input or of its order fails the test. The
  prune removes temp files of the cache and partial cube files of the tone map that are older than
  1 hour.
- **Thumbnail cache folder:** the thumbnails of phewr sit at the top of `~/Library/Caches/phewr`
  (`default_disk_cache_dir` in `app/disk_cache/mod.rs`). Do not move them, because a move makes
  the cache of the maintainer cold. The test `the_cache_folder_stays_where_it_was` holds the path.
  `prune_disk_cache` refuses a folder that holds a subfolder, so phewr must keep no subfolder
  there. The shared module has no default folder. Another app passes
  `thumbnail_dir_below(<its cache root>)`.
- **Folder prefetch:** after the cursor rests on a folder cell for `FOLDER_PREFETCH_DELAY`
  (300 ms) and `App::has_pending_work` is false, one thread decodes the first screen of that
  folder into the disk cache (`app/folder_prefetch/`). It computes what Enter computes, with
  the same functions (`scanner::folder_listing`, the shape sample, `folder_zoom_rows`,
  `GalleryState::for_folder`), and takes the union of both cursor orders. It pauses before each
  file while the grid has work, so one running decode can overlap the grid. A cursor move, a
  folder change, a zoom, a resize, a mode change or the quit cancels it, and the next start
  waits for the old thread. `..` takes part only after a real cursor move. A folder that this
  session prefetched, or one with fewer than 2 files, does not start. Only the folder switch
  bench ticks the prefetch. The prefetch decodes on the global rayon pool with the normal libheif
  share. A pool of its own and one libheif tile thread gave no gain beyond the spread
  (2026-11-03, `docs/app-state.md`).
- **Image colors:** phewr converts every image into Display P3 (`[terminal] colorspace`), because
  Ghostty copies the pixels of a Kitty image without a conversion. The sources, the transform
  cache and the cubes of the player are in `docs/images-and-colors.md` ("Color conversion").
  moxcms 0.9.1 truncated the output table of a known curve, and 0.9.2 rounds again. The
  workaround `rounding_output_curve` went with the update (2026-11-09, thumbnail cache v22).
- **JPEG XL colors:** jxl-oxide 0.12 converts ICC v2 samples wrong. A lossless file with an ICC
  profile renders in its own space, and `moxcms` converts it (16 bit for deeper files, in tasks
  of 64 rows). A transform error, XYB and the SDR enum encodings stay with jxl-oxide. A PQ or HLG
  file (an enum encoding, or an ICC profile with a PQ or HLG `cicp` tag) keeps its 16-bit signal
  for the tone map of `pictures/decode/hdr/`. jxl-oxide 0.12 clips XYB samples outside the sRGB gamut
  before the BT.2020 conversion, so a lossy HDR JPEG XL loses saturation there, and a test marks
  the limit. The codestream orientation applies, the EXIF orientation does not. A gray file with
  a PQ or HLG enum encoding renders its 16-bit signal and takes the tone map of `pictures/decode/hdr/`
  like an RGB file (`Cicp` BT.2100). A gray file with a PQ or HLG ICC profile stays with
  jxl-oxide. 34 of 40 public JPEG XL files match their reference within 4 levels
  (`docs/images-and-colors.md`, "JPEG XL").
- **JPEG XL sample copy:** every route copies the rendered grids of jxl-oxide in tasks of rows
  on all cores (`render_samples`), with the codestream orientation, the clamp and the rounding
  of jxl-oxide. Patches (a reference frame before the keyframe), layers, a frame offset, a crop
  and spot colors take the one-thread `ImageStream`. The copy reads `ImageBuffer` of
  `jxl-render` and depends on its private layout: each grid starts at the image origin. For this
  reason, `jxl-oxide = "=0.12.6"` and `jxl-render = "=0.12.4"` are exact pins. An update is a
  deliberate step: set `jxl-render` to the version of the new jxl-oxide, then run
  `parallel_copy_matches_stream` and a byte comparison of the decode on real files.
- **`image` decoders:** the `image` crate has only the decoders that the scanner lists
  (`default-features = false`). If you add a format to the scanner, add the `image` feature
  too. The `image` feature `avif` is the encoder, not a decoder.
- **Embedded HEIC thumbnails:** a cold HEIC cell decodes in two phases. Phase 1 decodes the
  embedded thumbnail (iPhone: 320x240) and shows it as a preview (`start_preview_decode`, at most
  `MAX_PREVIEW_JOBS`). The thumbnail must have the same `irot` and `imir` as the primary image.
  `pictures/decode/heif_boxes.rs` reads them. The preview goes out at its own resolution, padded to the
  whole cells of its own aspect, and the terminal scales it to those cells with `c=` / `r=`. A
  preview of another aspect than the primary image shows other margins than the sharp picture,
  never a stretch (`kitty_output::grid::tests::square_preview_of_wide_photo_keeps_its_shape`).
  Phase 2 is the sharp decode through `MAX_THUMB_JOBS`. The sharp jobs of libheif files wait
  until every preview is out (`sharp_job_waits`). Other formats start at once. A preview never
  goes into the disk cache, and a disk cache hit skips the preview. The sharp image gets a new
  Kitty id. `emit_new_thumbnails_to` deletes the preview image (`a=d,d=I`) before the transmit
  (`app.stale_image_ids`). The sharp decode still uses the embedded thumbnail when it is at least
  the fitted size of the primary image. Fullscreen, zoom and prefetch pass
  `EmbeddedThumbnail::Deny`. A PQ preview maps with the HDR10 peak of the primary image, like the
  sharp picture, so the colors do not jump at the swap. It takes the peak from the boxes of the
  `irot` and `imir` check and reads no box twice.
- **libheif reads through a stream.** `open_heif_context` opens a file with
  `libheif_rs::StreamReader`. `HeifContext::read_from_file` copies the whole file first, which
  costs 30 ms to 40 ms per 24 MP photo.
- **The `sips` route:** sips writes an uncompressed TIFF with the Orientation tag into a temp
  file. The grid and the fullscreen view ask for a copy at the size of their box (`-Z`), and the
  zoom gets the full image. A relative path gets `./` in front, because sips reads `-x` as a
  function name. JPEG, PNG and HEIC never take this route. RAW and PSD try the embedded JPEG
  first (`pictures/decode/embedded_jpeg/`): the largest JPEG of the TIFF IFDs, the CR3 boxes, the RAF
  header, the ORF maker note or PSD resource 1036. It is the grid preview. A JPEG covers a box
  when its fit into the box needs no upscale: one side reaches the box (`EmbeddedJpeg::covers`).
  A JPEG that covers the box is the sharp thumbnail and goes into the disk cache, so sips does
  not run for the cell. The fullscreen view and the prefetch also take a JPEG that covers their
  box, so a 3200x2400 preview serves the 2736x2052 fit of a 3392x2052 box. The zoom keeps sips.
  An ORF holds its preview in the Olympus maker note (`pictures/decode/embedded_jpeg/orf.rs`). An OM-1
  ORF gives a 3200x2400 preview in about 1 ms, and its grid cell decodes in about 20 ms instead
  of 1.3 s to 1.6 s of sips. `sips -Z` reads no preview: it develops the RAW and then scales (0.5 s to
  1.1 s per file). The real-file numbers are in `docs/images-and-colors.md` ("sips").
  `sips::decode_with_preview` reads the embedded JPEG once and corrects each RAW develop with it.
  sips ignores the orientation of RAF and ARW, also of real portrait shots. If the known
  orientation turns by 90 or 270 degrees and the develop keeps the unturned aspect, phewr turns
  it. For RAF and ARW, an orientation of 2, 3 or 4 (and 5 to 8 with a square size) comes from
  a pixel compare: `sips::flip_gap` scales the develop and a small reference to 32x32 gray
  pictures. The develop turns when the oriented candidate is closer by more than `FLIP_MARGIN`
  (2.0 gray levels). The reference is the smallest JPEG of an ARW (IFD1) or the EXIF thumbnail of
  the RAF preview (`embedded_jpeg::decode_reference`), because the large preview takes 31 ms to
  63 ms to decode. A tie, or a file without a reference, keeps the develop. If a RAW develop has only zero color samples, the embedded JPEG takes its place (sips
  develops `P1000475.RW2` black). A PSD never takes this fallback. The info panel shows the
  oriented size (`sips::shown_dimensions`).
- **Embedded JPEG orientation:** the container orientation (IFD0 of a TIFF RAW or an ORF, `CMT1`
  of a CR3) wins over the EXIF of the embedded JPEG. Without a container orientation, the EXIF of
  the JPEG applies (RAF). The turbo path, the `image` fallback and the cell shape use this one
  rule. The DCT scaling of a rotated JPEG compares against the turned box. libjpeg-turbo decodes
  at the smallest of 1/8, 1/4, 1/2 and 1 that covers the fit of the image into the box, never
  smaller than the fit (`pick_scaling_factor`, `52c9222`). The other eighths run in plain C
  without NEON and take 1.1 to 1.6 times the full decode, so phewr never uses them.
