//! The fakes and the app builders of the video tests. No test starts a real mpv or ffmpeg.

use crate::app::{App, ViewMode};
use crate::config::VideoPlayer;
use crate::image_list::SharedImageList;
use crate::mpv;
use crate::run_loop::LoopState;
use crate::video_handoff::VideoState;
use std::cell::RefCell;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use tuilibr::pictures::kitty::{KittyBackend, Transfer};
use tuilibr::pictures::motion::decoder::Decoder;
pub(crate) use tuilibr::pictures::test_support::video::{
    FakeDecoder, animation_info, clip_info, fake_decoder, info_of, real_ffmpeg, real_ffprobe,
    shared, write_clip,
};

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
