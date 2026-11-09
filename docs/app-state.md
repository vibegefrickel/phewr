# App state

This page holds the design facts of the background jobs and the caches of `src/app/`. AGENTS.md
holds the rules and the layout of the files.

## Thumbnail jobs

The code is in `app/thumbnails/`. `mod.rs` holds `ThumbnailJobs`, the scheduling, the job bounds
and `THUMB_CACHE_LIMITS`. `decode.rs` holds the work of one job. `pictures/jobs/queue.rs` holds
the queues and `Priority`, and `pictures/jobs/pool.rs` holds the pool of the cache reads.
`pictures/jobs/thumbnail_cache.rs` holds the memory cache.

### Slots and limits

- `LibheifSlot` counts the running libheif decodes. It gives each context a share of the tile
  threads.
- The fullscreen and zoom decodes run inside `foreground`. They take all cores.
- At most 4 thumbnail decodes (`MAX_THUMB_JOBS`) run in parallel.
  - 24 parallel decodes of 24 MP HEIC frames held 2 GB of pixels and ran 40 libheif threads on
    10 cores. The first thumbnail took 1 s.
- At most 2 prefetch decodes (`MAX_PREFETCH_JOBS`) run in parallel.
- At most 4 preview decodes (`MAX_PREVIEW_JOBS`) run in parallel.
  - The embedded thumbnail of an iPhone HEIC is 320x240. A preview decode takes a few
    milliseconds.
  - The preview phase also decodes the embedded JPEG of RAW and PSD files.
- At most 64 cache reads (`MAX_CACHE_READS`) run in parallel.
- The cell at the cursor decodes first, then the other cells of the screen in grid order.
  `render_gallery_images` spawns the cursor cell first and gives each job
  `Priority::on_screen(position, Some(cursor))`. A free slot starts a job at once, so the rank
  orders only the waiting jobs.
- The other cells wait in a queue. `JobQueue::push` puts a job behind each job with the same or a
  lower rank. `next_sharp_job` starts the cells that show nothing before the HEIC sharp jobs.
- A failed thumbnail decode sends `None` and frees its slot.
- The thumbnail and preview queues are `JobSlots` with an image index per job. A job whose index
  left the list starts nothing.
- `pictures::decode::spawn_foreground` runs a foreground decode on the thread pool and returns
  its receiver.

### Phases of a job

- `start_thumb_job` runs the phases of a cold cell.
- Preview phase: the job takes the embedded thumbnail at its own size.
- Sharp phase: the job decodes the file and writes the result into the disk cache.
- Cache read phase: the job only reads the disk cache, on its own threads. It sends `None` on a
  miss.
- `poll_thumbnails`: the sharp jobs wait until every preview is done. So a cold HEIC grid shows
  all its previews first.
- A waiting sharp decode does not start while the cache read of its cell runs.

### Cache reads

- A cell whose sharp job waits reads the disk cache ahead (`ThumbPhase::CacheRead`). So a hit
  never waits behind a cold decode.
- The cache reads run on their own pool (`cache_read_pool`) of at most 8 threads.
- `LibheifSlot` and `MAX_THUMB_JOBS` do not count these threads. A miss holds one only for a
  `stat`.
- The pool takes cores from a cold decode only for the hits of a mixed grid.
- A hit costs about 0.75 ms (a QOI read and decode). A disk cache read takes about 1 ms. So the
  run loop polls more often while one runs (`cache_reads_pending`).
- A hit drops the waiting jobs of its cell.
- A miss leaves the cell in the queue and in `ThumbnailJobs::cache_misses` (`App::thumb_jobs`).
  The later jobs of that cell skip their own cache lookup.

### Cache key and animation flag

- Each job computes its cache key (`CacheEntry`: one `stat` and one `realpath`) at most once, at
  its first use (`LazyEntry`). It reads and writes through the key.
- A HEIC preview after a missed cache read computes no key.
- The job reads the animation header only when `App::animated` holds no flag for the same file
  stamp (`ThumbnailJobs::flag_stamps`).
- A warm cell after a zoom reads no header. A changed file reads it again.

### Fields and calls

- `ThumbnailJobs::sharp` holds at most `MAX_THUMB_JOBS` running jobs. `previews` holds at most
  `MAX_PREVIEW_JOBS`. `cache_reads` holds at most `MAX_CACHE_READS`. The three bounds and
  `THUMB_CACHE_LIMITS` (512 MB, at least 200 entries) are in `app/thumbnails/mod.rs`.
