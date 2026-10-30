//! The background directory scan and the lists of supported extensions.

use crate::image_list::{FolderEntry, SharedImageList};
use std::ffi::OsString;
use std::fs::DirEntry;
use std::path::{Path, PathBuf};
use std::thread;

const BATCH_SIZE: usize = 64;

/// The formats of the `image` crate. JPEG and PNG come first, because most files are JPEG or PNG.
/// An `.apng` file is a PNG: the decode guesses the format from the bytes.
pub(crate) const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "apng", "gif", "webp", "bmp", "tiff", "tif", "ico", "exr", "hdr", "qoi",
    "tga",
];

/// JPEG XL, decoded by `jxl-oxide`.
pub(crate) const JXL_EXTENSIONS: &[&str] = &["jxl"];

/// The formats that macOS `sips` reads and the `image` crate does not: camera RAW, Photoshop,
/// JPEG 2000, PDF (the first page), SVG and some rare formats. `sips --formats` lists them.
#[cfg(all(feature = "sips", target_os = "macos"))]
pub(crate) const SIPS_EXTENSIONS: &[&str] = &[
    // Camera RAW.
    "3fr", "arw", "cr2", "cr3", "crw", "dcr", "dng", "erf", "fff", "iiq", "mos", "mrw", "nef",
    "nrw", "orf", "pef", "raf", "raw", "rw2", "rwl", "sr2", "srf", "srw",
    // Other formats of ImageIO.
    "psd", "jp2", "pdf", "svg", "icns", "sgi", "dds", "pict",
];

/// The formats that libheif decodes. The `image` crate has no AVIF decoder, so AVIF is here.
#[cfg(feature = "heic")]
pub(crate) const LIBHEIF_EXTENSIONS: &[&str] = &["heic", "heif", "hif", "avif"];

#[cfg(feature = "video")]
pub(crate) const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov", "mkv", "avi", "webm", "m4v"];

/// The extension test of the scanner and the decoders. It ignores ASCII case and allocates
/// nothing.
pub(crate) fn has_extension(path: &Path, list: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| list.iter().any(|e| ext.eq_ignore_ascii_case(e)))
}

pub(crate) fn is_supported(path: &Path) -> bool {
    if has_extension(path, IMAGE_EXTENSIONS) || has_extension(path, JXL_EXTENSIONS) {
        return true;
    }
    #[cfg(all(feature = "sips", target_os = "macos"))]
    if has_extension(path, SIPS_EXTENSIONS) {
        return true;
    }
    #[cfg(feature = "heic")]
    if has_extension(path, LIBHEIF_EXTENSIONS) {
        return true;
    }
    #[cfg(feature = "video")]
    if has_extension(path, VIDEO_EXTENSIONS) {
        return true;
    }
    false
}

/// Scan `paths` in the background. One folder also lists its subfolders, so that the grid can
/// walk the tree. A list of several paths has no one parent, so it lists no folders.
pub(crate) fn spawn(paths: Vec<PathBuf>, list: SharedImageList) {
    thread::spawn(move || {
        match paths.as_slice() {
            [dir] if dir.is_dir() => scan_folder(dir, &list),
            _ => scan_paths(paths, &list),
        }
        list.mark_complete();
    });
}

