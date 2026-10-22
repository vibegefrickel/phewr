//! The builders and helpers that the tests of more than one module share.

use crate::app::App;
use crate::config::Config;
use crate::image_list::SharedImageList;
use crate::keys::{handle_event, handle_event_at};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use image::RgbaImage;
#[cfg(feature = "heic")]
use libheif_rs::{
    Channel, ColorSpace, CompressionFormat, EncoderQuality, EncodingOptions, HeifContext, Image,
    ImageOrientation, LibHeif, RgbChroma,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// A temp directory with the subdirectory `synthetic-images`. The tree goes away on drop.
/// Both paths are canonical, because the picker canonicalizes its directories.
pub(crate) fn synthetic_tree() -> (TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let child = root.join("synthetic-images");
    fs::create_dir(&child).unwrap();
    (temp, root, child)
}

pub(crate) fn press(app: &mut App, code: KeyCode) -> bool {
    press_with(app, code, KeyModifiers::NONE)
}

pub(crate) fn press_with(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> bool {
    handle_event(app, &Event::Key(KeyEvent::new(code, modifiers))).unwrap()
}

pub(crate) fn char_key(app: &mut App, c: char) -> bool {
    press(app, KeyCode::Char(c))
}

/// A mouse event at the screen cell `(column, row)`, through the real dispatch, with `now` as
/// the clock of the double-click.
pub(crate) fn mouse_at(
    app: &mut App,
    kind: MouseEventKind,
    (column, row): (u16, u16),
    modifiers: KeyModifiers,
    now: Instant,
) -> bool {
    let event = Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers,
    });
    handle_event_at(app, &event, now).unwrap()
}

pub(crate) fn mouse(app: &mut App, kind: MouseEventKind, at: (u16, u16)) -> bool {
    mouse_at(app, kind, at, KeyModifiers::NONE, Instant::now())
}

/// A left click: the button goes down and up at `at`.
pub(crate) fn click_at(app: &mut App, at: (u16, u16), now: Instant) -> bool {
    mouse_at(
        app,
        MouseEventKind::Down(MouseButton::Left),
        at,
        KeyModifiers::NONE,
        now,
    ) || mouse_at(
        app,
        MouseEventKind::Up(MouseButton::Left),
        at,
        KeyModifiers::NONE,
        now,
    )
}

pub(crate) fn click(app: &mut App, at: (u16, u16)) -> bool {
    click_at(app, at, Instant::now())
}

/// The center of the visible gallery cell `vis`.
pub(crate) fn cell_center(app: &App, vis: usize) -> (u16, u16) {
    let cell = app.gallery.cell_rect(vis);
    (cell.x + cell.width / 2, cell.y + cell.height / 2)
}

/// A left click on the center of the visible gallery cell `vis`.
pub(crate) fn click_cell(app: &mut App, vis: usize, now: Instant) -> bool {
    let at = cell_center(app, vis);
    click_at(app, at, now)
}

/// A mouse event on the center of the visible gallery cell `vis`.
pub(crate) fn mouse_cell(app: &mut App, kind: MouseEventKind, vis: usize) -> bool {
    let at = cell_center(app, vis);
    mouse(app, kind, at)
}

/// Draw one frame into a test backend, so that the app records the screen layout.
pub(crate) fn draw_frame(app: &mut App, width: u16, height: u16) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| crate::ui::draw(frame, app, None))
        .unwrap();
}

/// Paths that do not exist: a decode fails in the background and writes no cache.
pub(crate) fn photo_paths(count: usize) -> Vec<PathBuf> {
    (0..count)
        .map(|index| PathBuf::from(format!("/photos/photo-{index:02}.jpg")))
        .collect()
}

/// A scanned gallery of `count` photos in a grid of 3 columns and 2 visible rows.
pub(crate) fn gallery_app_with(count: usize, config: &Config) -> App {
    let mut app = App::new_with_config((8, 16), SharedImageList::new(), config);
    app.images = photo_paths(count);
    app.filenames = app
        .images
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    app.gallery.filtered_indices = (0..count).collect();
    app.gallery.zoom_rows = 2;
    // 3:2 cells of 10 rows are 30 columns wide: 3 columns of 38 cells and 4 gap columns.
    app.gallery.update_grid(Rect::new(0, 0, 120, 24));
    app.scan_complete = true;
    app.needs_render = false;
    app
}

pub(crate) fn gallery_app(count: usize) -> App {
    gallery_app_with(count, &Config::default())
}

/// A scanned grid of the `folders` below `/photos` and then the `files`, with the cursor on
/// the first entry. The paths do not exist.
pub(crate) fn folder_app(folders: &[&str], files: &[&str]) -> App {
    let mut app = gallery_app(0);
    app.images = folders
        .iter()
        .chain(files)
        .map(|name| PathBuf::from("/photos").join(name))
        .collect();
    app.filenames = folders
        .iter()
        .chain(files)
        .map(|name| (*name).to_owned())
        .collect();
    app.folder_count = folders.len();
    app.gallery.filtered_indices = (0..app.images.len()).collect();
    app
}

pub(crate) fn fullscreen_app(count: usize, index: usize) -> App {
    let mut app = gallery_app(count);
    app.enter_fullscreen_at(index);
    app
}

