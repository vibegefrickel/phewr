# Video and animations

This page holds the design facts of the terminal player, the probe, the color cubes, the start
ahead of Enter, the animated images, the grid loop frames and the mpv window. AGENTS.md holds the
rules. This page holds the reasons and the numbers.

Since 2026-11-19 (tuilibr phase 4d), `tuilibr::pictures::motion` holds the probe and the ffmpeg
runs. It holds the color cubes and the animation check. It holds the first frame and the loop
frames. phewr keeps the terminal player (`player/`), the prestart, the mpv window and the Live
pairing. phewr also keeps the grid jobs and the budget. The module map is in `tuilibr/docs/pictures.md`, "The motion module". The facts below name
the module of each part.

## Terminal player

### Frame path

- ffmpeg runs as a subprocess (`tuilibr::pictures::motion::decoder`, behind the `Decoder` trait).
- ffmpeg decodes on the media engine of the Mac (`-hwaccel videotoolbox`).
- ffmpeg scales and turns the frame on the GPU with `scale_vt` and `transpose_vt`.
- For 180 degrees, the graph adds `hflip,vflip`. The option `-noautorotate` is on.
- The GPU chain downloads the frames in the format that VideoToolbox gives back. ffmpeg picks it
  from the chroma layout and the depth of the stream (`videotoolbox_best_pixel_format`), and
  `hwdownload` takes no other format. 4:2:0 gives `nv12` or `p010le`, 4:2:2 `nv16` or `p210le`,
  and 4:4:4 `nv24`, `p410le` or, above 10 bits, `p416le`. A stream with alpha gives `ayuv64le`.
- The probe reads `codec_name` and `pix_fmt` into `VideoInfo::samples` and `VideoInfo::hardware`.
  ffmpeg 9 offers VideoToolbox no HEVC or VP9 stream above 10 bits (tested on an M5). Such a
  stream starts in software, because a GPU run fails after 160 ms to 190 ms.
- The software chain converts `yuv420p12le` into `rgb48le` without an 8-bit or 10-bit step. On a
  12-bit gradient, 252 levels reach the cube, against 107 through `p010le` and 28 through `nv12`.
- A 12-bit ProRes 4444 clip decodes on the GPU and downloads as `p416le`. Its frame equals the
  software frame byte for byte.
- For HLG and PQ, the graph adds `format=rgb48le,lut3d=...:interp=tetrahedral`.
- An SDR video or animation gets a `lut3d` cube too, unless its primaries equal the target
  primaries (`Cube::for_video`). See "Color cubes".
- ffmpeg writes raw frames into a pipe. A thread reads the pipe one frame at a time, so the run
  loop never waits for ffmpeg.
- ffmpeg writes `-pix_fmt rgb24` for a video and `rgba` for an animation.
- `pixel_format` in `motion::decoder::command` is the one source for the frame layout.
- If a GPU run gives no frame, the decoder runs again in software (`Pipeline::Software`).
- An animated image decodes in software only, because the AV1 decoder of an AVIS file stops with
  an error when `-hwaccel videotoolbox` is on.

### Frame rate

- A constant-rate video gets no `-r`. ffmpeg writes its frames at `r_frame_rate`, and the pacing
  uses the same rate.
- iPhone clips (30 and 29.99) stay constant-rate.
- A video with a variable frame rate gets `-fps_mode cfr -r`.
- The probe calls a video variable when `r_frame_rate` and `avg_frame_rate` differ by more than
  1 % (`VARIABLE_FPS_TOLERANCE`).
- The output rate of a variable video is `r_frame_rate`. If the base rate is more than 1.5 times
  the average (`BURST_RATIO`), the base rate is a burst.
- For a burst, the output rate is the average, at least 24 fps (`MIN_BURST_FPS`) and never above
  `r_frame_rate`. The floor keeps ffmpeg from duplicating most frames.