/// Scan one folder argument: one `read_dir`, one sort. The folders must come before the first
/// file, so the list waits for the end of the read. The files then go out sorted in batches.
fn scan_folder(dir: &Path, list: &SharedImageList) {
    let listing = read_folder(dir, list, true);
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

/// Scan a list of files and folders: no subfolders, and each batch sorted for the first screen.
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

/// The supported files of `dir` and their names, sorted by `name_key`, for the tests.
#[cfg(test)]
pub(crate) fn files_of(dir: &Path) -> (Vec<PathBuf>, Vec<String>) {
    let (_, files, names) = folder_listing(dir);
    (files, names)
}

/// The subfolders and the supported files of `dir` from one read, as a switch to it lists them:
/// the folders sorted after `..`, the files and their names sorted by `name_key`. The folder
/// prefetch reads a folder one time with it, without a thread and without the errors.
pub(crate) fn folder_listing(dir: &Path) -> (Vec<FolderEntry>, Vec<PathBuf>, Vec<String>) {
    let list = SharedImageList::new();
    let listing = read_folder(dir, &list, true);
    list.push_batch(listing.files);
    let (files, names) = list.entries_since(0);
    (folder_entries(dir, listing.folder_names), files, names)
}

/// The canonical form of `p`, or `p` itself when the file system cannot resolve it.
pub(crate) fn canonicalize_or_self(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The entries of the subfolders of `dir`, sorted by name, after `..` for the parent, for the
/// tests. A hidden folder and a symlink stay out. A folder that viewr cannot read stays in:
/// `Enter` on it shows the read error, and a check would cost one `opendir` per subfolder.
#[cfg(test)]
pub(crate) fn subfolders(dir: &Path) -> Vec<FolderEntry> {
    folder_listing(dir).0
}

/// The sorted entries for the folder names of `dir`, with `..` first. The paths are canonical, so
/// that the cursor can find the folder that the user came from after a step to the parent.
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

/// The `..` entry of `dir`. The root has no parent and gets none.
fn parent_entry(dir: &Path) -> Option<FolderEntry> {
    dir.parent().map(|parent| FolderEntry {
        path: parent.to_path_buf(),
        name: "..".to_owned(),
    })
}

/// What one directory entry is for the scan.
enum Item {
    Folder(OsString),
    File(PathBuf),
}

/// The folders and the supported files of one directory, from one `read_dir`.
struct Listing {
    folder_names: Vec<OsString>,
    /// Sorted by `name_key`.
    files: Vec<PathBuf>,
}

/// Read `dir` one time. The read errors go to `list`. Without `want_files`, the files cost no
/// stat and no sort.
fn read_folder(dir: &Path, list: &SharedImageList, want_files: bool) -> Listing {
    let mut listing = Listing {
        folder_names: Vec::new(),
        files: Vec::new(),
    };
    walk(dir, list, want_files, |item| match item {
        Item::Folder(name) => listing.folder_names.push(name),
        Item::File(path) => listing.files.push(path),
    });
    listing.files.sort_by_cached_key(|path| name_key(path));
    listing
}

fn scan_dir(dir: &Path, list: &SharedImageList, batch: &mut Vec<PathBuf>) {
    walk(dir, list, true, |item| {
        if let Item::File(path) = item {
            batch.push(path);
            if batch.len() >= BATCH_SIZE {
                flush_batch(list, batch);
            }
        }
    });
}

/// `read_dir`, and the one place that the tests count.
fn open_dir(dir: &Path) -> std::io::Result<std::fs::ReadDir> {
    #[cfg(test)]
    tests::OPENED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(dir.to_path_buf());
    std::fs::read_dir(dir)
}

/// Visit the folders (not hidden, no symlink) and the supported files of `dir`.
fn walk(dir: &Path, list: &SharedImageList, want_files: bool, mut visit: impl FnMut(Item)) {
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
                if let Some(item) = classify(&entry, list, want_files) {
                    visit(item);
                }
            }
            Err(error) => list.push_error(format!("{}: {error}", dir.display())),
        }
    }
}

