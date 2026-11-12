//! The state file: the zoom level of the grid, per folder and as a global fallback.
use super::{MAX_VISIBLE_ROWS, MIN_VISIBLE_ROWS};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::Duration;

/// The most folders with a remembered zoom level.
const MAX_ZOOM_FOLDERS: usize = 200;

/// The remembered grid zoom, written at each zoom.
#[derive(Default, Deserialize, serde::Serialize)]
pub(super) struct StateFile {
    /// The last zoom level. A folder without its own entry falls back to it.
    pub(super) visible_rows: Option<u16>,
    /// A monotonic counter for the last-used order of the folder entries.
    #[serde(default)]
    pub(super) zoom_counter: u64,
    /// The folder zoom levels, keyed by the canonical folder path.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) zoom: BTreeMap<PathBuf, ZoomValue>,
}

/// A zoom value. The load takes the simple form `"/path" = 3`. The save writes the form with an
/// order, and the prune drops the least recently used folders.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(untagged)]
pub(super) enum ZoomValue {
    Rows(u16),
    Detailed(ZoomEntry),
}

impl ZoomValue {
    fn rows(&self) -> u16 {
        match self {
            Self::Rows(rows) => *rows,
            Self::Detailed(entry) => entry.rows,
        }
    }

    fn order(&self) -> u64 {
        match self {
            Self::Rows(_) => 0,
            Self::Detailed(entry) => entry.order,
        }
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub(super) struct ZoomEntry {
    rows: u16,
    order: u64,
}

pub(crate) fn load_folder_zoom(path: &Path, folder: &Path) -> Option<u16> {
    let state = read_state(path);
    state
        .zoom
        .get(folder)
        .map(ZoomValue::rows)
        .map(|rows| rows.clamp(MIN_VISIBLE_ROWS, MAX_VISIBLE_ROWS))
}

pub(crate) fn load_global_zoom(path: &Path) -> Option<u16> {
    let state = read_state(path);
    state
        .visible_rows
        .map(|rows| rows.clamp(MIN_VISIBLE_ROWS, MAX_VISIBLE_ROWS))
}

/// One change of the state file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StateChange {
    /// Remember the rows for the folder (if any) and as the global fallback.
    SetRows { folder: Option<PathBuf>, rows: u16 },
    /// Forget the zoom level of the folder. The global fallback stays.
    ClearFolder(PathBuf),
}

impl StateChange {
    /// Apply the change in memory. The result tells whether the state differs now.
    fn apply(&self, state: &mut StateFile) -> bool {
        match self {
            Self::SetRows { folder, rows } => {
                let rows = (*rows).clamp(MIN_VISIBLE_ROWS, MAX_VISIBLE_ROWS);
                state.visible_rows = Some(rows);
                if let Some(folder) = folder {
                    state.zoom_counter += 1;
                    state.zoom.insert(
                        folder.clone(),
                        ZoomValue::Detailed(ZoomEntry {
                            rows,
                            order: state.zoom_counter,
                        }),
                    );
                }
                prune_oldest_zoom_entries(state);
                true
            }
            Self::ClearFolder(folder) => state.zoom.remove(folder).is_some(),
        }
    }
}

/// Apply the changes, in their order, to one read of the file. If the state differs, write it
/// once. A broken file stays as it is.
fn apply_changes(path: &Path, changes: &[StateChange]) {
    let Some(mut state) = read_state_to_change(path) else {
        return;
    };
    let mut changed = false;
    for change in changes {
        changed |= change.apply(&mut state);
    }
    if changed {
        write_state(path, &state);
    }
}

/// Remember `rows` for `folder` and as the global fallback, now. The app queues a change instead.
#[cfg(test)]
pub(crate) fn save_visible_rows(path: &Path, folder: Option<&Path>, rows: u16) {
    let change = StateChange::SetRows {
        folder: folder.map(Path::to_path_buf),
        rows,
    };
    apply_changes(path, &[change]);
}

/// Forget the zoom level of `folder`, now. The global fallback stays.
#[cfg(test)]
pub(crate) fn clear_folder_zoom(path: &Path, folder: &Path) {
    apply_changes(path, &[StateChange::ClearFolder(folder.to_path_buf())]);
}

/// How long the writer waits for the next change before it writes, so a run of key presses gives
/// one write.
const WRITE_DELAY: Duration = Duration::from_millis(100);

enum Message {
    Change(PathBuf, StateChange),
    /// Write what is queued now, then answer.
    #[cfg(test)]
    Flush(mpsc::Sender<()>),
}

/// The one writer thread of the state file. The thread starts with the first change. The drop
/// closes the channel, and the thread writes the queued changes before the drop returns.
pub(crate) struct StateWriter {
    thread: Option<(mpsc::Sender<Message>, std::thread::JoinHandle<()>)>,
    /// The changes that are not on disk yet, oldest first. A read applies them over the file.
    pending: Arc<Mutex<Vec<(PathBuf, StateChange)>>>,
    delay: Duration,
}

impl Default for StateWriter {
    fn default() -> Self {
        Self {
            thread: None,
            pending: Arc::default(),
            delay: WRITE_DELAY,
        }
    }
}

impl StateWriter {
    /// A writer that waits `delay` before it writes, so a test controls the batch.
    #[cfg(test)]
    pub(crate) fn with_delay(delay: Duration) -> Self {
        let mut writer = Self::default();
        writer.delay = delay;
        writer
    }

