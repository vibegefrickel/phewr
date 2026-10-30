//! The list of scanned entries that the scanner fills and the UI reads.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// What an entry of the list is. The folders come first, so the decoders never see a folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryKind {
    Folder,
    File,
}

/// A subfolder of the scanned folder, or its parent as `..`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FolderEntry {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
}

struct Inner {
    paths: Vec<PathBuf>,
    filenames: Vec<String>,
    /// The first `folder_count` entries are folders.
    folder_count: usize,
    errors: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct SharedImageList {
    inner: Arc<Mutex<Inner>>,
    len: Arc<AtomicUsize>,
    complete: Arc<AtomicBool>,
}

impl SharedImageList {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                paths: Vec::new(),
                filenames: Vec::new(),
                folder_count: 0,
                errors: Vec::new(),
            })),
            len: Arc::new(AtomicUsize::new(0)),
            complete: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn push_batch(&self, paths: Vec<PathBuf>) {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        for path in paths {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            inner.paths.push(path);
            inner.filenames.push(name);
        }
        self.len.store(inner.paths.len(), Ordering::Release);
    }

    /// Put the folders at the front of the list. The scanner calls it before the first batch of
    /// files, so that the folders stay a prefix of the list.
    pub(crate) fn push_folders(&self, folders: Vec<FolderEntry>) {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        debug_assert!(
            inner.paths.len() == inner.folder_count,
            "push_folders must come before the first batch of files"
        );
        let at = inner.folder_count;
        let count = folders.len();
        let (paths, names): (Vec<PathBuf>, Vec<String>) = folders
            .into_iter()
            .map(|folder| (folder.path, folder.name))
            .unzip();
        inner.paths.splice(at..at, paths);
        inner.filenames.splice(at..at, names);
        inner.folder_count += count;
        self.len.store(inner.paths.len(), Ordering::Release);
    }

    /// The count of the folder entries at the front of the list.
    pub(crate) fn folder_count(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .folder_count
    }

    pub(crate) fn mark_complete(&self) {
        self.complete.store(true, Ordering::Release);
    }

    pub(crate) fn push_error(&self, error: String) {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .errors
            .push(error);
    }

    pub(crate) fn drain_errors(&self) -> Vec<String> {
        std::mem::take(
            &mut self
                .inner
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .errors,
        )
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.complete.load(Ordering::Acquire)
    }

    pub(crate) fn len(&self) -> usize {
        self.len.load(Ordering::Acquire)
    }

    /// Copies the entries from `known_len` on. The list keeps them.
    pub(crate) fn entries_since(&self, known_len: usize) -> (Vec<PathBuf>, Vec<String>) {
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let paths = inner.paths[known_len..].to_vec();
        let filenames = inner.filenames[known_len..].to_vec();
        (paths, filenames)
    }

    /// Replace the list. The first `folder_count` entries of `paths` are folders.
    pub(crate) fn replace_all(
        &self,
        paths: Vec<PathBuf>,
        filenames: Vec<String>,
        folder_count: usize,
    ) {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.paths = paths;
        inner.filenames = filenames;
        inner.folder_count = folder_count;
        self.len.store(inner.paths.len(), Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::{FolderEntry, SharedImageList};
    use std::path::PathBuf;

    #[test]
    fn paths_and_filenames_remain_aligned() {
        let list = SharedImageList::new();
        list.push_batch(vec![PathBuf::from("synthetic-a.png")]);
        let (paths, filenames) = list.entries_since(0);
        assert_eq!(paths, vec![PathBuf::from("synthetic-a.png")]);
        assert_eq!(filenames, vec!["synthetic-a.png"]);
    }

    #[test]
    fn errors_are_drained_once() {
        let list = SharedImageList::new();
        list.push_error("synthetic scanner error".into());
        assert_eq!(list.drain_errors(), vec!["synthetic scanner error"]);
        assert!(list.drain_errors().is_empty());
    }

    #[test]
    fn batch_push_updates_len_and_entries_since() {
        let list = SharedImageList::new();
        list.push_batch(vec![PathBuf::from("a.png"), PathBuf::from("b.png")]);

        assert_eq!(list.len(), 2);
        assert!(!list.is_complete());

        let (paths, filenames) = list.entries_since(0);
        assert_eq!(paths, vec![PathBuf::from("a.png"), PathBuf::from("b.png")]);
        assert_eq!(filenames, vec!["a.png", "b.png"]);

        let (empty, _) = list.entries_since(2);
        assert!(empty.is_empty());
    }

    #[test]
    fn mark_complete_and_replace_all() {
        let list = SharedImageList::new();
        list.push_batch(vec![PathBuf::from("a.png")]);
        list.mark_complete();

        assert!(list.is_complete());

        list.replace_all(vec![PathBuf::from("x.png")], vec!["x.png".into()], 0);
        assert_eq!(list.len(), 1);
        assert_eq!(list.folder_count(), 0);

        let (paths, filenames) = list.entries_since(0);
        assert_eq!(paths, vec![PathBuf::from("x.png")]);
        assert_eq!(filenames, vec!["x.png"]);
    }

    #[test]
    fn folders_go_to_the_front_and_count_as_a_prefix() {
        let list = SharedImageList::new();
        list.push_folders(vec![FolderEntry {
            path: PathBuf::from("/x"),
            name: "..".into(),
        }]);
        list.push_folders(vec![FolderEntry {
            path: PathBuf::from("/x/y/sub"),
            name: "sub".into(),
        }]);
        list.push_batch(vec![PathBuf::from("/x/y/a.png")]);

        assert_eq!(list.len(), 3);
        assert_eq!(list.folder_count(), 2);
        let (paths, filenames) = list.entries_since(0);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/x"),
                PathBuf::from("/x/y/sub"),
                PathBuf::from("/x/y/a.png")
            ]
        );
        assert_eq!(filenames, vec!["..", "sub", "a.png"]);

        list.replace_all(paths, filenames, 1);
        assert_eq!(list.folder_count(), 1);
    }

    #[test]
    fn filenames_use_default_for_root_path() {
        let list = SharedImageList::new();
        list.push_batch(vec![PathBuf::from("/")]);

        let (_, filenames) = list.entries_since(0);
        assert_eq!(filenames, vec![""]);
    }
}
