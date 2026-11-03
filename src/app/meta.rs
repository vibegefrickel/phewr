//! The facts of the file under the cursor for the status row. A thread reads them: a cursor
//! move never waits for a header, an EXIF block or `ffprobe`.

use super::App;
use crate::file_meta::{self, FileMeta};
use lru::LruCache;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

/// One entry is about 200 bytes.
const META_CACHE_ENTRIES: NonZeroUsize = NonZeroUsize::new(256).unwrap();

/// How the thread finds the length of a motion. The tests replace both steps.
#[derive(Clone, Copy)]
pub(crate) struct MotionLookup {
    /// The probe cache, no `ffprobe`.
    peek: fn(&Path) -> Option<f64>,
    /// One `ffprobe`, only on the meta thread.
    probe: fn(&Path) -> Option<f64>,
}

pub(crate) struct MetaState {
    cache: LruCache<PathBuf, FileMeta>,
    in_flight: Option<(PathBuf, Receiver<FileMeta>)>,
    motion: MotionLookup,
    /// The probe of one motion. It is no part of `meta_pending`, so it holds back no other work.
    motion_probe: Option<(PathBuf, Receiver<Option<f64>>)>,
    /// The stills whose motion was probed once. A failed probe gets no retry.
    probed: LruCache<PathBuf, ()>,
}

impl Default for MetaState {
    fn default() -> Self {
        Self {
            cache: LruCache::new(META_CACHE_ENTRIES),
            in_flight: None,
            motion_probe: None,
            probed: LruCache::new(META_CACHE_ENTRIES),
            motion: MotionLookup {
                peek: file_meta::motion_length_cached,
                probe: file_meta::motion_length_probed,
            },
        }
    }
}