/// Sort one entry into a folder or a supported file, so that `walk` has one rule for both lists.
fn classify(entry: &DirEntry, list: &SharedImageList, want_files: bool) -> Option<Item> {
    let path = entry.path();
    let file_type = match entry.file_type() {
        Ok(file_type) => file_type,
        Err(error) => {
            list.push_error(format!("{}: {error}", path.display()));
            return None;
        }
    };
    // A symlink to a folder stays out: its canonical parent is not the folder of the link, so
    // `..` from inside would not lead back here.
    if file_type.is_dir() {
        let name = entry.file_name();
        return (!name.to_string_lossy().starts_with('.')).then_some(Item::Folder(name));
    }
    if !want_files {
        return None;
    }
    // `file_type` does not follow symlinks, so a link to a file is neither `is_file()` nor
    // `is_dir()`. `metadata()` follows the link, so that linked media shows up too.
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

/// The one sort key of the file lists: the lowercase file name, then the path. The batches of the
/// scan and the folders use it, and so must the sort at the end of the scan (`sort_by_name` in
/// `app/scan.rs`), so that the first screen does not reorder when the scan ends.
pub(crate) fn name_key(path: &Path) -> (String, PathBuf) {
    (lowercase_name(path), path.to_path_buf())
}

/// The lowercase file name, the first part of `name_key`.
fn lowercase_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// True when `paths` already follow `name_key`. It builds one key at a time and copies no path,
/// so the end of the scan can skip the sort and the list copy when the scanner sorted the files.
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
mod tests {
    use super::{
        BATCH_SIZE, files_of, is_sorted_by_name, is_supported, name_key, parent_entry, scan_dir,
        spawn, subfolders,
    };
    use crate::image_list::SharedImageList;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::thread;
    use std::time::{Duration, Instant};

    /// Every directory that `open_dir` opened, so that a test counts the `read_dir` calls.
    pub(super) static OPENED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    fn opened_count(dir: &Path) -> usize {
        let opened = OPENED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        opened
            .iter()
            .filter(|opened| opened.starts_with(dir))
            .count()
    }

    /// Wait at most 2 s for the scanner thread, so a hang fails the test.
    fn wait_for_complete(list: &SharedImageList) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !list.is_complete() {
            assert!(Instant::now() < deadline, "the scanner did not finish");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn supported_extensions_are_case_insensitive() {
        assert!(is_supported(Path::new("synthetic.PNG")));
        assert!(is_supported(Path::new("synthetic.jpeg")));
    }

    #[cfg(feature = "heic")]
    #[test]
    fn heic_extensions_are_supported() {
        assert!(is_supported(Path::new("IMG_0001.HEIC")));
        assert!(is_supported(Path::new("photo.heif")));
        assert!(is_supported(Path::new("photo.AVIF")));
    }

    #[cfg(not(feature = "heic"))]
    #[test]
    fn avif_needs_the_heic_feature() {
        assert!(!is_supported(Path::new("photo.avif")));
    }

    #[cfg(feature = "video")]
    #[test]
    fn video_extensions_are_supported() {
        assert!(is_supported(Path::new("clip.MP4")));
        assert!(is_supported(Path::new("clip.webm")));
    }

    #[test]
    fn the_formats_of_image_and_jpeg_xl_are_supported() {
        for name in ["a.exr", "a.HDR", "a.qoi", "a.tga", "a.jxl", "a.APNG"] {
            assert!(is_supported(Path::new(name)), "{name}");
        }
    }

    #[cfg(all(feature = "sips", target_os = "macos"))]
    #[test]
    fn the_sips_formats_are_supported() {
        for name in ["a.CR3", "a.nef", "a.dng", "a.psd", "a.pdf", "a.svg"] {
            assert!(is_supported(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn unsupported_extensions_are_rejected() {
        assert!(!is_supported(Path::new("synthetic.txt")));
        assert!(!is_supported(Path::new("synthetic")));
    }

    #[cfg(unix)]
    #[test]
    fn scan_dir_follows_symlinked_files() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let gallery = root.path().join("gallery");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&gallery).unwrap();

        let target = source.join("real.png");
        fs::write(&target, b"synthetic").unwrap();
        let link = gallery.join("linked.png");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let list = SharedImageList::new();
        let mut batch: Vec<PathBuf> = Vec::new();
        scan_dir(&gallery, &list, &mut batch);

        assert_eq!(batch, vec![link]);
    }

    #[cfg(unix)]
    #[test]
    fn scan_dir_reports_a_broken_symlink() {
        let root = tempfile::tempdir().unwrap();
        let link = root.path().join("gone.png");
        std::os::unix::fs::symlink(root.path().join("missing.png"), &link).unwrap();

        let list = SharedImageList::new();
        let mut batch: Vec<PathBuf> = Vec::new();
        scan_dir(root.path(), &list, &mut batch);

        assert!(batch.is_empty());
        let errors = list.drain_errors();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("gone.png"), "{errors:?}");
    }

    #[test]
    fn scan_dir_lists_hidden_files_and_apple_double_files() {
        // The current behavior: only the extension decides, so `.x.jpg` and the AppleDouble
        // file `._x.jpg` of a foreign volume show up as images.
        let root = tempfile::tempdir().unwrap();
        for name in [".x.jpg", "._x.jpg", "a.jpg"] {
            fs::write(root.path().join(name), b"synthetic").unwrap();
        }

        let list = SharedImageList::new();
        let mut batch: Vec<PathBuf> = Vec::new();
        scan_dir(root.path(), &list, &mut batch);

        let mut names: Vec<String> = batch
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["._x.jpg", ".x.jpg", "a.jpg"]);
    }

    #[test]
    fn a_batch_sorts_by_the_lowercase_name_like_the_end_of_the_scan() {
        let root = tempfile::tempdir().unwrap();
        for name in ["b.png", "B2.png", "a.png", "C.png"] {
            fs::write(root.path().join(name), b"synthetic").unwrap();
        }

        let (_, names) = files_of(root.path());

        assert_eq!(names, ["a.png", "b.png", "B2.png", "C.png"]);
    }

    #[test]
    fn scan_dir_skips_subdirectories_and_unsupported_files() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("nested.png")).unwrap();
        fs::write(root.path().join("notes.txt"), b"synthetic").unwrap();
        fs::write(root.path().join("photo.jpg"), b"synthetic").unwrap();

        let list = SharedImageList::new();
        let mut batch: Vec<PathBuf> = Vec::new();
        scan_dir(root.path(), &list, &mut batch);

        assert_eq!(batch, vec![root.path().join("photo.jpg")]);
        assert!(list.drain_errors().is_empty());
    }

    #[test]
    fn subfolders_come_sorted_after_the_parent_without_hidden_ones() {
        let root = tempfile::tempdir().unwrap();
        let dir = fs::canonicalize(root.path()).unwrap();
        for name in ["zebra", "Apple", ".hidden", "mango"] {
            fs::create_dir(dir.join(name)).unwrap();
        }
        fs::write(dir.join("photo.jpg"), b"synthetic").unwrap();

        let folders = subfolders(&dir);

        let names: Vec<&str> = folders.iter().map(|folder| folder.name.as_str()).collect();
        assert_eq!(names, ["..", "Apple", "mango", "zebra"]);
        assert_eq!(folders[0].path, dir.parent().unwrap());
        assert_eq!(folders[1].path, dir.join("Apple"));
    }

    #[test]
    fn an_unreadable_subfolder_is_listed_and_fails_on_enter() {
        let root = tempfile::tempdir().unwrap();
        let dir = fs::canonicalize(root.path()).unwrap();
        let locked = dir.join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let names: Vec<String> = subfolders(&dir)
            .into_iter()
            .map(|folder| folder.name)
            .collect();
        // Entering the folder is a scan of that folder: the read error is the note.
        let list = SharedImageList::new();
        let mut batch: Vec<PathBuf> = Vec::new();
        scan_dir(&locked, &list, &mut batch);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(names, ["..", "locked"]);
        let errors = list.drain_errors();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("locked"), "{errors:?}");
    }

    #[test]
    fn one_folder_argument_reads_each_directory_one_time() {
        let root = tempfile::tempdir().unwrap();
        let dir = fs::canonicalize(root.path()).unwrap();
        for index in 0..20 {
            fs::create_dir(dir.join(format!("sub{index:02}"))).unwrap();
        }
        fs::write(dir.join("photo.jpg"), b"synthetic").unwrap();

        let list = SharedImageList::new();
        spawn(vec![dir.clone()], list.clone());
        wait_for_complete(&list);

        assert_eq!(list.folder_count(), 21);
        assert_eq!(
            opened_count(&dir),
            1,
            "no second read, no opendir per subfolder"
        );
    }

    #[test]
    fn one_folder_comes_out_in_the_final_order_from_the_first_batch() {
        let root = tempfile::tempdir().unwrap();
        let dir = fs::canonicalize(root.path()).unwrap();
        let count = 2 * BATCH_SIZE + 5;
        for index in (0..count).rev() {
            let name = if index % 2 == 0 { "img" } else { "IMG" };
            fs::write(dir.join(format!("{name}{index:03}.png")), b"synthetic").unwrap();
        }

        let list = SharedImageList::new();
        spawn(vec![dir], list.clone());
        wait_for_complete(&list);

        let (paths, _) = list.entries_since(list.folder_count());
        let mut expected = paths.clone();
        expected.sort_by_cached_key(|path| name_key(path));
        assert_eq!(paths.len(), count);
        assert_eq!(paths, expected);
        assert!(is_sorted_by_name(&paths));
    }

    #[test]
    fn is_sorted_by_name_follows_the_name_key() {
        let paths = |names: &[&str]| -> Vec<PathBuf> {
            names
                .iter()
                .map(|name| PathBuf::from("/x").join(name))
                .collect()
        };
        assert!(is_sorted_by_name(&[]));
        assert!(is_sorted_by_name(&paths(&[
            "a.png", "b.png", "B2.png", "C.png"
        ])));
        assert!(!is_sorted_by_name(&paths(&["b.png", "A.png"])));
        assert!(!is_sorted_by_name(&paths(&["B.png", "a.png"])));
    }

    #[test]
    fn the_root_has_no_parent_entry() {
        assert_eq!(parent_entry(Path::new("/")), None);
        let entry = parent_entry(Path::new("/photos/2026")).unwrap();
        assert_eq!(entry.name, "..");
        assert_eq!(entry.path, Path::new("/photos"));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_to_a_folder_stays_out() {
        let root = tempfile::tempdir().unwrap();
        let dir = fs::canonicalize(root.path()).unwrap();
        fs::create_dir(dir.join("real")).unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("link")).unwrap();
        let names: Vec<String> = subfolders(&dir)
            .into_iter()
            .map(|folder| folder.name)
            .collect();
        assert_eq!(names, ["..", "real"]);
    }

    #[test]
    fn one_folder_lists_its_subfolders_first_and_several_paths_list_none() {
        let root = tempfile::tempdir().unwrap();
        let dir = fs::canonicalize(root.path()).unwrap();
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("photo.jpg"), b"synthetic").unwrap();

        let list = SharedImageList::new();
        spawn(vec![dir.clone()], list.clone());
        wait_for_complete(&list);
        assert_eq!(list.folder_count(), 2);
        let (paths, filenames) = list.entries_since(0);
        assert_eq!(filenames, ["..", "sub", "photo.jpg"]);
        assert_eq!(paths[1], dir.join("sub"));

        let list = SharedImageList::new();
        spawn(vec![dir.clone(), dir.join("photo.jpg")], list.clone());
        wait_for_complete(&list);
        assert_eq!(list.folder_count(), 0);
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn spawn_reports_missing_and_unsupported_paths() {
        let root = tempfile::tempdir().unwrap();
        let text = root.path().join("notes.txt");
        fs::write(&text, b"synthetic").unwrap();

        let list = SharedImageList::new();
        spawn(vec![text, root.path().join("missing")], list.clone());
        wait_for_complete(&list);

        assert_eq!(list.len(), 0);
        let errors = list.drain_errors();
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(
            errors
                .iter()
                .all(|error| error.starts_with("unsupported media format")),
            "{errors:?}"
        );
        assert!(errors[0].contains("notes.txt"), "{errors:?}");
    }

    #[test]
    fn scan_dir_reports_a_directory_that_it_cannot_read() {
        let root = tempfile::tempdir().unwrap();
        let list = SharedImageList::new();
        let mut batch: Vec<PathBuf> = Vec::new();
        scan_dir(&root.path().join("missing"), &list, &mut batch);
        let errors = list.drain_errors();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("missing"), "{errors:?}");
    }

    #[test]
    fn spawn_flushes_full_batches_in_name_order() {
        let root = tempfile::tempdir().unwrap();
        let gallery = root.path().join("gallery");
        fs::create_dir(&gallery).unwrap();
        let count = BATCH_SIZE + 3;
        for index in (0..count).rev() {
            fs::write(gallery.join(format!("img{index:03}.png")), b"synthetic").unwrap();
        }
        let files: Vec<PathBuf> = (0..count)
            .map(|index| root.path().join(format!("direct{index:03}.png")))
            .collect();
        for file in &files {
            fs::write(file, b"synthetic").unwrap();
        }

        let list = SharedImageList::new();
        let mut paths = files;
        paths.push(gallery);
        spawn(paths, list.clone());
        wait_for_complete(&list);

        assert_eq!(list.len(), 2 * count);
        let (all, _) = list.entries_since(0);
        let names: Vec<String> = all
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        // Each batch is sorted by file name. The first batch is a full batch of direct files.
        let first_batch = &names[..BATCH_SIZE];
        assert!(first_batch.windows(2).all(|pair| pair[0] <= pair[1]));
        assert!(names.iter().any(|name| name == "img000.png"));
        assert!(list.drain_errors().is_empty());
    }

    #[test]
    fn files_of_lists_the_files_in_the_order_of_the_scan() {
        let root = tempfile::tempdir().unwrap();
        let count = BATCH_SIZE + 5;
        for index in (0..count).rev() {
            fs::write(root.path().join(format!("img{index:03}.png")), b"synthetic").unwrap();
        }
        fs::write(root.path().join("notes.txt"), b"text").unwrap();
        fs::create_dir(root.path().join("sub")).unwrap();

        let list = SharedImageList::new();
        spawn(vec![root.path().to_path_buf()], list.clone());
        wait_for_complete(&list);
        let (scanned, scanned_names) = list.entries_since(list.folder_count());
        let (files, names) = files_of(root.path());
        assert_eq!(files.len(), count, "no folder, no unsupported file");
        assert_eq!(files, scanned);
        assert_eq!(names, scanned_names);
    }

    /// Time `spawn` on `VIEWR_TEST_DIR`: the first entry and the end of the scan, best of 20.
    #[test]
    #[ignore = "timing: needs VIEWR_TEST_DIR"]
    fn bench_scan_of_a_folder() {
        let Some(dir) = std::env::var_os("VIEWR_TEST_DIR") else {
            return;
        };
        let (mut first, mut complete) = (Duration::MAX, Duration::MAX);
        for _ in 0..20 {
            let list = SharedImageList::new();
            let start = Instant::now();
            spawn(vec![PathBuf::from(&dir)], list.clone());
            while list.len() == 0 {
                thread::yield_now();
            }
            first = first.min(start.elapsed());
            while !list.is_complete() {
                thread::yield_now();
            }
            complete = complete.min(start.elapsed());
        }
        eprintln!("bench scan: first entry {first:?}, complete {complete:?}");
    }
}