pub(crate) fn text(out: &[u8]) -> String {
    String::from_utf8_lossy(out).into_owned()
}

pub(crate) const RED: [u8; 3] = [255, 0, 0];
pub(crate) const BLUE: [u8; 3] = [0, 0, 255];

/// Give each test its own temp directory; it goes away with the `TempDir`.
pub(crate) fn test_dir() -> TempDir {
    tempfile::tempdir().unwrap()
}

/// A checked-in file below `tests/fixtures`. The animated images there (`anim.gif`,
/// `anim.webp`, `anim.png`, `anim.avif`) hold 3 frames of 16x12 pixels, the stills one.
#[cfg(feature = "video")]
pub(crate) fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

pub(crate) fn assert_close(pixel: [u8; 4], expected: [u8; 3]) {
    let close = pixel
        .iter()
        .zip(expected)
        .all(|(&got, want)| got.abs_diff(want) < 60);
    assert!(close, "pixel {pixel:?}, expected {expected:?}");
}

pub(crate) fn app() -> App {
    App::new((8, 16), SharedImageList::new())
}

/// An app with the synthetic paths `names` in the list and the grid. The files do not exist.
pub(crate) fn app_with(names: &[&str]) -> App {
    let mut app = app();
    app.images = names.iter().map(PathBuf::from).collect();
    app.filenames = names.iter().map(|name| (*name).to_owned()).collect();
    app.gallery.filtered_indices = (0..names.len()).collect();
    app
}

/// An app with real PNG files `names` in `dir`.
pub(crate) fn app_with_files(dir: &Path, names: &[&str]) -> App {
    let mut app = app();
    for name in names {
        let path = dir.join(name);
        RgbaImage::new(8, 8).save(&path).unwrap();
        app.images.push(path);
        app.filenames.push((*name).to_owned());
    }
    app.gallery.filtered_indices = (0..names.len()).collect();
    app
}

/// Poll `done` every 10 ms for at most 2 s, so background work cannot hang a test.
pub(crate) fn wait_until(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !done() {
        assert!(Instant::now() < deadline, "background work did not finish");
        thread::sleep(Duration::from_millis(10));
    }
}

pub(crate) fn overdue() -> Instant {
    Instant::now()
        .checked_sub(Duration::from_millis(1))
        .unwrap()
}

#[cfg(feature = "heic")]
pub(crate) fn solid_image(width: u32, height: u32, rgb: [u8; 3]) -> Image {
    let mut image = Image::new(width, height, ColorSpace::Rgb(RgbChroma::Rgb)).unwrap();
    image
        .create_plane(Channel::Interleaved, width, height, 8)
        .unwrap();
    let planes = image.planes_mut();
    let plane = planes.interleaved.unwrap();
    for y in 0..height as usize {
        for x in 0..width as usize {
            let start = y * plane.stride + x * 3;
            plane.data[start..start + 3].copy_from_slice(&rgb);
        }
    }
    image
}

#[cfg(feature = "heic")]
pub(crate) fn options(orientation: ImageOrientation) -> EncodingOptions {
    let mut options = EncodingOptions::new().unwrap();
    options.set_image_orientation(orientation);
    options
}

#[cfg(feature = "heic")]
/// Write a red 256x128 primary image. A blue thumbnail of 64x32 is optional, with
/// its own orientation.
pub(crate) fn write_heif(
    path: &Path,
    format: CompressionFormat,
    orientation: ImageOrientation,
    thumbnail: Option<ImageOrientation>,
) {
    let lib_heif = LibHeif::new();
    let mut ctx = HeifContext::new().unwrap();
    let mut encoder = lib_heif.encoder_for_format(format).unwrap();
    encoder.set_quality(EncoderQuality::Lossy(90)).unwrap();
    let primary = ctx
        .encode_image(
            &solid_image(256, 128, RED),
            &mut encoder,
            Some(options(orientation)),
        )
        .unwrap();
    if let Some(thumbnail_orientation) = thumbnail {
        ctx.encode_thumbnail(
            &solid_image(256, 128, BLUE),
            &primary,
            64,
            &mut encoder,
            Some(options(thumbnail_orientation)),
        )
        .unwrap();
    }
    ctx.write_to_file(path.to_str().unwrap()).unwrap();
}

/// A terminal that reads the Kitty sequences of a test app: the stored images with their bytes,
/// the placements, and the eviction of unplaced images above a storage limit, as Ghostty does.
pub(crate) mod kitty {
    use std::collections::{BTreeMap, HashMap};

