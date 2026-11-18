# Live Photos

This page holds the facts and the choices for the pairing of an iPhone Live Photo. A Live Photo
is a still (HEIC or JPEG) and a short `.mov` with the same base name. phewr has no pairing
today. The grid lists the still and the movie as two cells. The maintainer decides the design
from this page.

## Summary

- The 9 iPhone folders of the maintainer (`~/Pictures/iphone-YYYYMMDD/`, 4002 files) hold no
  Live Photo. No still has a `.mov` partner, and no file carries a content identifier.
- phewr can read both identifiers in safe Rust with the crates that it has today. A read costs
  0.02 ms to 0.05 ms per file from the page cache.
- A base-name pass over the sorted list costs 0.11 ms for 10 000 names. It fits on the scan
  thread before the first batch.
- An identifier pass costs 0.2 s to 2.3 s for a folder of 10 000 files. It cannot run before
  the first batch.
- The recommendation: pair by base name at the scan, hide the `.mov`, mark the still, and play
  the motion on Enter in the fullscreen view. When the motion plays, phewr reads the
  identifiers.
- Built on 2026-11-03 (`4e2d22b`): choice B and choice C. The maintainer decided the questions
  3 to 8 with the recommended answers, then asked for the grid roll of choice C. The roll is on
  whenever the grid loops are on (`[gallery] animate`), with no key of its own. A Live still
  gets no play stamp with `animate = false`, a click on the middle of the picture plays like
  Enter, and a JPEG twin of a HEIC pair stays a plain cell (maintainer, 2026-11-03).
- Question 2, partly: `~/Pictures/phewr-test/12-live-photos/` holds real iPhone stills (HEIC
  and one JPEG) and copies of one real iPhone 14 Pro Live Photo movie with the identifier of
  each still. phewr reads all 11 identifiers like exiftool, so the tag places hold for HEIC and
  for JPEG. No pair there comes from a camera as a pair. Question 1 stays open.
- Question 7 (maintainer, 2026-11-09): an edited copy `IMG_E1234.HEIC` pairs only with
  `IMG_E1234.MOV`, as the code does (`edited_copy_pairs_with_own_motion`). The original still
  keeps its own motion.

## The real folders

The measurement ran on 2026-11-03 (`research/live-photos/measure.sh`). The folders hold these
files:

| Type | Files | Note |
|---|---|---|
| HEIC | 3374 | 3369 with the Apple tags 0x0017 and 0x002b |
| JPEG | 100 | 71 with the Apple tags 0x0017 and 0x002b |
| MOV | 284 | 6 shorter than 4 s, median 33 s |
| MP4, PNG, WebP, GIF | 115 | screenshots and files of other apps |
| AAE | 129 | the edit files of the Photos app, phewr lists none |

The counts of the pairs:

- Stills with a `.mov` of the same base name: 0.
- Stills with the content identifier (Apple tag 0x0011): 0 of 3474.
- Movies with the key `com.apple.quicktime.content.identifier`: 0 of 284.
- Name pairs without the identifier: 0. Identifiers without a name pair: 0. The folders hold
  neither case, because they hold no identifier.

What the files show:

- The folders hold the originals, 65 edited copies (`IMG_E1234.HEIC`, 38 HEIC, 26 JPEG, 1 MOV)
  and 129 AAE files. This is the layout of an import that keeps the originals and the edits.
- 3440 of 3474 stills carry the tag 0x0017 (`LivePhotoVideoIndex`). The exiftool table says
  that this tag needs the tag 0x0011 to be valid. So 0x0017 alone does not mark a Live Photo.
- The stills keep their full MakerNote. A transfer that removes only the tag 0x0011 is not
  likely. The likely cause is that Live Photo was off in the Camera app. The files do not prove
  it.
- The numbers of the stills and the movies interleave (`IMG_7693.HEIC`, `IMG_7698.MOV`). The
  camera uses one counter for photos and videos. A plain video of the same phone never takes the
  base name of a still.
