//! The facts of the file under the cursor for the status row. A thread reads them, so a cursor
//! move never waits for a header, an EXIF block or `ffprobe`.

use super::{App, ViewMode};
use crate::file_meta::{self, FileMeta};
use lru::LruCache;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

/// The facts that the cache keeps. One entry is about 200 bytes.
const META_CACHE_ENTRIES: NonZeroUsize = NonZeroUsize::new(256).unwrap();

pub(crate) struct MetaState {
    cache: LruCache<PathBuf, FileMeta>,
    /// The one read that runs, with the path that it reads.
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
    /// The file whose facts the status row shows: the selected entry in the grid, the open file
    /// in the other views. A folder has none.
    fn meta_subject(&self) -> Option<PathBuf> {
        let index = if self.mode == ViewMode::Gallery {
            self.gallery.selected_index()?
        } else {
            self.current
        };
        if self.is_folder(index) {
            return None;
        }
        self.images.get(index).cloned()
    }

    /// Start a read for the subject if the cache does not hold it. One read runs at a time. A
    /// newer subject starts when the running read ends, so fast cursor moves start no threads.
    pub(crate) fn request_meta(&mut self) {
        if self.meta.in_flight.is_some() {
            return;
        }
        let Some(path) = self.meta_subject() else {
            return;
        };
        if self.meta.cache.contains(&path) {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let job = path.clone();
        thread::spawn(move || {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "the app drops the receiver only when it quits"
            )]
            let _ = sender.send(file_meta::read(&job));
        });
        self.meta.in_flight = Some((path, receiver));
    }

    /// Take a finished read into the cache and draw it.
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

    /// The status line of the subject, or `None` before its read ends.
    pub(crate) fn meta_line(&self) -> Option<String> {
        let path = self.meta_subject()?;
        self.meta.cache.peek(&path).map(FileMeta::describe)
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
    fn the_status_line_follows_the_cursor_after_one_read_each() {
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

        // A cached file starts no read.
        app.gallery.cursor = 0;
        app.request_meta();
        assert!(!app.meta_pending());
        assert_eq!(app.meta_line().as_deref(), Some(first.as_str()));
    }

    #[test]
    fn the_fullscreen_view_shows_the_open_file() {
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
}