    /// One placement on the screen: the image, the cell of the cursor, and the cell box.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct Placement {
        pub id: u32,
        /// The cursor as `(column, row)`, 1-based like `CSI row ; column H`.
        pub cell: (u16, u16),
        pub cell_box: Option<(u16, u16)>,
    }

    #[derive(Debug)]
    pub(crate) struct FakeTerminal {
        /// The stored images by id, with their pixel bytes, in the order of their transmit.
        stored: BTreeMap<u64, (u32, usize)>,
        sequence: u64,
        pub placements: Vec<Placement>,
        /// The placements of ids that the terminal does not store: a black cell each.
        pub black_cells: Vec<u32>,
        pub limit: usize,
        cursor: (u16, u16),
        /// The open synchronized updates: `CSI ? 2026 h` adds one, `CSI ? 2026 l` ends one.
        pub synchronized_depth: i32,
    }

    impl FakeTerminal {
        pub(crate) fn with_limit(limit: usize) -> Self {
            Self {
                stored: BTreeMap::new(),
                sequence: 0,
                placements: Vec::new(),
                black_cells: Vec::new(),
                limit,
                cursor: (1, 1),
                synchronized_depth: 0,
            }
        }

        pub(crate) fn new() -> Self {
            Self::with_limit(usize::MAX)
        }

        pub(crate) fn stored_bytes(&self) -> usize {
            self.stored.values().map(|(_, bytes)| bytes).sum()
        }

        pub(crate) fn stored_ids(&self) -> Vec<u32> {
            self.stored.values().map(|(id, _)| *id).collect()
        }

        pub(crate) fn stores(&self, id: u32) -> bool {
            self.stored.values().any(|(stored, _)| *stored == id)
        }

        /// Read every sequence in `bytes`: the cursor moves, the Kitty commands and the
        /// synchronized updates.
        pub(crate) fn feed(&mut self, bytes: &[u8]) {
            let mut rest = bytes;
            while let Some(start) = rest.iter().position(|&byte| byte == 0x1b) {
                rest = &rest[start..];
                let consumed = if rest.starts_with(b"\x1b_G") {
                    self.kitty_command(&rest[3..])
                } else if rest.starts_with(b"\x1b[") {
                    self.csi(&rest[2..])
                } else {
                    0
                };
                rest = &rest[(consumed + 1).min(rest.len())..];
            }
        }

        /// The CSI `body` after `ESC [`. Returns the length of the sequence after the `ESC`.
        fn csi(&mut self, body: &[u8]) -> usize {
            let Some(end) = body.iter().position(u8::is_ascii_alphabetic) else {
                return body.len();
            };
            let params = String::from_utf8_lossy(&body[..end]);
            match (params.as_ref(), body[end]) {
                ("?2026", b'h') => self.synchronized_depth += 1,
                ("?2026", b'l') => self.synchronized_depth -= 1,
                (_, b'H') => {
                    let mut fields = params.split(';').map(str::parse::<u16>);
                    if let (Some(Ok(row)), Some(Ok(column))) = (fields.next(), fields.next()) {
                        self.cursor = (column, row);
                    }
                }
                _ => {}
            }
            1 + end + 1
        }

        /// The Kitty command `body` after `ESC _ G`. Returns the length of the sequence after
        /// the `ESC`.
        fn kitty_command(&mut self, body: &[u8]) -> usize {
            let end = body
                .windows(2)
                .position(|pair| pair == b"\x1b\\")
                .unwrap_or(body.len());
            let command = String::from_utf8_lossy(&body[..end]);
            let keys: HashMap<&str, &str> = command
                .split(';')
                .next()
                .unwrap_or_default()
                .split(',')
                .filter_map(|key| key.split_once('='))
                .collect();
            let number = |key: &str| keys.get(key).and_then(|value| value.parse::<u32>().ok());
            let cell_box = match (number("c"), number("r")) {
                (Some(cols), Some(rows)) => Some((cols as u16, rows as u16)),
                _ => None,
            };
            match keys.get("a").copied() {
                Some("T") => {
                    let id = number("i").unwrap_or(0);
                    let bytes = (number("s").unwrap_or(0) * number("v").unwrap_or(0) * 4) as usize;
                    self.store(id, bytes);
                    self.place(id, cell_box);
                }
                Some("p") => self.place(number("i").unwrap_or(0), cell_box),
                Some("d") => match keys.get("d").copied() {
                    Some("a") => self.placements.clear(),
                    Some("A") => {
                        self.placements.clear();
                        self.stored.clear();
                    }
                    Some("I") => {
                        let id = number("i").unwrap_or(0);
                        self.stored.retain(|_, (stored, _)| *stored != id);
                        self.placements.retain(|placement| placement.id != id);
                    }
                    _ => {}
                },
                _ => {}
            }
            2 + end + 2
        }

        /// Store an image. Above the limit, the oldest unplaced images go first, then the
        /// oldest placed ones. The new image stays.
        fn store(&mut self, id: u32, bytes: usize) {
            self.stored.retain(|_, (stored, _)| *stored != id);
            self.sequence += 1;
            self.stored.insert(self.sequence, (id, bytes));
            while self.stored_bytes() > self.limit {
                let placed = |id: u32, placements: &[Placement]| {
                    placements.iter().any(|placement| placement.id == id)
                };
                let others = self.stored.iter().filter(|(_, (stored, _))| *stored != id);
                let victim = others
                    .clone()
                    .find(|(_, (stored, _))| !placed(*stored, &self.placements))
                    .or_else(|| others.clone().next())
                    .map(|(sequence, _)| *sequence);
                let Some(victim) = victim else {
                    return;
                };
                self.stored.remove(&victim);
            }
        }

        fn place(&mut self, id: u32, cell_box: Option<(u16, u16)>) {
            if !self.stores(id) {
                self.black_cells.push(id);
                return;
            }
            self.placements.push(Placement {
                id,
                cell: self.cursor,
                cell_box,
            });
        }
    }

    #[test]
    fn the_fake_terminal_stores_places_and_deletes_like_the_protocol() {
        let mut terminal = FakeTerminal::new();
        terminal.feed(b"\x1b[?2026h\x1b[3;5H\x1b_Ga=T,q=2,C=1,f=32,s=2,v=1,i=7,m=1;AAAA\x1b\\\x1b_Gm=0;AAAA\x1b\\");
        assert_eq!(terminal.synchronized_depth, 1);
        assert_eq!(terminal.stored_bytes(), 8);
        assert_eq!(
            terminal.placements,
            vec![Placement {
                id: 7,
                cell: (5, 3),
                cell_box: None
            }]
        );

        terminal.feed(b"\x1b_Ga=d,d=a,q=2\x1b\\\x1b[1;1H\x1b_Ga=p,q=2,C=1,i=7,c=4,r=2\x1b\\\x1b_Ga=p,q=2,C=1,i=8\x1b\\\x1b[?2026l");
        assert_eq!(terminal.placements.len(), 1);
        assert_eq!(terminal.placements[0].cell_box, Some((4, 2)));
        assert_eq!(terminal.black_cells, vec![8]);
        assert_eq!(terminal.synchronized_depth, 0);

        terminal.feed(b"\x1b_Ga=d,d=I,i=7,q=2\x1b\\");
        assert!(!terminal.stores(7));
        assert!(terminal.placements.is_empty());

        terminal.feed(b"\x1b_Ga=T,q=2,C=1,f=32,s=1,v=1,i=9,m=0;AAAA\x1b\\\x1b_Ga=d,d=A,q=2\x1b\\");
        assert!(terminal.stored_ids().is_empty());
    }

    #[test]
    fn the_fake_terminal_evicts_unplaced_images_at_its_limit() {
        let mut terminal = FakeTerminal::with_limit(10);
        let transmit = |id: u32| format!("\x1b_Ga=T,q=2,C=1,f=32,s=1,v=1,i={id},m=0;AAAA\x1b\\");
        terminal.feed(format!("{}{}{}", transmit(1), transmit(2), transmit(3)).as_bytes());
        assert_eq!(terminal.stored_bytes(), 8);
        assert_eq!(terminal.stored_ids(), vec![2, 3], "the oldest image went");
        terminal.feed(b"\x1b_Ga=d,d=a,q=2\x1b\\");
        terminal.feed(transmit(4).as_bytes());
        assert_eq!(terminal.stored_ids(), vec![3, 4]);
        terminal.feed(b"\x1b_Ga=p,q=2,C=1,i=2\x1b\\");
        assert_eq!(terminal.black_cells, vec![2]);
    }
}

