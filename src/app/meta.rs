//! The facts of the file under the cursor for the status row. A thread reads them: a cursor
//! move never waits for a header, an EXIF block or `ffprobe`.

use super::App;
use crate::file_meta::{self, FileMeta};
use lru::LruCache;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

/// One entry is about 200 bytes.
const META_CACHE_ENTRIES: NonZeroUsize = NonZeroUsize::new(256).unwrap();

pub(crate) struct MetaState {
    cache: LruCache<PathBuf, FileMeta>,
    in_flight: Option<(PathBuf, Receiver<FileMeta>)>,
}

impl Default for MetaState {
    fn default() -> Self {
        Self {
            cache: LruCache::new(META_CACHE_ENTRIES),
            in_flight: None,
        }
    }
}

impl App {
    /// Start a read for the subject if the cache does not hold it. One read runs at a time, and
    /// fast cursor moves start no threads.
    pub(crate) fn request_meta(&mut self) {
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
        let started = thread::Builder::new()
            .name("phewr-meta".to_owned())
            .spawn(move || {
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "the app drops the receiver only when it quits"
                )]
                let _ = sender.send(file_meta::read(&job));
            });
        // Without a thread, the status row shows no facts, and the next request tries again.
        if started.is_ok() {
            self.meta.in_flight = Some((path, receiver));
        }
    }

    pub(crate) fn poll_meta(&mut self) {
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

    /// The status line of the subject, or `None` before its read ends. A Live Photo still
    /// starts with `Live`.
    pub(crate) fn meta_line(&self) -> Option<String> {
        let path = self.view_file()?;
        let facts = self.meta.cache.peek(&path).map(FileMeta::describe)?;
        if self.live.motion(&path).is_some() {
            return Some(format!("Live \u{b7} {facts}"));
        }
        Some(facts)
    }

    pub(crate) fn meta_pending(&self) -> bool {
        self.meta.in_flight.is_some()
    }
}

#[cfg(test)]
mod tests {
    use crate::app::ViewMode;
    use crate::test_support::{app_with_files, wait_until};

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