- Examples of a burst: a phone clip at 60 with an average of 30, and an iPhone slow motion clip
  at 120 and 29.6.
- The rate of a variable video never exceeds 60 fps (`MAX_VARIABLE_FPS`). The code applies this
  cap to the variable output rate only.
- Reason: ffmpeg writes raw frames at `r_frame_rate`. The player once paced them at
  `avg_frame_rate`. A screen recording at 25 and 7.7 played 3.2 times too slow.
- The code is `pacing_fps` and `variable_fps` in `motion::probe::parse`.
- `VideoInfo.rate` is `Rate::Constant`, `Rate::Variable` or `Rate::Animation`.
- An animation decodes at a constant rate (`-fps_mode cfr -r`) at the rate of the probe. A frame
  with a long delay goes out more than once, so the pacing stays exact.

### Kitty write

- The player hands the due frame to the Kitty write. The write sends it through the temp file
  medium with one image id.
- One frame is one temp file. The image id is `u32::MAX - 1`.
- The grid ids count up from 1. The loop frames count up from 2^31.
- The write uses `c=` and `r=` for the cell box and no `X=` and `Y=`.
- The first frame of a new video removes all other placements in the same synchronized update
  (`App::clear_before_video`). The photo of the view before does not show around the frame.
- The frame box is the window in pixels, capped at 1920x1080 (`MAX_FRAME` in `motion::frame_box`).
- A resize moves the cell box. The frame size stays until the next ffmpeg start.

### Payload cap

- `encoder::MAX_PAYLOAD_BYTES` (64 MiB) caps one Kitty payload, because Ghostty 1.4.0 refuses
  65 MiB.
- The fullscreen and prefetch decode boxes shrink below the cap before the decode (`capped_box`,
  3 bytes per pixel). No opaque decode runs the writer fallback.
- A transparent image above the cap and a zoom crop still shrink in the writer with Catmull-Rom.
  They get `c=` only, so the rows follow the aspect ratio.
- `within_payload_cap` in `kitty_output/mod.rs` uses the resize of the decoders. A 6016x3384
  opaque RGB image (61 MB) stays as it is.

### Frame queue and pool

- The reader thread holds 2 frames (`FRAME_QUEUE`). If the terminal is slow, it blocks ffmpeg on
  the pipe. The player has no pause, so the slow terminal is the only cause.
- The reader of a run owns 5 frame buffers (`FRAME_POOL`, `FramePool`). No run allocates a
  zeroed 6 MB `Vec` per frame.
- A `PooledFrame` returns its buffer on drop. This holds for a frame that the catch-up rule
  drops. A frame of an old run returns to the pool of its own run.
- A short frame from ffmpeg counts as dropped. The screen keeps the last frame, and `poll_at`
  returns `false`.

### Real time

- The rule is in AGENTS.md. A 6 s clip with a slow terminal (3 intervals per write) played for
  18 s with the old moving clock. It plays for at most 6.6 s now. A test holds this.
- `MAX_LAG` (300 ms) and `RESTART_LEAD` (0.3 s) are in `player/mod.rs`. `FRAME_QUEUE` is in
  `tuilibr::pictures::motion::decoder`.

### Decoder details

- `LoopSampling::Keyframes`: the decoder skips the frames between the keyframes. A 4K clip takes
  about 0.15 s.
- `loop_args` caps a software decode at 2 threads (`-threads 2 -filter_threads 1`). Without the
  cap, a software decode of a 4K clip took 10 s of CPU on 7 cores.
- `cube_format`: the 8-bit SDR cube is the cheapest step. It adds about 1 ms per 1920x1080 frame.

## Probe

The code of this section is in `tuilibr::pictures::motion::probe`. See `tuilibr/docs/pictures.md`.

- The probe is one `ffprobe -of json` process. `Player::open` after a cold thumbnail starts no
  ffprobe, because the probe cache has the `VideoInfo`.
