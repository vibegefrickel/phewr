//! The background directory scan. A folder argument is one `read_dir`, and its first batch is
//! in the final order.

mod extensions;

#[cfg(feature = "heic")]
pub(crate) use extensions::LIBHEIF_EXTENSIONS;
#[cfg(all(feature = "sips", target_os = "macos"))]
pub(crate) use extensions::SIPS_EXTENSIONS;
#[cfg(feature = "video")]
pub(crate) use extensions::VIDEO_EXTENSIONS;
pub(crate) use extensions::{IMAGE_EXTENSIONS, JXL_EXTENSIONS, has_extension, is_supported};

use crate::image_list::{FolderEntry, SharedImageList};
use std::ffi::OsString;
use std::fs::DirEntry;
use std::path::{Path, PathBuf};
use std::thread;

const BATCH_SIZE: usize = 64;

/// One folder lists its subfolders too. Several paths have no common parent and list no
/// folders.
pub(crate) fn spawn(paths: Vec<PathBuf>, list: SharedImageList) {
    thread::spawn(move || {
        match paths.as_slice() {
            [dir] if dir.is_dir() => scan_folder(dir, &list),
            _ => scan_paths(paths, &list),
        }
        list.mark_complete();
    });
}

/// The folders come before the first file. The list waits for the end of the read, then the
/// files go out sorted in batches.
fn scan_folder(dir: &Path, list: &SharedImageList) {
    let listing = read_folder(dir, list);
    list.push_folders(folder_entries(dir, listing.folder_names));
    let mut files = listing.files.into_iter();
    loop {
        let batch: Vec<PathBuf> = files.by_ref().take(BATCH_SIZE).collect();
        if batch.is_empty() {
            break;
        }
        list.push_batch(batch);
    }
}

/// No subfolders, and each batch sorted for the first screen.
fn scan_paths(paths: Vec<PathBuf>, list: &SharedImageList) {
    let mut batch = Vec::with_capacity(BATCH_SIZE);
    for path in paths {
        if path.is_dir() {
            scan_dir(&path, list, &mut batch);
        } else if is_supported(&path) {
            batch.push(path);
            if batch.len() >= BATCH_SIZE {
                flush_batch(list, &mut batch);
            }
        } else {
            list.push_error(format!("unsupported media format: {}", path.display()));
        }
    }
    if !batch.is_empty() {
        flush_batch(list, &mut batch);
    }
}

#[cfg(test)]
pub(crate) fn files_of(dir: &Path) -> (Vec<PathBuf>, Vec<String>) {
    let (_, files, names) = folder_listing(dir);
    (files, names)
}

/// The subfolders (`..` first) and the sorted files of `dir` from one read, as a switch lists
/// them. The folder prefetch reads without a thread and without the errors.
pub(crate) fn folder_listing(dir: &Path) -> (Vec<FolderEntry>, Vec<PathBuf>, Vec<String>) {
    let list = SharedImageList::new();
    let listing = read_folder(dir, &list);
    list.push_batch(listing.files);
    let batch = list.batch_since(0);
    (
        folder_entries(dir, listing.folder_names),
        batch.paths,
        batch.filenames,
    )
}