/// The fakes and the app builders of the video tests. No test starts a real mpv or ffmpeg.
#[cfg(feature = "video")]
pub(crate) mod video {
    use crate::app::{App, ViewMode};
    use crate::config::VideoPlayer;
    use crate::encoder::{KittyBackend, Transfer};
    use crate::image_list::SharedImageList;
    use crate::mpv;
    use crate::player::{Decoder, FrameStream, VideoInfo};
    use crate::run_loop::LoopState;
    use crate::video_handoff::VideoState;
    use std::cell::{Cell, RefCell};
    use std::ffi::OsString;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::mpsc;

    pub(crate) type Log = Rc<RefCell<Vec<String>>>;

    /// Records each mpv start. `fails` makes each start fail. `window` sends the exit code of
    /// the last window, so that the test closes it.
    pub(crate) struct FakeRunner {
        pub(crate) log: Log,
        pub(crate) fails: bool,
        pub(crate) window: Option<mpsc::Sender<Option<i32>>>,
    }

    impl mpv::Runner for FakeRunner {
        fn spawn(
            &mut self,
            program: &Path,
            args: &[OsString],
        ) -> io::Result<mpsc::Receiver<Option<i32>>> {
            let args: Vec<String> = args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            self.log
                .borrow_mut()
                .push(format!("spawn {} {}", program.display(), args.join(" ")));
            if self.fails {
                return Err(io::Error::from(io::ErrorKind::NotFound));
            }
            let (exit_tx, exit_rx) = mpsc::channel();
            self.window = Some(exit_tx);
            Ok(exit_rx)
        }
    }

    /// A runner that records its starts in the returned log.
    pub(crate) fn fake_runner() -> (Log, FakeRunner) {
        let log = Log::default();
        let runner = FakeRunner {
            log: Rc::clone(&log),
            fails: false,
            window: None,
        };
        (log, runner)
    }

    /// A decoder that records each ffmpeg start and feeds the frames that the test gives it.
    pub(crate) struct FakeDecoder {
        pub(crate) info: VideoInfo,
        pub(crate) fail_probe: Cell<bool>,
        pub(crate) fail_start: Cell<bool>,
        starts: RefCell<Vec<String>>,
        sender: RefCell<Option<mpsc::SyncSender<Vec<u8>>>>,
    }

    impl FakeDecoder {
        /// The command lines of the ffmpeg starts so far.
        pub(crate) fn starts(&self) -> Vec<String> {
            self.starts.borrow().clone()
        }

        /// The command line of the last ffmpeg start.
        pub(crate) fn last_args(&self) -> String {
            self.starts.borrow().last().cloned().unwrap()
        }