    /// The folder level with the queued changes applied over the file.
    pub(crate) fn folder_zoom(&self, path: &Path, folder: &Path) -> Option<u16> {
        let mut rows = load_folder_zoom(path, folder);
        for (_, change) in self.pending_for(path) {
            match change {
                StateChange::SetRows {
                    folder: Some(f),
                    rows: set,
                } if f == folder => {
                    rows = Some(set.clamp(MIN_VISIBLE_ROWS, MAX_VISIBLE_ROWS));
                }
                StateChange::ClearFolder(f) if f == folder => rows = None,
                _ => {}
            }
        }
        rows
    }

    /// The global level with the queued changes applied over the file.
    pub(crate) fn global_zoom(&self, path: &Path) -> Option<u16> {
        let mut rows = load_global_zoom(path);
        for (_, change) in self.pending_for(path) {
            if let StateChange::SetRows { rows: set, .. } = change {
                rows = Some(set.clamp(MIN_VISIBLE_ROWS, MAX_VISIBLE_ROWS));
            }
        }
        rows
    }

    fn pending_for(&self, path: &Path) -> Vec<(PathBuf, StateChange)> {
        let pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        pending
            .iter()
            .filter(|(candidate, _)| candidate == path)
            .cloned()
            .collect()
    }

    /// Queue a change of the file at `path`. The UI thread does no file I/O.
    pub(crate) fn send(&mut self, path: &Path, change: StateChange) {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((path.to_path_buf(), change.clone()));
        let (pending, delay) = (Arc::clone(&self.pending), self.delay);
        let (sender, _) = self.thread.get_or_insert_with(|| {
            let (sender, receiver) = mpsc::channel();
            let thread = std::thread::spawn(move || write_loop(&receiver, &pending, delay));
            (sender, thread)
        });
        // A send fails only if the thread is gone. The state file is a convenience, so the
        // change is lost.
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a lost zoom level is harmless"
        )]
        let _ = sender.send(Message::Change(path.to_path_buf(), change));
    }

    /// Wait until the queued changes are on disk.
    #[cfg(test)]
    pub(crate) fn flush(&self) {
        let Some((sender, _)) = &self.thread else {
            return;
        };
        let (done, answer) = mpsc::channel();
        if sender.send(Message::Flush(done)).is_ok() {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a dead thread has nothing to flush"
            )]
            let _ = answer.recv();
        }
    }
}

impl Drop for StateWriter {
    fn drop(&mut self) {
        if let Some((sender, handle)) = self.thread.take() {
            drop(sender);
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a panicked writer has nothing left to write"
            )]
            let _ = handle.join();
        }
    }
}

/// Collect the changes until the delay passes without a new one, then write them.
fn write_loop(
    receiver: &mpsc::Receiver<Message>,
    unwritten: &Mutex<Vec<(PathBuf, StateChange)>>,
    delay: Duration,
) {
    while let Ok(first) = receiver.recv() {
        let mut batch = Vec::new();
        let mut pending = Some(first);
        let mut open = true;
        while let Some(message) = pending.take() {
            match message {
                Message::Change(path, change) => batch.push((path, change)),
                #[cfg(test)]
                Message::Flush(done) => {
                    write_batch(&mut batch, unwritten);
                    #[expect(
                        clippy::let_underscore_must_use,
                        reason = "the waiting test may be gone"
                    )]
                    let _ = done.send(());
                }
            }
            match receiver.recv_timeout(delay) {
                Ok(next) => pending = Some(next),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => open = false,
            }
        }
        write_batch(&mut batch, unwritten);
        if !open {
            return;
        }
    }
}