pub(crate) fn canonicalize_or_self(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// A folder that viewr cannot read stays in. `Enter` on it shows the error, and a check costs
/// one `opendir` per subfolder.
#[cfg(test)]
pub(crate) fn subfolders(dir: &Path) -> Vec<FolderEntry> {
    folder_listing(dir).0
}

/// The canonical paths let the cursor find the folder of origin after a step to the parent.
fn folder_entries(dir: &Path, names: Vec<OsString>) -> Vec<FolderEntry> {
    let dir = canonicalize_or_self(dir);
    let mut folders: Vec<FolderEntry> = names
        .into_iter()
        .map(|name| FolderEntry {
            path: dir.join(&name),
            name: name.to_string_lossy().into_owned(),
        })
        .collect();
    folders.sort_by_cached_key(|folder| name_key(&folder.path));
    folders.splice(0..0, parent_entry(&dir));
    folders
}

/// The root has no parent and gets no `..` entry.
fn parent_entry(dir: &Path) -> Option<FolderEntry> {
    dir.parent().map(|parent| FolderEntry {
        path: parent.to_path_buf(),
        name: "..".to_owned(),
    })
}

enum Item {
    Folder(OsString),
    File(PathBuf),
}

struct Listing {
    folder_names: Vec<OsString>,
    /// Sorted by `name_key`.
    files: Vec<PathBuf>,
}

/// The read errors go to `list`.
fn read_folder(dir: &Path, list: &SharedImageList) -> Listing {
    let mut listing = Listing {
        folder_names: Vec::new(),
        files: Vec::new(),
    };
    walk(dir, list, |item| match item {
        Item::Folder(name) => listing.folder_names.push(name),
        Item::File(path) => listing.files.push(path),
    });
    listing.files.sort_by_cached_key(|path| name_key(path));
    listing
}

fn scan_dir(dir: &Path, list: &SharedImageList, batch: &mut Vec<PathBuf>) {
    walk(dir, list, |item| {
        if let Item::File(path) = item {
            batch.push(path);
            if batch.len() >= BATCH_SIZE {
                flush_batch(list, batch);
            }
        }
    });
}

/// The tests count the calls.
fn open_dir(dir: &Path) -> std::io::Result<std::fs::ReadDir> {
    #[cfg(test)]
    tests::OPENED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(dir.to_path_buf());
    std::fs::read_dir(dir)
}

/// Folders without symlinks, and supported files. Hidden entries stay out.
fn walk(dir: &Path, list: &SharedImageList, mut visit: impl FnMut(Item)) {
    let entries = match open_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            list.push_error(format!("{}: {error}", dir.display()));
            return;
        }
    };

    for entry in entries {
        match entry {
            Ok(entry) => {
                if let Some(item) = classify(&entry, list) {
                    visit(item);
                }
            }
            Err(error) => list.push_error(format!("{}: {error}", dir.display())),
        }
    }
}

fn classify(entry: &DirEntry, list: &SharedImageList) -> Option<Item> {
    let path = entry.path();
    let name = entry.file_name();
    // Hidden entries stay out before a file type check can push an error. That covers the
    // AppleDouble files `._x.jpg` of a foreign volume.
    if name.to_string_lossy().starts_with('.') {
        return None;
    }
    let file_type = match entry.file_type() {
        Ok(file_type) => file_type,
        Err(error) => {
            list.push_error(format!("{}: {error}", path.display()));
            return None;
        }
    };
    // A symlink to a folder stays out. Its canonical parent is not the folder of the link, and
    // `..` from inside does not lead back here.
    if file_type.is_dir() {
        return Some(Item::Folder(name));
    }
    // `file_type` does not follow symlinks, and a link to a file is neither a file nor a folder.
    // `metadata()` follows the link.
    let is_regular_file = if file_type.is_symlink() {
        match std::fs::metadata(&path) {
            Ok(target) => target.is_file(),
            Err(error) => {
                list.push_error(format!("{}: {error}", path.display()));
                false
            }
        }
    } else {
        file_type.is_file()
    };
    (is_regular_file && is_supported(&path)).then_some(Item::File(path))
}

/// The lowercase file name, then the path. The sort at the end of the scan (`sort_by_name` in
/// `app/scan.rs`) must use it too, or the first screen reorders.
pub(crate) fn name_key(path: &Path) -> (String, PathBuf) {
    (lowercase_name(path), path.to_path_buf())
}

fn lowercase_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// True when `paths` follow `name_key`. It copies no path, and the end of the scan skips the
/// sort when the scanner sorted the files.
pub(crate) fn is_sorted_by_name(paths: &[PathBuf]) -> bool {
    let mut keys = paths.iter().map(|path| (lowercase_name(path), path));
    let Some(mut previous) = keys.next() else {
        return true;
    };
    keys.all(|key| {
        let in_order = previous <= key;
        previous = key;
        in_order
    })
}

fn flush_batch(list: &SharedImageList, batch: &mut Vec<PathBuf>) {
    batch.sort_by_cached_key(|path| name_key(path));
    list.push_batch(std::mem::take(batch));
    *batch = Vec::with_capacity(BATCH_SIZE);
}

#[cfg(test)]
mod tests;