- `iphone-20251123` spans the counter wrap from `IMG_9999` to `IMG_0001`. It holds no base-name
  collision.

No real Live Photo was on this machine. The Photos library holds no originals. For this reason,
the probe read synthetic files (see "Where the identifier sits").

## Where the identifier sits

### The still

- The identifier is the ASCII tag 0x0011 (`ContentIdentifier`) of the Apple MakerNote. The
  value is a UUID as text.
- The MakerNote is the EXIF tag 0x927C of the Exif IFD. A HEIC file holds the EXIF in its
  `Exif` item, and a JPEG file holds it in APP1.
- The Apple note starts with `Apple iOS\0`, a version of 2 bytes and the byte order `MM`. A
  big-endian IFD follows at byte 14. Its offsets count from the start of the note.
- kamadak-exif 0.6 reads both containers with `Reader::read_from_container`. It returns the
  MakerNote as `Value::Undefined` and does not parse it. `info.rs` uses this reader today.
- The parse of the Apple IFD is about 25 lines of safe Rust (`apple_tag` in
  `research/live-photos/probe/src/main.rs`). It needs no new dependency.
- The same parse read the tag 0x002b (`PhotoIdentifier`) of all 3474 real stills. The result
  matched exiftool for 3474 of 3474 files.
- The tag 0x0017 holds the time of the still inside the movie, in units of `RunTimeScale`. The
  pairing does not need it.

### The movie

- The identifier is the `mdta` key `com.apple.quicktime.content.identifier` in `moov/meta`.
- `moov/meta` holds `hdlr` (handler `mdta`), `keys` and `ilst`. The `keys` box lists the key
  names. An `ilst` item has the 1-based key index as its box type.
- The item holds a `data` box: a type of 4 bytes (1 is UTF-8), a locale of 4 bytes, then the
  value.
- The QuickTime `meta` box has no version and no flags. The ISO `meta` box of `heif_boxes.rs`
  has 4 bytes of them. A reader must accept both forms.
- The Live Photo movie also has a timed metadata track with
  `com.apple.quicktime.still-image-time`. The pairing does not need it.
- All 284 real movies put `moov` after `mdat`. The reader seeks over `mdat` at the top level,
  like `read_meta` in `heif_boxes.rs`. It reads only the `moov` box.
- ffprobe shows the key as a format tag:
  `-show_entries format_tags=com.apple.quicktime.content.identifier`.

### Synthetic test files

- `research/live-photos/make-synthetic.sh` copies a real HEIC, a real JPEG and a real 2.3 s
  iPhone clip into `synthetic/`. exiftool writes one identifier into each copy of the movie.
- The box reader of the probe and ffprobe read the identifier from both movies.
- exiftool cannot add a new tag to a MakerNote. For this reason, the synthetic stills carry no
  tag 0x0011. The probe read the tag 0x002b of the same IFD instead.
- One real Live Photo pair is necessary to make sure that the iPhone writes the tags as the
  exiftool tables say.

### Cost per file

The numbers are medians over all real files of the folders. "Repeat" is the second run, from
the page cache. "First" is the first run of the probe in this session. This page holds no cold
read after a reboot.

| Way | Still | Movie | Repeat | First | Dependency |
|---|---|---|---|---|---|
| kamadak-exif and the Apple IFD | yes | no | 0.03 ms to 0.05 ms | 0.35 ms | in phewr today |
| box reader for `moov/meta` | no | yes | 0.02 ms to 0.04 ms | 0.35 ms | none |
| ffprobe format tag, own process | no | yes | 31 ms | 31 ms | ffprobe on PATH |
| ffprobe format tag, in the probe of `player/probe/` | no | yes | 0 ms extra | 0 ms extra | ffprobe on PATH |
| exiftool | yes | yes | 22 ms | 22 ms | Perl, not an option |

- The probe of `player/probe/` runs one ffprobe of about 31 ms per video. 20 runs with and
  without the extra entry `format_tags=com.apple.quicktime.content.identifier` took 0.62 s
  each. The entry costs no measurable time.
