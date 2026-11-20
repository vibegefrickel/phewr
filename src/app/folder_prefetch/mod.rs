//! The folder prefetch: while the cursor rests on a folder cell, one thread decodes the first
//! screen of that folder into the disk cache, as a folder switch computes it. A cursor move, a
//! switch, a zoom, a resize or the quit cancels it.

#[cfg(test)]
mod tests;
mod worker;

use self::worker::{Control, Request, prefetch};
use super::{App, ViewMode};
use ratatui::layout::Rect;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// A cursor that walks across the folders starts nothing.
pub(crate) const FOLDER_PREFETCH_DELAY: Duration = Duration::from_millis(300);

/// A start that waits for a stopping thread does not spin the run loop.
const START_POLL: Duration = Duration::from_millis(10);

/// A change of any part cancels the prefetch.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RestKey {
    folder: PathBuf,
    current_dir: Option<PathBuf>,
    grid_rect: Rect,
    zoom_rows: usize,
    cell_px: (u32, u32),
}

struct Running {
    key: RestKey,
    control: Arc<Control>,
    handle: JoinHandle<()>,
}

impl Running {
    /// Stop the thread before its next file. It can still finish the file that it decodes.
    fn cancel(&self) {
        self.control.cancel.store(true, Ordering::Release);
        self.handle.thread().unpark();
    }

    fn set_paused(&self, paused: bool) {
        let was_paused = self.control.paused.swap(paused, Ordering::AcqRel);
        if was_paused && !paused {
            self.handle.thread().unpark();
        }
    }
}

#[derive(Default)]
pub(crate) struct FolderPrefetch {
    rest: Option<(RestKey, Instant)>,
    running: Option<Running>,
    /// The next start waits for these threads: at most one prefetch decodes at a time.
    stopping: Vec<JoinHandle<()>>,
    done: HashSet<PathBuf>,
    /// True after the cursor left the `..` entry. Every switch puts the cursor on `..`.
    parent_armed: bool,
}

impl FolderPrefetch {
    /// A folder switch puts the cursor on `..` again.
    pub(super) const fn disarm_parent(&mut self) {
        self.parent_armed = false;
    }

    fn cancel(&mut self) {
        if let Some(running) = self.running.take() {
            running.cancel();
            self.stopping.push(running.handle);
        }
    }

    fn is_stopping(&mut self) -> bool {
        self.stopping.retain(|handle| !handle.is_finished());
        !self.stopping.is_empty()
    }

    /// The rest that waits for its start: no thread runs, and the folder is not done.
    fn waiting_rest(&self) -> Option<&(RestKey, Instant)> {
        self.rest
            .as_ref()
            .filter(|(key, _)| self.running.is_none() && !self.done.contains(&key.folder))
    }
}

impl Drop for FolderPrefetch {
    /// The quit stops the thread before its next file without a join.
    fn drop(&mut self) {
        if let Some(running) = &self.running {
            running.cancel();
        }
    }
}

impl App {
    /// Call it after each draw. It starts, pauses and cancels the prefetch.
    pub(crate) fn tick_folder_prefetch(&mut self, now: Instant) {
        if self
            .selected_entry()
            .is_some_and(|index| !self.is_parent_entry(index))
        {
            self.folder_prefetch.parent_armed = true;
        }
        let key = self.rest_key();
        let busy = self.has_pending_work();
        let prefetch = &mut self.folder_prefetch;
        if let Some(running) = prefetch
            .running
            .take_if(|running| running.handle.is_finished())
        {
            prefetch.done.insert(running.key.folder);
        }
        if prefetch
            .running
            .as_ref()
            .is_some_and(|running| Some(&running.key) != key.as_ref())
        {
            prefetch.cancel();
        }
        if let Some(running) = &prefetch.running {
            running.set_paused(busy);
            return;
        }
        let Some(key) = key else {
            prefetch.rest = None;
            return;
        };
        if prefetch.rest.as_ref().is_none_or(|(rest, _)| *rest != key) {
            prefetch.rest = Some((key, now));
        }
        let due = prefetch
            .waiting_rest()
            .filter(|(_, since)| now.duration_since(*since) >= FOLDER_PREFETCH_DELAY)
            .map(|(key, _)| key.clone());
        if let Some(key) = due
            && !busy
            && !prefetch.is_stopping()
        {
            self.start_folder_prefetch(key);
        }
    }

    /// The wait until the rest starts the prefetch. `None` without a waiting rest.
    pub(crate) fn folder_prefetch_timeout(&self, now: Instant) -> Option<Duration> {
        let (_, since) = self.folder_prefetch.waiting_rest()?;
        let due = *since + FOLDER_PREFETCH_DELAY;
        Some(due.saturating_duration_since(now).max(START_POLL))
    }

    #[cfg(test)]
    pub(crate) fn folder_prefetch_running(&self) -> Option<&Path> {
        self.folder_prefetch
            .running
            .as_ref()
            .map(|running| running.key.folder.as_path())
    }

    #[cfg(test)]
    pub(crate) fn folder_prefetched(&self, folder: &Path) -> bool {
        self.folder_prefetch.done.contains(folder)
    }

    /// The folder under the cursor and the grid around it, in the gallery only.
    fn rest_key(&self) -> Option<RestKey> {
        if self.mode != ViewMode::Gallery {
            return None;
        }
        let index = self
            .gallery
            .selected_index()
            .filter(|&index| self.is_folder(index))
            .filter(|&index| self.folder_prefetch.parent_armed || !self.is_parent_entry(index))?;
        Some(RestKey {
            folder: self.images[index].clone(),
            current_dir: self.current_dir.clone(),
            grid_rect: self.gallery.grid_rect,
            zoom_rows: self.gallery.zoom_rows,
            cell_px: self.cell_px,
        })
    }

    fn selected_entry(&self) -> Option<usize> {
        self.gallery
            .selected_index()
            .filter(|&index| index < self.images.len())
    }

    /// The thread starts paused while the grid has work. A folder without a thread counts as
    /// done: the run loop does not try again every 10 ms.
    fn start_folder_prefetch(&mut self, key: RestKey) {
        let request = self.folder_request(&key.folder);
        let control = Arc::new(Control::default());
        control
            .paused
            .store(self.has_pending_work(), Ordering::Release);
        let worker = Arc::clone(&control);
        let started = thread::Builder::new()
            .name("phewr-folder-prefetch".to_owned())
            .spawn(move || prefetch(&request, &worker));
        match started {
            Ok(handle) => {
                self.folder_prefetch.running = Some(Running {
                    key,
                    control,
                    handle,
                });
            }
            Err(_) => {
                self.folder_prefetch.done.insert(key.folder);
            }
        }
    }

    fn folder_request(&self, folder: &Path) -> Request {
        Request {
            folder: folder.to_path_buf(),
            from: self.gallery_dir(),
            grid: self.gallery.clone(),
            zoom_rows: self.folder_zoom_rows(folder),
            cell_px: self.cell_px,
            cache_dir: self.disk_cache_dir.clone(),
            live_photos: self.live_photos,
        }
    }
}