        /// Give the player one decoded frame.
        pub(crate) fn feed(&self, frame: Vec<u8>) {
            self.sender
                .borrow()
                .as_ref()
                .expect("no ffmpeg run to feed")
                .send(frame)
                .expect("the player dropped its stream");
        }

        /// End the current ffmpeg run, as the end of the clip does.
        pub(crate) fn end(&self) {
            self.sender.borrow_mut().take();
        }

        /// True while a player still reads the current run. The check sends an empty frame, so
        /// call it only at the end of a test.
        pub(crate) fn streaming(&self) -> bool {
            self.sender
                .borrow()
                .as_ref()
                .is_some_and(|sender| sender.try_send(Vec::new()).is_ok())
        }
    }

    impl Decoder for FakeDecoder {
        fn probe(&self, _video: &Path) -> io::Result<VideoInfo> {
            if self.fail_probe.get() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "no video stream",
                ));
            }
            Ok(self.info.clone())
        }

        fn start(
            &self,
            program: &Path,
            args: &[OsString],
            _frame_bytes: usize,
        ) -> io::Result<FrameStream> {
            let args: Vec<String> = args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            self.starts
                .borrow_mut()
                .push(format!("{} {}", program.display(), args.join(" ")));
            if self.fail_start.get() {
                return Err(io::Error::from(io::ErrorKind::NotFound));
            }
            let (frame_tx, frame_rx) = mpsc::sync_channel(64);
            *self.sender.borrow_mut() = Some(frame_tx);
            Ok(FrameStream::new(frame_rx, None))
        }
    }

    /// A fake decoder whose probe answers `info`.
    pub(crate) fn fake_decoder(info: VideoInfo) -> Rc<FakeDecoder> {
        Rc::new(FakeDecoder {
            info,
            fail_probe: Cell::new(false),
            fail_start: Cell::new(false),
            starts: RefCell::new(Vec::new()),
            sender: RefCell::new(None),
        })
    }

    /// The decoder as the player takes it.
    pub(crate) fn shared(decoder: &Rc<FakeDecoder>) -> Rc<dyn Decoder> {
        let decoder: Rc<FakeDecoder> = Rc::clone(decoder);
        decoder
    }

    /// The header of an SDR clip of 2 seconds without a rotation.
    pub(crate) fn info_of(width: u32, height: u32, fps: f64) -> VideoInfo {
        VideoInfo {
            width,
            height,
            rotation: 0,
            fps,
            duration: 2.0,
            hdr: None,
            ten_bit: false,
            animation: false,
            stream: 0,
        }
    }

    /// The header of an animated image of 3 frames at 10 frames per second: 0.3 seconds.
    pub(crate) fn animation_info() -> VideoInfo {
        VideoInfo {
            duration: 0.3,
            animation: true,
            ..info_of(64, 48, 10.0)
        }
    }

    /// The header of the test clip: 640x480 at 10 frames per second.
    pub(crate) fn clip_info() -> VideoInfo {
        info_of(640, 480, 10.0)
    }

    /// The loop state with `player` configured, mpv and ffmpeg on PATH, and `decoder`.
    pub(crate) fn state_with(player: VideoPlayer, decoder: Rc<FakeDecoder>) -> LoopState {
        LoopState {
            status: None,
            late_answers: crate::keys::LateAnswerFilter::default(),
            cell_size_from_terminal: false,
            mouse: true,
            mouse_captured: true,
            video: VideoState {
                player,
                mpv: Some(PathBuf::from("/bin/mpv")),
                ffmpeg: Some(PathBuf::from("/bin/ffmpeg")),
                decoder,
            },
        }
    }

    /// The loop state of the terminal player, with a fake decoder for the test clip.
    pub(crate) fn terminal_state() -> LoopState {
        state_with(VideoPlayer::Terminal, fake_decoder(clip_info()))
    }

    /// The loop state of the mpv window.
    pub(crate) fn window_state() -> LoopState {
        state_with(VideoPlayer::Window, fake_decoder(clip_info()))
    }

    /// Give `app` the temp file medium, which the terminal player needs.
    pub(crate) fn with_temp_file_medium(mut app: App) -> App {
        app.graphics = KittyBackend::new(Transfer::TempFile);
        app
    }

    /// An app with a photo and a video, in video mode for the video.
    pub(crate) fn video_app() -> App {
        gallery_app_on_video(&["a.jpg", "clip.mp4"], 1)
    }

    /// An app in the fullscreen view that steps from a photo onto a video.
    pub(crate) fn fullscreen_video_app() -> App {
        fullscreen_app_on_video(&["a.jpg", "clip.mp4", "b.jpg"], false)
    }

    /// An app in a running slideshow that steps from a photo onto a video.
    pub(crate) fn slideshow_video_app() -> App {
        fullscreen_app_on_video(&["a.jpg", "clip.mp4", "b.jpg"], true)
    }

    /// An app with `names` below `/videos` that steps from the first file onto the second,
    /// a video, in the fullscreen view. `slideshow` starts a slideshow on the first file.
    pub(crate) fn fullscreen_app_on_video(names: &[&str], slideshow: bool) -> App {
        let mut app = with_temp_file_medium(App::new((8, 16), SharedImageList::new()));
        app.images = names
            .iter()
            .map(|name| Path::new("/videos").join(name))
            .collect();
        app.filenames = names.iter().map(|name| (*name).to_owned()).collect();
        app.gallery.filtered_indices = (0..names.len()).collect();
        app.enter_fullscreen_at(0);
        if slideshow {
            app.start_slideshow();
        }
        app.next();
        assert_eq!(app.mode, ViewMode::Video);
        app
    }

    /// An app with `names` below `/videos` in the gallery, in the video view of `index`.
    pub(crate) fn gallery_app_on_video(names: &[&str], index: usize) -> App {
        let mut app = with_temp_file_medium(App::new((8, 16), SharedImageList::new()));
        app.images = names
            .iter()
            .map(|name| Path::new("/videos").join(name))
            .collect();
        app.filenames = names.iter().map(|name| (*name).to_owned()).collect();
        app.gallery.filtered_indices = (0..names.len()).collect();
        app.enter_video(index);
        app
    }

    /// The arguments of the last mpv start in `log`.
    pub(crate) fn last_start(log: &Log) -> String {
        starts(log).last().cloned().unwrap()
    }

    /// The mpv starts in `log`.
    pub(crate) fn starts(log: &Log) -> Vec<String> {
        log.borrow()
            .iter()
            .filter(|entry| entry.starts_with("spawn "))
            .cloned()
            .collect()
    }
}