- The meta thread (`app/meta.rs`) reads the EXIF of the still under the cursor for the date
  (`info::date_taken`). The tag 0x0011 can come from the same read.

### Is the base name enough?

- For one iPhone and one import, yes. The camera counter gives a plain video no name of a still,
  and the folders hold 0 collisions in 284 movies.
- The rule fails in these cases. None of them is in the real folders:
  - Two phones or two cameras write into one folder.
  - The counter runs past `IMG_9999` two times, and one folder holds both runs.
  - A tool renames the files on export.
- A failure hides a real video behind a still. The user can still play it with the Live Photo
  key, so no file becomes unreachable.

## Scan cost

- Today, one folder argument is one `read_dir` and one sort. The first batch waits for the sort
  of the whole folder: 6.3 ms for 10 000 files (`gotchas.md`, "Entry kinds").
- The pairing pass of the probe runs after the sort, on the sorted names:

| Pass | 1000 names | 10 000 names |
|---|---|---|
| `HashMap` of the lowercase stems | 0.18 ms | 1.84 ms |
| a scan of the 3 sorted neighbors of each `.mov` | 0.012 ms | 0.108 ms |

- A still and its `.mov` sort near each other, because `name_key` sorts by the lowercase full
  name. Only a name with the same stem and an extension between `heic` and `mov` can sort
  between them.
- An example: `IMG_1234.HEIC`, `IMG_1234.JPG`, `IMG_1234.MOV` sort in this order. When a tool
  exports a HEIC still as JPEG next to the original, a folder holds such a set.
- `IMG_1234.edited.heic` sorts before `IMG_1234.heic` (`e` before `h`), not between the still
  and the `.mov`.
- The neighbor pass looks 3 names back, so it finds the HEIC still also behind a JPEG twin and
  one more name, like `IMG_1234.live.heic`.
- The neighbor pass adds about 2 % to the first batch of a folder of 10 000 files. It can run on
  the scan thread before the first batch.
- An identifier pass reads two files per pair. Take a folder of 10 000 files with 3334 pairs,
  on one thread. The pass costs 0.2 s to 0.3 s from the page cache and 2.3 s on the first read.
- Such a pass must run on a thread after the first batch. It reads every pair of the folder,
  not only the visible cells. It competes with the thumbnail decodes for the disk.
- A pass that hides a `.mov` after the first batch changes the list under the user. The
  indices of the cursor, the thumbnail jobs and the grid loops change. The marks survive,
  because they are paths.
- So only the base-name rule keeps the cell list stable from the first batch.
- The folder prefetch must pair with the same function. It computes the first screen from
  `scanner::folder_listing`, and a different list gives a different first screen.

### Other start paths

- Several file arguments stay a plain file list, without the folder sort (`scan_paths`).
  Choice B keeps every file of such a list as a cell. The user named each file, so phewr
  hides none.
- One file argument turns into its folder and a cursor target (`cli::start_paths`). yazi
  starts phewr like this (`phewr %h`). If the target is a hidden `.mov`, the target must
  map to its still. `App::place_cursor_after_scan` and the `return_to` of Backspace then
  point at the still. Open question 8 holds the options.

## The choices

### A: keep both cells (today)

- Cost: no code.
- Cell count: a folder of Live Photos shows two cells per photo.
- Grid loops: each visible `.mov` cell starts a first-frame ffmpeg run (about 180 ms to 260 ms,
  `video.md`) and a grid loop job. Half of the screen loops.
- A clip of 3 s has fewer than 12 keyframes. So its loop decodes at a constant rate. This takes
  0.4 s to 1 s for 3 s to 10 s of 4K (`video.md`, "Grid loop frames").
- Folder prefetch: it decodes the first frame of each movie of the first screen too.
- Slideshow: it shows the still, then plays the motion one time.
- Search, marks and the status row: they treat the two files as two files.

