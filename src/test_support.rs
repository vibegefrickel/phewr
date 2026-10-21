//! The builders and helpers that the tests of more than one module share.

use crate::app::App;
use crate::config::Config;
use crate::image_list::SharedImageList;
use crate::keys::handle_event;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use image::RgbaImage;
#[cfg(feature = "heic")]
use libheif_rs::{
    Channel, ColorSpace, CompressionFormat, EncoderQuality, EncodingOptions, HeifContext, Image,
    ImageOrientation, LibHeif, RgbChroma,
};
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

/// The fakes and the app builders of the mpv hand-off tests. No test starts a real mpv.
#[cfg(feature = "video")]
pub(crate) mod video {
    use crate::app::{App, ViewMode};
    use crate::image_list::SharedImageList;
    use crate::mpv;
    use crate::run_loop::LoopState;
    use crate::video_handoff::{TerminalHandoff, VideoState};
    use std::cell::{Cell, RefCell};
    use std::ffi::OsString;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::mpsc;

    pub(crate) type Log = Rc<RefCell<Vec<String>>>;

    /// A terminal that records each step. `raw` is the raw mode that the runner sees.
    pub(crate) struct FakeTerminal {
        pub(crate) log: Log,
        pub(crate) raw: Rc<Cell<bool>>,
        pub(crate) fail_suspend: bool,
        pub(crate) resized: bool,
    }

    impl TerminalHandoff for FakeTerminal {
        fn geometry(&self, _cell_px: (u32, u32)) -> Option<mpv::Geometry> {
            None
        }

        fn longest_side(&self, _cell_px: (u32, u32)) -> u32 {
            1600
        }

        fn suspend(&mut self, _app: &mut App) -> io::Result<()> {
            self.log.borrow_mut().push("suspend".to_owned());
            if self.fail_suspend {
                return Err(io::Error::other("broken pipe"));
            }
            Ok(())
        }

        fn ensure_raw_mode(&mut self) -> io::Result<()> {
            self.log.borrow_mut().push("raw mode".to_owned());
            self.raw.set(true);
            Ok(())
        }

        fn resume(&mut self, _app: &mut App) -> io::Result<()> {
            self.log.borrow_mut().push("resume".to_owned());
            Ok(())
        }

        fn discard_input(&mut self) -> io::Result<bool> {
            self.log.borrow_mut().push("discard input".to_owned());
            Ok(self.resized)
        }
    }

    /// Records each start; `exit` is the exit code of mpv or the error of the start.
    /// `window` sends the exit code of the last window, so that the test closes it.
    /// `exits` holds the exit codes of the next runs in the terminal; `exit` comes after.
    pub(crate) struct FakeRunner {
        pub(crate) log: Log,
        pub(crate) raw: Rc<Cell<bool>>,
        pub(crate) exit: Result<Option<i32>, io::ErrorKind>,
        pub(crate) exits: Vec<Option<i32>>,
        pub(crate) window: Option<mpsc::Sender<Option<i32>>>,
    }

    impl FakeRunner {
        fn record(&self, verb: &str, program: &Path, args: &[OsString]) -> io::Result<()> {
            let args: Vec<String> = args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            let mode = if self.raw.get() { "raw" } else { "cooked" };
            self.log.borrow_mut().push(format!(
                "{verb} ({mode}) {} {}",
                program.display(),
                args.join(" ")
            ));
            self.exit.map(drop).map_err(io::Error::from)
        }
    }

    impl mpv::Runner for FakeRunner {
        fn run(&mut self, program: &Path, args: &[OsString]) -> io::Result<Option<i32>> {
            self.record("run", program, args)?;
            if !self.exits.is_empty() {
                return Ok(self.exits.remove(0));
            }
            Ok(self.exit?)
        }

        fn spawn(
            &mut self,
            program: &Path,
            args: &[OsString],
        ) -> io::Result<mpsc::Receiver<Option<i32>>> {
            self.record("spawn", program, args)?;
            let (exit_tx, exit_rx) = mpsc::channel();
            self.window = Some(exit_tx);
            Ok(exit_rx)
        }
    }

    /// An app with a photo and a video, in video mode for the video.
    pub(crate) fn video_app() -> App {
        let mut app = App::new((8, 16), SharedImageList::new());
        app.images = vec![
            PathBuf::from("/videos/a.jpg"),
            PathBuf::from("/videos/clip.mp4"),
        ];
        app.filenames = vec!["a.jpg".to_owned(), "clip.mp4".to_owned()];
        app.gallery.filtered_indices = vec![0, 1];
        app.enter_video(1);
        app
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
        let mut app = App::new((8, 16), SharedImageList::new());
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

    pub(crate) fn state(player: mpv::Player) -> LoopState {
        LoopState {
            status: None,
            video: VideoState {
                player,
                fallback_note: None,
            },
        }
    }

    /// The fakes start in cooked mode, so the log shows that the hand-off turns raw mode on.
    pub(crate) fn fakes(
        exit: Result<Option<i32>, io::ErrorKind>,
    ) -> (Log, FakeTerminal, FakeRunner) {
        let log = Log::default();
        let raw = Rc::new(Cell::new(false));
        let terminal = FakeTerminal {
            log: Rc::clone(&log),
            raw: Rc::clone(&raw),
            fail_suspend: false,
            resized: false,
        };
        let runner = FakeRunner {
            log: Rc::clone(&log),
            raw,
            exit,
            exits: Vec::new(),
            window: None,
        };
        (log, terminal, runner)
    }

    pub(crate) fn terminal_state() -> LoopState {
        state(mpv::Player::Terminal(PathBuf::from("/bin/mpv")))
    }

    pub(crate) fn window_state() -> LoopState {
        state(mpv::Player::Window(PathBuf::from("/bin/mpv")))
    }

    /// The arguments of the last mpv start in `log`.
    pub(crate) fn last_start(log: &Log) -> String {
        log.borrow()
            .iter()
            .rev()
            .find(|entry| entry.starts_with("run ") || entry.starts_with("spawn "))
            .cloned()
            .unwrap()
    }

    /// An app with `names` below `/videos` in the gallery, in the video view of `index`.
    pub(crate) fn gallery_app_on_video(names: &[&str], index: usize) -> App {
        let mut app = App::new((8, 16), SharedImageList::new());
        app.images = names
            .iter()
            .map(|name| Path::new("/videos").join(name))
            .collect();
        app.filenames = names.iter().map(|name| (*name).to_owned()).collect();
        app.gallery.filtered_indices = (0..names.len()).collect();
        app.enter_video(index);
        app
    }

    /// The mpv starts in `log`.
    pub(crate) fn starts(log: &Log) -> Vec<String> {
        log.borrow()
            .iter()
            .filter(|entry| entry.starts_with("run ") || entry.starts_with("spawn "))
            .cloned()
            .collect()
    }
}