/// A JPEG of `width` x `height` pixels with the left half red and the right half blue.
#[cfg(all(feature = "sips", target_os = "macos"))]
pub(crate) fn red_left_blue_right_jpeg(width: u32, height: u32) -> Vec<u8> {
    let mut image = image::RgbImage::new(width, height);
    for (x, _, pixel) in image.enumerate_pixels_mut() {
        *pixel = image::Rgb(if x < width / 2 { RED } else { BLUE });
    }
    encode_jpeg(&image)
}

/// A JPEG of `width` x `height` pixels in one color.
#[cfg(all(feature = "sips", target_os = "macos"))]
pub(crate) fn solid_jpeg(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
    encode_jpeg(&image::RgbImage::from_pixel(width, height, image::Rgb(rgb)))
}

#[cfg(all(feature = "sips", target_os = "macos"))]
fn encode_jpeg(image: &image::RgbImage) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 95)
        .encode_image(image)
        .unwrap();
    out.into_inner()
}

/// One IFD in little-endian TIFF: `(tag, type, count, value)` per entry. A SHORT (3) takes the
/// low two bytes of the value field.
#[cfg(all(feature = "sips", target_os = "macos"))]
fn tiff_ifd(entries: &[(u16, u16, u32, u32)], next: u32) -> Vec<u8> {
    let mut ifd = Vec::new();
    ifd.extend_from_slice(&u16::try_from(entries.len()).unwrap().to_le_bytes());
    for &(tag, kind, count, value) in entries {
        ifd.extend_from_slice(&tag.to_le_bytes());
        ifd.extend_from_slice(&kind.to_le_bytes());
        ifd.extend_from_slice(&count.to_le_bytes());
        if kind == 3 {
            ifd.extend_from_slice(&u16::try_from(value).unwrap().to_le_bytes());
            ifd.extend_from_slice(&[0, 0]);
        } else {
            ifd.extend_from_slice(&value.to_le_bytes());
        }
    }
    ifd.extend_from_slice(&next.to_le_bytes());
    ifd
}

/// `jpeg` with an APP1 EXIF segment after the SOI marker that holds `orientation`.
#[cfg(all(feature = "sips", target_os = "macos"))]
pub(crate) fn with_exif_orientation(jpeg: &[u8], orientation: u32) -> Vec<u8> {
    let mut exif = b"Exif\0\0".to_vec();
    exif.extend(tiff_with_orientation(orientation));
    let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
    out.extend_from_slice(&u16::try_from(exif.len() + 2).unwrap().to_be_bytes());
    out.extend(exif);
    out.extend_from_slice(&jpeg[2..]);
    out
}

/// A TIFF header `II*` with IFD0 at byte 8 and the orientation `orientation` in IFD0.
#[cfg(all(feature = "sips", target_os = "macos"))]
fn tiff_with_orientation(orientation: u32) -> Vec<u8> {
    let mut tiff = vec![b'I', b'I', 42, 0, 8, 0, 0, 0];
    tiff.extend(tiff_ifd(&[(0x112, 3, 1, orientation)], 0));
    tiff
}