- An animation reads at most 16 packets (`TIMING_PACKETS`). The probe asks for one more
  (`PACKET_LIMIT`), and that packet tells whether the walk reached the end.
- Reason for 16: the timing walk of a 181 MB GIF of 1280x720 pixels costs about 15 ms for 16
  frames and 250 ms for all 300 frames. The walk runs on the UI thread before the first frame.
- The rate of an animation is the GCD of the frame delays, or the mean rate above 50 fps
  (`MAX_ANIMATION_FPS`). The length comes from the container.
- An AVIS file holds two video streams. The probe maps the stream with the most frames.
- The probe reads at most 34 packets for an `.avif` file, because `-read_intervals` counts the
  packets of all video streams.
- The code is in `motion::probe` (`mod.rs` and `parse/`).

### The software mark

- When the GPU run of a file gives no frame and the software run gives one, the decoder calls
  `Decoder::remember_software`. `motion::probe::remember_software` clears `hardware` in the probe cache
  entry of the file, so `Pipeline::first` starts in software at the next probe.
- The callers are `with_fallback` (the first frame and the grid loops) and `Player::poll_at`.
  `with_fallback` takes the decoder and the path. One loop job keeps the result in
  `Run::software`, so a long clip with 12 seeks pays the failed GPU run once.
- A changed mtime or length drops the entry, and the next probe starts on the GPU again.
- A cancel, a timeout and a software failure set no flag. The flag lives in memory for the
  session only.
- A player that is already open keeps its pipeline, also a prestart player.
- A short GPU failure under load also marks the file for the session. The cost is the software
  speed for that file until phewr quits.
- Real case on an M5: VideoToolbox rejects VP8. The first start of a VP8 WebM takes about
  110 ms, the second 32 ms. AV1 on M1 and M2 takes the same path, untested.

## Color cubes

The code of this section is in `tuilibr::pictures::motion::tone_map`. `CUBE_VERSION` lives there.
See `tuilibr/docs/pictures.md`.