impl App {
    /// Start a read for the subject if the cache does not hold it. One read runs at a time, and
    /// fast cursor moves start no threads.
    pub(crate) fn request_meta(&mut self) {
        self.request_motion_length();
        if self.meta.in_flight.is_some() {
            return;
        }
        let Some(path) = self.view_file() else {
            return;
        };
        if self.meta.cache.contains(&path) {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let job = path.clone();
        let motion = self.live.motion(&path).map(Path::to_path_buf);
        let lookup = self.meta.motion;
        let started = thread::Builder::new()
            .name("phewr-meta".to_owned())
            .spawn(move || {
                let mut meta = file_meta::read(&job);
                meta.motion_seconds = motion.as_deref().and_then(lookup.peek);
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "the app drops the receiver only when it quits"
                )]
                let _ = sender.send(meta);
            });
        // Without a thread, the status row shows no facts, and the next request tries again.
        if started.is_ok() {
            self.meta.in_flight = Some((path, receiver));
        }
    }

    /// Start the probe of the motion of the subject if it is a Live still with facts and
    /// without a length. One probe runs at a time, and each still gets one try.
    fn request_motion_length(&mut self) {
        if self.meta.motion_probe.is_some() {
            return;
        }
        let Some(path) = self.view_file() else {
            return;
        };
        let Some(motion) = self.live.motion(&path).map(Path::to_path_buf) else {
            return;
        };
        let needs_length = self
            .meta
            .cache
            .peek(&path)
            .is_some_and(|meta| meta.motion_seconds.is_none());
        if !needs_length || self.meta.probed.contains(&path) {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let probe = self.meta.motion.probe;
        let started = thread::Builder::new()
            .name("phewr-motion".to_owned())
            .spawn(move || {
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "the app drops the receiver only when it quits"
                )]
                let _ = sender.send(probe(&motion));
            });
        if started.is_ok() {
            self.meta.probed.put(path.clone(), ());
            self.meta.motion_probe = Some((path, receiver));
        }
    }

    pub(crate) fn poll_meta(&mut self) {
        self.poll_motion_length();
        let Some((path, receiver)) = &self.meta.in_flight else {
            return;
        };
        match receiver.try_recv() {
            Ok(meta) => {
                let path = path.clone();
                self.meta.cache.put(path, meta);
                self.meta.in_flight = None;
                self.needs_render = true;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.meta.in_flight = None,
        }
    }

    /// Put the probed length into the facts of the still.
    fn poll_motion_length(&mut self) {
        let Some((path, receiver)) = &self.meta.motion_probe else {
            return;
        };
        let length = match receiver.try_recv() {
            Err(TryRecvError::Empty) => return,
            Ok(length) => length,
            Err(TryRecvError::Disconnected) => None,
        };
        if let Some(meta) = self.meta.cache.get_mut(path) {
            meta.motion_seconds = length;
            self.needs_render = length.is_some();
        }
        self.meta.motion_probe = None;
    }

    /// The status line of the subject, or `None` before its read ends. A Live Photo still
    /// starts with `Live`, and with `Live 2.8 s` once the motion length is known.
    pub(crate) fn meta_line(&self) -> Option<String> {
        let path = self.view_file()?;
        let meta = self.meta.cache.peek(&path)?;
        let facts = meta.describe();
        if self.live.motion(&path).is_some() {
            let length = meta
                .motion_seconds
                .map_or_else(String::new, |s| format!(" {}", file_meta::format_motion(s)));
            return Some(format!("Live{length} \u{b7} {facts}"));
        }
        Some(facts)
    }

    pub(crate) fn meta_pending(&self) -> bool {
        self.meta.in_flight.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::MotionLookup;
    use crate::app::ViewMode;
    use crate::test_support::{app_with_files, wait_until};
    use std::path::Path;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A Live app whose still `a.png` pairs with an existing `a.mov`.
    fn live_app(dir: &Path) -> crate::app::App {
        let mut app = app_with_files(dir, &["a.png"]);
        let still = app.images[0].clone();
        std::fs::write(still.with_extension("mov"), "motion").unwrap();
        app.set_live_photos(crate::scanner::LivePhotos::from_pairs([(
            still.clone(),
            still.with_extension("mov"),
        )]));
        app
    }

    /// Run the facts read and the motion probe, as the run loop does.
    fn read_to_end(app: &mut crate::app::App) {
        app.request_meta();
        wait_until(|| {
            app.poll_meta();
            app.request_meta();
            !app.meta_pending() && app.meta.motion_probe.is_none()
        });
    }

    fn no_length(_: &Path) -> Option<f64> {
        None
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "it fills a fn pointer of MotionLookup"
    )]
    fn length_2_8(_: &Path) -> Option<f64> {
        Some(2.8)
    }

    fn must_not_probe(_: &Path) -> Option<f64> {
        panic!("a cached length needs no probe");
    }

    #[test]
    #[cfg(feature = "video")]
    fn live_length_comes_from_the_probe_cache() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = live_app(dir.path());
        let motion = app.images[0].with_extension("mov");
        crate::player::seed_probe(
            &motion,
            crate::player::VideoInfo {
                duration: 2.83,
                ..crate::test_support::video::clip_info()
            },
        );
        read_to_end(&mut app);
        let line = app.meta_line().unwrap();
        assert!(
            line.starts_with("Live 2.8 s \u{b7} 8\u{d7}8 \u{b7} "),
            "{line}"
        );
    }

    #[test]
    fn live_length_comes_from_a_meta_thread_probe() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = live_app(dir.path());
        app.meta.motion = MotionLookup {
            peek: no_length,
            probe: length_2_8,
        };
        read_to_end(&mut app);
        let line = app.meta_line().unwrap();
        assert!(line.starts_with("Live 2.8 s \u{b7} "), "{line}");
    }

    static PROBE_GATE: Mutex<()> = Mutex::new(());

    #[expect(
        clippy::unnecessary_wraps,
        reason = "it fills a fn pointer of MotionLookup"
    )]
    fn gated_length(_: &Path) -> Option<f64> {
        drop(PROBE_GATE.lock().unwrap());
        Some(2.8)
    }

    #[test]
    fn live_without_probe_shows_no_length_until_it_ends() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = live_app(dir.path());
        app.meta.motion = MotionLookup {
            peek: no_length,
            probe: gated_length,
        };
        let gate = PROBE_GATE.lock().unwrap();
        app.request_meta();
        wait_until(|| {
            app.poll_meta();
            app.request_meta();
            app.meta_line().is_some() && app.meta.motion_probe.is_some()
        });
        let early = app.meta_line().unwrap();
        assert!(early.starts_with("Live \u{b7} 8\u{d7}8"), "{early}");
        assert!(!app.meta_pending(), "the probe holds back no other work");
        app.gallery.cursor = 0;
        drop(gate);
        wait_until(|| {
            app.poll_meta();
            app.meta.motion_probe.is_none()
        });
        let late = app.meta_line().unwrap();
        assert!(late.starts_with("Live 2.8 s \u{b7} "), "{late}");
    }

    static COUNTED_PROBES: AtomicUsize = AtomicUsize::new(0);

    fn counted_failure(_: &Path) -> Option<f64> {
        COUNTED_PROBES.fetch_add(1, Ordering::SeqCst);
        None
    }

    #[test]
    fn failed_probe_keeps_live_without_length_and_tries_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = live_app(dir.path());
        app.meta.motion = MotionLookup {
            peek: no_length,
            probe: counted_failure,
        };
        read_to_end(&mut app);
        read_to_end(&mut app);
        assert!(app.meta_line().unwrap().starts_with("Live \u{b7} "));
        assert_eq!(COUNTED_PROBES.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cached_length_starts_no_probe() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = live_app(dir.path());
        app.meta.motion = MotionLookup {
            peek: length_2_8,
            probe: must_not_probe,
        };
        read_to_end(&mut app);
        assert!(app.meta_line().unwrap().starts_with("Live 2.8 s \u{b7} "));
    }

    #[test]
    fn live_still_starts_with_live() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_files(dir.path(), &["a.png", "b.png"]);
        let still = app.images[0].clone();
        app.set_live_photos(crate::scanner::LivePhotos::from_pairs([(
            still.clone(),
            still.with_extension("mov"),
        )]));

        app.request_meta();
        wait_until(|| {
            app.poll_meta();
            !app.meta_pending()
        });
        let line = app.meta_line().unwrap();
        assert!(line.starts_with("Live \u{b7} 8\u{d7}8 \u{b7} "), "{line}");

        app.gallery.cursor = 1;
        app.request_meta();
        wait_until(|| {
            app.poll_meta();
            !app.meta_pending()
        });
        assert!(!app.meta_line().unwrap().contains("Live"));
    }

    #[test]
    fn status_line_follows_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_files(dir.path(), &["a.png", "b.png"]);
        assert_eq!(app.meta_line(), None, "nothing before the read");

        app.request_meta();
        assert!(app.meta_pending());
        wait_until(|| {
            app.poll_meta();
            !app.meta_pending()
        });
        let first = app.meta_line().unwrap();
        assert!(first.starts_with("8\u{d7}8 \u{b7} "), "{first}");
        assert!(first.contains("PNG"));

        app.gallery.cursor = 1;
        assert_eq!(app.meta_line(), None);
        app.request_meta();
        wait_until(|| {
            app.poll_meta();
            !app.meta_pending()
        });
        assert!(app.meta_line().is_some());

        app.gallery.cursor = 0;
        app.request_meta();
        assert!(!app.meta_pending());
        assert_eq!(app.meta_line().as_deref(), Some(first.as_str()));
    }

    #[test]
    fn fullscreen_shows_open_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_files(dir.path(), &["a.png", "b.png"]);
        app.mode = ViewMode::Fullscreen;
        app.current = 1;
        app.request_meta();
        wait_until(|| {
            app.poll_meta();
            !app.meta_pending()
        });
        app.gallery.cursor = 0;
        assert!(
            app.meta_line().is_some(),
            "the cursor of the grid does not count"
        );
    }

    #[test]
    fn disconnected_read_starts_again() {
        use std::sync::mpsc;

        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_files(dir.path(), &["a.png"]);
        let path = app.view_file().unwrap();

        // A thread that ended without sending leaves the receiver disconnected.
        let receiver = mpsc::channel::<crate::file_meta::FileMeta>().1;
        app.meta.in_flight = Some((path.clone(), receiver));
        app.poll_meta();
        assert!(
            app.meta.in_flight.is_none(),
            "the failed read is cleared without caching"
        );
        assert!(app.meta_line().is_none(), "no facts were stored");

        app.request_meta();
        assert!(
            app.meta.in_flight.as_ref().is_some_and(|(p, _)| *p == path),
            "a new read starts for the same file"
        );
        wait_until(|| {
            app.poll_meta();
            !app.meta_pending()
        });
        assert!(app.meta_line().is_some());
    }
}