/// Write the batch, one read and at most one write per file.
fn write_batch(
    batch: &mut Vec<(PathBuf, StateChange)>,
    pending: &Mutex<Vec<(PathBuf, StateChange)>>,
) {
    let mut paths: Vec<PathBuf> = batch.iter().map(|(path, _)| path.clone()).collect();
    paths.sort();
    paths.dedup();
    for path in paths {
        let changes: Vec<StateChange> = batch
            .iter()
            .filter(|(candidate, _)| *candidate == path)
            .map(|(_, change)| change.clone())
            .collect();
        apply_changes(&path, &changes);
    }
    // The batch is the oldest part of the queue: a read sees each change in the file or here.
    let mut queue = pending.lock().unwrap_or_else(PoisonError::into_inner);
    let written = batch.len().min(queue.len());
    queue.drain(..written);
    batch.clear();
}

fn prune_oldest_zoom_entries(state: &mut StateFile) {
    if state.zoom.len() <= MAX_ZOOM_FOLDERS {
        return;
    }
    let mut entries: Vec<(PathBuf, ZoomValue)> =
        std::mem::take(&mut state.zoom).into_iter().collect();
    entries.sort_by_key(|(_, value)| std::cmp::Reverse(value.order()));
    entries.truncate(MAX_ZOOM_FOLDERS);
    state.zoom = entries.into_iter().collect();
}

/// The state file for a change. A missing file gives the default. A broken file gives `None`:
/// the save leaves it alone, and a bad line cannot erase the folder table.
fn read_state_to_change(path: &Path) -> Option<StateFile> {
    match std::fs::read_to_string(path) {
        Ok(contents) => toml::from_str(&contents).ok(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(StateFile::default()),
        Err(_) => None,
    }
}

pub(super) fn read_state(path: &Path) -> StateFile {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| toml::from_str(&contents).ok())
        .unwrap_or_default()
}

/// A temp file of a save older than this is the leftover of a crash.
const STALE_TEMP_AGE: Duration = Duration::from_hours(1);

pub(super) fn write_state(path: &Path, state: &StateFile) {
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    prune_stale_temp_files(path);
    if let Ok(contents) = toml::to_string(state) {
        // Write beside the file and rename. A crash never leaves half a state file.
        let mut temp = path.as_os_str().to_owned();
        temp.push(format!(".{}.tmp", std::process::id()));
        let temp = PathBuf::from(temp);
        if std::fs::write(&temp, contents).is_err() || std::fs::rename(&temp, path).is_err() {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a leftover temp file is harmless"
            )]
            let _ = std::fs::remove_file(&temp);
        }
    }
}