- `CUBE_VERSION` is 4. Version 3 added the target space to the cube names.
- The SDR cube has 17 points (`SDR_LUT_SIZE`). The HDR cube has 33 points (`HDR_LUT_SIZE`).
- The SDR cube runs in 8 bit. At 1080p, it costs 1.4 ms to 2 ms per frame. See
  [Images and colors](images-and-colors.md#color-conversion) for its values and its sources.
- ffmpeg parses a cube at each start. 33 points cost about 42 ms. 17 points cost about 16 ms.
- A 17-point HDR cube saves about 20 ms per start. Its colors differ by up to 39 levels from the
  33-point cube (mean error 0.84 to 1.09 levels against 0.51 to 0.56). HDR keeps 33 points.
- The smooth SDR conversion stays within 1 level at 17 points.
- `SDR_WHITE_NITS = 500` matches the macOS SDR export on three iPhone clips.
- A PQ cube name holds its peak bucket (`tonemap-pq<nits>-<target>-v4.cube`). The frame probe
  runs only for a PQ stream without stream metadata, and the probe cache keeps the result. Before
  `af61a4b`, the stream probe did not ask for `max_luminance` and `max_content`, so every PQ
  stream paid the frame probe. The `video/` fixtures of tuilibr test both paths.
- Probe cost on a quiet machine: the stream probe takes 15.9 ms, the frame probe 17.7 ms (1080p
  HEVC) to 20.3 ms (1080p AV1). At a load of 2.2, the stream probe took about 30 ms, and the 4K
  frame probe 47.9 ms (HEVC with SEI), 52.8 ms (HEVC without metadata) and 40.3 ms (AV1). A PQ
  file without HDR10 metadata pays the frame probe for nothing.
- The Apple HDR10 example clip (MaxCLL 1375, bucket 2000): the midtones stay within 1 % of the
  1000-nit map. The highlights from 1000 to 1375 nits no longer clip at white. They come out
  about 10 levels darker, with detail.
- A PQ or HLG still maps with a copy of the same math. See
  [Images and colors](images-and-colors.md#hdr-stills).

## Start ahead of Enter

- ffmpeg needs about 180 ms to 260 ms for its first frame. This time holds the process start,
  the `VideoToolbox` setup and, for HDR, the parse of the 33-point cube.
- When the gallery cursor rests on a video or an animated image for 200 ms (`PRESTART_DELAY`)
  and the grid has no work, `video_handoff/prestart/mod.rs` opens the player of that cell.
- `prestart_timeout` wakes the run loop at the end of `PRESTART_DELAY`.
- ffmpeg decodes the first frames into the queue and then blocks on the pipe. A long rest costs
  no decode work.
- Enter takes the player (`Player::begin_at`, `take_prestart`).
- In the fullscreen view of a Live Photo still, the same rest opens the player of its motion
  `.mov` with `Repeat::Never`. `take_prestart` takes a player only for the same file, the same
  repeat and the same frame limit. After a motion played, its still starts no second player
  until a step to another file or to the grid.
- On 4 motions of `12-live-photos`, the first frame comes 122 ms to 127 ms after Enter without a
  rest, and 0.1 ms to 0.2 ms after a rest of 500 ms (`real_rest_starts_motion_ahead`).
- After a rest of 500 ms or more, the first frame comes 1 ms to 2 ms after Enter. Without a rest,
  it comes about 240 ms after Enter.
- The clock of a run starts at its first frame. The clip plays in real time from Enter, and
  nothing drops.
- At most one such player exists. It belongs to the file under the cursor and to the frame box.
- These events drop the player: a cursor move, a step to another file, a view change, the help
  popup, a slideshow and an mpv window.
- A new frame box (a resize, the info panel) drops the player and starts it again for the new box.
- A run that plays once (a running slideshow) drops the prestarted player before its own ffmpeg
  starts. Two ffmpeg runs never overlap.
- A player that waits `PRESTART_LIFETIME` (10 s) for Enter ends. It holds an ffmpeg process with
  its `VideoToolbox` session (452 MB resident for a 4K HLG clip, at 0 % CPU) and 3 frame
  buffers (19 MB at 1920x1080, 6.2 MB each as `rgb24`). The 3 buffers are the 2 frames of the
  queue (`FRAME_QUEUE`) and the frame that the reader holds while it waits to send. The pool
  limit is 5 (`FRAME_POOL`), but ffmpeg blocks before the pool makes more.
- The drop of a `FrameStream` kills ffmpeg and reaps it on a thread. The UI thread does not wait
  about 6 ms for the exit.
- A file whose probe or start fails gets no second try until the cursor leaves.
- The pass that starts the player takes about 1 ms on the UI thread. This time holds the probe
  from the cache, the cube lookup and the spawn of ffmpeg.
- The UI thread runs no ffprobe. Without an entry in the probe cache, a thread runs the probe and
  writes the cube (`Decoder::prepare_on_thread`), the start waits for it, and the run loop polls
  every 10 ms. A cold probe on the UI thread took 25 ms to 40 ms. With the thread, the slowest
  pass takes less than 1 ms, and a cold rest starts 240 ms to 290 ms after its begin. A failed
  probe gets no second try during the rest. This also covers a gallery video whose thumbnail
  came from the disk cache without a probe.
- If the HDR cube is not on disk yet, the pass writes it and takes about 23 ms. The first-frame
  thumbnail of an HDR video writes the same cube. So this happens only for the first HDR video of
  a new cache directory.

## Animated images

- An animated GIF, WebP, APNG or AVIS plays in the terminal player like a video.
- The thumbnail job reads the animation flag into `App::animated`, so the draw reads no file.
- The job stamps the large play triangle into the still of a video or an animated image
  (`icons::stamp_play`, white with a dark edge, 30 % of the height).
- The stamp happens after the disk cache read. The cache keeps the still without the stamp.
- The loop frames replace the stamped still, so the stamp means "not playing yet".
- There is no small `▶` marker in the label or in the gap.
- `App::opens_in_player` reads the header of the one file after a key press.
- `App::is_video` drives the loading icon and the skips after a failed open (`App::fullscreen_on_video`).
- ffmpeg gets no `-hwaccel` for an animation. See "Frame path".
- `q` and Esc from a playing animation go to the grid. An animation plays like a video in every
  view (2026-11-19): Enter in the grid and each fullscreen step open the player with the loop. The
  still of an animation shows only when no player can play at all, with a note.
- In a running slideshow, the terminal player holds an animation for at least the slideshow
  interval (`Player::play_at_least`). The mpv window has no such hold.
- The rest of the facts are under "Probe". The header check of the animation formats is
  `tuilibr::pictures::motion::animation`.
- ffprobe reports no profile for WebP and APNG. The probe reads their ICC profile or cICP chunk
  (`motion::animation::embedded_primaries`). See
  [Images and colors](images-and-colors.md#color-conversion).

## Grid loop frames

- The code is `tuilibr::pictures::motion::loop_frames` (`LoopFrames`, `decode_loop`, `loop_of`).
  It makes the frames of the grid loops with ffmpeg. phewr calls it through `session_loop` in
  `video/`. The state and the jobs are in [App state](app-state.md#grid-loops).
- A video loop shows 12 frames spread over the whole clip. Each frame shows for 333 ms
  (`VIDEO_FRAME_INTERVAL`). The loop takes 4 s.
- One ffmpeg run decodes only the keyframes (`-skip_frame nokey`). It takes 0.11 s to 0.26 s for
  a 4K clip.
- iPhone clips have one keyframe per second. A clip of at most 20 s (`SHORT_CLIP_SECONDS`) with
  fewer than 12 keyframes decodes at a constant rate instead. This takes 0.4 s to 1 s for 3 s to
  10 s of 4K.
- A clip longer than 60 s (`LONG_CLIP_SECONDS`) seeks to 12 points, because its keyframe run
  reads the whole file.
- An animated image keeps its own frames and delays. It has at most 48 frames
  (`MAX_ANIMATION_FRAMES`) and 12 fps (`MAX_LOOP_FPS`) and at least one frame per minute
  (`MIN_ANIMATION_FPS`).
- Above the caps, ffmpeg drops frames evenly.
- The frames get the rotation, the scale and the colors of the first-frame thumbnail
  (`motion::first_frame`, called through `session_first_frame` in `video/`). They have half the width and the height of the grid picture.
- The terminal scales the frames to the cells of the still. So 12 frames take 3 times the bytes of
  the still, and a screen of loops fits the storage budget of the terminal.

## mpv window

- The rules are in AGENTS.md. The code is in `mpv.rs`, `video_handoff/window.rs` and
  `app/video.rs`.
- A slideshow window (`Playback::Once`) gets the IPC socket `mpv-<pid>-<n>.sock` in the
  thumbnail cache directory, else in `$TMPDIR/phewr`. Each window has its own socket, and the
  path stays below 100 bytes. The runner removes a stale socket before the start and the socket
  after the exit.
- The reader thread tries to connect for 2 s, because mpv creates the socket after its start. A
  clip that ends before the connect, or a quit in the first 20 ms, gives no reason and continues.
- mpv 0.41 sends `{"event":"end-file","reason":"eof"}` at the natural end and
  `{"event":"end-file","reason":"quit"}` on the `quit` command. Both exit with code 0. A SIGTERM
  sends no `end-file` line and exits with -15.
- Each mpv start gets `--input-commands` with `keybind l "quit 10"` and `keybind h "quit 11"`
  (`mpv::NEXT_FILE`, `mpv::PREVIOUS_FILE`).
- `App::step_from_video` keeps `video_return`. A run of videos from the gallery ends in the
  gallery.