### B: hide the `.mov`, mark the still, play the motion on Enter (recommended)

- The scan pairs by base name and leaves the `.mov` out of the list. A side table maps the
  still to its movie.
- The fullscreen view of a Live Photo still plays the motion on Enter, one time, in the terminal
  player. Then it returns to the still. Today, Enter on a still does nothing.
- The player needs changes for this:
  - Outside a slideshow, the player loops each clip today. The motion needs a run that plays
    one time (`Repeat::Never`, today only for a slideshow).
  - `leave_player` always goes to the grid today (`App::exit_video`). The end of the motion
    and `q` must go back to the fullscreen view of the still.
  - The player, `App::video_return` and the mpv steps work on list indices (`App::current`).
    (`shows_video_placeholder` was in this list until 2026-11-18.) The hidden `.mov` has no index. The player must take the path
    of the movie from the side table, and `current` stays on the still.
  - With `current` on the still, `step_from_video` (`l` and `h` in mpv) steps from the still.
    This is the correct step for a pair.
- A click on the middle of the picture does the same, for the mouse. Today, that click does
  nothing. `p` opens the motion in the mpv window.
- The status row shows one more fact for the still: `Live`. The side table gives it at no
  cost. The length of the motion needs one ffprobe of about 31 ms per still under the cursor.
  For this reason, the status row shows the length only after the motion played once, from the
  probe cache.
- A mark on the cell: the picture covers the text of its cell. A symbol can go into the gap
  above the cell, where the border lives. The grid has no label row since 2026-11-17.
- Cost: the scan pass (0.11 ms per 10 000 names), the side table, the mark, and the player
  changes above. When the motion plays, phewr reads the identifiers. The probe adds the
  format tag, and the meta thread reads the tag 0x0011.
- Speed: the cell count of a folder of Live Photos halves. The grid starts no ffmpeg for the
  motions, and the folder prefetch decodes only the stills. A Live Photo cell costs what a
  still costs.
- Marks: a mark on the still stands for the pair. Marks only count today.
- Search: the `.mov` has the same stem, so the search finds the same photos.
- Slideshow: it shows the still only. A config key can play the motion one time.
- Prestart: a rest on the still in the fullscreen view can start the player ahead of Enter,
  like the prestart of `video_handoff/prestart/`.

### C: B, and loop the motion in the grid

- Every Live Photo cell gets a grid loop of its motion.
- With Live Photo on in the Camera app, almost every photo is a Live Photo. Then almost every
  cell loops. Each cell costs a loop job of 0.4 s to 1 s. Its 12 frames take 3 times the bytes
  of the still in the terminal (`video.md`, "Grid loop frames").
- This works against the speed goal. It can be a config key that is off by default.

### D: B, and play the motion on a rest of the grid cursor

- When the cursor rests on a Live Photo cell, the grid loops the motion of that cell only.
- One loop job takes 0.4 s to 1 s, so the motion shows late. The grid has no player inside a
  cell today. The grid loop of one cell is the only path.
- Cost: medium. Effect: one ffmpeg run per rest, and a cell that changes under the cursor.

### E: keep both cells, but order and mark the `.mov`

- The `.mov` cell gets a mark and no grid loop. It shows its first frame only.
- Cost: low. The cell count stays doubled, and the first-frame runs stay.

### Recommendation

Choice B, provisional until open question 2 has an answer. These facts support it:

- It removes work: half the cells, no first frame and no loop job for the motions.
- It keeps the first batch stable. The name pass costs 0.11 ms per 10 000 names on the scan
  thread.
- It uses the terminal player with a run that plays one time and a return to the still. A clip
  of 2 s to 3 s plays in real time and drops frames, never in slow motion.
- It needs no `unsafe` code and no new dependency. The identifier reads cost 0.05 ms or less
  per file. They run only at the start of the motion.
- The tag places and the read cost come from the exiftool tables, synthetic movies and the tag
  0x002b. A real pair can change them.