/// Remove the stale temp files `<name>.*.tmp` beside the state file. A fresh one can belong to
/// a running phewr.
fn prune_stale_temp_files(path: &Path) {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return;
    };
    let prefix = format!("{}.", name.to_string_lossy());
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name().to_string_lossy().into_owned();
        let stale = file_name.starts_with(&prefix)
            && Path::new(&file_name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("tmp"))
            && entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age > STALE_TEMP_AGE);
        if stale {
            #[expect(clippy::let_underscore_must_use, reason = "the next save tries again")]
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::load_at;
    use super::super::tests::test_root;
    use super::*;

    #[test]
    fn folder_zoom_keeps_global_fallback() {
        let root = test_root();
        let config_path = root.path().join("config.toml");
        let state_path = root.path().join("state.toml");
        let folder_a = PathBuf::from("/photos/a");
        let folder_b = PathBuf::from("/photos/b");

        save_visible_rows(&state_path, Some(&folder_a), 3);
        assert_eq!(load_folder_zoom(&state_path, &folder_a), Some(3));
        assert_eq!(load_folder_zoom(&state_path, &folder_b), None);
        let config = load_at(&config_path, &state_path);
        assert_eq!(config.visible_rows, 3, "the global fallback is also set");

        save_visible_rows(&state_path, Some(&folder_b), 7);
        assert_eq!(load_folder_zoom(&state_path, &folder_a), Some(3));
        assert_eq!(load_folder_zoom(&state_path, &folder_b), Some(7));
    }

    #[test]
    fn reset_key_keeps_configured_default() {
        let root = test_root();
        let config_path = root.path().join("config.toml");
        let state_path = root.path().join("state.toml");
        std::fs::write(&config_path, "[gallery]\nvisible_rows = 8\n").unwrap();
        save_visible_rows(&state_path, None, 3);

        let config = load_at(&config_path, &state_path);
        assert_eq!(config.visible_rows, 3);
        assert_eq!(config.configured_visible_rows, 8);
    }

    #[test]
    fn clear_keeps_global_level() {
        let root = test_root();
        let state_path = root.path().join("state.toml");
        let folder = PathBuf::from("/photos/a");

        save_visible_rows(&state_path, Some(&folder), 5);
        clear_folder_zoom(&state_path, &folder);
        assert_eq!(load_folder_zoom(&state_path, &folder), None);
        let config = load_at(&root.path().join("config.toml"), &state_path);
        assert_eq!(config.visible_rows, 5, "the global fallback stays");
    }

    #[test]
    fn clamps_folder_zoom() {
        let root = test_root();
        let state_path = root.path().join("state.toml");
        let folder = PathBuf::from("/photos/a");

        std::fs::write(
            &state_path,
            "[zoom]\n\"/photos/a\" = 1\n\"/photos/b\" = 99\n",
        )
        .unwrap();
        assert_eq!(load_folder_zoom(&state_path, &folder), Some(2));
        assert_eq!(
            load_folder_zoom(&state_path, &PathBuf::from("/photos/b")),
            Some(12)
        );
    }

    #[test]
    fn loads_state_without_zoom_table() {
        let root = test_root();
        let state_path = root.path().join("state.toml");
        std::fs::write(&state_path, "visible_rows = 5\n").unwrap();

        let config = load_at(&root.path().join("config.toml"), &state_path);
        assert_eq!(config.visible_rows, 5);
        assert_eq!(load_folder_zoom(&state_path, &PathBuf::from("/any")), None);
    }

    #[test]
    fn drops_oldest_folders_past_limit() {
        let root = test_root();
        let state_path = root.path().join("state.toml");

        for index in 0..MAX_ZOOM_FOLDERS + 10 {
            let folder = PathBuf::from(format!("/photos/{index}"));
            save_visible_rows(&state_path, Some(&folder), 3);
        }

        let state: StateFile = read_state(&state_path);
        assert_eq!(state.zoom.len(), MAX_ZOOM_FOLDERS);
        assert!(
            !state.zoom.contains_key(&PathBuf::from("/photos/0")),
            "the oldest folders are dropped"
        );
        assert!(
            state
                .zoom
                .contains_key(&PathBuf::from(format!("/photos/{}", MAX_ZOOM_FOLDERS + 9))),
            "the newest folders stay"
        );
    }

    #[test]
    fn save_removes_stale_temp_files() {
        let root = test_root();
        let state_path = root.path().join("state.toml");
        let stale = root.path().join("state.toml.111.tmp");
        let fresh = root.path().join("state.toml.222.tmp");
        let other = root.path().join("other.toml.333.tmp");
        for path in [&stale, &fresh, &other] {
            std::fs::write(path, "x").unwrap();
        }
        let two_hours_ago = std::time::SystemTime::now() - Duration::from_hours(2);
        for path in [&stale, &other] {
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(two_hours_ago)
                .unwrap();
        }

        save_visible_rows(&state_path, None, 6);

        assert!(!stale.exists());
        assert!(fresh.exists());
        assert!(other.exists(), "only the temp files of the state file go");
        assert!(state_path.exists());
    }

    #[test]
    fn write_needs_usable_parent() {
        let root = test_root();
        let blocker = root.path().join("file");
        std::fs::write(&blocker, "not a directory").unwrap();
        let state = StateFile {
            visible_rows: Some(6),
            ..StateFile::default()
        };

        write_state(&blocker.join("state.toml"), &state);
        assert!(blocker.is_file());

        write_state(Path::new("/"), &state);
    }

    #[test]
    fn keeps_broken_state_file() {
        let root = test_root();
        let path = root.path().join("state.toml");
        let broken = "visible_rows = 4\n[zoom\nbad line";
        std::fs::write(&path, broken).unwrap();

        save_visible_rows(&path, Some(Path::new("/photos")), 5);
        clear_folder_zoom(&path, Path::new("/photos"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
    }

    #[test]
    fn save_leaves_no_temp_file() {
        let root = test_root();
        let path = root.path().join("state.toml");
        save_visible_rows(&path, Some(Path::new("/photos")), 5);
        assert_eq!(load_folder_zoom(&path, Path::new("/photos")), Some(5));
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);

        // A rename onto a directory fails: the target stays and the temp file goes.
        let blocked = root.path().join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        write_state(&blocked, &StateFile::default());
        assert!(blocked.is_dir());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
    }
}
