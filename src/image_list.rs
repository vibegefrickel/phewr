//! The list of scanned entries that the scanner fills and the UI reads.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// The folders come first, and the decoders never see a folder.
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

/// New entries of the list and the list length that they end at.
pub(crate) struct Batch {
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) filenames: Vec<String>,
    pub(crate) len: usize,
}

struct Inner {
    paths: Vec<PathBuf>,
    filenames: Vec<String>,
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

    /// The scanner calls it before the first batch of files. The folders stay a prefix.
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

    /// The entries from `known_len` on with the length after them, from one lock. A batch that
    /// the scanner pushes during the poll then comes in the next poll, not twice.
    pub(crate) fn batch_since(&self, known_len: usize) -> Batch {
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        Batch {
            paths: inner.paths[known_len..].to_vec(),
            filenames: inner.filenames[known_len..].to_vec(),
            len: inner.paths.len(),
        }
    }

    /// The first `folder_count` entries of `paths` are folders.
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
    use super::{Batch, FolderEntry, SharedImageList};
    use std::path::PathBuf;

    #[test]
    fn paths_and_filenames_remain_aligned() {
        let list = SharedImageList::new();
        list.push_batch(vec![PathBuf::from("synthetic-a.png")]);
        let Batch {
            paths, filenames, ..
        } = list.batch_since(0);
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
    fn batch_push_updates_len() {
        let list = SharedImageList::new();
        list.push_batch(vec![PathBuf::from("a.png"), PathBuf::from("b.png")]);

        assert_eq!(list.len(), 2);
        assert!(!list.is_complete());

        let Batch {
            paths, filenames, ..
        } = list.batch_since(0);
        assert_eq!(paths, vec![PathBuf::from("a.png"), PathBuf::from("b.png")]);
        assert_eq!(filenames, vec!["a.png", "b.png"]);

        let Batch { paths: empty, .. } = list.batch_since(2);
        assert!(empty.is_empty());
    }

    #[test]
    fn batch_ends_at_its_own_length() {
        let list = SharedImageList::new();
        list.push_batch(vec![PathBuf::from("a.png")]);
        let known_len = list.len();
        // The scanner pushes after the poll read the length.
        list.push_batch(vec![PathBuf::from("b.png"), PathBuf::from("c.png")]);
        let batch = list.batch_since(known_len);
        assert_eq!(
            batch.paths,
            [PathBuf::from("b.png"), PathBuf::from("c.png")]
        );
        assert_eq!(batch.filenames, ["b.png", "c.png"]);
        assert_eq!(batch.len, 3);
        assert!(list.batch_since(batch.len).paths.is_empty());
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

        let Batch {
            paths, filenames, ..
        } = list.batch_since(0);
        assert_eq!(paths, vec![PathBuf::from("x.png")]);
        assert_eq!(filenames, vec!["x.png"]);
    }

    #[test]
    fn folders_form_prefix() {
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
        let Batch {
            paths, filenames, ..
        } = list.batch_since(0);
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

        let Batch { filenames, .. } = list.batch_since(0);
        assert_eq!(filenames, vec![""]);
    }
}