- It follows the Photos app: the still shows, and a press plays the motion.

## As built

- The pair rule: the 3 names before each `.mov`, HEIC and HEIF before JPEG, the stems compare
  without regard to ASCII case, one motion per still, and only `.mov` is a motion.
- The cost (`just bench-live`, 2026-11-04, load below 4): the pairing takes 0.17 ms (0.12 ms to
  0.20 ms) for 3334 pairs, 3.2 % of the first entry, below the 5 % limit. Without pairs, it
  takes about 0 ms. The share of the end of the scan (1.7 %) is lower, because the scan pushes
  6669 entries instead of 10 002. The design estimated 0.108 ms for the neighbor pass.
- The cold grid of the real folder: every Live cell loops after 1435 ms with Kitty animations
  and after 1595 ms with ticks.
- The identifier read runs on the UI thread at the start of the motion, not in the probe and not
  on the meta thread. It costs 0.04 ms to 0.6 ms per real file. A `moov` box above 1 MB counts as
  no Live Photo, and phewr reads only its header. The largest box below the cap (420 kB) takes
  0.61 ms on the first read. A missing identifier counts as a match.
- Choice C: the loop of the real 2.83 s motion takes 0.59 s (a constant-rate run, 12 frames).
  Each Live cell costs one loop job, and the cell count stays halved.
- The Live mark sits in the gap above the cell (`◎ Live`).
- The status row shows the motion length (`Live 2.8 s`, `7a62965`). The meta thread takes it
  from the probe cache, else a second thread runs one ffprobe. A failed probe gets no retry.
- A rest of 200 ms in the fullscreen view starts the motion ahead of Enter (`b82f164`). The first
  frame comes about 0.1 ms after Enter instead of about 123 ms.
- Not built: choice D, the motion in the slideshow, and a key of its own for the roll.

## Open questions

1. Is Live Photo on in the Camera app?
   - a. No. Then the real folders gain nothing, and the feature waits for a real pair.
   - b. Yes, but the import drops the movies. Then the import tool is the problem, not phewr.
2. Can the maintainer add one real Live Photo pair to `~/Pictures/phewr-test/`? One HEIC pair
   and one JPEG pair ("Most Compatible") are necessary to make sure that the tags sit as this
   page says.
3. Which choice?
   - a. B (recommended).
   - b. B with C as a config key that is off by default.
   - c. A: no pairing, the `.mov` stays a cell.
4. The identifiers of a name pair differ. What does phewr do?
   - a. phewr plays the motion and shows a note in the status row (recommended).
   - b. phewr shows the `.mov` as its own cell after the next directory switch.
   - c. phewr ignores the identifier and trusts the base name.
5. Does the slideshow play the motion?
   - a. No, it shows the still only (recommended).
   - b. Yes, one time after the still, like a video today.
6. Is there a config key to turn the pairing off?
   - a. Yes: `[gallery] live_photos = false` shows both cells (recommended).
   - b. No.
7. Does an edited copy (`IMG_E1234.HEIC`) pair with `IMG_E1234.MOV` only, or also with
   `IMG_1234.MOV`? The folders hold 65 edited copies and no edited Live Photo.
8. One file argument names a `.mov` that has a still (yazi `phewr %h`). Where does the
   cursor go?
   - a. To the still of the pair (recommended).
   - b. The `.mov` stays a cell for this start.

## Measurement files

`research/` is in `.gitignore`. The files stay on the machine of the maintainer.

- `research/live-photos/measure.sh`: runs every measurement of this page again. It reads the
  iPhone folders only. It takes about 90 s, most of it in exiftool.
- `research/live-photos/probe/`: the Rust program of the readers and the pairing passes
  (`still`, `photo-id`, `movie`, `names`, `synthetic`). It is not a member of the phewr package.
- `research/live-photos/make-synthetic.sh`: the synthetic pairs in `synthetic/`.
- `research/live-photos/exiftool-ids.csv`: the Apple tags of every file, from exiftool.
