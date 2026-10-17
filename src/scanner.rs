use crate::image_list::SharedImageList;
use std::path::{Path, PathBuf};
use std::thread;

const BATCH_SIZE: usize = 64;

/// The formats of the `image` crate. JPEG and PNG come first, because most files are JPEG or PNG.
pub(crate) const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "tiff", "tif", "ico", "exr", "hdr", "qoi", "tga",
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
const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov", "mkv", "avi", "webm", "m4v"];

pub(crate) fn is_supported(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str());
    let Some(ext) = ext else { return false };
    let ext = ext.to_ascii_lowercase();
    if IMAGE_EXTENSIONS.contains(&ext.as_str()) || JXL_EXTENSIONS.contains(&ext.as_str()) {
        return true;
    }
    #[cfg(all(feature = "sips", target_os = "macos"))]
    if SIPS_EXTENSIONS.contains(&ext.as_str()) {
        return true;
    }
    #[cfg(feature = "heic")]
    if LIBHEIF_EXTENSIONS.contains(&ext.as_str()) {
        return true;
    }
    #[cfg(feature = "video")]
    if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        return true;
    }
    false
}

pub(crate) fn spawn(paths: Vec<PathBuf>, list: SharedImageList) {
    thread::spawn(move || {
        let mut batch = Vec::with_capacity(BATCH_SIZE);

        for path in paths {
            if path.is_dir() {
                scan_dir(&path, &list, &mut batch);
            } else if is_supported(&path) {
                batch.push(path);
                if batch.len() >= BATCH_SIZE {
                    flush_batch(&list, &mut batch);
                }
            } else {
                list.push_error(format!("unsupported media format: {}", path.display()));
            }
        }

        if !batch.is_empty() {
            flush_batch(&list, &mut batch);
        }

        list.mark_complete();
    });
}

fn scan_dir(dir: &Path, list: &SharedImageList, batch: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            list.push_error(format!("{}: {error}", dir.display()));
            return;
        }
    };

    for entry in entries {
        match entry {
            Ok(entry) => {
                let path = entry.path();
                match entry.file_type() {
                    Ok(file_type) => {
                        // `file_type` comes from the directory entry and does not
                        // follow symlinks, so a link pointing at a file reports
                        // neither `is_file()` nor `is_dir()`. Resolve the target
                        // with `metadata()` (which follows symlinks) so linked
                        // media is picked up too.
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

                        if is_regular_file && is_supported(&path) {
                            batch.push(path);
                            if batch.len() >= BATCH_SIZE {
                                flush_batch(list, batch);
                            }
                        }
                    }
                    Err(error) => list.push_error(format!("{}: {error}", path.display())),
                }
            }
            Err(error) => list.push_error(format!("{}: {error}", dir.display())),
        }
    }
}

fn flush_batch(list: &SharedImageList, batch: &mut Vec<PathBuf>) {
    batch.sort_by(|left, right| {
        left.file_name()
            .cmp(&right.file_name())
            .then_with(|| left.cmp(right))
    });
    list.push_batch(std::mem::take(batch));
    *batch = Vec::with_capacity(BATCH_SIZE);
}

#[cfg(test)]
mod tests {
    use super::{BATCH_SIZE, is_supported, scan_dir, spawn};
    use crate::image_list::SharedImageList;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::thread;
    use std::time::{Duration, Instant};

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
        for name in ["a.exr", "a.HDR", "a.qoi", "a.tga", "a.jxl"] {
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
        let (all, _) = list.drain_since(0);
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
}
