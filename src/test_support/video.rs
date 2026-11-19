//! The fakes and the app builders of the video tests. No test starts a real mpv or ffmpeg.

use crate::app::{App, ViewMode};
use crate::config::VideoPlayer;
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
use tuilibr::pictures::kitty::{KittyBackend, Transfer};

pub(crate) type Log = Rc<RefCell<Vec<String>>>;

/// Records each mpv start. `fails` makes each start fail. `window` sends the exit code of the
/// last window.
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
        _socket: Option<&Path>,
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

pub(crate) fn fake_runner() -> (Log, FakeRunner) {
    let log = Log::default();
    let runner = FakeRunner {
        log: Rc::clone(&log),
        fails: false,
        window: None,
    };
    (log, runner)
}

/// A decoder that records each ffmpeg start and feeds the frames of the test.
pub(crate) struct FakeDecoder {
    pub(crate) info: VideoInfo,
    pub(crate) fail_probe: Cell<bool>,
    pub(crate) fail_start: Cell<bool>,
    starts: RefCell<Vec<String>>,
    /// The files that `remember_software` marked. A later probe of them answers
    /// `hardware: false`, like the probe cache.
    remembered: RefCell<Vec<PathBuf>>,
    sender: RefCell<Option<mpsc::SyncSender<Vec<u8>>>>,
    /// The frames of the next runs, one list per start. A start without a list waits for
    /// [`FakeDecoder::feed`].
    runs: RefCell<std::collections::VecDeque<Vec<Vec<u8>>>>,
    /// True while the probe cache holds no entry: `cached_probe` answers `None`.
    pub(crate) cold: Cell<bool>,
    /// The probes that ran while `cold` was true, the ffprobe runs on the calling thread.
    cold_probes: Cell<usize>,
    /// The files of `prepare_on_thread`, and the sender of the last one while `cold`.
    prepared: RefCell<Vec<PathBuf>>,
    prepare_sender: RefCell<Option<mpsc::Sender<bool>>>,
}

impl FakeDecoder {
    pub(crate) fn starts(&self) -> Vec<String> {
        self.starts.borrow().clone()
    }

    pub(crate) fn remembered(&self) -> usize {
        self.remembered.borrow().len()
    }

    pub(crate) fn last_args(&self) -> String {
        self.starts.borrow().last().cloned().unwrap()
    }

    pub(crate) fn queue_run(&self, frames: Vec<Vec<u8>>) {
        self.runs.borrow_mut().push_back(frames);
    }

    pub(crate) fn feed(&self, frame: Vec<u8>) {
        self.sender
            .borrow()
            .as_ref()
            .expect("no ffmpeg run to feed")
            .send(frame)
            .expect("the player dropped its stream");
    }

    pub(crate) fn cold_probes(&self) -> usize {
        self.cold_probes.get()
    }

    pub(crate) fn prepared(&self) -> Vec<PathBuf> {
        self.prepared.borrow().clone()
    }

    /// End the last `prepare_on_thread`. A success fills the cache.
    pub(crate) fn finish_prepare(&self, ready: bool) {
        if ready {
            self.cold.set(false);
        }
        let sender = self.prepare_sender.borrow_mut().take();
        sender
            .expect("no probe on a thread")
            .send(ready)
            .expect("the prestart dropped its receiver");
    }

    pub(crate) fn end(&self) {
        self.sender.borrow_mut().take();
    }

    /// True while a player reads the current run. The check sends an empty frame: call it at the
    /// end of a test only.
    pub(crate) fn streaming(&self) -> bool {
        self.sender
            .borrow()
            .as_ref()
            .is_some_and(|sender| sender.try_send(Vec::new()).is_ok())
    }
}

impl Decoder for FakeDecoder {
    fn probe(&self, video: &Path) -> io::Result<VideoInfo> {
        if self.fail_probe.get() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "no video stream",
            ));
        }
        if self.cold.get() {
            self.cold_probes.set(self.cold_probes.get() + 1);
        }
        let mut info = self.info.clone();
        if self.remembered.borrow().iter().any(|path| path == video) {
            info.hardware = false;
        }
        Ok(info)
    }

    fn remember_software(&self, video: &Path) {
        self.remembered.borrow_mut().push(video.to_path_buf());
    }

    fn cached_probe(&self, video: &Path) -> Option<VideoInfo> {
        if self.cold.get() {
            return None;
        }
        self.probe(video).ok()
    }

    fn prepare_on_thread(&self, video: &Path, _cube_dirs: Vec<PathBuf>) -> mpsc::Receiver<bool> {
        let (sender, receiver) = mpsc::channel();
        self.prepared.borrow_mut().push(video.to_path_buf());
        if self.cold.get() {
            *self.prepare_sender.borrow_mut() = Some(sender);
        } else {
            // A warm cache: only a failing probe comes here, and it ends at once.
            sender
                .send(self.probe(video).is_ok())
                .expect("the receiver is alive");
        }
        receiver
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
        if let Some(frames) = self.runs.borrow_mut().pop_front() {
            for frame in frames {
                frame_tx.send(frame).map_err(io::Error::other)?;
            }
            return Ok(FrameStream::new(frame_rx, None));
        }
        *self.sender.borrow_mut() = Some(frame_tx);
        Ok(FrameStream::new(frame_rx, None))
    }
}