- `park_thumbnails`: the folder switch calls it before it drops the image list.
- `warm_disk_cache`: the folder prefetch calls it.
- `pad_for_grid`, `pad_loop_frame` and `thumb_target_px` are in `pictures/resize/padding.rs`. The
  placement rules are in [Grid and Kitty](grid-and-kitty.md#padding-and-placement).

## Disk cache

- The code is in `pictures/jobs/disk_cache/`. `mod.rs` holds `FileStamp`, `CacheEntry`,
  `LazyEntry`, `thumbnail_dir_below` and the key. `write.rs` holds the temp file write and the
  prune.
- The module has no default folder. `app/disk_cache/mod.rs` holds `default_disk_cache_dir`, the
  folder of phewr (`~/Library/Caches/phewr`). The prune refuses a folder that holds a subfolder.
- The loop files are in `app/disk_cache/loop_file.rs`. The file is `<hash>.loop`. It holds QOI
  frames with their times.
- The key version of a loop file is `phewr-loop-v1` (`LOOP_CACHE_VERSION`). Change the version
  when the loop output changes.
- The prune removes the loop files before the stills. It removes each kind oldest first.

## Folder prefetch

- The rules are in `gotchas.md` ("Folder prefetch"). The code is in `app/folder_prefetch/`.
- `mod.rs` holds the state and the rest of the cursor. `worker.rs` holds the thread and the first
  screen.
- The start delay is 300 ms (`FOLDER_PREFETCH_DELAY`).
- The prefetch decodes on the global rayon pool, and its libheif decode counts in the share. On
  2026-11-03, a pool of 2 threads of its own gave the same grid time as main (2009 ms against
  2004 ms, median of 10 interleaved runs, `03-landscape` during `01-mixed-big`). One libheif tile
  thread per prefetch decode gave 1924 ms (4 %, inside the spread), and the prefetch of a 35-cell
  HEIC folder took 12.2 s instead of 3.9 s. The prefetch slows the cold grid by about 240 ms when
  it never pauses. The tested design is in the archived report of the worker `prefetch-pool`.

## Grid loops

The code is in `app/grid_loops/`. The frame making is in [Video](video.md#grid-loop-frames).

### Loop contents

- Every visible video and animated cell loops. The loop replaces the stamped still.
- A video takes 12 keyframes (`-skip_frame nokey`), 333 ms each.
- A clip of at most 20 s with fewer than 12 keyframes decodes 12 frames at a constant rate. A clip
  above 60 s seeks to 12 points.
- An animation keeps its frames and delays. It has at most 48 frames and 12 fps and at least one
  frame per minute.
- The frames are half the size of the grid picture. They are padded to the cells of the still
  (`pad_loop_frame`), and the terminal scales them.

### Two paths

The probe at the start or the config picks the path (`Loops` in `KittyBackend`).

- Animation path (Ghostty 1.4.0 and later):
  - The first frame is the root image (`a=t`). It has one id from 2^31 (`LOOP_ID_BASE`).
  - The further frames go out with `a=f,z=<ms>`.
  - `a=a,r=1,z=` sets the gap of the root frame. `a=a,s=3,v=1` loops forever.
  - `a=p` places the animation over the still. `d=i` removes the placement of the still. All
    this happens in one synchronized update (`transmit_animation_to` in
    `kitty_output/loops.rs`).
  - phewr sends nothing per tick. `loop_frames_due` and `grid_loop_timeout` skip an animation.
  - A hidden or replaced cell sends one `d=I`.
  - A grid redraw places the running animation again by its id.
  - A `d=A` (the help popup, the info panel, the fullscreen view, the zoom) makes the grid send
    the animation again with all frames.
- Ticks path (herdr and older terminals):
  - Each frame goes out once with `a=t` and an id from 2^31.
  - A tick sends `a=d,d=i` for the old frame and `a=p` for the new one (`play_grid_loops`). This
    costs about 75 bytes per cell, in one synchronized update.
  - All loops share one clock of 12 ticks per second (`TICKS_PER_SECOND`).
  - Each loop starts at a multiple of 4 ticks (`START_ALIGN`). So the frames of all cells change
    in the same pass of the run loop.
  - The files are `cell.rs` (`CellLoop`, `tick_ends`) and `mod.rs`.

### Jobs

- Only visible cells loop.
- A loop above `TERMINAL_IMAGE_BUDGET` keeps its still until the visible cells change.
- A cell that leaves the screen deletes its frames with `d=I`.
- Loop jobs start after all stills of the screen are out. At most 4 run (`MAX_LOOP_JOBS`), each on
  its own thread.
- Each ffmpeg run takes at most 2 decode threads (`-threads 2 -filter_threads 1`).
- New stills or another view cancel the running loop runs. The flag ends ffmpeg within 20 ms. The
  cells get new jobs later.
- A `ResultGuard` sends `Failed` for a job that panics.
- The help popup, the info panel and the other views stop the loops.
- `[gallery] animate = false` turns the loops off.
- In herdr, the loops take the ticks, and the animation probe does not go out (`chosen_loops`
  in `pictures/kitty/protocol/probe.rs`). herdr 0.9.5 answers the probe itself but plays no Kitty
  animation. `PHEWR_KITTY_LOOPS` and `[gallery] loops` win over herdr.
- With `[gallery] animate = false`, the help popup shows `loops: off`.
- `App::drop_hidden_loops`: the render of the grid calls it, also after a cursor move without a
  scroll.
- `App::loop_plays` and `App::pretend_loop_job` are test-only. The tests of the Kitty output read
  `loop_plays`. The tests of the run loop use `pretend_loop_job`.

### Bench and tests

- The bench `grid_loops` reads `07-video` and `06-animation` beside `PHEWR_TEST_DIR`. It needs
  ffmpeg on PATH. It measures both paths.
- `FakeTerminal` reads `a=t`, `a=f`, `a=a` and `d=i`.

## Fullscreen view

- `pre_decode_hovered` (the hover decode) passes the size hint. Without it, the decode came back
  at full size, and `take_resized` scaled a 24 MP frame on the UI thread. This took 50 ms to
  70 ms.
