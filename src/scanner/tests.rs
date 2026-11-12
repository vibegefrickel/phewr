//! The tests of the directory scan on temp folders.

use super::{
    BATCH_SIZE, Item, classify, files_of, is_sorted_by_name, name_key, parent_entry, scan_dir,
    spawn, subfolders, walk_with,
};
use crate::image_list::{Batch, SharedImageList};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

/// Every directory that `open_dir` opened.
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

/// Wait at most 2 s, and a hang fails the test.
fn wait_for_complete(list: &SharedImageList) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !list.is_complete() {
        assert!(Instant::now() < deadline, "the scanner did not finish");
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
#[test]
fn follows_file_symlinks() {
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
fn reports_broken_symlink() {
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

/// One entry fails with `kind`. The other two entries must reach the list.
fn scan_with_file_type_error(kind: std::io::ErrorKind) -> (Vec<String>, Vec<String>) {
    let root = tempfile::tempdir().unwrap();
    let gone = root.path().join("gone.png");
    for name in ["a.png", "gone.png", "z.png"] {
        fs::write(root.path().join(name), b"synthetic").unwrap();
    }

    let list = SharedImageList::new();
    let mut files = Vec::new();
    walk_with(
        root.path(),
        &list,
        |item| {
            if let Item::File(path) = item {
                files.push(path);
            }
        },
        |entry, list| {
            if entry.path() == gone {
                classify(entry, list, || {
                    Err(std::io::Error::new(kind, "injected file-type error"))
                })
            } else {
                classify(entry, list, || entry.file_type())
            }
        },
    );

    let mut names: Vec<String> = files
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    (names, list.drain_errors())
}

#[test]
fn vanished_file_is_skipped_without_error() {
    let (files, errors) = scan_with_file_type_error(std::io::ErrorKind::NotFound);

    assert_eq!(files, ["a.png", "z.png"]);
    assert!(errors.is_empty());
}

#[test]
fn file_type_error_is_reported_and_scan_continues() {
    let (files, errors) = scan_with_file_type_error(std::io::ErrorKind::PermissionDenied);

    assert_eq!(files, ["a.png", "z.png"]);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("gone.png"), "{errors:?}");
    assert!(errors[0].contains("injected file-type error"), "{errors:?}");
}

#[test]
fn skips_hidden_and_apple_double() {
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
    assert_eq!(names, ["a.jpg"]);
}

#[test]
fn batch_sorts_by_lowercase_name() {
    let root = tempfile::tempdir().unwrap();
    for name in ["b.png", "B2.png", "a.png", "C.png"] {
        fs::write(root.path().join(name), b"synthetic").unwrap();
    }

    let (_, names) = files_of(root.path());

    assert_eq!(names, ["a.png", "b.png", "B2.png", "C.png"]);
}

#[test]
fn skips_folders_and_unsupported() {
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
fn subfolders_sort_after_parent() {
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
fn lists_unreadable_subfolder() {
    let root = tempfile::tempdir().unwrap();
    let dir = fs::canonicalize(root.path()).unwrap();
    let locked = dir.join("locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    let names: Vec<String> = subfolders(&dir)
        .into_iter()
        .map(|folder| folder.name)
        .collect();
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
fn reads_each_folder_once() {
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
fn first_batch_has_final_order() {
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

    let paths = list.batch_since(list.folder_count()).paths;
    let mut expected = paths.clone();
    expected.sort_by_cached_key(|path| name_key(path));
    assert_eq!(paths.len(), count);
    assert_eq!(paths, expected);
    assert!(is_sorted_by_name(&paths));
}

#[test]
fn sorted_check_follows_name_key() {
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
fn root_has_no_parent() {
    assert_eq!(parent_entry(Path::new("/")), None);
    let entry = parent_entry(Path::new("/photos/2026")).unwrap();
    assert_eq!(entry.name, "..");
    assert_eq!(entry.path, Path::new("/photos"));
}

#[cfg(unix)]
#[test]
fn skips_folder_symlinks() {
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
fn only_one_folder_lists_subfolders() {
    let root = tempfile::tempdir().unwrap();
    let dir = fs::canonicalize(root.path()).unwrap();
    fs::create_dir(dir.join("sub")).unwrap();
    fs::write(dir.join("photo.jpg"), b"synthetic").unwrap();

    let list = SharedImageList::new();
    spawn(vec![dir.clone()], list.clone());
    wait_for_complete(&list);
    assert_eq!(list.folder_count(), 2);
    let Batch {
        paths, filenames, ..
    } = list.batch_since(0);
    assert_eq!(filenames, ["..", "sub", "photo.jpg"]);
    assert_eq!(paths[1], dir.join("sub"));

    let list = SharedImageList::new();
    spawn(vec![dir.clone(), dir.join("photo.jpg")], list.clone());
    wait_for_complete(&list);
    assert_eq!(list.folder_count(), 0);
    assert_eq!(list.len(), 2);
}

#[test]
fn reports_missing_and_unsupported() {
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
fn reports_unreadable_folder() {
    let root = tempfile::tempdir().unwrap();
    let list = SharedImageList::new();
    let mut batch: Vec<PathBuf> = Vec::new();
    scan_dir(&root.path().join("missing"), &list, &mut batch);
    let errors = list.drain_errors();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("missing"), "{errors:?}");
}

#[test]
fn flushes_batches_in_name_order() {
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
    let all = list.batch_since(0).paths;
    let names: Vec<String> = all
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    let first_batch = &names[..BATCH_SIZE];
    assert!(first_batch.windows(2).all(|pair| pair[0] <= pair[1]));
    assert!(names.iter().any(|name| name == "img000.png"));
    assert!(list.drain_errors().is_empty());
}

#[test]
fn files_of_matches_scan_order() {
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
    let Batch {
        paths: scanned,
        filenames: scanned_names,
        ..
    } = list.batch_since(list.folder_count());
    let (files, names) = files_of(root.path());
    assert_eq!(files.len(), count, "no folder, no unsupported file");
    assert_eq!(files, scanned);
    assert_eq!(names, scanned_names);
}

/// The first entry and the end of the scan on `PHEWR_TEST_DIR`, best of 20.
#[test]
#[ignore = "timing: needs PHEWR_TEST_DIR"]
fn bench_folder_scan() {
    let Some(dir) = std::env::var_os("PHEWR_TEST_DIR") else {
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

/// The first entry of a scan of `PHEWR_TEST_DIR` with and without the Live Photo pairs, in
/// turns: the best and the median of 41 runs each.
#[test]
#[ignore = "timing: needs PHEWR_TEST_DIR"]
fn bench_live_pairs_scan() {
    let Some(dir) = std::env::var_os("PHEWR_TEST_DIR") else {
        return;
    };
    let mut times = [Vec::new(), Vec::new()];
    for _ in 0..41 {
        for (live_photos, runs) in [false, true].into_iter().zip(times.iter_mut()) {
            let list = SharedImageList::new();
            let start = Instant::now();
            super::spawn_with(vec![PathBuf::from(&dir)], list.clone(), live_photos);
            while list.len() == 0 {
                thread::yield_now();
            }
            runs.push(start.elapsed());
            wait_for_complete(&list);
        }
    }
    let [off, on] = times.map(|mut runs| {
        runs.sort_unstable();
        (runs[0], runs[runs.len() / 2])
    });
    eprintln!(
        "bench live pairs scan: first entry without pairs {:?} (median {:?}), with pairs {:?} \
         (median {:?})",
        off.0, off.1, on.0, on.1
    );
}