/// A TIFF-based RAW file as a camera writes it: IFD0 holds a 32x16 JPEG as one strip and the
/// orientation, a `SubIFD` holds the 64x32 JPEG preview as `JPEGInterchangeFormat`, a second
/// `SubIFD` holds a lossless JPEG strip (the sensor data), and IFD1 holds a 16x8 JPEG thumbnail.
#[cfg(all(feature = "sips", target_os = "macos"))]
pub(crate) fn write_tiff_raw(path: &Path, orientation: u32) {
    let (small, preview, thumb) = (
        red_left_blue_right_jpeg(32, 16),
        red_left_blue_right_jpeg(64, 32),
        red_left_blue_right_jpeg(16, 8),
    );
    let lossless = [
        0xFF, 0xD8, 0xFF, 0xC3, 0, 11, 16, 0, 4, 0, 4, 1, 1, 0x11, 0, 0xFF, 0xD9,
    ];
    let mut file = vec![b'I', b'I', 42, 0, 0, 0, 0, 0];
    let push = |file: &mut Vec<u8>, data: &[u8]| -> u32 {
        let offset = u32::try_from(file.len()).unwrap();
        file.extend_from_slice(data);
        if file.len() % 2 == 1 {
            file.push(0);
        }
        offset
    };
    let off_small = push(&mut file, &small);
    let off_preview = push(&mut file, &preview);
    let off_thumb = push(&mut file, &thumb);
    let off_lossless = push(&mut file, &lossless);
    let len = |data: &[u8]| u32::try_from(data.len()).unwrap();
    // The IFDs come after the data: IFD0 (7 entries), the SubIFD offsets, 2 SubIFDs, IFD1.
    let ifd0 = u32::try_from(file.len()).unwrap();
    let sub_array = ifd0 + 2 + 7 * 12 + 4;
    let sub1 = sub_array + 8;
    let sub2 = sub1 + 2 + 2 * 12 + 4;
    let ifd1 = sub2 + 2 + 3 * 12 + 4;
    file[4..8].copy_from_slice(&ifd0.to_le_bytes());
    file.extend(tiff_ifd(
        &[
            (0x100, 4, 1, 32),
            (0x101, 4, 1, 16),
            (0x103, 3, 1, 6),
            (0x111, 4, 1, off_small),
            (0x112, 3, 1, orientation),
            (0x117, 4, 1, len(&small)),
            (0x14A, 4, 2, sub_array),
        ],
        ifd1,
    ));
    file.extend_from_slice(&sub1.to_le_bytes());
    file.extend_from_slice(&sub2.to_le_bytes());
    file.extend(tiff_ifd(
        &[(0x201, 4, 1, off_preview), (0x202, 4, 1, len(&preview))],
        0,
    ));
    file.extend(tiff_ifd(
        &[
            (0x103, 3, 1, 7),
            (0x111, 4, 1, off_lossless),
            (0x117, 4, 1, len(&lossless)),
        ],
        0,
    ));
    file.extend(tiff_ifd(
        &[
            (0x103, 3, 1, 6),
            (0x111, 4, 1, off_thumb),
            (0x117, 4, 1, len(&thumb)),
        ],
        0,
    ));
    fs::write(path, file).unwrap();
}

/// An ISOBMFF box: the size, the type and the body.
#[cfg(all(feature = "sips", target_os = "macos"))]
fn isobmff_box(kind: [u8; 4], body: &[u8]) -> Vec<u8> {
    let mut data = u32::try_from(body.len() + 8)
        .unwrap()
        .to_be_bytes()
        .to_vec();
    data.extend_from_slice(&kind);
    data.extend_from_slice(body);
    data
}

/// A Canon CR3-like file: `ftyp`, `moov` with the Canon `uuid` box (`CMT1` with the orientation,
/// `THMB` with a 16x8 JPEG), the preview `uuid` box with `PRVW` (a 64x32 JPEG) and an `mdat`.
#[cfg(all(feature = "sips", target_os = "macos"))]
pub(crate) fn write_cr3(path: &Path, orientation: u32) {
    write_cr3_with_preview(path, orientation, (64, 32));
}

/// `write_cr3` with a `PRVW` JPEG of `size` pixels. The benchmarks use the camera size 1620x1080.
#[cfg(all(feature = "sips", target_os = "macos"))]
pub(crate) fn write_cr3_with_preview(path: &Path, orientation: u32, size: (u32, u32)) {
    let thumb = red_left_blue_right_jpeg(16, 8);
    let preview = red_left_blue_right_jpeg(size.0, size.1);
    let mut thmb = vec![0, 0, 0, 0, 0, 16, 0, 8];
    thmb.extend_from_slice(&u32::try_from(thumb.len()).unwrap().to_be_bytes());
    thmb.extend_from_slice(&[0, 1, 0, 0]);
    thmb.extend_from_slice(&thumb);
    let mut canon = vec![0x85, 0xC0, 0xB6, 0x87, 0x82, 0x0F, 0x11, 0xE0];
    canon.extend_from_slice(&[0x81, 0x11, 0xF4, 0xCE, 0x46, 0x2B, 0x6A, 0x48]);
    canon.extend(isobmff_box(*b"CNCV", b"CanonCR3_001/00.09.00/00.00.00"));
    canon.extend(isobmff_box(*b"CMT1", &tiff_with_orientation(orientation)));
    canon.extend(isobmff_box(*b"THMB", &thmb));
    let moov = isobmff_box(*b"moov", &isobmff_box(*b"uuid", &canon));
    let mut prvw = vec![0, 0, 0, 0, 0, 1];
    prvw.extend_from_slice(&u16::try_from(size.0).unwrap().to_be_bytes());
    prvw.extend_from_slice(&u16::try_from(size.1).unwrap().to_be_bytes());
    prvw.extend_from_slice(&[0, 1]);
    prvw.extend_from_slice(&u32::try_from(preview.len()).unwrap().to_be_bytes());
    prvw.extend_from_slice(&preview);
    let mut preview_uuid = vec![0xEA, 0xF4, 0x2B, 0x5E, 0x1C, 0x98, 0x4B, 0x88];
    preview_uuid.extend_from_slice(&[0xB9, 0xFB, 0xB7, 0xDC, 0x40, 0x6E, 0x4D, 0x16]);
    preview_uuid.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
    preview_uuid.extend(isobmff_box(*b"PRVW", &prvw));
    let mut file = isobmff_box(*b"ftyp", b"crx \0\0\0\x01crx isom");
    file.extend(moov);
    file.extend(isobmff_box(*b"uuid", &preview_uuid));
    file.extend(isobmff_box(*b"mdat", &[0xFF; 64]));
    fs::write(path, file).unwrap();
}