pub(crate) fn fake_decoder(info: VideoInfo) -> Rc<FakeDecoder> {
    Rc::new(FakeDecoder {
        info,
        fail_probe: Cell::new(false),
        fail_start: Cell::new(false),
        starts: RefCell::new(Vec::new()),
        remembered: RefCell::new(Vec::new()),
        sender: RefCell::new(None),
        runs: RefCell::new(std::collections::VecDeque::new()),
        cold: Cell::new(false),
        cold_probes: Cell::new(0),
        prepared: RefCell::new(Vec::new()),
        prepare_sender: RefCell::new(None),
    })
}

pub(crate) fn shared(decoder: &Rc<FakeDecoder>) -> Rc<dyn Decoder> {
    let decoder: Rc<FakeDecoder> = Rc::clone(decoder);
    decoder
}

pub(crate) fn info_of(width: u32, height: u32, fps: f64) -> VideoInfo {
    VideoInfo {
        width,
        height,
        rotation: 0,
        fps,
        duration: 2.0,
        hdr: None,
        hdr_peak: None,
        primaries: crate::player::tone_map::Primaries::Bt709,
        samples: crate::player::Samples::NV12,
        hardware: true,
        rate: crate::player::Rate::Constant,
        stream: 0,
    }
}

/// The header of an animated image: 3 frames at 10 frames per second.
pub(crate) fn animation_info() -> VideoInfo {
    VideoInfo {
        duration: 0.3,
        rate: crate::player::Rate::Animation,
        ..info_of(64, 48, 10.0)
    }
}

pub(crate) fn clip_info() -> VideoInfo {
    info_of(640, 480, 10.0)
}

/// The real ffmpeg on PATH, or `None` with a note: the test skips.
pub(crate) fn real_ffmpeg() -> Option<PathBuf> {
    let program = crate::yazi::find_on_path("ffmpeg", std::env::var_os("PATH").as_deref());
    if program.is_none() {
        println!("note: skipped, because ffmpeg is not on PATH");
    }
    program
}

/// The real ffprobe on PATH, or `None` with a note: the test skips.
pub(crate) fn real_ffprobe() -> Option<PathBuf> {
    let program = crate::yazi::find_on_path("ffprobe", std::env::var_os("PATH").as_deref());
    if program.is_none() {
        println!("note: skipped, because ffprobe is not on PATH");
    }
    program
}

/// Write an H.264 clip of 40 frames of 160x120 pixels (MP4 or MOV). A `variable` clip holds 20
/// frames at 60 and 20 at 15 frames per second (`r_frame_rate` 60, average 24). The other
/// clip has 24. `None` with a note when ffmpeg has no libx264.
pub(crate) fn write_clip(program: &Path, path: &Path, variable: bool) -> Option<PathBuf> {
    let source = if variable {
        "testsrc=size=160x120:rate=60"
    } else {
        "testsrc=size=160x120:rate=24"
    };
    let mut command = std::process::Command::new(program);
    command.args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"]);
    command.args(["-i", source, "-frames:v", "40"]);
    if variable {
        command.args(["-vf", "setpts='if(lt(N,20),N/60,20/60+(N-20)/15)/TB'"]);
        command.args(["-fps_mode", "passthrough"]);
    }
    command.args(["-c:v", "libx264", "-preset", "ultrafast"]);
    command.args(["-pix_fmt", "yuv420p"]);
    let written = command
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !written {
        println!("note: skipped, because ffmpeg could not write an H.264 clip");
    }
    written.then(|| path.to_path_buf())
}

pub(crate) fn state_with(player: VideoPlayer, decoder: Rc<FakeDecoder>) -> LoopState {
    state_with_decoder(player, decoder, PathBuf::from("/bin/ffmpeg"))
}

/// A loop state with any decoder and the ffmpeg program `ffmpeg`, for a test with the real
/// programs.
pub(crate) fn state_with_decoder(
    player: VideoPlayer,
    decoder: Rc<dyn Decoder>,
    ffmpeg: PathBuf,
) -> LoopState {
    LoopState {
        status: None,
        answers: tuilibr::terminal::answers::LateAnswerFilter::default(),
        cell_size_from_terminal: false,
        mouse: true,
        mouse_captured: true,
        video: VideoState {
            player,
            mpv: Some(PathBuf::from("/bin/mpv")),
            ffmpeg: Some(ffmpeg),
            decoder,
        },
    }
}

pub(crate) fn terminal_state() -> LoopState {
    state_with(VideoPlayer::Terminal, fake_decoder(clip_info()))
}

pub(crate) fn window_state() -> LoopState {
    state_with(VideoPlayer::Window, fake_decoder(clip_info()))
}

pub(crate) fn with_temp_file_medium(mut app: App) -> App {
    app.graphics = KittyBackend::new(Transfer::TempFile);
    app
}

pub(crate) fn video_app() -> App {
    gallery_app_on_video(&["a.jpg", "clip.mp4"], 1)
}

pub(crate) fn fullscreen_video_app() -> App {
    fullscreen_app_on_video(&["a.jpg", "clip.mp4", "b.jpg"], false)
}

pub(crate) fn slideshow_video_app() -> App {
    fullscreen_app_on_video(&["a.jpg", "clip.mp4", "b.jpg"], true)
}

/// An app in the fullscreen view that steps from the first of `names` onto the second, a video.
/// `slideshow` starts a slideshow.
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

pub(crate) fn last_start(log: &Log) -> String {
    starts(log).last().cloned().unwrap()
}

pub(crate) fn starts(log: &Log) -> Vec<String> {
    log.borrow()
        .iter()
        .filter(|entry| entry.starts_with("spawn "))
        .cloned()
        .collect()
}