/// A Fujifilm RAF-like file: the header names the offset and the length of a red 64x32 JPEG.
/// With `orientation`, the JPEG carries it in its own EXIF, as a Fujifilm camera writes it.
#[cfg(all(feature = "sips", target_os = "macos"))]
pub(crate) fn write_raf(path: &Path, orientation: Option<u32>) {
    let jpeg = solid_jpeg(64, 32, RED);
    let jpeg = match orientation {
        Some(orientation) => with_exif_orientation(&jpeg, orientation),
        None => jpeg,
    };
    let mut file = b"FUJIFILMCCD-RAW 0201FF129502".to_vec();
    file.resize(84, 0);
    file.extend_from_slice(&160_u32.to_be_bytes());
    file.extend_from_slice(&u32::try_from(jpeg.len()).unwrap().to_be_bytes());
    file.resize(160, 0);
    file.extend_from_slice(&jpeg);
    fs::write(path, file).unwrap();
}

/// A Photoshop file of 64x32 RGB pixels, red left and blue right, that sips reads. With
/// `thumbnail`, the image resources hold an `ImageReady` `MeSa` resource and then the JPEG
/// thumbnail 1036.
#[cfg(all(feature = "sips", target_os = "macos"))]
pub(crate) fn write_psd(path: &Path, thumbnail: bool) {
    let (width, height) = (64_u32, 32_u32);
    let mut file = b"8BPS\0\x01\0\0\0\0\0\0\0\x03".to_vec();
    file.extend_from_slice(&height.to_be_bytes());
    file.extend_from_slice(&width.to_be_bytes());
    file.extend_from_slice(&[0, 8, 0, 3]);
    file.extend_from_slice(&[0, 0, 0, 0]);
    let mut resources = Vec::new();
    if thumbnail {
        let jpeg = red_left_blue_right_jpeg(width, height);
        let mut body = Vec::new();
        for value in [1, width, height, width * 3, width * 3 * height] {
            body.extend_from_slice(&value.to_be_bytes());
        }
        body.extend_from_slice(&u32::try_from(jpeg.len()).unwrap().to_be_bytes());
        body.extend_from_slice(&[0, 24, 0, 1]);
        body.extend_from_slice(&jpeg);
        // A resource with another signature and an odd size comes first.
        resources.extend_from_slice(b"MeSa");
        resources.extend_from_slice(&7000_u16.to_be_bytes());
        resources.extend_from_slice(&[0, 0, 0, 0, 0, 3, 1, 2, 3, 0]);
        resources.extend_from_slice(b"8BIM");
        resources.extend_from_slice(&1036_u16.to_be_bytes());
        resources.extend_from_slice(&[0, 0]);
        resources.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
        resources.extend_from_slice(&body);
        if body.len() % 2 == 1 {
            resources.push(0);
        }
    }
    file.extend_from_slice(&u32::try_from(resources.len()).unwrap().to_be_bytes());
    file.extend_from_slice(&resources);
    file.extend_from_slice(&[0, 0, 0, 0]);
    file.extend_from_slice(&[0, 0]);
    for channel in 0..3 {
        for _y in 0..height {
            for x in 0..width {
                let rgb = if x < width / 2 { RED } else { BLUE };
                file.push(rgb[channel]);
            }
        }
    }
    fs::write(path, file).unwrap();
}

/// A TIFF-based RAW file whose IFD0 names one JPEG preview of `size` pixels, in one color. The
/// benchmarks measure the decode of a full-size preview with it.
#[cfg(all(feature = "sips", target_os = "macos"))]
pub(crate) fn write_tiff_raw_with_preview(path: &Path, size: (u32, u32)) {
    let jpeg = solid_jpeg(size.0, size.1, RED);
    let mut file = vec![b'I', b'I', 42, 0, 8, 0, 0, 0];
    let ifd = tiff_ifd(
        &[
            (0x112, 3, 1, 1),
            (0x201, 4, 1, 0),
            (0x202, 4, 1, u32::try_from(jpeg.len()).unwrap()),
        ],
        0,
    );
    let offset = u32::try_from(8 + ifd.len()).unwrap();
    file.extend(ifd);
    // The offset of the JPEG is the value of the second entry: 2 + 12 + 8 bytes into the IFD.
    file[8 + 2 + 12 + 8..8 + 2 + 12 + 12].copy_from_slice(&offset.to_le_bytes());
    file.extend_from_slice(&jpeg);
    fs::write(path, file).unwrap();
}
